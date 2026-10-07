use pa_environments::EnvironmentManager;
use pa_types::environments::{
    EnvironmentLanguage, ExecutionBackend, ExecutionPolicy, RecoveryRequest, SeatId,
};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn namespaces_and_uncertain_execution_are_scoped_to_one_seat() {
    seat_python_scope(ExecutionBackend::Local).await;
}

#[cfg(feature = "docker-integration")]
#[tokio::test]
async fn docker_seat_namespaces_and_recovery_are_isolated() {
    seat_python_scope(ExecutionBackend::Docker).await;
}

// Keep namespace isolation, interruption and recovery against the same instance.
#[allow(clippy::too_many_lines)]
async fn seat_python_scope(backend: ExecutionBackend) {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let policy = ExecutionPolicy {
        backend,
        python_execution: true,
        ..ExecutionPolicy::default()
    };
    let draft = manager
        .store()
        .create_draft(
            "seat Python",
            EnvironmentLanguage::Python,
            /*python_execution*/ true,
            &policy,
        )
        .unwrap();
    let manifest_path = draft.join("environment.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["roles"][1]["min_seats"] = json!(2);
    manifest["roles"][1]["max_seats"] = json!(2);
    std::fs::write(manifest_path, manifest.to_string()).unwrap();
    let report = manager
        .validate(draft.to_str().unwrap(), policy.clone())
        .await
        .unwrap();
    assert!(report.passed, "{report:?}");
    manager
        .save(draft.to_str().unwrap(), "python-seats")
        .unwrap();
    let active = manager.enter("python-seats", None, policy).await.unwrap();
    let one = SeatId::try_from("agent-1".to_owned()).unwrap();
    let two = SeatId::try_from("agent-2".to_owned()).unwrap();
    active
        .execute_python_seat(
            &one,
            "private_value = 7
from pathlib import Path
Path('private-file').write_text('private')",
            "one",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let result = active.execute_python_seat(&two, "assert 'private_value' not in globals()\nassert env.invoke('increment', {'amount': 2}) == 2", "two", CancellationToken::new()).await.unwrap();
    assert!(result["execution"]["error"].is_null(), "{result}");
    let signal = CancellationToken::new();
    let code = "from pathlib import Path\nimport time\nPath('ready').write_text('ready')\nwhile True:\n    time.sleep(1)";
    let pending = {
        let active = active.clone();
        let signal = signal.clone();
        let one = one.clone();
        tokio::spawn(async move {
            active
                .execute_python_seat(&one, code, "uncertain", signal)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while !active
            .path()
            .join("seats/agent-1/participant/ready")
            .exists()
        {
            assert!(!pending.is_finished());
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Another kernel runs while the first kernel is occupied.
    active
        .execute_python_seat(
            &two,
            "second_value = 9",
            "parallel",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    signal.cancel();
    assert!(pending.await.unwrap().is_err());
    let id = active.instance.id.clone();
    let snapshot = active.checkpoint().await.snapshot;
    active.leave().await.unwrap();
    drop(active);
    let resumed = manager.resume(&id).await.unwrap();
    assert!(resumed
        .execute_python_seat(&one, "1", "blocked", CancellationToken::new())
        .await
        .is_err());
    resumed
        .execute_python_seat(
            &two,
            "assert second_value == 9",
            "after-resume",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    resumed.leave().await.unwrap();
    drop(resumed);
    let recovery: RecoveryRequest = serde_json::from_value(json!({"snapshot":snapshot,"receipts":{},"python_receipts":{},"reset_python":false,"evidence":"Inspected seat 1's readiness marker; the cell mutated only its namespace. Reset that namespace.","reset_python_seats":["agent-1"],"seat_python_receipts":{"agent-1":{"uncertain":{"request":{"code":code},"success":false,"result":"cancelled"}}}})).unwrap();
    let receipt_path = root
        .path()
        .join("instances")
        .join(&id)
        .join("seats/agent-1/python-receipts.json");
    let before = std::fs::read(&receipt_path).unwrap();
    let mut invalid = recovery.clone();
    invalid
        .reset_python_seats
        .push(SeatId::try_from("missing-seat".to_owned()).unwrap());
    assert!(manager.recover(&id, &invalid).is_err());
    assert_eq!(
        std::fs::read(&receipt_path).unwrap(),
        before,
        "failed validation must not reset an earlier seat"
    );
    manager.recover(&id, &recovery).unwrap();
    let restored = manager.resume(&id).await.unwrap();
    restored
        .execute_python_seat(
            &one,
            "assert 'private_value' not in globals()",
            "reset",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    restored
        .execute_python_seat(
            &two,
            "assert second_value == 9",
            "still-persisted",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    restored.leave().await.unwrap();
}
