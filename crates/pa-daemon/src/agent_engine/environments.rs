//! Durable participant bindings and builder management host requests.
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use pa_core::kernel::shared::{host_handler, HostRequestHandlers};
use pa_environments::{ActiveInstance, EnvironmentManager};
use pa_types::environments::{EnvironmentSessionProfile, ExecutionPolicy, InstanceRun};
use serde_json::{json, Value};

use super::environment_builder::{control_run, intervene, management};
use super::AgentSessionEngine;
use crate::environment_host::Requester;

impl AgentSessionEngine {
    pub(crate) async fn environment_command(
        &self,
        operation: &str,
        params: &Value,
    ) -> Result<Value> {
        let manager = self.environment_manager()?;
        // Do not retain the host-slot lock across a call: cancellation and human
        // controls must reach the host while another request executes Python.
        let host = self.environment_host.lock().await.clone();
        if let Some(host) = host {
            if operation != "leave" {
                return host.client_operation(operation, params).await;
            }
        }
        if self.is_environment_host()? && operation != "leave" {
            bail!("Environment host is closed");
        }
        if self.seat_port()?.is_some() {
            bail!("seat clients may only receive authenticated wakes");
        }
        if self.participant_binding()?.is_some()
            && !matches!(operation, "status" | "observe" | "act" | "leave")
        {
            bail!("environment participant policy denies builder management");
        }
        match operation {
            "catalog" => Ok(json!({"definitions":manager.store().list()?})),
            "instances" => Ok(json!({"instances":manager.store().list_instances()?})),
            // The live runs; `include_ended` also lists the ended (terminal)
            // runs, whose transcripts stay readable through `transcript`.
            "controls" => {
                let include_ended = params["include_ended"].as_bool().unwrap_or(false);
                let mut runs = Vec::new();
                for instance in manager.store().list_instances()? {
                    let directory = manager.store().instance_path(&instance.id)?;
                    let path = directory.join("run.json");
                    if !path.exists() {
                        continue;
                    }
                    let state: crate::environment_host::RunState =
                        crate::environment_host::read(&path)?;
                    if include_ended || !state.phase.is_terminal() {
                        runs.push(InstanceRun {
                            instance,
                            phase: state.phase,
                            reason: state.reason,
                            builder_session_id: crate::environment_host::read(
                                &directory.join("builder-session.json"),
                            )
                            .ok(),
                            builder_saved_session_id: crate::environment_host::read(
                                &directory.join("builder-saved-session.json"),
                            )
                            .ok(),
                        });
                    }
                }
                Ok(json!({"runs":runs}))
            }
            // A run's transcript, read from its saved conversation without
            // attaching a session to it, so viewing never changes the run.
            "transcript" => {
                let path = manager
                    .store()
                    .instance_path(required(params, "id")?)?
                    .join("host-conversation.jsonl");
                if !path.exists() {
                    bail!("this run has no saved transcript");
                }
                let messages = tokio::task::spawn_blocking(move || {
                    crate::session_store::SessionFile::open_windowed(&path)
                        .map(|file| file.messages())
                })
                .await??;
                Ok(json!({"messages":messages}))
            }
            "pause" | "stop" => {
                control_run(
                    &manager,
                    &self.link,
                    &self.config,
                    required(params, "id")?,
                    operation,
                    "human",
                )
                .await
            }
            "act_for_seat" | "suspend_seat" | "resume_seat" => {
                intervene(
                    &manager,
                    &self.link,
                    required(params, "id")?,
                    operation,
                    params,
                    Requester::Human,
                )
                .await
            }
            "inspect" => Ok(serde_json::to_value(
                manager
                    .store()
                    .definition(required(params, "id")?, params["version"].as_str())?
                    .0,
            )?),
            "policy" => {
                let policy: ExecutionPolicy = serde_json::from_value(params["policy"].clone())?;
                manager.select_policy(required(params, "draft")?, &policy)?;
                Ok(json!({"policy":policy,"selected":true}))
            }
            "recover" => {
                manager.recover(
                    required(params, "id")?,
                    &serde_json::from_value(params["recovery"].clone())?,
                )?;
                Ok(json!({"recovered":true}))
            }
            "create" | "validate" | "save" => {
                let name = format!("{operation}_environment");
                management(&manager, &name, params).await
            }
            // From the builder a live view is always public: seat views belong to the
            // human at that run's own table.
            "view" => {
                let id = required(params, "id")?;
                let path = manager.store().instance_path(id)?;
                let host: Value = crate::environment_host::read(&path.join("host-session.json"))
                    .context(
                    "this run has no live host; replay an ended run with the viewer tool instead",
                )?;
                let response = self.link.request(json!({"type":"env","activeSessionId":host["activeSessionId"],"operation":"view","parameters":{"perspective":{"kind":"public"},"viewer":params["viewer"]}}), std::time::Duration::from_secs(180)).await?;
                if !response.success {
                    bail!(
                        "live view unavailable: {}",
                        response.error.unwrap_or_default()
                    );
                }
                Ok(response.data.unwrap_or(Value::Null))
            }
            "watch" => {
                let id = required(params, "id")?;
                let instance = manager.store().instance(id)?;
                if instance
                    .roster
                    .clone()
                    .unwrap_or_default()
                    .seats
                    .iter()
                    .any(|seat| seat.occupant == pa_types::environments::OccupantKind::Human)
                {
                    bail!("watch is available only for agents-only runs");
                }
                let path = manager.store().instance_path(id)?;
                let host: Value = crate::environment_host::read(&path.join("host-session.json"))?;
                let response = self.link.request(json!({"type":"env","activeSessionId":host["activeSessionId"],"operation":"status","parameters":{}}), std::time::Duration::from_secs(180)).await?;
                let session = if response.success {
                    json!({"activeSessionId":host["activeSessionId"]})
                } else if response
                    .error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("Unknown active session"))
                {
                    let previous: Value =
                        crate::environment_host::read(&path.join("participant-session.json"))?;
                    let config: Value =
                        crate::environment_host::read(&path.join("host-config.json"))?;
                    let link = crate::supervisor_link::SupervisorLink::owned(
                        self.link.socket_path().clone(),
                        required(&previous, "owner")?.into(),
                    );
                    link.request_success(json!({"type":"create","sessionPath":path.join("host-conversation.jsonl"),"config":config,"lifecycle":"client_owned"}), std::time::Duration::from_secs(180)).await?
                } else {
                    bail!("table unavailable: {}", response.error.unwrap_or_default());
                };
                crate::environment_telemetry::track(
                    crate::environment_telemetry::client(&self.config).as_ref(),
                    "environment_spectate",
                    "completed",
                    None,
                );
                Ok(json!({"session":session,"instance_id":id}))
            }
            "enter" | "resume" => {
                let parent = self
                    .config
                    .supervisor_link
                    .as_ref()
                    .context("entering environments requires a daemon session")?
                    .active_session_id
                    .clone();
                let owner = format!("env-builder-{parent}");
                let owned_link = crate::supervisor_link::SupervisorLink::owned(
                    self.link.socket_path().clone(),
                    owner.clone(),
                );
                if operation == "resume" {
                    let binding = manager
                        .store()
                        .instance_path(required(params, "id")?)?
                        .join("participant-session.json");
                    if binding.exists() {
                        let previous: Value = serde_json::from_slice(&std::fs::read(binding)?)?;
                        let link = crate::supervisor_link::SupervisorLink::owned(
                            self.link.socket_path().clone(),
                            required(&previous, "owner")?.into(),
                        );
                        let stopped = link.request(json!({"type":"complete_owned_session","activeSessionId":previous["activeSessionId"]}), std::time::Duration::from_secs(60)).await?;
                        if !stopped.success
                            && !stopped
                                .error
                                .as_deref()
                                .is_some_and(|error| error.starts_with("Unknown active session"))
                        {
                            bail!("previous participant did not stop: {:?}", stopped.error);
                        }
                    }
                }
                let instance =
                    if operation == "enter" {
                        let policy: ExecutionPolicy = serde_json::from_value(
                            params
                                .get("policy")
                                .context("select an execution policy before entering")?
                                .clone(),
                        )?;
                        let definition = manager
                            .store()
                            .definition(required(params, "id")?, params["version"].as_str())?
                            .0;
                        let mut roster = if let Some(roster) = params.get("roster") {
                            super::environment_builder::roster(roster)?
                        } else {
                            pa_environments::default_roster(&definition.manifest)?
                        };
                        let selection = self.current_selection();
                        let inherited = match (&selection.provider, &selection.model) {
                            (Some(provider), Some(model)) => Some(format!("{provider}/{model}")),
                            _ => None,
                        };
                        let allowlist =
                            crate::model_allowlist::load(&self.cwd(), &self.config.agent_dir);
                        for seat in roster.seats.iter_mut().filter(|seat| {
                            seat.occupant == pa_types::environments::OccupantKind::Agent
                        }) {
                            if self.config.faux_script.is_some() && seat.model.is_none() {
                                continue;
                            }
                            let model = crate::rlm_child_model::resolve_child_model(
                                &self.config.agent_dir,
                                seat.model.as_deref(),
                                inherited.as_deref(),
                                "environment seat",
                                &allowlist,
                            )?;
                            if let Some(thinking) = seat.thinking {
                                crate::rlm_child_model::assert_thinking_supported(
                                    &self.config.agent_dir,
                                    Some(thinking.wire_name()),
                                    &model,
                                )?;
                            }
                            seat.model = Some(model);
                        }
                        manager
                            .enter_with_roster(
                                required(params, "id")?,
                                params["version"].as_str(),
                                policy,
                                params
                                    .get("setup_arguments")
                                    .cloned()
                                    .unwrap_or_else(|| json!({})),
                                roster,
                            )
                            .await?
                    } else {
                        manager.resume(required(params, "id")?).await?
                    };
                let id = instance.instance.id.clone();
                let path = instance.path().join("host-conversation.jsonl");
                let definition = instance.definition.clone();
                let roster = instance.roster();
                let local_python_warning = instance.instance.policy.backend
                    == pa_types::environments::ExecutionBackend::Local
                    && instance.instance.policy.python_execution
                    && roster
                        .seats
                        .iter()
                        .filter(|seat| seat.occupant == pa_types::environments::OccupantKind::Agent)
                        .count()
                        > 1;
                instance.leave().await?;
                drop(instance);
                let mut defaults = json!({});
                let selection = self.current_selection();
                defaults["provider"] = json!(selection.provider);
                defaults["model"] = json!(selection.model);
                if let Some(script) = &self.config.faux_script {
                    let script_path = manager
                        .store()
                        .instance_path(&id)?
                        .join("faux-participant.json");
                    std::fs::write(&script_path, script)?;
                    defaults["script"] = json!(script_path);
                }
                let token_path = manager
                    .store()
                    .instance_path(&id)?
                    .join("builder-token.json");
                let builder_token: String = if token_path.exists() {
                    crate::environment_host::read(&token_path)?
                } else {
                    let token = format!(
                        "{}{}",
                        uuid::Uuid::new_v4().simple(),
                        uuid::Uuid::new_v4().simple()
                    );
                    crate::environment_host::write(&token_path, &token)?;
                    token
                };
                let config = json!({"cwd":manager.store().instance_path(&id)?,"sessionDir":manager.store().instance_path(&id)?,"environmentHost":{"instance":id,"builder_token":builder_token,"seat_defaults":defaults}});
                crate::environment_host::write(
                    &manager.store().instance_path(&id)?.join("host-config.json"),
                    &config,
                )?;
                // Root session names are daemon-wide; each game needs its own
                // name, stable across resumes of the same persisted instance.
                let name = format!("{} ({id})", definition.manifest.name);
                crate::environment_host::write(
                    &manager
                        .store()
                        .instance_path(&id)?
                        .join("builder-session.json"),
                    &parent,
                )?;
                // The active id changes whenever the builder reopens; its saved
                // session id does not, so the run stays with its builder.
                let saved_session = self
                    .session_file
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .and_then(|file| pa_core::session::manager::read_session_header(&file));
                if let Some(header) = saved_session {
                    crate::environment_host::write(
                        &manager
                            .store()
                            .instance_path(&id)?
                            .join("builder-saved-session.json"),
                        &header.id,
                    )?;
                }
                let response = owned_link.request_success(json!({"type":"create","sessionPath":path,"name":name,"config":config,"lifecycle":"client_owned"}), std::time::Duration::from_secs(180)).await?;
                crate::environment_host::write(
                    &manager
                        .store()
                        .instance_path(&id)?
                        .join("participant-session.json"),
                    &json!({"activeSessionId":response["activeSessionId"],"owner":owner}),
                )?;
                std::fs::write(
                    manager
                        .store()
                        .instance_path(&id)?
                        .join("builder-session.json"),
                    serde_json::to_vec(&parent)?,
                )?;
                let telemetry = crate::environment_telemetry::client(&self.config);
                crate::environment_telemetry::track(
                    telemetry.as_ref(),
                    "environment_table_view",
                    "completed",
                    None,
                );
                if definition.manifest.protocol_version == 2 {
                    crate::environment_telemetry::track(
                        telemetry.as_ref(),
                        "environment_contract_v2",
                        "completed",
                        None,
                    );
                }
                if roster.seats.len() > 2 {
                    crate::environment_telemetry::track(
                        telemetry.as_ref(),
                        "environment_multi_seat",
                        "completed",
                        None,
                    );
                }
                Ok(
                    json!({"session":response,"instance_id":id,"definition":definition,"warning":if local_python_warning { Some("Local Python seats share host-user filesystem access. Choose Docker for adversarial play.") } else { None }}),
                )
            }
            "status" | "observe" | "act" => {
                let instance = self
                    .active_environment()
                    .await?
                    .context("this session is an environment builder")?;
                if operation == "act" {
                    let id = required(params, "operation_id")?;
                    let result = instance
                        .invoke(
                            &pa_types::environments::SeatId::legacy_human(),
                            required(params, "action")?,
                            params["arguments"].clone(),
                            id,
                            tokio_util::sync::CancellationToken::new(),
                        )
                        .await?;
                    return Ok(
                        json!({"result":result,"observation":instance.observe(&pa_types::environments::SeatId::legacy_human()).await?}),
                    );
                }
                Ok(
                    json!({"instance":instance.instance,"definition":instance.definition,"actions":instance.actions,"observation":instance.observe(&pa_types::environments::SeatId::legacy_human()).await?}),
                )
            }
            "leave" => {
                let id = self
                    .participant_binding()?
                    .context("this session is not an environment participant")?;
                // Leaving must also work after the runtime has closed, without
                // resuming the instance just to find the builder session.
                let parent: String = serde_json::from_slice(&std::fs::read(
                    manager
                        .store()
                        .instance_path(&id)?
                        .join("builder-session.json"),
                )?)?;
                if self.close_environment_on_detach()? {
                    self.leave_environment().await?;
                    self.mark_session_closed();
                }
                Ok(json!({"builder_session_id":parent}))
            }
            _ => bail!("unknown environment command"),
        }
    }

    pub(crate) fn environment_manager(&self) -> Result<EnvironmentManager> {
        let broker = pa_environments::command_integrations(
            &self.config.agent_dir.join("environment-integrations.json"),
            std::time::Duration::from_secs(120),
        )?;
        EnvironmentManager::new(self.config.agent_dir.join("environments"), Some(broker))
    }

    fn binding_file(&self) -> Option<PathBuf> {
        self.session_file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|path| path.with_extension("environment.json"))
    }

    pub(crate) fn participant_binding(&self) -> Result<Option<String>> {
        let resources = self
            .create_resources
            .read()
            .map_err(|_| anyhow::anyhow!("participant config lock poisoned"))?;
        let configured = resources
            .environment_host
            .as_ref()
            .map(|host| host.instance.clone())
            .or_else(|| {
                resources
                    .environment_seat
                    .as_ref()
                    .map(|seat| seat.instance.clone())
            })
            .or_else(|| resources.environment_instance.clone());
        if let Some(path) = self.binding_file().filter(|path| path.exists()) {
            let binding: EnvironmentSessionProfile = serde_json::from_slice(&std::fs::read(path)?)?;
            if configured
                .as_ref()
                .is_some_and(|id| id != &binding.instance_id)
            {
                bail!("participant binding conflicts with the saved session");
            }
            return Ok(Some(binding.instance_id));
        }
        Ok(configured)
    }

    pub(crate) fn participant_profile(&self) -> Result<Option<EnvironmentSessionProfile>> {
        self.binding_file()
            .filter(|path| path.exists())
            .map(|path| serde_json::from_slice(&std::fs::read(path)?).map_err(Into::into))
            .transpose()
    }

    pub(crate) async fn active_environment(&self) -> Result<Option<Arc<ActiveInstance>>> {
        if self.session_is_closed() {
            bail!("participant session has been left; explicitly resume its instance");
        }
        if self
            .create_resources
            .read()
            .map_err(|_| anyhow::anyhow!("environment config lock poisoned"))?
            .environment_seat
            .is_some()
        {
            return Ok(None);
        }
        let Some(id) = self.participant_binding()? else {
            return Ok(None);
        };
        let mut slot = self.environment.lock().await;
        if let Some(instance) = slot.as_ref() {
            return Ok(Some(Arc::clone(instance)));
        }
        let instance = self.environment_manager()?.resume(&id).await?;
        let path = self
            .binding_file()
            .context("participant sessions require transcript persistence")?;
        let profile = EnvironmentSessionProfile {
            role: if self.is_environment_host()? {
                pa_types::environments::ProfileRole::Host
            } else {
                pa_types::environments::ProfileRole::Legacy
            },
            instance_id: instance.instance.id.clone(),
            definition_id: instance.instance.definition_id.clone(),
            version: instance.instance.version.clone(),
            policy: instance.instance.policy.clone(),
            python_execution: instance.definition.manifest.python_execution
                && instance.instance.policy.python_execution,
        };
        // Written before the participant session is assembled. A later create
        // cannot omit this binding to regain builder tools.
        if path.exists() {
            let saved: EnvironmentSessionProfile = serde_json::from_slice(&std::fs::read(&path)?)?;
            if saved != profile {
                instance.leave().await?;
                bail!(
                    "participant profile conflicts with the pinned definition or execution policy"
                );
            }
        } else {
            let temporary = path.with_extension("environment.tmp");
            std::fs::write(&temporary, serde_json::to_vec(&profile)?)?;
            std::fs::File::open(&temporary)?.sync_all()?;
            std::fs::rename(temporary, path)?;
        }
        *slot = Some(Arc::clone(&instance));
        Ok(Some(instance))
    }

    pub(crate) fn close_environment_on_detach(&self) -> Result<bool> {
        let Some(id) = self.participant_binding()? else {
            return Ok(false);
        };
        if !self.is_environment_host()? {
            return Ok(true);
        }
        Ok(self
            .environment_manager()?
            .store()
            .instance(&id)?
            .roster
            .unwrap_or_default()
            .seats
            .iter()
            .any(|seat| seat.occupant == pa_types::environments::OccupantKind::Human))
    }

    pub(crate) fn is_environment_host(&self) -> Result<bool> {
        Ok(self
            .create_resources
            .read()
            .map_err(|_| anyhow::anyhow!("environment config lock poisoned"))?
            .environment_host
            .is_some())
    }

    pub(crate) fn seat_port(&self) -> Result<Option<Arc<crate::environment_seat::LinkSeatPort>>> {
        let config = self
            .create_resources
            .read()
            .map_err(|_| anyhow::anyhow!("environment config lock poisoned"))?
            .environment_seat
            .clone();
        config
            .map(|config| {
                crate::environment_seat::LinkSeatPort::new(
                    &self.config.agent_dir.join("environments"),
                    self.link.socket_path().clone(),
                    config,
                )
            })
            .transpose()
    }

    pub(crate) fn persist_seat_profile(&self) -> Result<()> {
        if let Some(port) = self.seat_port()? {
            let binding = port.binding();
            let instance = self
                .environment_manager()?
                .store()
                .instance(&binding.instance_id)?;
            let profile = EnvironmentSessionProfile {
                role: pa_types::environments::ProfileRole::Seat {
                    seat: binding.seat,
                    role: binding.role,
                },
                instance_id: binding.instance_id,
                definition_id: binding.definition_id,
                version: binding.version,
                policy: instance.policy,
                python_execution: binding.python,
            };
            let path = self.binding_file().context("seat transcript required")?;
            if path.exists()
                && crate::environment_host::read::<EnvironmentSessionProfile>(&path)? != profile
            {
                bail!("seat binding conflicts with saved profile");
            }
            crate::environment_host::write(&path, &profile)?;
        }
        Ok(())
    }

    pub(crate) async fn leave_environment(&self) -> Result<()> {
        // Keep the host reachable for completion reports while its seats stop.
        // Taking it before close would make a settling seat lose its endpoint.
        let host = self.environment_host.lock().await.clone();
        if let Some(host) = host {
            host.close().await?;
            self.environment_host.lock().await.take();
        }
        let instance = self.environment.lock().await.take();
        if let Some(instance) = instance {
            instance.leave().await?;
        }
        Ok(())
    }

    pub(super) fn environment_builder_handlers(&self) -> HostRequestHandlers {
        let root = self.config.agent_dir.join("environments");
        let mut handlers = HostRequestHandlers::default();
        let telemetry = (self.config.telemetry_disabled != Some(true)).then(|| {
            let settings =
                pa_core::settings::SettingsManager::create(self.cwd(), &self.config.agent_dir);
            pa_core::session_engine::telemetry::build_client(&settings, &self.config.agent_dir)
        });
        for operation in [
            "create_environment",
            "select_policy",
            "validate_environment",
            "save_environment",
            "list_environments",
        ] {
            let root = root.clone();
            let telemetry = telemetry.clone();
            handlers.register(
                format!("environments.{operation}"),
                host_handler(move |payload| {
                    let root = root.clone();
                    let telemetry = telemetry.clone();
                    async move {
                        let manager = EnvironmentManager::new(root, None)?;
                        let result = management(&manager, operation, &payload.data).await;
                        if let Some(telemetry) = telemetry.filter(|_| {
                            matches!(operation, "select_policy" | "validate_environment")
                        }) {
                            let mut properties = pa_telemetry::base_properties("daemon");
                            properties.set("feature_id", json!(uuid::Uuid::new_v4().to_string()));
                            properties.set(
                                "feature_name",
                                json!(if operation == "select_policy" {
                                    "environment_builder_policy"
                                } else {
                                    "environment"
                                }),
                            );
                            properties.set(
                                "outcome",
                                json!(if result.is_ok() {
                                    "completed"
                                } else {
                                    "failed"
                                }),
                            );
                            telemetry.track("agent feature outcome", properties);
                        }
                        result
                    }
                }),
            );
        }
        super::environment_builder::register(&mut handlers, &self.config, &self.session_file);
        handlers
    }
}

pub(crate) fn required<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} is required"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_engine::AgentEngineConfig;
    use pa_core::kernel::shared::HostRequestPayload;

    #[tokio::test]
    async fn builder_policy_selection_enables_validation_and_save() {
        let root = tempfile::tempdir().unwrap();
        let engine = AgentSessionEngine::new(AgentEngineConfig {
            cwd: root.path().into(),
            agent_dir: root.path().join("agent"),
            provider: None,
            model: None,
            api_key: None,
            thinking: None,
            session_dir: None,
            session_file: None,
            faux_script: None,
            supervisor_link: None,
            telemetry_disabled: Some(true),
            cron_store: None,
            queued_steering_probe: None,
        })
        .unwrap();
        let handlers = engine.environment_builder_handlers();
        let created =
            handlers.get("environments.create_environment").unwrap()(HostRequestPayload {
                data: json!({"description":"counter", "language":"python"}),
                cell_source_code: None,
            })
            .await
            .unwrap();
        let draft = created["draft"].as_str().unwrap();
        let manager = engine.environment_manager().unwrap();
        assert!(manager.selected_policy(draft).is_err());
        let policy = json!({"backend":"local", "call_timeout_seconds":30});
        let selected = handlers.get("environments.select_policy").unwrap()(HostRequestPayload {
            data: json!({"draft":draft, "policy":policy}),
            cell_source_code: None,
        })
        .await
        .unwrap();
        let expected = ExecutionPolicy {
            call_timeout_seconds: 30,
            ..ExecutionPolicy::default()
        };
        assert_eq!(selected, json!({"policy":expected,"selected":true}));
        assert_eq!(manager.selected_policy(draft).unwrap(), expected);
        let report =
            handlers.get("environments.validate_environment").unwrap()(HostRequestPayload {
                data: json!({"draft":draft}),
                cell_source_code: None,
            })
            .await
            .unwrap();
        assert_eq!(report["passed"], true, "{report}");
        assert_eq!(report["policy"], json!(expected));
        let saved = handlers.get("environments.save_environment").unwrap()(HostRequestPayload {
            data: json!({"draft":draft,"id":"counter"}),
            cell_source_code: None,
        })
        .await
        .unwrap();
        assert_eq!(saved["id"], "counter");
        assert!(manager.store().list_instances().unwrap().is_empty());
        let manifest_path = PathBuf::from(draft).join("environment.json");
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["instructions"] = json!("changed instructions");
        std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(manager.selected_policy(draft).is_err());
    }
}
