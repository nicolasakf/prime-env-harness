//! Environment catalog, execution selections, and schema-driven action input.
mod events;
mod form;
mod runs;
use super::{AgentView, DaemonCommand, Duration, Map, Result, SessionUi, SubmitBehavior, Value};
use crate::autocomplete::environments::{self, Mode};
pub(crate) use runs::EnvironmentRunsUpdate;
use serde_json::json;

pub(crate) struct EnvironmentRequestFailure {
    pub(crate) origin: String,
    pub(crate) error: String,
}

impl SessionUi {
    pub(super) fn update_environment_completions(&self, view: &mut AgentView) {
        let mut items = environments::action_items(&self.environment_actions);
        if let Some(seats) = self.environment_status["roster"]["seats"].as_array() {
            items.extend(seats.iter().filter_map(|seat| {
                seat["id"]
                    .as_str()
                    .map(|id| crate::autocomplete::CompletionItem {
                        value: format!("msg {id} "),
                        label: id.to_owned(),
                        description: Some(format!(
                            "{} · {}",
                            seat["role"].as_str().unwrap_or_default(),
                            seat["occupant"].as_str().unwrap_or_default()
                        )),
                        argument_hint: Some("Message text".into()),
                        source_tag: None,
                    })
            }));
        }
        items.extend(environments::instance_items(&self.environment_instances));
        if self.environment_profile.is_none() {
            items.extend(environments::run_items(&self.environment_runs));
        }
        items.extend(
            self.environment_instances
                .iter()
                .filter(|instance| {
                    instance.roster.as_ref().is_some_and(|roster| {
                        roster.seats.iter().all(|seat| {
                            seat.occupant == pa_types::environments::OccupantKind::Agent
                        })
                    })
                })
                .map(|instance| crate::autocomplete::CompletionItem {
                    value: format!("watch {}", instance.id),
                    label: instance.id.clone(),
                    description: Some("Watch its public agents-only table".into()),
                    argument_hint: None,
                    source_tag: None,
                }),
        );
        items.extend(environments::definition_items(
            &self.environment_definitions,
        ));
        view.editor
            .set_autocomplete_argument_completions("env", items);
    }

    fn queue_environment_request(
        &mut self,
        operation: &str,
        mut parameters: Value,
        view: &mut AgentView,
    ) -> Result<()> {
        if !self
            .client
            .supports_server_capability(pa_types::environments::ENVIRONMENT_CAPABILITY)
        {
            anyhow::bail!("daemon does not support environments_v1");
        }
        if self.environment_pending.is_some() && operation != "leave" {
            anyhow::bail!("an environment operation is already running");
        }
        let origin = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        parameters["ui_origin"] = json!(origin);
        self.environment_pending = Some(origin.clone());
        self.track_feature_outcome("environment", "initiated", None);
        let client = self.client.clone();
        let active_session_id = self.active_session_id.clone();
        let operation = operation.to_owned();
        let failures = self.environment_failures.clone();
        self.note(
            "Environment operation started. /env leave cancels participant activity.",
            view,
        );
        tokio::spawn(async move {
            let result = tokio::time::timeout(
                Duration::from_mins(13),
                client.request_ok(DaemonCommand::Env {
                    id: None,
                    active_session_id,
                    operation,
                    parameters,
                    rest: Map::default(),
                }),
            )
            .await;
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    let _ = failures.send(EnvironmentRequestFailure { origin, error: format!("Environment request failed: {error:#}. Inspect /env status; use /env leave before retrying an uncertain operation.") });
                }
                Err(error) => {
                    let _ = failures.send(EnvironmentRequestFailure { origin, error: format!("Environment request timed out: {error}. Its outcome may be uncertain; inspect the instance before retrying.") });
                }
            }
        });
        Ok(())
    }
    /// Run phases change without the builder prompt being told, so the pause
    /// and stop pickers refetch them each time they open.
    pub(super) async fn refresh_environment_runs(&mut self) -> Result<()> {
        let result = self.environment_request("controls", json!({})).await?;
        self.environment_runs = serde_json::from_value(result["runs"].clone())?;
        Ok(())
    }

    pub(super) async fn environment_request(
        &self,
        operation: &str,
        parameters: Value,
    ) -> Result<Value> {
        if !self
            .client
            .supports_server_capability(pa_types::environments::ENVIRONMENT_CAPABILITY)
        {
            anyhow::bail!("daemon does not support environments_v1");
        }
        self.bounded_request(
            Duration::from_mins(13),
            DaemonCommand::Env {
                id: None,
                active_session_id: self.active_session_id.clone(),
                operation: operation.into(),
                parameters,
                rest: Map::default(),
            },
        )
        .await
    }

    pub(super) async fn handle_environment(
        &mut self,
        args: &str,
        view: &mut AgentView,
    ) -> Result<()> {
        let (operation, rest) = args
            .trim()
            .split_once(char::is_whitespace)
            .unwrap_or((args.trim(), ""));
        let (selector, detail) = rest
            .trim()
            .split_once(char::is_whitespace)
            .unwrap_or((rest.trim(), ""));
        let mode = if self.environment_profile.is_some() {
            Mode::Participant
        } else {
            Mode::Builder
        };
        if operation.is_empty() || operation == "help" {
            self.track_feature_outcome("environment_command_help", "completed", None);
            self.note(&environments::help(selector, mode), view);
            if selector == "act" && mode == Mode::Participant {
                let state = self.environment_request("status", json!({})).await?;
                let actions: Vec<pa_types::environments::ActionDescriptor> =
                    serde_json::from_value(state["actions"].clone())?;
                for action in actions.iter().filter(|action| {
                    action
                        .participants
                        .contains(&pa_types::environments::RoleName::legacy_human())
                }) {
                    self.note(&environments::action_help(action), view);
                }
            }
            if operation.is_empty() {
                view.editor.set_text("/env ");
                view.editor
                    .request_autocomplete(/*force*/ false, /*explicit_tab*/ false);
            }
            return Ok(());
        }
        if self.environment_profile.is_some()
            && !matches!(
                operation,
                "act"
                    | "status"
                    | "observe"
                    | "leave"
                    | "cancel"
                    | "seats"
                    | "msg"
                    | "pause"
                    | "resume_run"
                    | "stop"
                    | "resume"
            )
        {
            anyhow::bail!("Leave the environment before using builder commands.");
        }
        let result = match operation {
            "enter" | "resume" | "pause" | "stop"
                if selector.is_empty() && self.environment_profile.is_none() =>
            {
                let (items, none_found) = match operation {
                    "enter" => {
                        let catalog = self.environment_request("catalog", json!({})).await?;
                        self.environment_definitions = serde_json::from_value(catalog["definitions"].clone())?;
                        (environments::definition_items(&self.environment_definitions), "No saved environments. Use /env create <description> to build one.")
                    }
                    "resume" => {
                        let result = self.environment_request("instances", json!({})).await?;
                        self.environment_instances = serde_json::from_value(result["instances"].clone())?;
                        (environments::instance_items(&self.environment_instances), "No environment sessions to resume. Use /env enter to start one.")
                    }
                    _ => {
                        self.refresh_environment_runs().await?;
                        let prefix = format!("{operation} ");
                        let mut items = environments::run_items(&self.environment_runs);
                        items.retain(|item| item.value.starts_with(&prefix));
                        (items, if operation == "pause" { "No running environment sessions to pause." } else { "No environment sessions to stop." })
                    }
                };
                self.track_feature_outcome("environment_instance_picker", "completed", None);
                self.update_environment_completions(view);
                if items.is_empty() {
                    self.note(none_found, view);
                } else {
                    view.editor.set_text(&format!("/env {operation} "));
                    view.editor.request_autocomplete(/*force*/ false, /*explicit_tab*/ false);
                    self.note("Choose an environment session. ↑/↓ navigate; Tab/Enter selects; Enter submits. Escape closes.", view);
                }
                return Ok(());
            }
            "list" | "catalog" => {
                let result = self.environment_request("catalog", json!({})).await?;
                self.track_feature_outcome("environment_list", "completed", None);
                if operation == "list" {
                    let definitions: Vec<pa_types::environments::Definition> =
                        serde_json::from_value(result["definitions"].clone())?;
                    if definitions.is_empty() {
                        self.note("No saved environments.", view);
                    } else {
                        let entries: std::collections::BTreeSet<_> = definitions
                            .into_iter()
                            .map(|definition| (definition.id, definition.manifest.name))
                            .collect();
                        let rows: Vec<_> = entries.into_iter()
                            .map(|(id, name)| format!("{id} — {name}"))
                            .collect();
                        self.plain_row(&rows.join("\n"), view);
                    }
                    return Ok(());
                }
                result
            }
            "create" => {
                if rest.trim().is_empty() {
                    anyhow::bail!("usage: /env create <description>");
                }
                let created = self
                    .environment_request("create", json!({"description":rest}))
                    .await?;
                let draft = created["draft"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("builder did not return a draft"))?;
                let message = format!("Build this executable environment: {rest}\nDraft: {draft}\nRead the environments skill and start with its design conversation: raise the open design decisions with a recommendation for each, accept deferrals, and confirm a short design summary before writing code. Then choose Python or TypeScript as appropriate, implement the contract and behavioral checks, lock dependencies, and explain actions, capabilities and the Docker recommendation. Ask the user which execution policy they want and wait for their answer, then apply it with environments.select_policy; they may also use /env policy manually. Do not choose or expand permissions or execute generated code before that selection. After policy selection, validate automatically without asking for separate approval, repair at most three times, then save the validated definition. Save first and wait for the user to enter.");
                self.send_prompt(&message, SubmitBehavior::Steer, view)?;
                created
            }
            "policy" => {
                let policy: Value = serde_json::from_str(detail).map_err(|error| anyhow::anyhow!("usage: /env policy <draft> {{\"backend\":\"local\"|\"docker\",...}}: {error}"))?;
                self.environment_request("policy", json!({"draft":selector,"policy":policy}))
                    .await?
            }
            "validate" => {
                self.queue_environment_request("validate", json!({"draft":selector}), view)?;
                return Ok(());
            }
            "save" => {
                let saved = self.environment_request("save", json!({"draft":selector,"id":detail.trim()})).await?;
                let definition: pa_types::environments::Definition = serde_json::from_value(saved.clone())?;
                self.environment_definitions.retain(|existing| existing.id != definition.id || existing.version != definition.version);
                self.environment_definitions.push(definition);
                self.update_environment_completions(view);
                saved
            }
            "inspect" => {
                self.environment_request("inspect", json!({"id":selector}))
                    .await?
            }
            "enter" => {
                let definition = self
                    .environment_request("inspect", json!({"id":selector}))
                    .await?;
                if detail.trim().starts_with('{') {
                    let parameters: Value = serde_json::from_str(detail)?;
                    if parameters.get("roster").is_some() {
                        let mut parameters = parameters;
                        parameters["id"] = json!(selector);
                        self.queue_environment_request("enter", parameters, view)?;
                        return Ok(());
                    }
                }
                let (policy_text, roster) = if detail.trim().starts_with("local ") || detail.trim().starts_with("docker ") {
                    let (backend, roster) = detail.trim().split_once(' ').expect("matched backend and roster");
                    (backend, Some(serde_json::from_str::<Value>(roster)?))
                } else { (detail.trim(), None) };
                let policy = if policy_text.is_empty() { None } else {
                    Some(match policy_text {
                        "local" | "docker" => json!({"backend":policy_text}),
                        _ => serde_json::from_str(policy_text)?,
                    })
                };
                let definition: pa_types::environments::Definition = serde_json::from_value(definition)?;
                if let Some(roster) = roster {
                    self.queue_environment_request("enter", json!({"id":selector,"policy":policy.ok_or_else(|| anyhow::anyhow!("choose execution policy"))?,"roster":roster}), view)?;
                    return Ok(());
                }
                if policy.is_none() || definition.manifest.setup_schema.is_some() {
                    self.track_feature_outcome("environment_setup_form", "initiated", None);
                    view.editor.cancel_autocomplete();
                    view.environment_panel = Some(crate::environment_panel::EnvironmentPanel::setup(&definition, policy, view.editor.keybindings()));
                    self.dirty = true;
                    return Ok(());
                }
                let policy = policy.expect("explicit execution policy");
                self.queue_environment_request(
                    "enter",
                    json!({"id":selector,"policy":policy}),
                    view,
                )?;
                return Ok(());
            }
            "resume" if self.environment_profile.is_some() => self.environment_request("resume_run", json!({})).await?,
            "seats" => self.environment_request("status", json!({})).await?["roster"]["seats"].clone(),
            "status" | "observe" => {
                let status = self.environment_request(operation, json!({})).await?;
                self.environment_actions = serde_json::from_value(status["actions"].clone())?;
                self.environment_status = status.clone();
                self.update_environment_completions(view);
                self.note(
                    &format!(
                        "Environment {} · {}{}",
                        status["instance"]["id"].as_str().unwrap_or("table"),
                        status["run"]["phase"].as_str().unwrap_or("unknown"),
                        status["run"]["reason"].as_str().map_or(String::new(), |reason| format!(" · {reason}")),
                    ),
                    view,
                );
                status["observation"].clone()
            }
            "pause" | "resume_run" | "stop" => {
                let parameters = if self.environment_profile.is_some() { json!({}) } else { json!({"id":selector}) };
                let result = self.environment_request(operation, parameters).await?;
                if self.environment_profile.is_none() {
                    self.environment_runs.retain(|run| run.instance.id != selector);
                    self.update_environment_completions(view);
                    // A pause leaves the run listed: re-read instead of
                    // keeping the local removal.
                    self.spawn_environment_runs_refresh();
                }
                result
            }
            "msg" => self.environment_request("msg", json!({"to":selector,"text":detail,"operation_id":format!("human-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos())})).await?,
            "runs" => self.environment_request("instances", json!({})).await?,
            "watch" => { self.queue_environment_request("watch", json!({"id":selector}), view)?; return Ok(()); }
            "resume" => {
                self.queue_environment_request("resume", json!({"id":selector}), view)?;
                return Ok(());
            }
            "leave" => {
                self.queue_environment_request("leave", json!({}), view)?;
                return Ok(());
            }
            "act" => {
                let shown_version = self.environment_status["view_version"].as_u64();
                let state = self.environment_request("status", json!({})).await?;
                self.environment_actions = serde_json::from_value(state["actions"].clone())?;
                self.update_environment_completions(view);
                if detail.trim().is_empty() {
                    self.open_environment_actions(view);
                    if let Some(panel) = view.environment_panel.as_mut().filter(|_| !selector.is_empty()) {
                        panel.select_action(selector)?;
                    }
                    return Ok(());
                }
                let arguments: Value = serde_json::from_str(detail)?;
                // Bound to the state last shown, not the status just fetched for the action list.
                self.queue_environment_request("act", json!({"action":selector,"arguments":arguments,"operation_id":format!("human-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()),"view_version":shown_version}), view)?;
                return Ok(());
            }
            "recover" => {
                self.environment_request(
                    "recover",
                    json!({"id":selector,"recovery":serde_json::from_str::<Value>(detail)?}),
                )
                .await?
            }
            "cancel" => {
                view.environment_panel = None;
                view.editor.set_text("");
                return Ok(());
            }
            _ => anyhow::bail!(
                "usage: /env [list|catalog|create|inspect|policy|validate|save|enter|resume|pause|stop|act|status|leave]"
            ),
        };
        self.plain_row(&serde_json::to_string_pretty(&result)?, view);
        if operation == "save" {
            self.plain_row(
                &format!(
                    "Saved {} version {}. Select /env enter {} to review execution choices.",
                    result["id"].as_str().unwrap_or("environment"),
                    result["version"].as_str().unwrap_or("unknown"),
                    result["id"].as_str().unwrap_or("environment")
                ),
                view,
            );
        }
        Ok(())
    }
}
impl SessionUi {}
