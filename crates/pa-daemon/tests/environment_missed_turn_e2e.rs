#![cfg(unix)]
//! An environment that declares `missed_turns` hears about every turn a seat lets
//! run past its deadline, and may resolve the turn instead of the run pausing.
#[path = "support/environment_daemon.rs"]
mod support;
use serde_json::{json, Value};
use support::start;

/// One seat. Its first missed turn draws a warning; its second ends the session.
const ENVIRONMENT: &str = r#"from sdk import finish, notify, progress


class Environment:
    def initialize(self, params):
        self.seat = params["seats"][0]["id"]
        notify([self.seat], "Your turn", actionable=True)
        progress(0, waiting_for=[self.seat])
        return {}

    def describe_actions(self):
        return [{"name": "act", "description": "Act once.", "input_schema": {"type": "object"},
                 "output_schema": {"type": "object"}, "roles": ["player"]}]

    def observe(self, seat):
        return {"you": seat}

    def handle_input(self, seat, text):
        raise ValueError("talk is disabled")

    def invoke(self, seat, action, arguments):
        raise ValueError("this test expects no action")

    def missed_turn(self, seat, report):
        if report["misses"] == 1:
            notify([seat], "You missed the deadline; act now.", wake=True, actionable=True)
        else:
            progress(1, waiting_for=[])
            finish(f"{seat} missed {report['misses']} turns")
        return {}

    def snapshot(self):
        return {"seat": self.seat}

    def restore(self, snapshot):
        self.seat = snapshot["seat"]
        return {}

    def dispose(self):
        return {}


environment = Environment()
"#;

#[test]
fn an_environment_resolves_a_seat_that_keeps_missing_its_deadline() {
    let root = tempfile::tempdir().unwrap();
    // Every model request outlasts the 1s deadline.
    let slow = json!({"delayMs":8000,"content":[{"type":"toolCall","id":"act","name":"env_act","arguments":{}}]});
    let responses = json!([slow, slow, slow, slow]);
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
        &json!({"description":"missed turns","language":"python"}),
    )["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = std::path::Path::new(&draft);
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(path.join("environment.json")).unwrap()).unwrap();
    manifest["protocol_version"] = json!(2);
    manifest["roles"] = json!([{"name":"player","min_seats":1,"max_seats":1}]);
    manifest["agent_context"] = json!("stateless");
    manifest["missed_turns"] = json!(true);
    std::fs::write(path.join("environment.json"), manifest.to_string()).unwrap();
    std::fs::write(path.join("environment.py"), ENVIRONMENT).unwrap();
    std::fs::write(path.join("checks.py"), "").unwrap();
    let policy = json!({"backend":"local"});
    client.environment(&builder, "policy", &json!({"draft":draft,"policy":policy}));
    let report = client.environment(&builder, "validate", &json!({"draft":draft}));
    assert_eq!(report["passed"], true, "{report}");
    client.environment(&builder, "save", &json!({"draft":draft,"id":"missed"}));
    let entered = client.environment(
        &builder,
        "enter",
        &json!({"id":"missed","policy":policy,"roster":{"seats":[{"id":"player-1","role":"player","occupant":"agent"}],"limits":{"seat_turn_timeout_seconds":1}}}),
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
    let checkpoint: Value =
        serde_json::from_slice(&std::fs::read(instance.join("checkpoint.json")).unwrap()).unwrap();
    let mut receipts: Vec<&Value> = checkpoint["receipts"]
        .as_object()
        .unwrap()
        .values()
        .filter(|receipt| receipt["request"]["method"] == "missed_turn")
        .collect();
    receipts.sort_by_key(|receipt| receipt["sequence"].as_u64());
    let reports: Vec<&Value> = receipts
        .iter()
        .map(|receipt| &receipt["request"]["params"])
        .collect();
    assert_eq!(
        reports,
        vec![
            &json!({"cause":"deadline","misses":1,"limit":2}),
            &json!({"cause":"deadline","misses":2,"limit":2}),
        ],
        "{checkpoint}"
    );
    assert_eq!(
        checkpoint["finished"]["summary"], "player-1 missed 2 turns",
        "{checkpoint}"
    );
    client.environment(&host, "leave", &json!({}));
    assert_eq!(client.request(&json!({"type":"shutdown"}))["success"], true);
    supervisor.0.wait().unwrap();
}
