mod support;
use pa_environments::EnvironmentManager;
use pa_types::environments::{EnvironmentLanguage, EnvironmentManifest, ExecutionPolicy};
use serde_json::json;
use support::LegacyDraft;

async fn save_relay(manager: &EnvironmentManager, name: &str) {
    let draft = manager
        .store()
        .create_draft(
            name,
            EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    let mut manifest: EnvironmentManifest =
        serde_json::from_slice(&std::fs::read(draft.join("environment.json")).unwrap()).unwrap();
    manifest.protocol_version = 2;
    manifest.roles = serde_json::from_value(
        json!([{"name":"player","min_seats":2,"max_seats":2,"occupants":["agent"]}]),
    )
    .unwrap();
    manifest.setup_schema = Some(json!({
        "type":"object",
        "properties":{
            "rounds":{"type":"integer","default":3,"minimum":1},
            "label":{"type":"string","default":"standard"}
        },
        "additionalProperties":false
    }));
    std::fs::write(
        draft.join("environment.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(
        draft.join("environment.py"),
        include_str!("../fixtures/relay/environment.py"),
    )
    .unwrap();
    std::fs::write(draft.join("checks.py"), "").unwrap();
    let report = manager
        .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
        .await
        .unwrap();
    assert!(report.passed, "{:?}", report.diagnostics);
    manager.save(draft.to_str().unwrap(), name).unwrap();
}

#[tokio::test]
async fn instances_expose_their_own_resolved_setup_and_keep_it_after_resume() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    save_relay(&manager, "relay").await;
    let defaulted = manager
        .enter("relay", None, ExecutionPolicy::default())
        .await
        .unwrap();
    let overridden = manager
        .enter_with_arguments(
            "relay",
            None,
            ExecutionPolicy::default(),
            json!({"rounds":50}),
        )
        .await
        .unwrap();
    assert_eq!(
        defaulted.setup_config().unwrap(),
        Some(json!({"rounds":3,"label":"standard"}))
    );
    assert_eq!(
        overridden.setup_config().unwrap(),
        Some(json!({"rounds":50,"label":"standard"}))
    );
    let id = overridden.instance.id.clone();
    let checkpoint = overridden.checkpoint().await;
    overridden.leave().await.unwrap();
    drop(overridden);
    let restored = manager.resume(&id).await.unwrap();
    assert_eq!(restored.checkpoint().await, checkpoint);
    assert_eq!(
        manager.store().instance_config(&id).unwrap(),
        Some(json!({"rounds":50,"label":"standard"}))
    );
    assert_eq!(
        manager
            .store()
            .instance_config(&defaulted.instance.id)
            .unwrap(),
        Some(json!({"rounds":3,"label":"standard"}))
    );
    restored.leave().await.unwrap();
    defaulted.leave().await.unwrap();
}

#[tokio::test]
async fn protocol_one_instances_have_no_participant_setup() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let draft = manager
        .store()
        .create_legacy_draft(
            "counter",
            EnvironmentLanguage::Python,
            /*python*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    std::fs::write(draft.join("checks.py"), "").unwrap();
    let report = manager
        .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
        .await
        .unwrap();
    assert!(report.passed, "{:?}", report.diagnostics);
    manager.save(draft.to_str().unwrap(), "counter").unwrap();
    let active = manager
        .enter("counter", None, ExecutionPolicy::default())
        .await
        .unwrap();
    assert_eq!(active.setup_config().unwrap(), None);
    active.leave().await.unwrap();
}
