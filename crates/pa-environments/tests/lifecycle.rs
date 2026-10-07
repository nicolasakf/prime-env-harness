mod support;
use pa_environments::EnvironmentManager;
use pa_types::environments::{
    EnvironmentLanguage, ExecutionPolicy, OperationReceipt, RecoveryRequest, SeatId,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use support::LegacyDraft;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn setup_is_validated_defaulted_recorded_and_not_reinitialized_on_resume() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let policy = ExecutionPolicy::default();
    let draft = manager
        .store()
        .create_legacy_draft(
            "setup",
            EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &policy,
        )
        .unwrap();
    let file = draft.join("environment.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    manifest["setup_schema"] = json!({"type":"object","properties":{"start":{"type":"integer","minimum":1,"default":4},"enabled":{"type":"boolean","default":false}},"required":["start"],"additionalProperties":false});
    fs::write(&file, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let code = include_str!("../fixtures/counter/environment.py").replace(
        "self.value = 0\n        return {}",
        "self.value = params.get('start', 0)\n        return {}",
    );
    fs::write(draft.join("environment.py"), code).unwrap();
    let selector = draft.to_str().unwrap();
    manager.select_policy(selector, &policy).unwrap();
    let report = manager.validate(selector, policy.clone()).await.unwrap();
    assert!(report.passed, "{:?}", report.diagnostics);
    manager.save(selector, "setup").unwrap();
    assert!(manager
        .enter_with_arguments("setup", None, policy.clone(), json!({"start":0}))
        .await
        .is_err());
    assert_eq!(manager.store().list_instances().unwrap(), vec![]);
    let active = manager
        .enter_with_arguments("setup", None, policy, json!({"start":9}))
        .await
        .unwrap();
    assert_eq!(
        active.observe(&SeatId::legacy_human()).await.unwrap(),
        json!({"value":9})
    );
    let checkpoint: serde_json::Value =
        serde_json::from_slice(&fs::read(active.path().join("checkpoint.json")).unwrap()).unwrap();
    assert_eq!(
        checkpoint["receipts"]["initialization"]["request"]["params"],
        json!({"start":9,"enabled":false})
    );
    let id = active.instance.id.clone();
    active
        .invoke(
            &SeatId::legacy_human(),
            "increment",
            json!({"amount":2}),
            "increment",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    active.leave().await.unwrap();
    drop(active);
    let resumed = manager.resume(&id).await.unwrap();
    assert_eq!(
        resumed.observe(&SeatId::legacy_human()).await.unwrap(),
        json!({"value":11})
    );
    resumed.leave().await.unwrap();
}

#[test]
fn instance_listing_orders_by_creation_time_newest_first_then_id() {
    let root = tempfile::tempdir().unwrap();
    let store = pa_environments::EnvironmentStore::new(root.path()).unwrap();
    assert_eq!(store.list_instances().unwrap(), vec![]);
    fs::create_dir(root.path().join("instances/session-artifacts")).unwrap();
    assert_eq!(store.list_instances().unwrap(), vec![]);
    let mut expected = Vec::new();
    for (id, definition_id, created_at) in [
        ("session-b", "cards", 1_000),
        ("session-a", "counter", 3_000),
        ("session-c", "cards", 3_000),
    ] {
        let instance = pa_types::environments::Instance {
            id: id.into(),
            definition_id: definition_id.into(),
            version: "pinned-version".into(),
            policy: ExecutionPolicy::default(),
            created_at: Some(created_at),
            roster: None,
        };
        let path = store.instance_path(id).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(
            path.join("instance.json"),
            serde_json::to_vec(&instance).unwrap(),
        )
        .unwrap();
        expected.push(instance);
    }
    expected.rotate_left(1);
    assert_eq!(store.list_instances().unwrap(), expected);
}

#[test]
fn legacy_instance_uses_metadata_creation_time_without_rewriting_it() {
    let root = tempfile::tempdir().unwrap();
    let store = pa_environments::EnvironmentStore::new(root.path()).unwrap();
    let path = store.instance_path("legacy").unwrap();
    fs::create_dir(&path).unwrap();
    let file = path.join("instance.json");
    let saved = json!({"id":"legacy","definition_id":"cards","version":"pinned","policy":{"backend":"local"}}).to_string();
    fs::write(&file, &saved).unwrap();
    let metadata = fs::metadata(&file).unwrap();
    let created = metadata.created().or_else(|_| metadata.modified()).unwrap();
    let expected = pa_types::environments::Instance {
        id: "legacy".into(),
        definition_id: "cards".into(),
        version: "pinned".into(),
        policy: ExecutionPolicy::default(),
        roster: None,
        created_at: Some(
            created
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .try_into()
                .unwrap(),
        ),
    };
    assert_eq!(store.list_instances().unwrap(), vec![expected]);
    assert_eq!(fs::read_to_string(file).unwrap(), saved);
}

#[tokio::test]
// This single trace verifies that the same identities and version survive a full lifecycle.
#[allow(clippy::too_many_lines)]
async fn independent_instances_deduplicate_and_resume_pinned_versions() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let policy = ExecutionPolicy {
        python_execution: true,
        ..ExecutionPolicy::default()
    };
    for language in [EnvironmentLanguage::Python, EnvironmentLanguage::TypeScript] {
        let draft = manager
            .store()
            .create_legacy_draft("counter", language, true, &ExecutionPolicy::default())
            .unwrap();
        let draft_id = draft.to_str().unwrap();
        let report = manager.validate(draft_id, policy.clone()).await.unwrap();
        assert!(report.passed, "{:?}", report.diagnostics);
        let definition = manager
            .save(
                draft_id,
                match language {
                    EnvironmentLanguage::Python => "python",
                    EnvironmentLanguage::TypeScript => "typescript",
                },
            )
            .unwrap();
        let first = manager
            .enter(&definition.id, None, policy.clone())
            .await
            .unwrap();
        let second = manager
            .enter(&definition.id, None, policy.clone())
            .await
            .unwrap();
        assert!(
            manager.resume(&first.instance.id).await.is_err(),
            "concurrent ownership must fail"
        );
        for _ in 0..2 {
            assert_eq!(
                first
                    .invoke(
                        &SeatId::legacy_human(),
                        "increment",
                        json!({"amount":2}),
                        "same-operation",
                        CancellationToken::new()
                    )
                    .await
                    .unwrap(),
                json!(2)
            );
        }
        assert!(first
            .invoke(
                &SeatId::legacy_human(),
                "increment",
                json!({"amount":3}),
                "same-operation",
                CancellationToken::new()
            )
            .await
            .is_err());
        assert!(first
            .invoke(
                &SeatId::legacy_human(),
                "increment",
                json!({"amount":0}),
                "illegal",
                CancellationToken::new()
            )
            .await
            .is_err());
        assert_eq!(
            second.observe(&SeatId::legacy_agent()).await.unwrap(),
            json!({"value":0})
        );
        first.execute_python("remembered = 17\nassert env.invoke('increment', {'amount': 1}) == 3\nassert env.observe()['value'] == 3", "python-action", CancellationToken::new()).await.unwrap();
        assert!(first
            .execute_python(
                "env.integration('ungranted', {})",
                "denied-broker",
                CancellationToken::new()
            )
            .await
            .is_err());
        let id = first.instance.id.clone();
        first.leave().await.unwrap();
        drop(first);
        let resumed = manager.resume(&id).await.unwrap();
        assert_eq!(resumed.instance.version, definition.version);
        assert_eq!(
            resumed.observe(&SeatId::legacy_human()).await.unwrap(),
            json!({"value":3})
        );
        resumed
            .execute_python(
                "assert remembered == 17",
                "restored-python",
                CancellationToken::new(),
            )
            .await
            .unwrap();
        resumed.leave().await.unwrap();
        second.leave().await.unwrap();
        fs::write(draft.join("resources.txt"), "new version").unwrap();
        assert!(
            manager.save(draft_id, &definition.id).is_err(),
            "editing invalidates validation"
        );
        assert!(
            manager
                .validate(draft_id, policy.clone())
                .await
                .unwrap()
                .passed
        );
        let newer = manager.save(draft_id, &definition.id).unwrap();
        assert_ne!(definition.version, newer.version);
        drop(resumed);
        let pinned = manager.resume(&id).await.unwrap();
        assert_eq!(pinned.instance.version, definition.version);
        pinned.leave().await.unwrap();
    }
}

#[tokio::test]
async fn uncertain_crash_blocks_resume_until_explicit_reconciliation() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let draft = manager
        .store()
        .create_legacy_draft(
            "crash recovery",
            EnvironmentLanguage::Python,
            false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    let source = "import os\n".to_owned() + &fs::read_to_string(draft.join("environment.py")).unwrap().replace("self.value += arguments[\"amount\"]", "if arguments['amount'] == 99:\n            os._exit(1)\n        self.value += arguments[\"amount\"]");
    fs::write(draft.join("environment.py"), source).unwrap();
    assert!(
        manager
            .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
            .await
            .unwrap()
            .passed
    );
    manager.save(draft.to_str().unwrap(), "crash").unwrap();
    let active = manager
        .enter("crash", None, ExecutionPolicy::default())
        .await
        .unwrap();
    let id = active.instance.id.clone();
    assert!(active
        .invoke(
            &SeatId::legacy_agent(),
            "increment",
            json!({"amount":99}),
            "uncertain",
            CancellationToken::new()
        )
        .await
        .is_err());
    assert!(active.leave().await.is_err());
    drop(active);
    assert!(manager.resume(&id).await.is_err());
    let intent = serde_json::from_slice(
        &fs::read(
            manager
                .store()
                .instance_path(&id)
                .unwrap()
                .join("operations/uncertain.json"),
        )
        .unwrap(),
    )
    .unwrap();
    manager.recover(&id, &RecoveryRequest { snapshot:json!({"value":0}), receipts:BTreeMap::from([("uncertain".into(),OperationReceipt {request:intent,success:false,result:json!("verified process exited before mutation"),..OperationReceipt::default()})]), python_receipts:BTreeMap::new(),seat_python_receipts:BTreeMap::new(),reset_python_seats:Vec::new(),reset_python:false,evidence:"No external call; inspected original checkpoint and crash trigger before increment.".into() }).unwrap();
    let resumed = manager.resume(&id).await.unwrap();
    assert_eq!(
        resumed.observe(&SeatId::legacy_human()).await.unwrap(),
        json!({"value":0})
    );
    assert!(resumed
        .invoke(
            &SeatId::legacy_agent(),
            "increment",
            json!({"amount":99}),
            "uncertain",
            CancellationToken::new()
        )
        .await
        .is_err());
    resumed.leave().await.unwrap();
}

#[tokio::test]
async fn explicit_recovery_can_reconcile_initialization_without_a_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let policy = ExecutionPolicy::default();
    let draft = manager
        .store()
        .create_legacy_draft(
            "initialization recovery",
            EnvironmentLanguage::Python,
            false,
            &policy,
        )
        .unwrap();
    let source = "import os\nfrom pathlib import Path\n".to_owned()
        + &fs::read_to_string(draft.join("environment.py")).unwrap().replace(
            "def initialize(self, params):\n        self.value = 0",
            "def initialize(self, params):\n        if Path.cwd().name == 'workspace':\n            os._exit(1)\n        self.value = 0",
        );
    fs::write(draft.join("environment.py"), source).unwrap();
    assert!(
        manager
            .validate(draft.to_str().unwrap(), policy.clone())
            .await
            .unwrap()
            .passed
    );
    manager
        .save(draft.to_str().unwrap(), "initialization")
        .unwrap();
    assert!(manager.enter("initialization", None, policy).await.is_err());
    let instance = fs::read_dir(root.path().join("instances"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(!instance.join("checkpoint.json").exists());
    let id = instance.file_name().unwrap().to_str().unwrap();
    assert!(manager.resume(id).await.is_err());
    let intent =
        serde_json::from_slice(&fs::read(instance.join("operations/initialization.json")).unwrap())
            .unwrap();
    manager
        .recover(
            id,
            &RecoveryRequest {
                snapshot: json!({"value": 0}),
                receipts: BTreeMap::from([(
                    "initialization".into(),
                    OperationReceipt {
                        request: intent,
                        success: false,
                        result: json!("inspected initialization crashed before mutating state"),
                        ..OperationReceipt::default()
                    },
                )]),
                python_receipts: BTreeMap::new(),
                seat_python_receipts: BTreeMap::new(),
                reset_python_seats: Vec::new(),
                reset_python: false,
                evidence: "Inspected crash trigger and confirmed no external operations occurred."
                    .into(),
            },
        )
        .unwrap();
    // Initialization would crash again if resume replayed it.
    let resumed = manager.resume(id).await.unwrap();
    assert_eq!(
        resumed.observe(&SeatId::legacy_human()).await.unwrap(),
        json!({"value": 0})
    );
    resumed.leave().await.unwrap();
}
