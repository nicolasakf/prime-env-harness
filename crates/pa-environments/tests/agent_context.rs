use pa_environments::EnvironmentManager;
use pa_types::environments::{
    EnvironmentLanguage, EnvironmentManifest, ExecutionPolicy, SeatId, ValidationReport,
    MAX_AGENT_VIEW_BYTES,
};
use serde_json::{json, Value};

/// Validate a stateless two-seat draft whose environment source is `source`.
async fn validate_stateless(
    manager: &EnvironmentManager,
    language: EnvironmentLanguage,
    role: &str,
    protocol_version: u32,
    source: &str,
) -> (String, ValidationReport) {
    let draft = manager
        .store()
        .create_draft(
            "stateless",
            language,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    let mut manifest: EnvironmentManifest =
        serde_json::from_slice(&std::fs::read(draft.join("environment.json")).unwrap()).unwrap();
    manifest.protocol_version = protocol_version;
    manifest.roles = serde_json::from_value(
        json!([{"name":role,"min_seats":3,"max_seats":3,"occupants":["agent"]}]),
    )
    .unwrap();
    manifest.agent_context = pa_types::environments::AgentContext::Stateless;
    std::fs::write(
        draft.join("environment.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let (environment, checks) = match language {
        EnvironmentLanguage::Python => ("environment.py", "checks.py"),
        EnvironmentLanguage::TypeScript => ("environment.ts", "checks.ts"),
    };
    std::fs::write(draft.join(environment), source).unwrap();
    std::fs::write(draft.join(checks), "").unwrap();
    let selector = draft.to_str().unwrap().to_owned();
    let report = manager
        .validate(&selector, ExecutionPolicy::default())
        .await
        .unwrap();
    (selector, report)
}

#[tokio::test]
async fn agent_view_defaults_to_observe_and_can_be_overridden_in_both_languages() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let relay = include_str!("../fixtures/relay/environment.py");
    let sealed = include_str!("../fixtures/sealed_bid/environment.ts");
    let cases = [
        (
            EnvironmentLanguage::Python,
            "player",
            relay.replace(
                "    def handle_input",
                "    def agent_view(self, seat):\n        return {**self.observe(seat), \"history\": [\"summary\"]}\n\n    def handle_input",
            ),
            Some(json!({"turn":0,"next":"player-1","you":"player-1","history":["summary"]})),
        ),
        (EnvironmentLanguage::Python, "player", relay.to_owned(), None),
        (
            EnvironmentLanguage::TypeScript,
            "bidder",
            sealed.replace(
                "    handle_input",
                "    agent_view(seat) { return { ...this.observe(seat), history: [\"summary\"] }; },\n    handle_input",
            ),
            Some(json!({"your_bid":null,"submitted":0,"history":["summary"]})),
        ),
        (EnvironmentLanguage::TypeScript, "bidder", sealed.to_owned(), None),
    ];
    for (index, (language, role, source, view)) in cases.into_iter().enumerate() {
        let (draft, report) = validate_stateless(&manager, language, role, 2, &source).await;
        assert!(report.passed, "{:?}", report.diagnostics);
        let id = format!("{role}-{index}");
        manager.save(&draft, &id).unwrap();
        let active = manager
            .enter(&id, None, ExecutionPolicy::default())
            .await
            .unwrap();
        let seat = SeatId::try_from(format!("{role}-1")).unwrap();
        let expected = match view {
            Some(view) => view,
            None => active.observe(&seat).await.unwrap(),
        };
        assert_eq!(active.agent_view(&seat).await.unwrap(), expected);
        active.leave().await.unwrap();
    }
}

#[tokio::test]
async fn validation_rejects_a_view_too_large_to_resend_every_turn() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let source = include_str!("../fixtures/relay/environment.py").replace(
        "    def handle_input",
        &format!(
            "    def agent_view(self, seat):\n        return {{\"history\": \"x\" * {}}}\n\n    def handle_input",
            MAX_AGENT_VIEW_BYTES + 1
        ),
    );
    let (_, report) =
        validate_stateless(&manager, EnvironmentLanguage::Python, "player", 2, &source).await;
    assert!(!report.passed);
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
    assert!(
        report.diagnostics[0].contains("agent_view is"),
        "{:?}",
        report.diagnostics
    );
}

#[tokio::test]
async fn stateless_context_requires_protocol_v2() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let draft = manager
        .store()
        .create_draft(
            "legacy",
            EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(draft.join("environment.json")).unwrap()).unwrap();
    manifest["protocol_version"] = json!(1);
    manifest["roles"] = json!([]);
    manifest["agent_context"] = json!("stateless");
    std::fs::write(draft.join("environment.json"), manifest.to_string()).unwrap();
    let report = manager
        .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
        .await
        .unwrap();
    assert!(!report.passed);
    assert!(
        report.diagnostics[0].contains("requires protocol v2"),
        "{:?}",
        report.diagnostics
    );
}
