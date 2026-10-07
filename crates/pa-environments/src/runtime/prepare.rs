//! Environment prepare responsibility.
use super::{
    bail, command_output, content_hash, copy_sources, docker_command, ensure_image, fs, json,
    local_python, sanitized_command, writable_workspace, write_json, Context, EnvironmentLanguage,
    EnvironmentManager, ExecutionBackend, ExecutionPolicy, OpenOptions, Path, PathBuf, Result,
    Uuid, NODE_IMAGE, PYTHON_IMAGE,
};

impl EnvironmentManager {
    // The lock covers copying, backend-specific installation, and publishing readiness.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn prepare(
        &self,
        source: &Path,
        language: EnvironmentLanguage,
        policy: &ExecutionPolicy,
    ) -> Result<PathBuf> {
        let hash = content_hash(source)?;
        let cache = self
            .store
            .root()
            .join("prepared")
            .join(format!("{hash}-{:?}", policy.backend));
        fs::create_dir_all(&cache)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(cache.join("prepare.lock"))?;
        // Preparation cannot race another builder or instance using the same cache.
        lock.try_lock()
            .context("dependencies are being prepared by another process")?;
        let prepared = cache.join("source");
        if cache.join("ready.json").exists() {
            if content_hash(&prepared)? != hash {
                bail!("prepared source integrity check failed");
            }
            return Ok(prepared);
        }
        if prepared.exists() {
            fs::remove_dir_all(&prepared)?;
        }
        copy_sources(source, &prepared)?;
        let seconds = policy.preparation_timeout_seconds;
        let scratch = cache.join("scratch");
        writable_workspace(&scratch)?;
        match policy.backend {
            ExecutionBackend::Local => match language {
                EnvironmentLanguage::Python => {
                    let mut command = sanitized_command(&local_python().await?);
                    command.args(["-m", "venv"]).arg(prepared.join(".venv"));
                    command_output(command, seconds).await?;
                    let requirements = fs::read_to_string(prepared.join("requirements.lock"))
                        .context("Python environment requires requirements.lock")?;
                    if requirements
                        .lines()
                        .any(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
                    {
                        let mut command =
                            sanitized_command(&prepared.join(".venv/bin/python").to_string_lossy());
                        command
                            .args([
                                "-m",
                                "pip",
                                "install",
                                "--require-hashes",
                                "--no-input",
                                "-r",
                            ])
                            .arg(prepared.join("requirements.lock"));
                        command_output(command, seconds).await?;
                    }
                }
                EnvironmentLanguage::TypeScript => {
                    let mut command = sanitized_command("npm");
                    command
                        .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"])
                        .current_dir(&prepared)
                        .env("HOME", &scratch)
                        .env("npm_config_cache", scratch.join("npm-cache"));
                    command_output(command, seconds).await?;
                }
            },
            ExecutionBackend::Docker => {
                ensure_image(language, &cache, seconds).await?;
                let image = match language {
                    EnvironmentLanguage::Python => PYTHON_IMAGE,
                    EnvironmentLanguage::TypeScript => NODE_IMAGE,
                };
                let mut command = sanitized_command("docker");
                command
                    .args([
                        "run",
                        "--rm",
                        "--cap-drop",
                        "ALL",
                        "--security-opt",
                        "no-new-privileges",
                        "--user",
                        "0:0",
                        "--mount",
                    ])
                    .arg(format!(
                        "type=bind,src={},dst=/definition",
                        prepared.display()
                    ))
                    .args(["--workdir", "/definition"]);
                command.args([
                    "--name",
                    &format!("prime-env-prepare-{}", Uuid::new_v4()),
                    "--cpus",
                    &policy.cpus.to_string(),
                    "--memory",
                    &format!("{}m", policy.memory_mib),
                    image,
                ]);
                match language {
                    EnvironmentLanguage::Python => {
                        command.args([
                            "python",
                            "-m",
                            "pip",
                            "install",
                            "--require-hashes",
                            "--no-input",
                            "--target",
                            "/definition/.python-deps",
                            "-r",
                            "requirements.lock",
                        ]);
                    }
                    EnvironmentLanguage::TypeScript => {
                        command.args(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"]);
                    }
                }
                command_output(command, seconds).await?;
            }
        }
        write_json(&cache.join("ready.json"), &json!({"hash":hash}))?;
        Ok(prepared)
    }

    pub(super) async fn run_checks(
        &self,
        source: &Path,
        scratch: &Path,
        language: EnvironmentLanguage,
        policy: &ExecutionPolicy,
    ) -> Result<()> {
        let checks = match language {
            EnvironmentLanguage::Python => "checks.py",
            EnvironmentLanguage::TypeScript => "checks.ts",
        };
        if !source.join(checks).is_file() {
            bail!("environment must provide {checks}");
        }
        let mut command = match policy.backend {
            ExecutionBackend::Local => {
                let mut command = match language {
                    EnvironmentLanguage::Python => {
                        sanitized_command(&source.join(".venv/bin/python").to_string_lossy())
                    }
                    EnvironmentLanguage::TypeScript => sanitized_command("node"),
                };
                command
                    .arg(source.join(checks))
                    .current_dir(scratch)
                    .env("PYTHONPATH", source)
                    .env("PYTHONDONTWRITEBYTECODE", "1");
                command
            }
            ExecutionBackend::Docker => {
                let image = match language {
                    EnvironmentLanguage::Python => PYTHON_IMAGE,
                    EnvironmentLanguage::TypeScript => NODE_IMAGE,
                };
                let name = format!("prime-env-checks-{}", Uuid::new_v4());
                let mut command = docker_command(image, source, scratch, policy, &name)?;
                command
                    .arg(match language {
                        EnvironmentLanguage::Python => "python",
                        EnvironmentLanguage::TypeScript => "node",
                    })
                    .arg(format!("/definition/{checks}"));
                command
            }
        };
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command_output(command, policy.call_timeout_seconds)
            .await
            .context("generated behavioral checks failed")
    }
}
