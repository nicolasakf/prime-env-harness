#![cfg(unix)]
//! The turn deadline also bounds a seat that already acted and is no longer
//! awaited: a model that keeps calling tools after its task is done is cut as
//! cleanup instead of running until it stops on its own.
#[path = "support/environment_daemon.rs"]
mod support;
use serde_json::{json, Value};
use support::start;

/// Two seats. The first acts once; the environment then waits on the second seat
/// without waking it, so the run stalls once no seat is busy.
const ENVIRONMENT: &str = r#"from sdk import notify, progress


class Environment:
    def initialize(self, params):
        self.seats = [seat["id"] for seat in params["seats"]]
        self.acted = False
        notify([self.seats[0]], "Your turn", actionable=True)
        progress(0, waiting_for=[self.seats[0]])
        return {}

    def describe_actions(self):
        return [{"name": "act", "description": "Act once.", "input_schema": {"type": "object"},
                 "output_schema": {"type": "object"}, "roles": ["player"]}]

    def observe(self, seat):
        return {"acted": self.acted, "you": seat}

    def handle_input(self, seat, text):
        raise ValueError("talk is disabled")

    def invoke(self, seat, action, arguments):
        if self.acted or seat != self.seats[0]:
            raise ValueError("not your turn")
        self.acted = True
        progress(1, waiting_for=[self.seats[1]])
        return {}

    def snapshot(self):
        return {"seats": self.seats, "acted": self.acted}

    def restore(self, snapshot):
        self.seats, self.acted = snapshot["seats"], snapshot["acted"]
        return {}

    def dispose(self):
        return {}


environment = Environment()
"#;

#[test]
fn a_seat_that_keeps_calling_tools_after_acting_is_cut_at_the_deadline() {
    let root = tempfile::tempdir().unwrap();
    let call = |id: &str| json!({"type":"toolCall","id":id,"name":"env_act","arguments":{}});
    // The seat acts, then spends far longer than the 1s deadline on a further call
    // the environment would reject.
    let responses = json!([
        {"content":[call("act")]},
        {"delayMs":8000,"content":[call("act-again")]},
        "done",
    ]);
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
        &json!({"description":"deadline","language":"python"}),
    )["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = std::path::Path::new(&draft);
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(path.join("environment.json")).unwrap()).unwrap();
    manifest["protocol_version"] = json!(2);
    manifest["roles"] = json!([{"name":"player","min_seats":2,"max_seats":2}]);
    manifest["agent_context"] = json!("stateless");
    std::fs::write(path.join("environment.json"), manifest.to_string()).unwrap();
    std::fs::write(path.join("environment.py"), ENVIRONMENT).unwrap();
    std::fs::write(path.join("checks.py"), "").unwrap();
    let policy = json!({"backend":"local"});
    client.environment(&builder, "policy", &json!({"draft":draft,"policy":policy}));
    let report = client.environment(&builder, "validate", &json!({"draft":draft}));
    assert_eq!(report["passed"], true, "{report}");
    client.environment(&builder, "save", &json!({"draft":draft,"id":"deadline"}));
    let seats: Vec<Value> = (1..=2)
        .map(|index| json!({"id":format!("player-{index}"),"role":"player","occupant":"agent"}))
        .collect();
    let entered = client.environment(
        &builder,
        "enter",
        &json!({"id":"deadline","policy":policy,"roster":{"seats":seats,"limits":{"seat_turn_timeout_seconds":1}}}),
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
    if status["run"]["phase"] != "stalled" {
        client.wait_event(|event| {
            event["type"] == "environment_status" && event["run"]["phase"] == "stalled"
        });
    }
    let instance = root
        .path()
        .join("agent/environments/instances")
        .join(entered["instance_id"].as_str().unwrap());
    let run: Value =
        serde_json::from_slice(&std::fs::read(instance.join("run.json")).unwrap()).unwrap();
    // The lingering follow-up was cut as cleanup: no retry spent, no deadline retry.
    assert_eq!(run["post_action_aborts"], json!({"player-1":1}), "{run}");
    assert_eq!(run["deadline_retries"], json!({}), "{run}");
    assert_eq!(run["idle_retries"], json!({}), "{run}");
    // The cut call never reached the environment: only the first action committed.
    let checkpoint: Value =
        serde_json::from_slice(&std::fs::read(instance.join("checkpoint.json")).unwrap()).unwrap();
    assert_eq!(
        checkpoint["receipts"].as_object().unwrap().len(),
        2,
        "{checkpoint}"
    );
    client.environment(&host, "leave", &json!({}));
    assert_eq!(client.request(&json!({"type":"shutdown"}))["success"], true);
    supervisor.0.wait().unwrap();
}
