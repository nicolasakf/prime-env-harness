//! Legacy definitions run through a public host and a private seat transcript.
#![cfg(unix)]
#[path = "support/environment_daemon.rs"]
mod support;
use serde_json::{json, Value};

#[test]
// Keep creation, concurrent instances and restored history in one legacy workflow.
#[allow(clippy::too_many_lines)]
fn create_validate_save_enter_act_leave_resume_with_readiness_events() {
    let root = tempfile::tempdir().unwrap();
    let agent = root.path().join("agent");
    let script = root.path().join("faux.json");
    let (mut supervisor, mut client) = support::start(
        root.path(),
        &json!({"engine":"faux","responses":[{"content":[{"type":"toolCall","id":"observe-private","name":"observe_environment","arguments":{}}]},"PRIVATE-ASSISTANT-REPLY"]}),
    );
    let response = client.request(&json!({"type":"create","config":{"cwd":root.path(),"script":script,"sessionDir":agent.join("sessions")},"telemetryDisabled":true}));
    assert_eq!(response["success"], true, "{response}");
    let builder = response["data"]["activeSessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let attach = client.request(&json!({"type":"attach","activeSessionId":builder}));
    assert_eq!(attach["success"], true);
    let draft = client.environment(
        &builder,
        "create",
        &json!({"description":"private card hands"}),
    )["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let manifest_path = std::path::Path::new(&draft).join("environment.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["protocol_version"] = json!(1);
    manifest.as_object_mut().unwrap().remove("roles");
    std::fs::write(manifest_path, manifest.to_string()).unwrap();
    std::fs::write(
        std::path::Path::new(&draft).join("environment.py"),
        include_str!("../../pa-environments/fixtures/cards/environment.py"),
    )
    .unwrap();
    std::fs::write(
        std::path::Path::new(&draft).join("checks.py"),
        include_str!("../../pa-environments/fixtures/cards/checks.py"),
    )
    .unwrap();
    let denied = client.request(&json!({"type":"env","activeSessionId":builder,"operation":"validate","parameters":{"draft":draft}}));
    assert_eq!(
        denied["success"], false,
        "execution needs explicit selection"
    );
    client.environment(
        &builder,
        "policy",
        &json!({"draft":draft,"policy":{"backend":"local"}}),
    );
    let validation = client.environment(&builder, "validate", &json!({"draft":draft}));
    assert_eq!(validation["passed"], true, "{validation}");
    let definition = client.environment(&builder, "save", &json!({"draft":draft,"id":"cards"}));
    let entered = client.environment(
        &builder,
        "enter",
        &json!({"id":"cards","policy":{"backend":"local"}}),
    );
    let participant = entered["session"]["activeSessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let instance = entered["instance_id"].as_str().unwrap().to_owned();
    let instances = client.environment(&builder, "instances", &json!({}));
    let store = pa_environments::EnvironmentStore::new(agent.join("environments")).unwrap();
    assert!(store.instance(&instance).unwrap().created_at.is_some());
    assert_eq!(
        instances,
        json!({"instances":[store.instance(&instance).unwrap()]})
    );
    assert!(client
        .events
        .iter()
        .any(|event| event["type"] == "environment_update"
            && event["operation"] == "enter"
            && event["phase"] == "ready"));
    assert_eq!(
        client.request(&json!({"type":"attach","activeSessionId":participant}))["success"],
        true
    );
    // Enter starts a fresh game even while another participant is resident.
    let second = client.environment(
        &builder,
        "enter",
        &json!({"id":"cards","policy":{"backend":"local"}}),
    );
    let second_participant = second["session"]["activeSessionId"].as_str().unwrap();
    assert_ne!(second["instance_id"], entered["instance_id"]);
    assert_ne!(
        second["session"]["sessionName"],
        entered["session"]["sessionName"]
    );
    let second_action = client.environment(second_participant, "act", &json!({"action":"play_card","arguments":{"card_id":"seven_of_hearts"},"operation_id":"second-play"}));
    assert_eq!(second_action["observation"]["hand"], json!([]));
    let first_state = client.environment(&participant, "status", &json!({}));
    assert_eq!(
        first_state["observation"]["hand"],
        json!(["seven_of_hearts"])
    );
    client.environment(second_participant, "leave", &json!({}));
    let state =
        client.request(&json!({"type":"get_connection_state","activeSessionId":participant}));
    assert_eq!(
        state["data"]["environmentProfile"]["version"], definition["version"],
        "{state}"
    );
    for command in [
        "bash",
        "fork",
        "navigate_tree",
        "goal",
        "create_session",
        "get_mcp_connections",
    ] {
        assert_eq!(
            client.request(&json!({"type":command,"activeSessionId":participant}))["success"],
            false,
            "{command}"
        );
    }
    let catalog =
        client.request(&json!({"type":"get_model_catalog","activeSessionId":participant}));
    assert_eq!(
        catalog["success"], false,
        "a model-less host has no model catalog: {catalog}"
    );
    let private = client.request(&json!({"type":"prompt_and_wait","activeSessionId":participant,"message":"private: PRIVATE-HUMAN-NOTE"}));
    assert_eq!(private["success"], true, "{private}");
    assert_eq!(private["data"]["forwarded"], false);
    assert_eq!(
        client.request(&json!({"type":"compact","activeSessionId":participant}))["success"],
        false
    );
    let action = client.environment(&participant,"act",&json!({"action":"play_card","arguments":{"card_id":"seven_of_hearts"},"operation_id":"play-once"}));
    assert_eq!(action["observation"]["hand"], json!([]));
    client.wait_event(|event| {
        event["type"] == "environment_seat_state"
            && event["run"]["settled"]["agent"].as_u64().is_some()
    });
    let messages = client.request(&json!({"type":"get_messages","activeSessionId":participant}));
    for secret in [
        "PRIVATE-HUMAN-NOTE",
        "ace_of_spades",
        "PRIVATE-ASSISTANT-REPLY",
    ] {
        assert!(!messages.to_string().contains(secret));
        assert!(!client
            .events
            .iter()
            .any(|event| event.to_string().contains(secret)));
    }
    let transcript = std::fs::read_to_string(
        agent
            .join("environments/instances")
            .join(&instance)
            .join("conversation.jsonl"),
    )
    .unwrap();
    assert!(
        transcript.contains("ace_of_spades"),
        "the model transcript must retain the agent hand"
    );
    let attached = client.request(
        &json!({"type":"attach","activeSessionId":participant,"capabilities":["chunked_snapshot"]}),
    );
    assert_eq!(attached["success"], true);
    assert!(!attached.to_string().contains("ace_of_spades"));
    let replay = client.environment(&participant,"act",&json!({"action":"play_card","arguments":{"card_id":"seven_of_hearts"},"operation_id":"play-once"}));
    assert_eq!(replay, action);
    let left = client.environment(&participant, "leave", &json!({}));
    assert_eq!(left, json!({"builder_session_id":builder}));
    // A stale participant tab or a retried request must still return to the
    // builder after the first leave has closed the session and runtime.
    assert_eq!(client.environment(&participant, "leave", &json!({})), left);
    let resumed = client.environment(&builder, "resume", &json!({"id":instance}));
    let resumed_id = resumed["session"]["activeSessionId"].as_str().unwrap();
    let restored = client.environment(resumed_id, "status", &json!({}));
    assert_eq!(
        restored["instance"],
        json!(store.instance(&instance).unwrap())
    );
    assert_eq!(restored["observation"]["hand"], json!([]));
    assert_eq!(restored["instance"]["version"], definition["version"]);
    let messages = client.request(&json!({"type":"get_messages","activeSessionId":resumed_id}));
    assert!(!messages.to_string().contains("PRIVATE-HUMAN-NOTE"));
    assert!(!messages.to_string().contains("ace_of_spades"));
    client.environment(resumed_id, "leave", &json!({}));
    assert_eq!(client.request(&json!({"type":"shutdown"}))["success"], true);
    supervisor.0.wait().unwrap();
}
