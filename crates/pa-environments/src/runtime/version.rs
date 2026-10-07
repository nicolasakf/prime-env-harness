//! Committed-state versions: the identity a mutation is bound to when it is generated.

/// What a mutation claims about the state its caller was shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewGuard {
    /// The caller is not bound to a view: builders, tests and the legacy
    /// single-agent port. Model-driven seats always supply a version.
    Unbound,
    /// The caller was shown the state at this version (see `ActiveInstance::version`).
    Version(u64),
}

impl From<Option<u64>> for ViewGuard {
    fn from(version: Option<u64>) -> Self {
        version.map_or(Self::Unbound, Self::Version)
    }
}

/// A mutation was refused because the instance committed an operation after the
/// view it was generated from. Nothing executed; the caller must reconsider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleView {
    pub expected: u64,
    pub current: u64,
}

impl std::fmt::Display for StaleView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "stale view: the environment changed since the state you were shown (version {} -> {}); nothing was executed",
            self.expected, self.current
        )
    }
}

impl std::error::Error for StaleView {}
