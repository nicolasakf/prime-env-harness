use super::{Deserialize, Instance, Serialize};

/// Dispatch phase of a persisted run, owned by its model-less host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Running,
    Paused,
    Stalled,
    NeedsRecovery,
    Stopped,
    Finished,
}

impl RunPhase {
    /// Terminal runs accept no further control or dispatch.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished | Self::Stopped | Self::NeedsRecovery)
    }

    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Stalled => "stalled",
            Self::NeedsRecovery => "needs recovery",
            Self::Stopped => "stopped",
            Self::Finished => "finished",
        }
    }

    /// Only a running run has dispatch to pause; a paused or stalled run is
    /// already idle and can only be stopped.
    #[must_use]
    pub fn is_pausable(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// A persisted instance whose run is not terminal, so it can still be stopped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceRun {
    pub instance: Instance,
    pub phase: RunPhase,
    pub reason: Option<String>,
    /// The active session that provisioned the run, which the activity dock
    /// scopes on; absent for runs persisted without a builder identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builder_session_id: Option<String>,
}
