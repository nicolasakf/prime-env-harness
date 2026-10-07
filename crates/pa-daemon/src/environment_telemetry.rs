//! Primitive-only adoption events for environment operations.
use crate::agent_engine::AgentEngineConfig;
use pa_telemetry::{base_properties, TelemetryClient};
use serde_json::json;

pub(crate) fn client(config: &AgentEngineConfig) -> Option<TelemetryClient> {
    if config.telemetry_disabled == Some(true) {
        return None;
    }
    let settings = pa_core::settings::SettingsManager::create(&config.cwd, &config.agent_dir);
    Some(pa_core::session_engine::telemetry::build_client(
        &settings,
        &config.agent_dir,
    ))
}

pub(crate) fn track(
    client: Option<&TelemetryClient>,
    feature: &str,
    outcome: &str,
    choice: Option<&str>,
) {
    if let Some(client) = client {
        let mut properties = base_properties("daemon");
        properties.set("feature_id", json!(uuid::Uuid::new_v4().to_string()));
        properties.set("feature_name", json!(feature));
        properties.set("outcome", json!(outcome));
        if let Some(choice) = choice {
            properties.set("configuration_choice", json!(choice));
        }
        client.track("agent feature outcome", properties);
    }
}
