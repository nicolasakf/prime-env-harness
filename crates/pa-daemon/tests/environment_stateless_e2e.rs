#![cfg(unix)]
//! A stateless seat's model request stays the size of its view while its
//! transcript keeps growing; conversation context is the control.
#[path = "support/environment_daemon.rs"]
mod support;
use serde_json::{json, Value};
use support::start;

const PADDING: usize = 4000;
const ROUNDS: u64 = 4;

/// The relay fixture with padded observations and a distinct `agent_view`, so
/// request growth is measurable and the view is told apart from `observe`.
fn relay_source() -> String {
    include_str!("../../pa-environments/fixtures/relay/environment.py")
        .replace("get(\"rounds\", 1)", &format!("get(\"rounds\", {ROUNDS})"))
        .replace(
            "\"you\": seat}",
            &format!("\"you\": seat, \"padding\": \"x\" * {PADDING}}}"),
        )
        .replace(
            "    def handle_input",
            "    def agent_view(self, seat):\n        return {**self.observe(seat), \"history\": [\"summary\"]}\n\n    def handle_input",
        )
}

/// An agents-only relay's results: `player-1`'s transcript rows and the run state.
struct Relay {
    rows: Vec<Value>,
    run: Value,
}

/// How a seat's faux model behaves on each turn.
#[derive(Clone, Copy)]
enum Turns {
    /// Act, then confirm.
    Prompt,
    /// The first turn ends without an action, which the host retries.
    IdleFirst,
    /// Act, then take longer than the 1s turn deadline to confirm.
    SlowAfterActing,
    /// Act, then attempt a second action out of turn, which the environment rejects.
    ActsOutOfTurnOnce,
}

/// Run an agents-only relay.
fn relay_run(agent_context: &str, turns: Turns) -> Relay {
    let root = tempfile::tempdir().unwrap();
    // Tool call IDs scope operation IDs, so each pass needs its own.
    let confirm = match turns {
        Turns::SlowAfterActing => {
            json!({"delayMs":2500,"content":[{"type":"text","text":"passed"}]})
        }
        Turns::Prompt | Turns::IdleFirst | Turns::ActsOutOfTurnOnce => json!("passed"),
    };
    let responses: Vec<Value> = matches!(turns, Turns::IdleFirst)
        .then(|| json!("let me think about it"))
        .into_iter()
        .chain((0..ROUNDS).flat_map(|round| {
            let pass = |id: String| json!({"content":[{"type":"toolCall","id":id,"name":"env_pass","arguments":{}}]});
            let mut turn = vec![pass(format!("pass-{round}"))];
            if matches!(turns, Turns::ActsOutOfTurnOnce) && round == 0 {
                turn.push(pass("pass-out-of-turn".into()));
            }
            turn.push(confirm.clone());
            turn
        }))
        .collect();
    let limits = match turns {
        Turns::SlowAfterActing => json!({"seat_turn_timeout_seconds":1}),
        Turns::Prompt | Turns::IdleFirst | Turns::ActsOutOfTurnOnce => json!({}),
    };
    let (mut supervisor, mut client) =
        start(root.path(), &json!({"engine":"faux","responses":responses}));
    let created = client.request(&json!({"type":"create","config":{"cwd":root.path(),"script":root.path().join("faux.json"),"sessionDir":root.path().join("agent/sessions")},"telemetryDisabled":true}));
    assert_eq!(created["success"], true, "{created}");
    let builder = created["data"]["activeSessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let draft = client.environment(
        &builder,
        "create",
        &json!({"description":"relay","language":"python"}),
    )["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = std::path::Path::new(&draft);
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(path.join("environment.json")).unwrap()).unwrap();
    manifest["protocol_version"] = json!(2);
    manifest["roles"] = json!([{"name":"player","min_seats":2,"max_seats":2}]);
    manifest["agent_context"] = json!(agent_context);
    std::fs::write(path.join("environment.json"), manifest.to_string()).unwrap();
    std::fs::write(path.join("environment.py"), relay_source()).unwrap();
    std::fs::write(path.join("checks.py"), "").unwrap();
    let policy = json!({"backend":"local"});
    client.environment(&builder, "policy", &json!({"draft":draft,"policy":policy}));
    let report = client.environment(&builder, "validate", &json!({"draft":draft}));
    assert_eq!(report["passed"], true, "{report}");
    client.environment(&builder, "save", &json!({"draft":draft,"id":"relay"}));
    let seats: Vec<Value> = (1..=2)
        .map(|index| json!({"id":format!("player-{index}"),"role":"player","occupant":"agent"}))
        .collect();
    let entered = client.environment(
        &builder,
        "enter",
        &json!({"id":"relay","policy":policy,"roster":{"seats":seats,"limits":limits}}),
    );
    let host = entered["session"]["activeSessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        client.request(&json!({"type":"attach","activeSessionId":host}))["success"],
        true
    );
    let status = client.environment(&host, "status", &json!({}));
    if status["run"]["phase"] != "finished" {
        client.wait_event(|event| {
            event["type"] == "environment_status" && event["run"]["phase"] == "finished"
        });
    }
    let instance = root
        .path()
        .join("agent/environments/instances")
        .join(entered["instance_id"].as_str().unwrap());
    let rows = std::fs::read_to_string(instance.join("seats/player-1/conversation.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let run = serde_json::from_slice(&std::fs::read(instance.join("run.json")).unwrap()).unwrap();
    client.environment(&host, "leave", &json!({}));
    assert_eq!(client.request(&json!({"type":"shutdown"}))["success"], true);
    supervisor.0.wait().unwrap();
    Relay { rows, run }
}

/// Prompt tokens of the first model request after each wake.
fn prompt_sizes(rows: &[Value]) -> Vec<u64> {
    rows.windows(2)
        .filter(|pair| pair[0]["customType"] == "environment_wake")
        .map(|pair| {
            let usage = &pair[1]["message"]["usage"];
            usage["input"].as_u64().unwrap() + usage["cacheRead"].as_u64().unwrap()
        })
        .collect()
}

#[test]
fn stateless_seat_requests_stay_flat_and_carry_the_view() {
    let Relay { rows, run } = relay_run("stateless", Turns::Prompt);
    let sizes = prompt_sizes(&rows);
    assert_eq!(sizes.len() as u64, ROUNDS, "one wake per round: {sizes:?}");
    assert!(
        sizes[sizes.len() - 1] <= sizes[0] + 50,
        "request size must not grow with the transcript: {sizes:?}"
    );
    let wake = rows
        .iter()
        .find(|row| row["customType"] == "environment_wake")
        .unwrap()["content"]
        .as_str()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(wake.strip_prefix("Environment wake:\n").unwrap()).unwrap(),
        json!({
            "state": {"turn":0,"next":"player-1","you":"player-1","padding":"x".repeat(PADDING),"history":["summary"]},
            "messages": ["Your turn"],
        })
    );
    // The host measured how long the seat took to act, once per turn, and no turn
    // ran past its deadline.
    assert_eq!(run["time_to_action"]["player-1"]["count"], json!(ROUNDS));
    assert_eq!(run["deadline_retries"], json!({}));
    let result = rows
        .iter()
        .find(|row| row["message"]["role"] == "toolResult")
        .unwrap()["message"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(result).unwrap(),
        json!({
            "result": 1,
            "state": {"turn":1,"next":"player-2","you":"player-1","padding":"x".repeat(PADDING),"history":["summary"]},
        })
    );
}

#[test]
fn conversation_seat_requests_grow_with_the_transcript() {
    let sizes = prompt_sizes(&relay_run("conversation", Turns::Prompt).rows);
    assert_eq!(sizes.len() as u64, ROUNDS, "one wake per round: {sizes:?}");
    assert!(
        sizes[sizes.len() - 1] > sizes[0] + 3 * PADDING as u64 / 8,
        "the control must show the growth the stateless mode removes: {sizes:?}"
    );
}

#[test]
fn a_retried_turn_is_told_so_and_the_next_turn_is_not() {
    let Relay { rows, .. } = relay_run("stateless", Turns::IdleFirst);
    let retries: Vec<Value> = rows
        .iter()
        .filter(|row| row["customType"] == "environment_wake")
        .map(|row| {
            let wake = row["content"].as_str().unwrap();
            let wake: Value =
                serde_json::from_str(wake.strip_prefix("Environment wake:\n").unwrap()).unwrap();
            wake["retry"].clone()
        })
        .collect();
    // The idle first turn is retried once; after the seat acts, turns are normal again.
    assert_eq!(
        retries,
        vec![Value::Null, json!(1), Value::Null, Value::Null, Value::Null]
    );
}

#[test]
fn a_seat_that_acted_is_never_retried_for_a_slow_confirmation() {
    let Relay { rows, run } = relay_run("stateless", Turns::SlowAfterActing);
    // The deadline bounds time to action. A lingering follow-up may be cut so the
    // seat's next wake gets through, but that is cleanup: no retry is spent and no
    // later turn is told to retry.
    assert_eq!(run["deadline_retries"], json!({}));
    assert_eq!(run["idle_retries"], json!({}));
    assert_eq!(run["time_to_action"]["player-1"]["count"], json!(ROUNDS));
    assert!(rows
        .iter()
        .filter(|row| row["customType"] == "environment_wake")
        .all(|row| !row["content"].as_str().unwrap().contains("\"retry\"")));
}

#[test]
fn a_rejected_action_returns_the_current_state_to_a_seat_that_cannot_observe() {
    let Relay { rows, .. } = relay_run("stateless", Turns::ActsOutOfTurnOnce);
    let rejected = rows
        .iter()
        .find(|row| row["message"]["role"] == "toolResult" && row["message"]["isError"] == true)
        .expect("the out-of-turn action is rejected");
    let text = rejected["message"]["content"][0]["text"].as_str().unwrap();
    let (error, state) = text.split_once("\nCurrent state: ").unwrap();
    assert!(error.contains("not your turn"), "{error}");
    assert_eq!(
        serde_json::from_str::<Value>(state).unwrap(),
        json!({"turn":1,"next":"player-2","you":"player-1","padding":"x".repeat(PADDING),"history":["summary"]})
    );
}
