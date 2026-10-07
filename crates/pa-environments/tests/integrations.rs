mod support;
use pa_environments::{command_integrations, EnvironmentManager};
use pa_types::environments::{EnvironmentLanguage, ExecutionPolicy, SeatId};
use serde_json::{json, Value};
use std::fs;
use std::time::Duration;
use support::LegacyDraft;
use tokio_util::sync::CancellationToken;

#[tokio::test]
// This trace distinguishes denial before dispatch from uncertainty after dispatch.
#[allow(clippy::too_many_lines)]
async fn broker_binds_identity_and_uncertain_external_calls_are_never_replayed() {
    let root = tempfile::tempdir().unwrap();
    let receipt = root.path().join("external.json");
    let adapter = root.path().join("adapter.py");
    fs::write(&adapter, format!("import json,sys\nfrom pathlib import Path\npayload=json.load(sys.stdin)\nPath({:?}).write_text(json.dumps(payload))\nprint('invalid' if payload['arguments']['uncertain'] else '7')\n", receipt.to_str().unwrap())).unwrap();
    let config = root.path().join("integrations.json");
    fs::write(&config, json!({"record":{"program":"/usr/bin/python3","arguments":[adapter],"input_schema":{"type":"object","properties":{"uncertain":{"type":"boolean"}},"required":["uncertain"]},"output_schema":{"type":"integer"}}}).to_string()).unwrap();
    let broker = command_integrations(&config, Duration::from_secs(5)).unwrap();
    let manager = EnvironmentManager::new(root.path().join("store"), Some(broker)).unwrap();
    let policy = ExecutionPolicy {
        integrations: vec!["record".into()],
        ..ExecutionPolicy::default()
    };
    let draft = manager
        .store()
        .create_legacy_draft(
            "external operations",
            EnvironmentLanguage::Python,
            false,
            &policy,
        )
        .unwrap();
    let code = "from sdk import host_call\n".to_owned() + &fs::read_to_string(draft.join("environment.py")).unwrap().replace("self.value += arguments[\"amount\"]", "if arguments['amount'] >= 90:\n            return host_call('integration.call', {'name': 'record', 'arguments': {'uncertain': arguments['amount'] == 99}, 'participant': 'human', 'instance_id': 'forged'})\n        self.value += arguments[\"amount\"]");
    fs::write(draft.join("environment.py"), code).unwrap();
    assert!(
        manager
            .validate(draft.to_str().unwrap(), policy.clone())
            .await
            .unwrap()
            .passed
    );
    manager.save(draft.to_str().unwrap(), "external").unwrap();
    let denied = manager
        .enter("external", None, ExecutionPolicy::default())
        .await
        .unwrap();
    assert!(denied
        .invoke(
            &SeatId::legacy_agent(),
            "increment",
            json!({"amount":90}),
            "denied",
            CancellationToken::new()
        )
        .await
        .is_err());
    assert!(!receipt.exists());
    denied.leave().await.unwrap();
    let active = manager.enter("external", None, policy).await.unwrap();
    assert_eq!(
        active
            .invoke(
                &SeatId::legacy_agent(),
                "increment",
                json!({"amount":90}),
                "external-once",
                CancellationToken::new()
            )
            .await
            .unwrap(),
        json!(7)
    );
    let external: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    assert_eq!(external["participant"], "agent");
    assert_eq!(external["instance_id"], active.instance.id);
    fs::remove_file(&receipt).unwrap();
    assert_eq!(
        active
            .invoke(
                &SeatId::legacy_agent(),
                "increment",
                json!({"amount":90}),
                "external-once",
                CancellationToken::new()
            )
            .await
            .unwrap(),
        json!(7)
    );
    assert!(!receipt.exists(), "duplicate receipt must not redispatch");
    assert!(active
        .invoke(
            &SeatId::legacy_agent(),
            "increment",
            json!({"amount":99}),
            "external-uncertain",
            CancellationToken::new()
        )
        .await
        .is_err());
    assert!(
        receipt.exists(),
        "external side effect occurred before the missing receipt"
    );
    assert!(active
        .invoke(
            &SeatId::legacy_agent(),
            "increment",
            json!({"amount":1}),
            "subsequent",
            CancellationToken::new()
        )
        .await
        .is_err());
    let id = active.instance.id.clone();
    assert!(active.leave().await.is_err());
    drop(active);
    assert!(manager.resume(&id).await.is_err());
}
