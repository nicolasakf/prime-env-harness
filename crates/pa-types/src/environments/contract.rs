use super::{Deserialize, ExecutionBackend, RoleDescriptor, RoleName, SeatId, Serialize, Value};

pub const ENVIRONMENT_PROTOCOL_VERSION: u32 = 1;
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u32] = &[1, 2];
pub const DRAFT_PROTOCOL_VERSION: u32 = 2;
pub const ENVIRONMENT_CAPABILITY: &str = "environments_v1";
pub const ENVIRONMENT_SEATS_CAPABILITY: &str = "environments_seats_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentLanguage {
    #[serde(rename = "typescript")]
    TypeScript,
    Python,
}

/// Custom-message type of the transcript row that delivers a seat's wake.
pub const ENVIRONMENT_WAKE_CUSTOM_TYPE: &str = "environment_wake";
/// Largest `agent_view` an environment may return for one seat, in serialized bytes.
pub const MAX_AGENT_VIEW_BYTES: usize = 32 * 1024;

/// How an environment feeds an agent seat's model on each turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContext {
    /// The seat's whole conversation is resent on every model request.
    #[default]
    Conversation,
    /// Each turn sees only the environment's `agent_view` for the seat, its
    /// private messages, and the turn's own tool calls. The environment, not the
    /// transcript, carries history; the seat gets no separate observe tool.
    Stateless,
}

impl AgentContext {
    #[must_use]
    pub fn is_conversation(&self) -> bool {
        matches!(self, Self::Conversation)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentManifest {
    pub protocol_version: u32,
    pub name: String,
    pub purpose: String,
    pub language: EnvironmentLanguage,
    pub instructions: String,
    /// Human setup parameters, available before environment code runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_schema: Option<Value>,
    #[serde(default)]
    pub python_execution: bool,
    #[serde(default)]
    pub recommended_backend: ExecutionBackend,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<RoleDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "AgentContext::is_conversation")]
    pub agent_context: AgentContext,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<RoleName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<RoleName>,
}

impl ActionDescriptor {
    #[must_use]
    pub fn allowed_roles(&self) -> &[RoleName] {
        if self.roles.is_empty() {
            &self.participants
        } else {
            &self.roles
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeatInputResult {
    pub observation: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanInputResult {
    pub observation: Value,
    /// Private environment-to-agent content. Clients receive a delivery notice;
    /// publish human-visible content through `observation` instead.
    #[serde(default)]
    pub agent_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerRequest {
    pub protocol_version: u32,
    pub id: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participant: Option<SeatId>,
    pub params: Value,
}
