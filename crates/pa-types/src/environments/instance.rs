use super::{
    ActionDescriptor, BTreeMap, Deserialize, EnvironmentFinish, EnvironmentManifest,
    EnvironmentNotice, EnvironmentProgress, ExecutionPolicy, InstanceRoster, RoleName, SeatId,
    Serialize, Value,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub content_hash: String,
    pub policy: ExecutionPolicy,
    pub passed: bool,
    pub diagnostics: Vec<String>,
    pub actions: Vec<ActionDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    pub id: String,
    pub version: String,
    pub manifest: EnvironmentManifest,
    pub validation: ValidationReport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub definition_id: String,
    pub version: String,
    pub policy: ExecutionPolicy,
    /// Creation time as Unix milliseconds; older metadata omits this field.
    #[serde(default)]
    pub created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roster: Option<InstanceRoster>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OperationReceipt {
    pub request: Value,
    pub success: bool,
    pub result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<EnvironmentNotice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<EnvironmentFinish>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<EnvironmentProgress>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceCheckpoint {
    pub protocol_version: u32,
    pub snapshot: Value,
    pub receipts: BTreeMap<String, OperationReceipt>,
    #[serde(default)]
    pub next_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<EnvironmentFinish>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<EnvironmentProgress>,
    /// Operation sequence of the receipt that published `progress`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub progress_sequence: u64,
    /// Malformed successful responses may have mutated state; require reconciliation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub requires_recovery: bool,
}

/// User-reconciled results for uncertain operations; never an instruction to replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequest {
    pub snapshot: Value,
    pub receipts: BTreeMap<String, OperationReceipt>,
    pub python_receipts: BTreeMap<String, OperationReceipt>,
    #[serde(default)]
    pub seat_python_receipts: BTreeMap<SeatId, BTreeMap<String, OperationReceipt>>,
    #[serde(default)]
    pub reset_python_seats: Vec<SeatId>,
    pub reset_python: bool,
    pub evidence: String,
}

/// Durable participant binding; never inferred from conversation text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSessionProfile {
    #[serde(default, skip_serializing_if = "ProfileRole::is_legacy")]
    pub role: ProfileRole,
    pub instance_id: String,
    pub definition_id: String,
    pub version: String,
    pub policy: ExecutionPolicy,
    pub python_execution: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileRole {
    #[default]
    Legacy,
    Host,
    Seat {
        seat: SeatId,
        role: RoleName,
    },
}

impl ProfileRole {
    #[must_use]
    pub fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy)
    }

    /// Whether this bound session may answer `command`.
    ///
    /// `heartbeats_list` is allowed on every role. The heartbeat panel's
    /// refresh is forwarded to every live worker, and one denial fails the
    /// merged catalog, so a host or seat would otherwise hide the jobs.
    #[must_use]
    pub fn permits_command(&self, command: &str) -> bool {
        match self {
            Self::Legacy => EnvironmentSessionProfile::permits_command(command),
            Self::Host => matches!(
                command,
                "attach"
                    | "detach"
                    | "get_state"
                    | "get_messages"
                    | "get_connection_state"
                    | "get_session_header"
                    | "get_session_stats"
                    | "get_queue"
                    | "get_commands"
                    | "env"
                    | "prompt"
                    | "prompt_and_wait"
                    | "kill"
                    | "shutdown"
                    | "mark_anthropic_warning_shown"
                    | "heartbeats_list"
            ),
            Self::Seat { .. } => matches!(
                command,
                "env" | "abort" | "kill" | "shutdown" | "compact" | "heartbeats_list"
            ),
        }
    }
}

impl EnvironmentSessionProfile {
    #[must_use]
    pub fn permits_command(command: &str) -> bool {
        matches!(
            command,
            "attach"
                | "detach"
                | "prompt"
                | "prompt_and_wait"
                | "abort"
                | "kill"
                | "shutdown"
                | "wait_for_idle"
                | "wait_for_headless_completion"
                | "get_state"
                | "get_messages"
                | "get_connection_state"
                | "get_session_header"
                | "get_session_stats"
                | "get_queue"
                | "get_last_assistant_text"
                | "get_system_prompt"
                | "get_tool_definition"
                | "get_commands"
                | "get_available_models"
                | "get_model_catalog"
                | "set_model"
                | "cycle_model"
                | "set_thinking_level"
                | "get_resource_snapshot"
                | "compact"
                | "abort_compaction"
                | "set_auto_compaction"
                | "env"
                | "mark_anthropic_warning_shown"
                | "update_snapshot"
                | "heartbeats_list"
        )
    }
}

// serde's `skip_serializing_if` passes the field by reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::{EnvironmentSessionProfile, ProfileRole, RoleName, SeatId};

    #[test]
    fn participant_roles_allow_the_heartbeat_catalog_read() {
        let seat = ProfileRole::Seat {
            seat: SeatId::try_from("seat-1".to_string()).expect("seat id"),
            role: RoleName::legacy_agent(),
        };
        for role in [ProfileRole::Legacy, ProfileRole::Host, seat] {
            assert!(
                role.permits_command("heartbeats_list"),
                "{role:?} must answer the heartbeat panel"
            );
            assert!(
                !role.permits_command("export_html"),
                "{role:?} stays barred from export"
            );
        }
        assert!(EnvironmentSessionProfile::permits_command(
            "heartbeats_list"
        ));
    }
}
