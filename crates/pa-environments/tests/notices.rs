mod support;
use pa_environments::EnvironmentManager;
use pa_types::environments::{EnvironmentLanguage, ExecutionPolicy, NoticeTarget, SeatId};
use serde_json::json;
use support::LegacyDraft;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn notices_commit_once_replay_and_survive_resume_in_both_sdks() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    for language in [EnvironmentLanguage::Python, EnvironmentLanguage::TypeScript] {
        let draft = manager
            .store()
            .create_legacy_draft(
                "notices",
                language,
                /*python_execution*/ false,
                &ExecutionPolicy::default(),
            )
            .unwrap();
        let (file, source) = match language {
            EnvironmentLanguage::Python => ("environment.py", format!("from sdk import notify\n{}", include_str!("../fixtures/counter/environment.py").replace("        return self.value", "        notify(['agent'], 'counter changed')\n        notify(['human'], wake=False)\n        return self.value"))),
            EnvironmentLanguage::TypeScript => ("environment.ts", format!("import {{ notify }} from './sdk.ts';\n{}", include_str!("../fixtures/counter/environment.ts").replace("        return value;", "        notify(['agent'], 'counter changed');\n        notify(['human'], undefined, { wake: false });\n        return value;"))),
        };
        std::fs::write(draft.join(file), source).unwrap();
        let report = manager
            .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
            .await
            .unwrap();
        assert!(report.passed, "{:?}", report.diagnostics);
        manager.save(draft.to_str().unwrap(), "notices").unwrap();
        let active = manager
            .enter("notices", None, ExecutionPolicy::default())
            .await
            .unwrap();
        let human = SeatId::legacy_human();
        let agent = SeatId::legacy_agent();
        let commits = active.commits();
        for _ in 0..2 {
            assert_eq!(
                active
                    .invoke(
                        &human,
                        "increment",
                        json!({"amount":2}),
                        "same-id",
                        CancellationToken::new()
                    )
                    .await
                    .unwrap(),
                json!(2)
            );
        }
        let notices = active.notices_since(&agent, 0).await;
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].0, 2);
        assert_eq!(notices[0].1.to, NoticeTarget::Seats(vec![agent.clone()]));
        assert_eq!(*commits.borrow(), 2);
        let checkpoint = active.checkpoint().await;
        let id = active.instance.id.clone();
        active.leave().await.unwrap();
        drop(active);
        let restored = manager.resume(&id).await.unwrap();
        assert_eq!(restored.checkpoint().await, checkpoint);
        assert_eq!(restored.notices_since(&agent, 0).await, notices);
        assert_eq!(
            restored
                .invoke(
                    &human,
                    "increment",
                    json!({"amount":2}),
                    "same-id",
                    CancellationToken::new()
                )
                .await
                .unwrap(),
            json!(2)
        );
        assert!(restored.notices_since(&agent, 2).await.is_empty());
        restored.leave().await.unwrap();
    }
}

#[tokio::test]
async fn invalid_targets_after_mutation_require_reconciliation() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let draft = manager
        .store()
        .create_legacy_draft(
            "invalid notice",
            EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    let source = format!(
        "from sdk import notify\n{}",
        include_str!("../fixtures/counter/environment.py").replace(
            "        return self.value",
            "        notify(['missing-seat'])\n        return self.value"
        )
    );
    std::fs::write(draft.join("environment.py"), source).unwrap();
    assert!(
        manager
            .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
            .await
            .unwrap()
            .passed
    );
    manager.save(draft.to_str().unwrap(), "invalid").unwrap();
    let active = manager
        .enter("invalid", None, ExecutionPolicy::default())
        .await
        .unwrap();
    assert!(active
        .invoke(
            &SeatId::legacy_human(),
            "increment",
            json!({"amount":1}),
            "invalid",
            CancellationToken::new()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("recovery"));
    let checkpoint = active.checkpoint().await;
    assert_eq!(checkpoint.snapshot, json!({"value":1}));
    assert!(checkpoint.requires_recovery);
    assert!(checkpoint.receipts["invalid"].notices.is_empty());
    assert!(active.observe(&SeatId::legacy_human()).await.is_err());
    let id = active.instance.id.clone();
    let checkpoint_path = active.path().join("checkpoint.json");
    let before = std::fs::read(&checkpoint_path).unwrap();
    assert!(active.leave().await.is_err());
    drop(active);
    assert!(manager.resume(&id).await.is_err());
    assert_eq!(
        manager.inspect(&id, &SeatId::legacy_human()).await.unwrap(),
        json!({"value":1})
    );
    assert_eq!(
        std::fs::read(checkpoint_path).unwrap(),
        before,
        "inspection must not reconcile or commit state"
    );
}
