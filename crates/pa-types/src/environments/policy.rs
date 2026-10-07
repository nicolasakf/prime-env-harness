use super::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackend {
    #[default]
    Local,
    Docker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountGrant {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionPolicy {
    pub backend: ExecutionBackend,
    pub python_execution: bool,
    pub networking: bool,
    pub mounts: Vec<MountGrant>,
    /// Names only; values are resolved on the host at worker startup.
    pub secrets: Vec<String>,
    pub integrations: Vec<String>,
    pub call_timeout_seconds: u64,
    pub preparation_timeout_seconds: u64,
    pub cpus: u32,
    pub memory_mib: u32,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            backend: ExecutionBackend::Local,
            python_execution: false,
            networking: false,
            mounts: Vec::new(),
            secrets: Vec::new(),
            integrations: Vec::new(),
            call_timeout_seconds: 120,
            preparation_timeout_seconds: 600,
            cpus: 2,
            memory_mib: 1024,
        }
    }
}
