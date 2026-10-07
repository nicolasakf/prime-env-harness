//! Transitional local seat port; remote seats use the same core boundary.
use anyhow::{bail, Result};
use pa_core::session_engine::environments::SeatPort;
use pa_environments::ActiveInstance;
use pa_types::environments::{SeatBinding, SeatCall, SeatId, SeatReply};
use serde_json::json;
use std::{future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

pub(crate) fn binding(instance: &ActiveInstance, seat: &SeatId) -> Result<SeatBinding> {
    let roster = instance.roster();
    let entry = roster
        .seats
        .iter()
        .find(|entry| &entry.id == seat)
        .ok_or_else(|| anyhow::anyhow!("unknown seat"))?;
    Ok(SeatBinding {
        protocol_version: instance.definition.manifest.protocol_version,
        instance_id: instance.instance.id.clone(),
        definition_id: instance.instance.definition_id.clone(),
        version: instance.instance.version.clone(),
        seat: seat.clone(),
        role: entry.role.clone(),
        actions: instance.actions.clone(),
        manifest_name: instance.definition.manifest.name.clone(),
        instructions: instance.definition.manifest.instructions.clone(),
        python: instance.instance.policy.python_execution
            && instance.definition.manifest.python_execution,
        direct_messages: roster.direct_messages,
        peer_roles: roster
            .seats
            .iter()
            .filter(|peer| peer.id != *seat)
            .map(|peer| (peer.id.clone(), peer.role.clone()))
            .collect(),
        peers: roster
            .seats
            .iter()
            .filter(|entry| &entry.id != seat)
            .map(|entry| entry.id.clone())
            .collect(),
        config: instance.setup_config()?,
        agent_context: instance.definition.manifest.agent_context,
        turn_timeout_seconds: roster.limits.seat_turn_timeout_seconds,
        max_output_tokens: roster.limits.max_output_tokens,
    })
}

pub(crate) struct InProcessPort(pub Arc<ActiveInstance>);
impl SeatPort for InProcessPort {
    fn call(&self, call: SeatCall) -> Pin<Box<dyn Future<Output = Result<SeatReply>> + Send + '_>> {
        Box::pin(async move {
            let result = match call {
                SeatCall::Observe => self.0.observe(&SeatId::legacy_agent()).await?,
                SeatCall::AgentView => self.0.agent_view(&SeatId::legacy_agent()).await?,
                SeatCall::Invoke {
                    action,
                    arguments,
                    operation_id,
                    view_version: _,
                } => {
                    self.0
                        .invoke(
                            &SeatId::legacy_agent(),
                            &action,
                            arguments,
                            &operation_id,
                            CancellationToken::new(),
                        )
                        .await?
                }
                SeatCall::Say {
                    text,
                    operation_id,
                    view_version: _,
                } => {
                    self.0
                        .input_seat(&SeatId::legacy_agent(), &text, &operation_id)
                        .await?
                }
                SeatCall::Python {
                    code,
                    operation_id,
                    view_version: _,
                } => {
                    self.0
                        .execute_python(&code, &operation_id, CancellationToken::new())
                        .await?
                }
                SeatCall::CancelPython { .. } => {
                    anyhow::bail!("in-process Python cancellation requires its execution signal")
                }
                SeatCall::TurnStarted { .. }
                | SeatCall::TurnSettled { .. }
                | SeatCall::TurnFailed { .. } => json!({}),
                SeatCall::Message { .. } => bail!("direct messages require an environment host"),
            };
            Ok(SeatReply {
                result,
                deliveries: Vec::new(),
                view: None,
                view_version: None,
                stale_view: false,
            })
        })
    }
}

#[cfg(test)]
#[path = "environment_port_tests.rs"]
mod tests;
