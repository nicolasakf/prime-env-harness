use pa_environments::EnvironmentManager;
use pa_types::environments::{EnvironmentLanguage, ExecutionPolicy, ValidationReport};
use serde_json::Value;

/// Validate a scaffolded draft that declares `missed_turns`, with its environment
/// source passed through `edit`.
async fn validate_missed_turns(
    manager: &EnvironmentManager,
    language: EnvironmentLanguage,
    edit: impl Fn(String) -> String,
) -> ValidationReport {
    let draft = manager
        .store()
        .create_draft(
            "missed turns",
            language,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    let path = draft.join("environment.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["missed_turns"] = Value::Bool(true);
    std::fs::write(&path, manifest.to_string()).unwrap();
    let source = draft.join(match language {
        EnvironmentLanguage::Python => "environment.py",
        EnvironmentLanguage::TypeScript => "environment.ts",
    });
    std::fs::write(&source, edit(std::fs::read_to_string(&source).unwrap())).unwrap();
    manager
        .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn declaring_missed_turns_requires_a_missed_turn_handler_in_both_languages() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    for language in [EnvironmentLanguage::Python, EnvironmentLanguage::TypeScript] {
        let report = validate_missed_turns(&manager, language, |source| source).await;
        assert!(!report.passed, "{language:?}");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("defines no missed_turn")),
            "{language:?}: {:?}",
            report.diagnostics
        );
    }
    let report = validate_missed_turns(&manager, EnvironmentLanguage::Python, |source| {
        source.replace(
            "    def handle_input",
            "    def missed_turn(self, participant, report):\n        return {}\n\n    def handle_input",
        )
    })
    .await;
    assert!(report.passed, "{:?}", report.diagnostics);
    let report = validate_missed_turns(&manager, EnvironmentLanguage::TypeScript, |source| {
        source.replace(
            "    handle_input(",
            "    missed_turn() { return {}; },\n    handle_input(",
        )
    })
    .await;
    assert!(report.passed, "{:?}", report.diagnostics);
}
