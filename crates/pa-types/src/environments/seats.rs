use super::{
    ActionDescriptor, AgentContext, Deserialize, NoticeTarget, RoleName, SeatId, Serialize, Value,
};

/// Calls permitted from a bound environment seat. Identity is supplied by its port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeatCall {
    Observe,
    /// The seat's current `agent_view`: what a stateless seat re-reads after a
    /// rejected action. Same as `Observe` for environments without a view.
    AgentView,
    /// `view_version` is the committed-state version the caller's model request
    /// was generated from, bound by the seat's tool layer and never chosen by the
    /// model. The host rejects the call as stale, without running environment
    /// code, when the instance has committed an engine operation since.
    Invoke {
        action: String,
        arguments: Value,
        operation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view_version: Option<u64>,
    },
    Say {
        text: String,
        operation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view_version: Option<u64>,
    },
    Message {
        to: SeatId,
        text: String,
        message_id: String,
    },
    /// The cell's `env.invoke` calls are bound to `view_version`, so a cell
    /// cannot act on a view newer than the request that wrote it.
    Python {
        code: String,
        operation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view_version: Option<u64>,
    },
    CancelPython {
        operation_id: String,
    },
    TurnStarted {
        through_seq: u64,
    },
    TurnSettled {
        through_seq: u64,
    },
    TurnFailed {
        through_seq: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeatReply {
    pub result: Value,
    pub deliveries: Vec<Delivery>,
    /// The seat's refreshed `agent_view`, returned by actions in stateless context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<Value>,
    /// Committed-state version of the view or observation this reply delivers to
    /// the model, read atomically with it. Absent when the reply carries no state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_version: Option<u64>,
    /// The call was refused before reaching the environment because its view was
    /// stale. `view` then holds the current view to reconsider; nothing executed
    /// and the call is never replayed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale_view: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryKind {
    Notice,
    DirectMessage,
    Status,
}

/// One durable table event. Its ID survives a partial append and rematerialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub seq: u64,
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_seq: Option<u64>,
    pub kind: DeliveryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<SeatId>,
    pub to: NoticeTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub wake: bool,
    /// The environment published this wake as a task for its recipients to act
    /// on. The host revalidates it against the environment's current wait state
    /// before it starts a model turn; ordinary notices and messages are never
    /// retired this way.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub actionable: bool,
    pub timestamp: u64,
}

impl Delivery {
    #[must_use]
    pub fn visible_to(&self, seat: &SeatId) -> bool {
        match &self.to {
            NoticeTarget::All => true,
            NoticeTarget::Seats(seats) => seats.contains(seat),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WakePayload {
    pub through_seq: u64,
    /// The seat's `observe` result, or its `agent_view` in stateless context.
    pub observation: Value,
    pub deliveries: Vec<Delivery>,
    /// Committed-state version of `observation`; the wake's tool calls are bound to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_version: Option<u64>,
    #[serde(default, skip_serializing_if = "AgentContext::is_conversation")]
    pub agent_context: AgentContext,
    /// Consecutive retries this turn follows; nonzero turns run with thinking off.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attempt: u64,
}

// serde's `skip_serializing_if` passes the field by reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// Immutable tool/prompt binding; contains no capability secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeatBinding {
    pub protocol_version: u32,
    pub instance_id: String,
    pub definition_id: String,
    pub version: String,
    pub seat: SeatId,
    pub role: RoleName,
    pub actions: Vec<ActionDescriptor>,
    pub manifest_name: String,
    pub instructions: String,
    pub python: bool,
    pub direct_messages: bool,
    pub peers: Vec<SeatId>,
    #[serde(default)]
    pub peer_roles: std::collections::BTreeMap<SeatId, RoleName>,
    /// Validated setup configuration this instance was initialized with, shared with every seat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(default)]
    pub agent_context: AgentContext,
    /// Seconds a turn may take to act, and the most tokens a request may emit.
    #[serde(default)]
    pub turn_timeout_seconds: u64,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
}
