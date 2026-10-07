mod support;
use pa_environments::EnvironmentManager;
use pa_types::environments::{EnvironmentLanguage, ExecutionBackend, ExecutionPolicy, SeatId};
use serde_json::json;
use std::fs;
use support::LegacyDraft;
use tokio_util::sync::CancellationToken;

#[tokio::test]
// Compare both domains through the identical public lifecycle in each backend.
#[allow(clippy::too_many_lines)]
async fn neutral_contract_enforces_cards_and_processes_documents() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let backends = if cfg!(feature = "docker-integration") {
        vec![ExecutionBackend::Local, ExecutionBackend::Docker]
    } else {
        vec![ExecutionBackend::Local]
    };
    for backend in backends {
        let policy = ExecutionPolicy {
            backend,
            ..ExecutionPolicy::default()
        };
        let draft = manager
            .store()
            .create_legacy_draft(
                "private card hands",
                EnvironmentLanguage::Python,
                false,
                &policy,
            )
            .unwrap();
        fs::write(
            draft.join("environment.py"),
            include_str!("../fixtures/cards/environment.py"),
        )
        .unwrap();
        fs::write(
            draft.join("checks.py"),
            include_str!("../fixtures/cards/checks.py"),
        )
        .unwrap();
        let report = manager
            .validate(draft.to_str().unwrap(), policy.clone())
            .await
            .unwrap();
        assert!(report.passed, "{:?}", report.diagnostics);
        let id = format!("cards-{backend:?}");
        manager.save(draft.to_str().unwrap(), &id).unwrap();
        let cards = manager.enter(&id, None, policy.clone()).await.unwrap();
        assert_eq!(
            cards.observe(&SeatId::legacy_agent()).await.unwrap(),
            json!({"hand":["ace_of_spades"],"opponent_count":1,"played":["five_of_hearts"]})
        );
        assert!(cards
            .input("private: hidden note", "private-note")
            .await
            .unwrap()
            .agent_message
            .is_none());
        assert!(cards
            .invoke(
                &SeatId::legacy_agent(),
                "play_card",
                json!({"card_id":"seven_of_hearts"}),
                "wrong-owner",
                CancellationToken::new()
            )
            .await
            .is_err());
        assert!(cards
            .invoke(
                &SeatId::legacy_agent(),
                "play_card",
                json!({"card_id":"ace_of_spades"}),
                "illegal-card",
                CancellationToken::new()
            )
            .await
            .is_err());
        assert!(cards
            .invoke(
                &SeatId::legacy_agent(),
                "play_card",
                json!({"card_id":"seven_of_hearts","participant":"human"}),
                "forged-identity",
                CancellationToken::new()
            )
            .await
            .is_err());
        cards
            .invoke(
                &SeatId::legacy_human(),
                "play_card",
                json!({"card_id":"seven_of_hearts"}),
                "legal",
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let instance_id = cards.instance.id.clone();
        cards.leave().await.unwrap();
        let restored = manager.resume(&instance_id).await.unwrap();
        assert_eq!(
            restored.observe(&SeatId::legacy_human()).await.unwrap(),
            json!({"hand":[],"opponent_count":1,"played":["five_of_hearts","seven_of_hearts"]})
        );
        restored.leave().await.unwrap();

        let draft = manager
            .store()
            .create_legacy_draft(
                "document categorization",
                EnvironmentLanguage::TypeScript,
                false,
                &policy,
            )
            .unwrap();
        fs::write(
            draft.join("environment.ts"),
            include_str!("../fixtures/documents/environment.ts"),
        )
        .unwrap();
        fs::write(
            draft.join("checks.ts"),
            include_str!("../fixtures/documents/checks.ts"),
        )
        .unwrap();
        let report = manager
            .validate(draft.to_str().unwrap(), policy.clone())
            .await
            .unwrap();
        assert!(report.passed, "{:?}", report.diagnostics);
        let id = format!("documents-{backend:?}");
        manager.save(draft.to_str().unwrap(), &id).unwrap();
        let documents = manager.enter(&id, None, policy).await.unwrap();
        assert!(documents
            .invoke(
                &SeatId::legacy_agent(),
                "submit_text",
                json!({"text":"forged"}),
                "forged-submit",
                CancellationToken::new()
            )
            .await
            .is_err());
        let id = documents
            .invoke(
                &SeatId::legacy_human(),
                "submit_text",
                json!({"text":"A research document"}),
                "submit",
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            documents
                .invoke(
                    &SeatId::legacy_agent(),
                    "categorize",
                    json!({"id":id,"category":"research"}),
                    "categorize",
                    CancellationToken::new()
                )
                .await
                .unwrap(),
            json!(3)
        );
        assert_eq!(
            documents.observe(&SeatId::legacy_human()).await.unwrap(),
            json!({"documents":[{"id":"document-1","words":3,"category":"research"}]})
        );
        documents.leave().await.unwrap();
    }
}
