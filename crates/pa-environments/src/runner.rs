//! Local and Docker process transport. Generated code never enters the host.
use anyhow::{bail, Context, Result};
use pa_types::environments::{
    EnvironmentLanguage, EnvironmentManifest, ExecutionBackend, ExecutionPolicy, SeatId,
    WorkerRequest, ENVIRONMENT_PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const PYTHON_IMAGE: &str = "prime-env-harness-python:1";
pub(crate) const NODE_IMAGE: &str = "prime-env-harness-node:1";

#[derive(Debug, Clone)]
pub struct HostCall {
    pub method: String,
    pub params: Value,
}
pub type HostCallHandler =
    Arc<dyn Fn(HostCall) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>> + Send + Sync>;

pub struct Worker {
    child: Child,
    #[cfg(unix)]
    process_group: Option<u32>,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    container: Option<String>,
    deadline: Duration,
    protocol_version: u32,
    diagnostic: Arc<std::sync::Mutex<String>>,
}

pub(crate) fn sanitized_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    for key in [
        "PATH",
        "SYSTEMROOT",
        "WINDIR",
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
        "DOCKER_CONFIG",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    // Docker needs its host-side CLI configuration; this is never forwarded
    // into either container.
    if program == "docker" {
        if let Some(home) = std::env::var_os("HOME") {
            command.env("HOME", home);
        }
    }
    command.kill_on_drop(true);
    command
}

pub(crate) async fn command_output(mut command: Command, seconds: u64) -> Result<()> {
    let args: Vec<_> = command.as_std().get_args().collect();
    let container = args
        .windows(2)
        .find(|pair| pair[0] == "--name")
        .map(|pair| pair[1].to_string_lossy().into_owned());
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let result = tokio::time::timeout(Duration::from_secs(seconds), command.output())
        .await
        .context("preparation timed out")
        .and_then(|result| result.map_err(Into::into));
    if result.is_err() {
        if let Some(container) = container {
            let mut cleanup = sanitized_command("docker");
            cleanup.args(["rm", "--force", &container]);
            let _ = cleanup.output().await;
        }
    }
    let output = result?;
    if !output.status.success() {
        bail!(
            "preparation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub(crate) async fn local_python() -> Result<String> {
    let candidates = std::env::var("PRIME_ENV_PYTHON").map_or_else(
        |_| {
            vec![
                "python3".into(),
                "python3.14".into(),
                "python3.13".into(),
                "python3.12".into(),
                "python3.11".into(),
                "python3.10".into(),
            ]
        },
        |selected| vec![selected],
    );
    for program in candidates {
        let mut command = sanitized_command(&program);
        command.args(["-c", "import sys; assert sys.version_info >= (3, 10)"]);
        if tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .is_ok_and(|output| output.is_ok_and(|output| output.status.success()))
        {
            return Ok(program);
        }
    }
    bail!("local execution requires Python 3.10 or newer; set PRIME_ENV_PYTHON to its executable")
}

pub(crate) fn validate_policy(policy: &ExecutionPolicy) -> Result<()> {
    if policy.call_timeout_seconds == 0
        || policy.preparation_timeout_seconds == 0
        || policy.cpus == 0
        || policy.memory_mib == 0
    {
        bail!("timeouts, CPUs, and memory must be positive");
    }
    for mount in &policy.mounts {
        let target = Path::new(&mount.target);
        if !target.is_absolute()
            || [
                "/definition",
                "/workspace",
                "/runtime",
                "/proc",
                "/sys",
                "/dev",
            ]
            .iter()
            .any(|reserved| target.starts_with(reserved))
            || target == Path::new("/")
            || target
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("mount target overlaps a protected runtime path");
        }
        let source = Path::new(&mount.source).canonicalize()?;
        let home = pa_types::platform::home_dir();
        if source == Path::new("/")
            || source.to_string_lossy().contains("docker.sock")
            || home.is_some_and(|home| {
                source == home
                    || [".prime", ".prime-env-harness", ".codex", ".aws", ".docker"]
                        .iter()
                        .any(|name| source.starts_with(home.join(name)))
            })
        {
            bail!("host configuration directories and the Docker socket cannot be mounted");
        }
    }
    for secret in &policy.secrets {
        super::store::component(secret)?;
    }
    Ok(())
}

/// Docker availability is an explicit preflight, never a local fallback.
/// # Errors
/// Returns a diagnostic when the Docker daemon cannot be reached.
pub async fn docker_preflight() -> Result<()> {
    let mut command = sanitized_command("docker");
    command.args(["info", "--format", "{{.ServerVersion}}"]);
    command_output(command, 15)
        .await
        .context("Docker unavailable; start Docker or explicitly select local execution")
}

pub(crate) async fn ensure_image(
    language: EnvironmentLanguage,
    root: &Path,
    seconds: u64,
) -> Result<()> {
    docker_preflight().await?;
    let (image, dockerfile) = match language {
        EnvironmentLanguage::Python => (PYTHON_IMAGE, include_str!("../docker/python.Dockerfile")),
        EnvironmentLanguage::TypeScript => (NODE_IMAGE, include_str!("../docker/node.Dockerfile")),
    };
    let mut inspect = sanitized_command("docker");
    inspect.args(["image", "inspect", image]);
    if tokio::time::timeout(Duration::from_secs(15), inspect.output())
        .await??
        .status
        .success()
    {
        return Ok(());
    }
    let context = root.join(format!("image-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&context)?;
    std::fs::write(context.join("Dockerfile"), dockerfile)?;
    let mut command = sanitized_command("docker");
    command.args(["build", "--tag", image]).arg(&context);
    let result = command_output(command, seconds).await;
    std::fs::remove_dir_all(context)?;
    result
}

pub(crate) fn docker_command(
    image: &str,
    source: &Path,
    workspace: &Path,
    policy: &ExecutionPolicy,
    container: &str,
) -> Result<Command> {
    validate_policy(policy)?;
    let mut command = sanitized_command("docker");
    command.args([
        "run",
        "--rm",
        "--interactive",
        "--name",
        container,
        "--user",
        "65532:65532",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--read-only",
        "--pids-limit",
        "128",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,size=128m",
    ]);
    command.args([
        "--cpus",
        &policy.cpus.to_string(),
        "--memory",
        &format!("{}m", policy.memory_mib),
    ]);
    if !policy.networking {
        command.args(["--network", "none"]);
    }
    command.arg("--mount").arg(format!(
        "type=bind,src={},dst=/definition,readonly",
        source.display()
    ));
    command.arg("--mount").arg(format!(
        "type=bind,src={},dst=/workspace",
        workspace.display()
    ));
    command.args([
        "--workdir",
        "/workspace",
        "--env",
        "PYTHONPATH=/definition:/definition/.python-deps",
        "--env",
        "PYTHONDONTWRITEBYTECODE=1",
    ]);
    for mount in &policy.mounts {
        let source = PathBuf::from(&mount.source).canonicalize()?;
        command.arg("--mount").arg(format!(
            "type=bind,src={},dst={}{}",
            source.display(),
            mount.target,
            if mount.writable { "" } else { ",readonly" }
        ));
    }
    for name in &policy.secrets {
        let key = format!("PRIME_ENV_SECRET_{name}");
        let value =
            std::env::var(&key).with_context(|| format!("named secret {name} is unavailable"))?;
        // Pass values in the CLI process environment, not argv or definition files.
        command.env(name, value).arg("--env").arg(name);
    }
    command.arg(image);
    Ok(command)
}

impl Worker {
    // Keep process construction and its readiness handshake in one auditable boundary.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn start(
        source: &Path,
        workspace: &Path,
        language: EnvironmentLanguage,
        policy: &ExecutionPolicy,
        kernel: bool,
    ) -> Result<Self> {
        validate_policy(policy)?;
        std::fs::create_dir_all(workspace)?;
        let container = (policy.backend == ExecutionBackend::Docker)
            .then(|| format!("prime-env-{}", Uuid::new_v4()));
        let mut command = match policy.backend {
            ExecutionBackend::Local => {
                let program = match language {
                    EnvironmentLanguage::Python => source
                        .join(".venv/bin/python")
                        .to_string_lossy()
                        .into_owned(),
                    EnvironmentLanguage::TypeScript => "node".into(),
                };
                let mut command = if kernel {
                    sanitized_command(&local_python().await?)
                } else {
                    sanitized_command(&program)
                };
                command
                    .current_dir(workspace)
                    .env("HOME", workspace)
                    .env("PYTHONPATH", source)
                    .env("PYTHONDONTWRITEBYTECODE", "1");
                for name in &policy.secrets {
                    command.env(
                        name,
                        std::env::var(format!("PRIME_ENV_SECRET_{name}"))
                            .with_context(|| format!("named secret {name} is unavailable"))?,
                    );
                }
                match language {
                    EnvironmentLanguage::Python => {
                        command.arg("-u").arg(source.join(if kernel {
                            "kernel.py"
                        } else {
                            "runner.py"
                        }));
                    }
                    EnvironmentLanguage::TypeScript => {
                        command.arg(source.join("runner.ts"));
                    }
                }
                command
            }
            ExecutionBackend::Docker => {
                let image = match language {
                    EnvironmentLanguage::Python => PYTHON_IMAGE,
                    EnvironmentLanguage::TypeScript => NODE_IMAGE,
                };
                let mut command = docker_command(
                    image,
                    source,
                    workspace,
                    policy,
                    container.as_deref().context("container name")?,
                )?;
                match language {
                    EnvironmentLanguage::Python => {
                        command.args([
                            "python",
                            "-u",
                            if kernel {
                                "/definition/kernel.py"
                            } else {
                                "/definition/runner.py"
                            },
                        ]);
                    }
                    EnvironmentLanguage::TypeScript => {
                        command.args(["node", "/definition/runner.ts"]);
                    }
                }
                command
            }
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().context("start environment worker")?;
        #[cfg(unix)]
        let process_group = child.id();
        let input = child.stdin.take().context("worker stdin")?;
        let output = BufReader::new(child.stdout.take().context("worker stdout")?);
        let diagnostic = Arc::new(std::sync::Mutex::new(String::new()));
        if let Some(mut stderr) = child.stderr.take() {
            let diagnostic = Arc::clone(&diagnostic);
            tokio::spawn(async move {
                let mut buffer = [0; 4096];
                while let Ok(count) = stderr.read(&mut buffer).await {
                    if count == 0 {
                        break;
                    }
                    let mut log = diagnostic
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    log.push_str(&String::from_utf8_lossy(&buffer[..count]));
                    if log.len() > 65536 {
                        log.clear();
                        log.push_str("[diagnostic output limit exceeded]");
                    }
                }
            });
        }
        let protocol_version = if kernel {
            ENVIRONMENT_PROTOCOL_VERSION
        } else {
            let manifest: EnvironmentManifest =
                crate::store::read_json(&source.join("environment.json"))?;
            if !pa_types::environments::SUPPORTED_PROTOCOL_VERSIONS
                .contains(&manifest.protocol_version)
            {
                bail!("unsupported environment protocol");
            }
            manifest.protocol_version
        };
        let mut worker = Self {
            child,
            #[cfg(unix)]
            process_group,
            input,
            output,
            container,
            deadline: Duration::from_secs(policy.call_timeout_seconds),
            protocol_version,
            diagnostic,
        };
        let ready = tokio::time::timeout(worker.deadline, worker.read_frame())
            .await
            .context("worker readiness timed out")??;
        if ready.get("kind").and_then(Value::as_str) != Some("ready")
            || ready.get("protocol_version").and_then(Value::as_u64)
                != Some(u64::from(protocol_version))
        {
            bail!("worker protocol readiness mismatch");
        }
        Ok(worker)
    }

    async fn read_frame(&mut self) -> Result<Value> {
        let mut bytes = Vec::new();
        loop {
            let buffer = self.output.fill_buf().await?;
            if buffer.is_empty() {
                let diagnostic = self
                    .diagnostic
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                bail!("worker exited before responding: {diagnostic}");
            }
            let count = buffer
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(buffer.len(), |index| index + 1);
            if bytes.len() + count > MAX_FRAME_BYTES {
                bail!("worker frame exceeds 8 MiB");
            }
            let end = buffer[count - 1] == b'\n';
            bytes.extend_from_slice(&buffer[..count]);
            self.output.consume(count);
            if end {
                return serde_json::from_slice(&bytes)
                    .context("invalid worker protocol JSON; stdout is reserved");
            }
        }
    }

    /// Send one request, enforcing a deadline and cancellation at the process boundary.
    /// # Errors
    /// Returns protocol failures, cancellation, or worker-reported domain errors.
    pub async fn request(
        &mut self,
        method: &str,
        participant: &SeatId,
        params: Value,
        signal: CancellationToken,
        host: Option<&HostCallHandler>,
    ) -> Result<Value> {
        let id = Uuid::new_v4().to_string();
        let request = WorkerRequest {
            protocol_version: self.protocol_version,
            id: id.clone(),
            method: method.into(),
            participant: if self.protocol_version == 2
                && matches!(
                    method,
                    "initialize" | "describe_actions" | "snapshot" | "restore" | "dispose"
                ) {
                None
            } else {
                Some(participant.clone())
            },
            params,
        };
        let deadline = self.deadline;
        let result = tokio::select! {
            () = signal.cancelled() => Err(anyhow::anyhow!("worker call cancelled; outcome may be uncertain")),
            result = tokio::time::timeout(deadline, async {
                self.input.write_all(&serde_json::to_vec(&request)?).await?;
                self.input.write_all(b"\n").await?;
                self.input.flush().await?;
                loop {
                    let frame = self.read_frame().await?;
                    match frame.get("kind").and_then(Value::as_str) {
                        Some("host_call") => {
                            let call_id = frame.get("id").and_then(Value::as_str).context("host call id")?;
                            let call = HostCall { method: frame.get("method").and_then(Value::as_str).context("host call method")?.into(), params: frame.get("params").cloned().unwrap_or(Value::Null) };
                            let result = match host { Some(host) => host(call).await, None => Err(anyhow::anyhow!("host integration was not granted")) };
                            if let Err(error) = &result {
                                if error.downcast_ref::<crate::UncertainIntegration>().is_some() {
                                    bail!("{error:#}");
                                }
                            }
                            let response = match result { Ok(result) => json!({"kind":"host_response","id":call_id,"success":true,"result":result}), Err(error) => json!({"kind":"host_response","id":call_id,"success":false,"error":error.to_string()}) };
                            self.input.write_all(&serde_json::to_vec(&response)?).await?;
                            self.input.write_all(b"\n").await?;
                            self.input.flush().await?;
                        }
                        Some("response") if frame.get("id").and_then(Value::as_str) == Some(&id) => return Ok(frame),
                        _ => bail!("unexpected worker protocol frame"),
                    }
                }
            }) => result.context("worker call timed out; outcome may be uncertain").and_then(std::convert::identity),
        };
        if result.is_err() {
            self.cancel().await;
        }
        result
    }

    /// Stop and reap the worker even if graceful disposal failed.
    /// # Errors
    /// Returns disposal failures after killing the process.
    pub async fn stop(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            self.cancel().await;
            return Ok(());
        }
        let response = self
            .request(
                "dispose",
                &SeatId::legacy_agent(),
                json!({}),
                CancellationToken::new(),
                None,
            )
            .await;
        self.cancel().await;
        let response = response?;
        if response.get("success") != Some(&Value::Bool(true)) {
            bail!("worker disposal failed: {}", response["error"]);
        }
        Ok(())
    }

    pub(crate) async fn cancel(&mut self) {
        if let Err(error) = self.terminate_process() {
            eprintln!("environment worker cancellation failed: {error:#}");
        }
        let _ = self.child.wait().await;
        if let Some(container) = &self.container {
            let mut command = sanitized_command("docker");
            command.args(["rm", "--force", container]);
            let _ = command.output().await;
        }
    }

    fn terminate_process(&mut self) -> Result<()> {
        #[cfg(unix)]
        if let Some(group) = self.process_group.take() {
            pa_types::platform::process::terminate_owned_process_group(group)?;
        }
        // The group signal may already have ended the direct child.
        if self.child.try_wait()?.is_none() {
            self.child.start_kill()?;
        }
        Ok(())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Err(error) = self.terminate_process() {
            eprintln!("environment worker cleanup failed: {error:#}");
        }
        if let Some(container) = &self.container {
            // The docker CLI child can die before its container; reap both.
            let _ = std::process::Command::new("docker")
                .args(["rm", "--force", container])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}
