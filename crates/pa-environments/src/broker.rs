//! Trusted host integrations. Generated environments can select names, never commands.
use crate::runner::sanitized_command;
use crate::{HostCall, HostCallHandler};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// An integration was dispatched but no valid completion receipt was obtained.
/// The caller must retain its intent for explicit reconciliation.
#[derive(Debug)]
pub struct UncertainIntegration(pub String);

impl std::fmt::Display for UncertainIntegration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for UncertainIntegration {}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Integration {
    program: PathBuf,
    #[serde(default)]
    arguments: Vec<String>,
    input_schema: Value,
    output_schema: Value,
    /// Environment variable name => named secret reference. Values never enter workers.
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// Load user-configured integrations outside definitions and instance workspaces.
/// Commands receive one JSON request on stdin, including host-bound identity,
/// and return a JSON result on stdout. Neither shell evaluation nor provider
/// environment variables are inherited.
/// # Errors
/// Rejects invalid host configuration or schemas.
pub fn command_integrations(config: &Path, deadline: Duration) -> Result<HostCallHandler> {
    let integrations: BTreeMap<String, Integration> = if config.exists() {
        serde_json::from_slice(&std::fs::read(config)?)?
    } else {
        BTreeMap::new()
    };
    for (name, integration) in &integrations {
        crate::store::component(name)?;
        if !integration.program.is_absolute() {
            bail!("integration executables must use absolute paths");
        }
        jsonschema::validator_for(&integration.input_schema)?;
        jsonschema::validator_for(&integration.output_schema)?;
    }
    Ok(Arc::new(move |call: HostCall| {
        let integrations = integrations.clone();
        Box::pin(async move {
            let name = call.params["name"].as_str().context("integration name")?;
            let integration = integrations
                .get(name)
                .context("selected integration is unavailable on this host")?;
            let arguments = &call.params["arguments"];
            jsonschema::validator_for(&integration.input_schema)?
                .validate(arguments)
                .map_err(|error| anyhow::anyhow!("invalid integration input: {error}"))?;
            let mut command = sanitized_command(&integration.program.to_string_lossy());
            command
                .args(&integration.arguments)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            for (variable, reference) in &integration.secrets {
                crate::store::component(reference)?;
                command.env(
                    variable,
                    std::env::var(format!("PRIME_ENV_INTEGRATION_SECRET_{reference}"))
                        .with_context(|| {
                            format!("integration secret {reference} is unavailable")
                        })?,
                );
            }
            let mut child = command.spawn().context("start host integration")?;
            let completion: Result<Value> = async {
                let mut input = child.stdin.take().context("integration stdin")?;
                input.write_all(&serde_json::to_vec(&call.params)?).await?;
                drop(input);
                let output = tokio::time::timeout(deadline, child.wait_with_output())
                .await
                .context(
                    "host integration timed out; reconcile its external receipt before recovery",
                )??;
                if !output.status.success() {
                    bail!("host integration {name} failed with {}", output.status);
                }
                if output.stdout.len() > 8 * 1024 * 1024 {
                    bail!("integration result exceeds 8 MiB");
                }
                let result: Value = serde_json::from_slice(&output.stdout)
                    .context("integration must return JSON")?;
                jsonschema::validator_for(&integration.output_schema)?
                    .validate(&result)
                    .map_err(|error| anyhow::anyhow!("invalid integration output: {error}"))?;
                Ok(result)
            }
            .await;
            completion.map_err(|error| {
                UncertainIntegration(format!(
                    "host integration {name}: {error:#}; reconcile external state before recovery"
                ))
                .into()
            })
        })
    }))
}
