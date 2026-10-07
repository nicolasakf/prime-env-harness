//! Builder provisioning, privileged reads, and run pause/resume/stop; builders never act or play.
use super::{environments::required, AgentEngineConfig};
use crate::{
    environment_host::{read, RunState},
    supervisor_link::SupervisorLink,
};
use anyhow::{bail, Context, Result};
use pa_core::kernel::shared::{host_handler, HostRequestHandlers};
use pa_environments::EnvironmentManager;
use pa_types::environments::{
    EnvironmentLanguage, ExecutionPolicy, InstanceRoster, OccupantKind, RunPhase, SeatId,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};

pub(super) fn register(handlers: &mut HostRequestHandlers, config: &AgentEngineConfig) {
    for operation in [
        "start_instance",
        "list_instances",
        "instance_status",
        "pause_instance",
        "resume_instance",
        "stop_instance",
        "observe_seat",
        "seat_transcript",
        "table_log",
    ] {
        let config = config.clone();
        handlers.register(format!("environments.{operation}"), host_handler(move |payload| {
            let config = config.clone();
            async move {
                let result = async {
                let manager = EnvironmentManager::new(config.agent_dir.join("environments"), None)?;
                let link_config = config.supervisor_link.as_ref().context("instance provisioning requires a daemon builder")?;
                let link = SupervisorLink::new(link_config.socket_path.clone());
                if operation == "list_instances" { return Ok(json!({"instances":manager.store().list_instances()?})); }
                let params = payload.data;
                if operation == "start_instance" {
                    let definition = manager.store().definition(required(&params, "definition")?, params["version"].as_str())?.0;
                    let policy: ExecutionPolicy = params.get("policy").map(|value| serde_json::from_value(value.clone())).transpose()?.unwrap_or_else(|| definition.validation.policy.clone());
                    assert_subset(&policy, &definition.validation.policy)?;
                    let roster = roster(&json!({"seats":params["seats"],"limits":params.get("limits").cloned().unwrap_or_else(|| json!({})),"direct_messages":definition.manifest.direct_messages.unwrap_or(true)}))?;
                    return link.request_success(json!({"type":"env","activeSessionId":link_config.active_session_id,"operation":"enter","parameters":{"id":definition.id,"version":definition.version,"policy":policy,"roster":roster,"setup_arguments":params.get("config").cloned().unwrap_or_else(|| json!({}))}}), Duration::from_secs(180)).await;
                }
                let id = required(&params, "instance")?;
                let path = manager.store().instance_path(id)?;
                let builder: String = read(&path.join("builder-session.json"))?;
                if builder != link_config.active_session_id { bail!("instance belongs to another builder"); }
                let control = match operation {
                    "pause_instance" => Some("pause"),
                    "resume_instance" => Some("resume_run"),
                    "stop_instance" => Some("stop"),
                    _ => None,
                };
                if let Some(control) = control {
                    return control_run(&manager, &link, &config, id, control, "builder").await;
                }
                let token: String = read(&path.join("builder-token.json"))?;
                let host: Value = read(&path.join("host-session.json"))?;
                let host_operation = match operation {
                    "instance_status" => "run_state",
                    "observe_seat" => "privileged_observe",
                    "seat_transcript" => "seat_transcript",
                    "table_log" => "table_log",
                    _ => bail!("unknown builder read"),
                };
                let mut parameters = params.clone(); parameters["builder_token"] = json!(token);
                let response = link.request(json!({"type":"env","activeSessionId":host["activeSessionId"],"operation":host_operation,"parameters":parameters}), Duration::from_secs(180)).await?;
                if response.success { return Ok(response.data.unwrap_or(Value::Null)); }
                if !response.error.as_deref().is_some_and(host_gone) { bail!("privileged read failed: {}", response.error.unwrap_or_default()); }
                match operation {
                    "instance_status" => read(&path.join("run.json")),
                    "seat_transcript" => {
                        let seat: SeatId = serde_json::from_value(params["seat"].clone())?;
                        let instance = manager.store().instance(id)?;
                        if !instance.roster.clone().unwrap_or_default().seats.iter().any(|entry| entry.id == seat && entry.occupant == OccupantKind::Agent) { bail!("unknown agent seat"); }
                        let definition = manager.store().definition(&instance.definition_id, Some(&instance.version))?.0;
                        let transcript = if definition.manifest.protocol_version == 1 { path.join("conversation.jsonl") } else { path.join("seats").join(seat.as_str()).join("conversation.jsonl") };
                        Ok(json!({"transcript":std::fs::read_to_string(transcript)?.lines().skip(params["since"].as_u64().unwrap_or_default() as usize).collect::<Vec<_>>().join("\n")}))
                    }
                    "observe_seat" => manager.inspect(id, &serde_json::from_value(params["seat"].clone())?).await,
                    "table_log" => {
                        // Ownership prevents offline readers from racing an append or
                        // accidentally truncating a live writer's partial table line.
                        let lease = std::fs::OpenOptions::new().read(true).write(true).open(path.join("owner.lock"))?;
                        lease.try_lock().context("instance is still owned")?;
                        Ok(json!(pa_environments::TableLog::open(&path)?.all_since(params["since_seq"].as_u64().unwrap_or_default())?))
                    }
                    _ => bail!("unknown offline read"),
                }
                }.await;
                let feature = match operation { "start_instance" => "environment_builder_provision", "list_instances" | "instance_status" | "observe_seat" | "seat_transcript" | "table_log" => "environment_privileged_read", "pause_instance" | "resume_instance" | "stop_instance" => return result, _ => unreachable!("registered operations") };
                crate::environment_telemetry::track(crate::environment_telemetry::client(&config).as_ref(), feature, if result.is_ok() { "completed" } else { "failed" }, None);
                result
            }
        }));
    }
}

/// A host that no longer exists or has closed; its run is then only persisted state.
fn host_gone(error: &str) -> bool {
    error.starts_with("Unknown active session") || error == "Environment host is closed"
}

/// Pauses, resumes, or stops a run for `requester` ("builder" API or "human" at the
/// builder prompt). A live host applies the control itself (and records its
/// telemetry); once the host has closed, a persisted run can still be paused or
/// stopped directly, but only a live host can resume.
pub(crate) async fn control_run(
    manager: &EnvironmentManager,
    link: &SupervisorLink,
    config: &AgentEngineConfig,
    id: &str,
    operation: &str,
    requester: &str,
) -> Result<Value> {
    let path = manager.store().instance_path(id)?;
    let host: Value = read(&path.join("host-session.json"))?;
    let (host_operation, parameters) = if requester == "builder" {
        let token: String = read(&path.join("builder-token.json"))?;
        (
            format!("builder_{operation}"),
            json!({"builder_token":token}),
        )
    } else {
        (operation.to_owned(), json!({}))
    };
    let response = link.request(json!({"type":"env","activeSessionId":host["activeSessionId"],"operation":host_operation,"parameters":parameters}), Duration::from_secs(180)).await?;
    if response.success {
        let status = response.data.unwrap_or(Value::Null);
        return Ok(json!({"instance_id":id,"run":status["run"]}));
    }
    let error = response.error.unwrap_or_default();
    if !host_gone(&error) {
        bail!("{operation} failed: {error}");
    }
    if operation == "resume_run" {
        bail!("the table host is closed; re-enter the instance with /env resume before resuming its run");
    }
    // Ownership prevents racing a host that is still opening or closing.
    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.join("owner.lock"))?;
    lease.try_lock().context("instance is still owned")?;
    let mut state: RunState = read(&path.join("run.json"))?;
    if state.phase.is_terminal() {
        bail!("terminal environment cannot be controlled");
    }
    state.phase = if operation == "pause" {
        RunPhase::Paused
    } else {
        RunPhase::Stopped
    };
    state.reason = Some(format!("{requester} requested {operation}"));
    crate::environment_host::write(&path.join("run.json"), &state)?;
    let choice = if requester == "human" {
        operation.to_owned()
    } else {
        format!("{requester}_{operation}")
    };
    crate::environment_telemetry::track(
        crate::environment_telemetry::client(config).as_ref(),
        "environment_run_control",
        "completed",
        Some(&choice),
    );
    Ok(json!({"instance_id":id,"run":state}))
}

pub(super) fn roster(value: &Value) -> Result<InstanceRoster> {
    let mut value = value.clone();
    let mut counts = BTreeMap::<String, usize>::new();
    for seat in value["seats"]
        .as_array_mut()
        .context("seats must be an array")?
    {
        let role = required(seat, "role")?.to_owned();
        let index = counts.entry(role.clone()).or_default();
        *index += 1;
        if seat.get("id").is_none() {
            seat["id"] = json!(format!("{role}-{index}"));
        }
    }
    serde_json::from_value(value).map_err(Into::into)
}

fn assert_subset(policy: &ExecutionPolicy, selected: &ExecutionPolicy) -> Result<()> {
    if policy.backend != selected.backend
        || (policy.python_execution && !selected.python_execution)
        || (policy.networking && !selected.networking)
        || !policy.mounts.iter().all(|grant| {
            selected.mounts.iter().any(|allowed| {
                allowed.source == grant.source
                    && allowed.target == grant.target
                    && (!grant.writable || allowed.writable)
            })
        })
        || !policy
            .secrets
            .iter()
            .all(|grant| selected.secrets.contains(grant))
        || !policy
            .integrations
            .iter()
            .all(|grant| selected.integrations.contains(grant))
        || policy.call_timeout_seconds > selected.call_timeout_seconds
        || policy.preparation_timeout_seconds > selected.preparation_timeout_seconds
        || policy.cpus > selected.cpus
        || policy.memory_mib > selected.memory_mib
    {
        bail!("builder policy cannot widen the user-selected validation policy");
    }
    Ok(())
}

pub(crate) async fn management(
    manager: &EnvironmentManager,
    operation: &str,
    params: &Value,
) -> Result<Value> {
    match operation {
        "create_environment" => {
            let language: EnvironmentLanguage =
                serde_json::from_value(params.get("language").cloned().unwrap_or(json!("python")))?;
            let path = manager.store().create_draft(
                required(params, "description")?,
                language,
                params["python_execution"].as_bool().unwrap_or(false),
                &ExecutionPolicy::default(),
            )?;
            Ok(
                json!({"draft":path,"execution_selected":false,"next":"Show requested capabilities and recommendation; ask which policy the user wants, wait for their answer, apply it with environments.select_policy, then validate automatically."}),
            )
        }
        "select_policy" => {
            let policy: ExecutionPolicy = serde_json::from_value(params["policy"].clone())?;
            manager.select_policy(required(params, "draft")?, &policy)?;
            Ok(json!({"policy":policy,"selected":true}))
        }
        "validate_environment" => {
            let draft = required(params, "draft")?;
            let policy = manager.selected_policy(draft)?;
            Ok(serde_json::to_value(
                manager.validate(draft, policy).await?,
            )?)
        }
        "save_environment" => Ok(serde_json::to_value(
            manager.save(required(params, "draft")?, required(params, "id")?)?,
        )?),
        "list_environments" => Ok(serde_json::to_value(manager.store().list()?)?),
        _ => bail!("unknown environment management operation"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provisioning_cannot_expand_a_selected_policy() {
        let selected = ExecutionPolicy {
            python_execution: true,
            networking: true,
            ..ExecutionPolicy::default()
        };
        let narrowed = ExecutionPolicy {
            call_timeout_seconds: 30,
            ..ExecutionPolicy::default()
        };
        assert_subset(&narrowed, &selected).unwrap();
        for value in [
            json!({"backend":"docker"}),
            json!({"secrets":["API_KEY"]}),
            json!({"integrations":["external"]}),
            json!({"cpus":selected.cpus+1}),
            json!({"call_timeout_seconds":selected.call_timeout_seconds+1}),
        ] {
            let policy: ExecutionPolicy = serde_json::from_value(value).unwrap();
            assert!(assert_subset(&policy, &selected).is_err());
        }
        let normalized = roster(&json!({"seats":[{"role":"player","occupant":"agent"},{"role":"player","occupant":"agent"}]})).unwrap();
        assert_eq!(
            normalized
                .seats
                .iter()
                .map(|seat| seat.id.as_str())
                .collect::<Vec<_>>(),
            ["player-1", "player-2"]
        );
    }
}
