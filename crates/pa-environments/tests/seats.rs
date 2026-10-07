use pa_environments::{EnvironmentManager, TableLog};
use pa_types::environments::{EnvironmentLanguage, EnvironmentManifest, ExecutionPolicy, SeatId};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
// Verify both language contracts through completion and restore with identical seats.
#[allow(clippy::too_many_lines)]
async fn multiple_seats_finish_and_replay_in_both_languages() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    for (language, role, file, source, action, arguments) in [
        (
            EnvironmentLanguage::Python,
            "player",
            "environment.py",
            include_str!("../fixtures/relay/environment.py"),
            "pass",
            json!({}),
        ),
        (
            EnvironmentLanguage::TypeScript,
            "bidder",
            "environment.ts",
            include_str!("../fixtures/sealed_bid/environment.ts"),
            "bid",
            json!({"amount":3}),
        ),
    ] {
        let draft = manager
            .store()
            .create_draft(
                "seats",
                language,
                /*python_execution*/ false,
                &ExecutionPolicy::default(),
            )
            .unwrap();
        let mut manifest: EnvironmentManifest =
            serde_json::from_slice(&std::fs::read(draft.join("environment.json")).unwrap())
                .unwrap();
        manifest.protocol_version = 2;
        manifest.roles = serde_json::from_value(
            json!([{"name":role,"min_seats":3,"max_seats":4,"occupants":["agent"]}]),
        )
        .unwrap();
        std::fs::write(
            draft.join("environment.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(draft.join(file), source).unwrap();
        let checks = match language {
            EnvironmentLanguage::Python => "checks.py",
            EnvironmentLanguage::TypeScript => "checks.ts",
        };
        std::fs::write(draft.join(checks), "").unwrap();
        let report = manager
            .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
            .await
            .unwrap();
        assert!(report.passed, "{:?}", report.diagnostics);
        manager.save(draft.to_str().unwrap(), role).unwrap();
        let active = manager
            .enter(role, None, ExecutionPolicy::default())
            .await
            .unwrap();
        let table = TableLog::open(active.path()).unwrap();
        for index in 1..=3 {
            let seat = SeatId::try_from(format!("{role}-{index}")).unwrap();
            active
                .invoke(
                    &seat,
                    action,
                    arguments.clone(),
                    &format!("turn-{index}"),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
        }
        let checkpoint = active.checkpoint().await;
        assert!(checkpoint.finished.is_some());
        table.materialize(&checkpoint).unwrap();
        let events = table.since(None, 0).unwrap();
        table.materialize(&checkpoint).unwrap();
        assert_eq!(table.since(None, 0).unwrap(), events);
        let seat = SeatId::try_from(format!("{role}-1")).unwrap();
        assert!(active
            .invoke(
                &seat,
                action,
                arguments.clone(),
                "new-turn",
                CancellationToken::new()
            )
            .await
            .is_err());
        // A committed operation remains replayable after completion and restart.
        active
            .invoke(
                &seat,
                action,
                arguments.clone(),
                "turn-1",
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let id = active.instance.id.clone();
        active.leave().await.unwrap();
        drop(active);
        let restored = manager.resume(&id).await.unwrap();
        assert_eq!(restored.checkpoint().await, checkpoint);
        restored
            .invoke(&seat, action, arguments, "turn-1", CancellationToken::new())
            .await
            .unwrap();
        restored.leave().await.unwrap();
    }
}

#[test]
fn interrupted_table_batch_recovers_each_notice_without_duplicates() {
    use pa_types::environments::{InstanceCheckpoint, OperationReceipt};
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let checkpoint: InstanceCheckpoint = serde_json::from_value(json!({"protocol_version":1,"snapshot":{},"receipts":{"a":{"request":{},"success":true,"result":{},"sequence":1,"notices":[{"to":"*","message":"first"},{"to":["agent"],"message":"second"}]}}})).unwrap();
    let table = TableLog::open(root.path()).unwrap();
    let mut partial = checkpoint.clone();
    partial.receipts.get_mut("a").unwrap().notices.truncate(1);
    table.materialize(&partial).unwrap();
    drop(table);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(root.path().join("table.jsonl"))
        .unwrap();
    file.write_all(b"{\"seq\":2,\"event_id\":").unwrap();
    file.sync_all().unwrap();
    let table = TableLog::open(root.path()).unwrap();
    table.materialize(&checkpoint).unwrap();
    let rows = table.since(Some(&SeatId::legacy_agent()), 0).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (row.seq, row.message.as_deref()))
            .collect::<Vec<_>>(),
        vec![(1, Some("first")), (2, Some("second"))]
    );
    assert_eq!(table.since(None, 0).unwrap(), vec![rows[0].clone()]);
    table.materialize(&checkpoint).unwrap();
    assert_eq!(table.since(Some(&SeatId::legacy_agent()), 0).unwrap(), rows);
    // Legacy receipts without sequence numbers have no pending deliveries.
    let legacy: OperationReceipt = serde_json::from_value(
        json!({"request":{"participant":"agent"},"success":true,"result":{}}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(legacy).unwrap(),
        json!({"request":{"participant":"agent"},"success":true,"result":{}})
    );
}

#[test]
fn roster_constraints_are_enforced_before_provisioning() {
    use pa_environments::{default_roster, validate_roster};
    use pa_types::environments::InstanceRoster;
    let manifest: EnvironmentManifest = serde_json::from_value(json!({"protocol_version":2,"name":"test","purpose":"test","language":"python","instructions":"test","roles":[{"name":"player","min_seats":2,"max_seats":3}]})).unwrap();
    let valid = default_roster(&manifest).unwrap();
    validate_roster(&manifest, &valid).unwrap();
    for seats in [
        json!([{"id":"a","role":"player","occupant":"agent"}]),
        json!([{"id":"a","role":"player","occupant":"agent"},{"id":"a","role":"player","occupant":"agent"}]),
        json!([{"id":"a","role":"player","occupant":"human"},{"id":"b","role":"player","occupant":"human"}]),
        json!([{"id":"a","role":"unknown","occupant":"agent"},{"id":"b","role":"player","occupant":"agent"}]),
    ] {
        let roster: InstanceRoster = serde_json::from_value(json!({"seats":seats})).unwrap();
        assert!(validate_roster(&manifest, &roster).is_err());
    }
    let mut limited = valid.clone();
    limited.limits.max_operations = 0;
    assert!(validate_roster(&manifest, &limited).is_err());
    let mut restricted = manifest;
    restricted.direct_messages = Some(false);
    assert!(validate_roster(&restricted, &valid).is_err());
    restricted.roles[0].occupants = vec![pa_types::environments::OccupantKind::Agent];
    assert!(validate_roster(&restricted, &valid).is_err());
    for invalid in ["", "with/slash", "space name", "é"] {
        assert!(SeatId::try_from(invalid.to_owned()).is_err());
    }
}

#[test]
fn old_manifest_roundtrips_without_new_fields() {
    let bytes = r#"{"protocol_version":1,"name":"cards","purpose":"game","language":"python","instructions":"play","python_execution":false,"recommended_backend":"local"}"#;
    let manifest: EnvironmentManifest = serde_json::from_str(bytes).unwrap();
    assert_eq!(serde_json::to_string(&manifest).unwrap(), bytes);
}
