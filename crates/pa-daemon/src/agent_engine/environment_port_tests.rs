use super::{binding, InProcessPort};
use pa_agent::scripted::ScriptedProvider;
use pa_agent::types::{Model, UsageCost};
use pa_core::session_engine::engine::{create_seat_session, SessionEngineConfig};
use pa_core::session_engine::PromptOptions;
use pa_environments::EnvironmentManager;
use pa_types::environments::{EnvironmentLanguage, ExecutionPolicy, SeatId};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn participant_tools_ignore_builder_resources_and_persist_actions() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().join("environments"), None).unwrap();
    let policy = ExecutionPolicy {
        python_execution: true,
        ..ExecutionPolicy::default()
    };
    let draft = manager
        .store()
        .create_draft(
            "counter participant",
            EnvironmentLanguage::Python,
            true,
            &policy,
        )
        .unwrap();
    let path = draft.join("environment.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["protocol_version"] = json!(1);
    manifest.as_object_mut().unwrap().remove("roles");
    std::fs::write(path, manifest.to_string()).unwrap();
    std::fs::write(
        draft.join("environment.py"),
        include_str!("../../../pa-environments/fixtures/counter/environment.py"),
    )
    .unwrap();
    assert!(
        manager
            .validate(draft.to_str().unwrap(), policy.clone())
            .await
            .unwrap()
            .passed
    );
    manager.save(draft.to_str().unwrap(), "counter").unwrap();
    let instance = manager.enter("counter", None, policy).await.unwrap();
    let cwd = instance.path().join("participant");
    std::fs::write(cwd.join("AGENTS.md"), "BUILDER_CONTEXT_MUST_NOT_LOAD").unwrap();
    let model = Model {
        id: "environment-fixture".into(),
        name: "fixture".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        cost: UsageCost::default(),
        context_window: 100_000,
        max_tokens: 4096,
    };
    let provider = Arc::new(ScriptedProvider::new(model.clone()));
    provider.push_tool_call_turn(
        None,
        vec![("counter-tool", "env_increment", json!({"amount":2}))],
    );
    provider.push_tool_call_turn(
        None,
        vec![(
            "python-tool",
            "ipython",
            json!({"code":"remembered = 12\nassert env.observe()['value'] == 2"}),
        )],
    );
    provider.push_text_turn("done");
    let engine = create_seat_session(
        SessionEngineConfig {
            cwd,
            agent_dir: root.path().join("builder"),
            model: Some(model),
            stream_fn: Some(provider.stream_fn()),
            custom_system_prompt: Some("BUILDER_PROMPT_MUST_NOT_LOAD".into()),
            generic_mcp_servers: vec!["BUILDER_MCP_MUST_NOT_LOAD".into()],
            ..SessionEngineConfig::default()
        },
        binding(&instance, &SeatId::legacy_agent()).unwrap(),
        Arc::new(InProcessPort(Arc::clone(&instance))),
    )
    .await
    .unwrap();
    assert!(
        engine.skills.is_empty()
            && engine.agents_files.is_empty()
            && engine.prompt_templates.is_empty()
    );
    assert!(!engine.system_prompt.contains("BUILDER_"));
    let names: Vec<_> = engine
        .session
        .agent()
        .state()
        .await
        .tools
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "observe_environment",
            "env_increment",
            "message_seat",
            "ipython"
        ]
    );
    assert!(engine
        .prompt("/goal start unrestricted", PromptOptions::default())
        .await
        .is_err());
    engine
        .prompt("Environment message: increment", PromptOptions::default())
        .await
        .unwrap();
    engine.session.agent().wait_for_idle().await;
    assert_eq!(
        instance.observe(&SeatId::legacy_human()).await.unwrap(),
        json!({"value":2})
    );
    let entries = engine.session.entries().await;
    assert!(!entries.is_empty());
    instance
        .execute_python(
            "assert remembered == 12",
            "verify-python",
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    instance.leave().await.unwrap();
}

#[tokio::test]
async fn seat_prompts_carry_each_instances_resolved_setup() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().join("environments"), None).unwrap();
    let policy = ExecutionPolicy::default();
    let draft = manager
        .store()
        .create_draft("relay", EnvironmentLanguage::Python, false, &policy)
        .unwrap();
    let path = draft.join("environment.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["protocol_version"] = json!(2);
    manifest["roles"] =
        json!([{"name":"player","min_seats":2,"max_seats":2,"occupants":["agent"]}]);
    manifest["setup_schema"] = json!({"type":"object","properties":{"rounds":{"type":"integer","default":3},"label":{"type":"string","default":"standard"}},"additionalProperties":false});
    std::fs::write(path, manifest.to_string()).unwrap();
    std::fs::write(
        draft.join("environment.py"),
        include_str!("../../../pa-environments/fixtures/relay/environment.py"),
    )
    .unwrap();
    std::fs::write(draft.join("checks.py"), "").unwrap();
    assert!(
        manager
            .validate(draft.to_str().unwrap(), policy.clone())
            .await
            .unwrap()
            .passed
    );
    manager.save(draft.to_str().unwrap(), "relay").unwrap();
    let model = Model {
        id: "environment-fixture".into(),
        name: "fixture".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        cost: UsageCost::default(),
        context_window: 100_000,
        max_tokens: 4096,
    };
    let seat = SeatId::try_from("player-1".to_owned()).unwrap();
    let mut prompts = Vec::new();
    for arguments in [json!({}), json!({"rounds":50})] {
        let instance = manager
            .enter_with_arguments("relay", None, policy.clone(), arguments)
            .await
            .unwrap();
        let engine = create_seat_session(
            SessionEngineConfig {
                cwd: instance.path().join("workspace"),
                agent_dir: root
                    .path()
                    .join(format!("builder-{}", instance.instance.id)),
                model: Some(model.clone()),
                stream_fn: Some(Arc::new(ScriptedProvider::new(model.clone())).stream_fn()),
                ..SessionEngineConfig::default()
            },
            binding(&instance, &seat).unwrap(),
            Arc::new(InProcessPort(Arc::clone(&instance))),
        )
        .await
        .unwrap();
        prompts.push(engine.system_prompt.clone());
        instance.leave().await.unwrap();
    }
    let rendered = |config: &str| {
        format!("Actual configuration for this instance (not definition defaults):\n{config}\nUse these values for this run.")
    };
    assert!(prompts[0].contains(&rendered(
        "{\n  \"rounds\": 3,\n  \"label\": \"standard\"\n}"
    )));
    assert!(prompts[1].contains(&rendered(
        "{\n  \"rounds\": 50,\n  \"label\": \"standard\"\n}"
    )));
}
