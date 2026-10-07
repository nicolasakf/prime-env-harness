//! Instance seats, occupant choices and bounded run configuration.
use super::{Deserialize, RoleName, SeatId, Serialize};

/// The smallest per-request output cap that leaves room for thinking and an action.
pub const MIN_MAX_OUTPUT_TOKENS: u64 = 2048;

fn one() -> usize {
    1
}
fn occupants() -> Vec<OccupantKind> {
    vec![OccupantKind::Human, OccupantKind::Agent]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccupantKind {
    Human,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleDescriptor {
    pub name: RoleName,
    #[serde(default)]
    pub description: String,
    #[serde(default = "one")]
    pub min_seats: usize,
    #[serde(default = "one")]
    pub max_seats: usize,
    #[serde(default = "occupants")]
    pub occupants: Vec<OccupantKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seat {
    pub id: SeatId,
    pub role: RoleName,
    pub occupant: OccupantKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<crate::ai::ModelThinkingLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunLimits {
    pub max_operations: u64,
    pub max_wakes_per_seat: u64,
    pub max_wall_clock_seconds: u64,
    /// Zero chooses 2 * agent count + 2 when the roster is provisioned.
    pub stall_turns: u64,
    /// Time a seat gets from the start of a turn to its action. A turn still
    /// running past it is aborted and retried with thinking off.
    pub seat_turn_timeout_seconds: u64,
    /// Most tokens one model request may emit, thinking included; `None` leaves
    /// the provider default. A turn that hits it without acting is retried with
    /// thinking off. At least [`MIN_MAX_OUTPUT_TOKENS`].
    pub max_output_tokens: Option<u64>,
    /// Consecutive re-prompts an awaited agent seat receives after a turn that
    /// left the environment unchanged or timed out; zero disables re-prompting.
    pub idle_turn_retries: u64,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_operations: 500,
            max_wakes_per_seat: 100,
            max_wall_clock_seconds: 7200,
            stall_turns: 0,
            seat_turn_timeout_seconds: 120,
            max_output_tokens: Some(8192),
            idle_turn_retries: 2,
        }
    }
}

fn direct_messages_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceRoster {
    pub seats: Vec<Seat>,
    #[serde(default)]
    pub limits: RunLimits,
    #[serde(default = "direct_messages_default")]
    pub direct_messages: bool,
}

impl Default for InstanceRoster {
    fn default() -> Self {
        Self {
            seats: vec![
                Seat {
                    id: SeatId::legacy_human(),
                    role: RoleName::legacy_human(),
                    occupant: OccupantKind::Human,
                    model: None,
                    thinking: None,
                },
                Seat {
                    id: SeatId::legacy_agent(),
                    role: RoleName::legacy_agent(),
                    occupant: OccupantKind::Agent,
                    model: None,
                    thinking: None,
                },
            ],
            limits: RunLimits::default(),
            direct_messages: true,
        }
    }
}
