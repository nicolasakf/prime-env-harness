use pa_environments::EnvironmentManager;
use pa_types::environments::{AgentContext, EnvironmentManifest, ExecutionPolicy, SeatId};

/// The card market is the reference stateless environment: validation runs its
/// behavioral checks, including privacy and bounded history in `agent_view`.
#[tokio::test]
async fn card_market_validates_and_serves_each_seat_its_view() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let draft = manager
        .store()
        .create_draft(
            "card market",
            pa_types::environments::EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    std::fs::write(
        draft.join("environment.json"),
        include_str!("../fixtures/card_market/environment.json"),
    )
    .unwrap();
    std::fs::write(
        draft.join("environment.py"),
        include_str!("../fixtures/card_market/environment.py"),
    )
    .unwrap();
    std::fs::write(
        draft.join("checks.py"),
        include_str!("../fixtures/card_market/checks.py"),
    )
    .unwrap();
    let manifest: EnvironmentManifest =
        serde_json::from_slice(&std::fs::read(draft.join("environment.json")).unwrap()).unwrap();
    assert_eq!(manifest.agent_context, AgentContext::Stateless);
    let report = manager
        .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
        .await
        .unwrap();
    assert!(report.passed, "{:?}", report.diagnostics);
    manager
        .save(draft.to_str().unwrap(), "card-market")
        .unwrap();
    let active = manager
        .enter("card-market", None, ExecutionPolicy::default())
        .await
        .unwrap();
    let seat = SeatId::try_from("player-1".to_owned()).unwrap();
    let view = active.agent_view(&seat).await.unwrap();
    assert_eq!(view["you"], "player-1");
    assert_eq!(
        view["available_actions"],
        serde_json::json!(["open_market"])
    );
    // Chat goes through the host, which fixes the result shape; only the active seat may speak.
    let reply = active.input_seat(&seat, "hello", "chat-1").await.unwrap();
    assert_eq!(reply, serde_json::json!({"observation": {"posted": 1}}));
    let other = SeatId::try_from("player-2".to_owned()).unwrap();
    let refusal = active
        .input_seat(&other, "hello", "chat-2")
        .await
        .unwrap_err();
    assert!(
        refusal.to_string().contains("only open on your own turn"),
        "{refusal:#}"
    );
    active.leave().await.unwrap();
}
