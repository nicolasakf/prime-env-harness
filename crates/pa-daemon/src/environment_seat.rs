//! Seat-bound host transport. Capability secrets never enter model tools or prompts.
use crate::{
    agent_engine::EnvironmentSeatConfig,
    environment_host::{read, write},
    supervisor_link::SupervisorLink,
};
use anyhow::{Context, Result};
use pa_core::session_engine::environments::SeatPort;
use pa_types::environments::{SeatBinding, SeatCall, SeatReply};
use serde_json::{json, Value};
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};

pub(crate) struct LinkSeatPort {
    path: PathBuf,
    config: EnvironmentSeatConfig,
    link: SupervisorLink,
    binding: SeatBinding,
}

impl LinkSeatPort {
    pub fn new(
        root: &std::path::Path,
        socket: PathBuf,
        config: EnvironmentSeatConfig,
    ) -> Result<Arc<Self>> {
        let store = pa_environments::EnvironmentStore::new(root)?;
        let instance = store.instance(&config.instance)?;
        let definition = store
            .definition(&instance.definition_id, Some(&instance.version))?
            .0;
        let setup_config = store.instance_config(&instance.id)?;
        let roster = instance.roster.unwrap_or_default();
        let seat = roster
            .seats
            .iter()
            .find(|seat| seat.id == config.seat)
            .context("unknown bound seat")?;
        let binding = SeatBinding {
            protocol_version: definition.manifest.protocol_version,
            instance_id: instance.id.clone(),
            definition_id: instance.definition_id,
            version: instance.version,
            seat: seat.id.clone(),
            role: seat.role.clone(),
            actions: definition.validation.actions,
            agent_context: definition.manifest.agent_context,
            turn_timeout_seconds: roster.limits.seat_turn_timeout_seconds,
            max_output_tokens: roster.limits.max_output_tokens,
            manifest_name: definition.manifest.name,
            instructions: definition.manifest.instructions,
            python: definition.manifest.python_execution && instance.policy.python_execution,
            direct_messages: roster.direct_messages,
            peer_roles: roster
                .seats
                .iter()
                .filter(|peer| peer.id != seat.id)
                .map(|peer| (peer.id.clone(), peer.role.clone()))
                .collect(),
            peers: roster
                .seats
                .iter()
                .filter(|peer| peer.id != seat.id)
                .map(|peer| peer.id.clone())
                .collect(),
            config: setup_config,
        };
        let path = store.instance_path(&instance.id)?;
        let expected: String = read(
            &path
                .join("seats")
                .join(config.seat.as_str())
                .join("token.json"),
        )?;
        crate::environment_host::authenticate(&expected, &config.token)?;
        Ok(Arc::new(Self {
            path,
            config,
            link: SupervisorLink::new(socket),
            binding,
        }))
    }
    pub fn binding(&self) -> SeatBinding {
        self.binding.clone()
    }

    pub async fn settled(&self, through_seq: u64) -> Result<()> {
        self.call(SeatCall::TurnSettled { through_seq }).await?;
        let path = self
            .path
            .join("seats")
            .join(self.config.seat.as_str())
            .join("inbox.json");
        let _lease = crate::environment_host::inbox_lock(&path)?;
        let mut inbox: SeatInbox = if path.exists() {
            read(&path)?
        } else {
            SeatInbox::default()
        };
        inbox.settled = inbox.settled.max(through_seq);
        if inbox
            .pending
            .as_ref()
            .is_some_and(|payload| payload.through_seq <= inbox.settled)
        {
            inbox.pending = None;
        }
        write(&path, &inbox)
    }
}

impl SeatPort for LinkSeatPort {
    fn call(&self, call: SeatCall) -> Pin<Box<dyn Future<Output = Result<SeatReply>> + Send + '_>> {
        Box::pin(async move {
            let binding: Value = read(&self.path.join("host-session.json"))?;
            let mut command = json!({"type":"env","activeSessionId":binding["activeSessionId"],"operation":"seat_call","parameters":{"seat":self.config.seat,"token":self.config.token,"call":call}});
            let response = self
                .link
                .request(command.clone(), Duration::from_secs(180))
                .await?;
            let data = if response.success {
                response.data.context("missing seat reply")?
            } else if response
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("Unknown active session"))
            {
                let binding: Value = read(&self.path.join("host-session.json"))?;
                command["activeSessionId"] = binding["activeSessionId"].clone();
                self.link
                    .request_success(command, Duration::from_secs(180))
                    .await?
            } else {
                anyhow::bail!(
                    "environment host call failed: {}",
                    response.error.unwrap_or_default()
                );
            };
            serde_json::from_value(data).map_err(Into::into)
        })
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SeatInbox {
    pub admitted: u64,
    pub settled: u64,
    pub pending: Option<pa_types::environments::WakePayload>,
}
