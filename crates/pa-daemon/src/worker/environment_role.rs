//! Resolve the durable role once at create; bind host events to public storage.
use super::{emit_worker_event_with, json, Arc, Result, Value, Worker};
use anyhow::Context;

impl Worker {
    pub(super) async fn prepare_environment_role(&self) -> Result<bool> {
        let Some(engine) = &self.agent_engine else {
            return Ok(false);
        };
        let config = engine
            .create_resources
            .read()
            .map_err(|_| anyhow::anyhow!("environment config lock poisoned"))?
            .environment_host
            .clone();
        if let Some(config) = config {
            let instance = engine
                .active_environment()
                .await?
                .context("host instance unavailable")?;
            let core = Arc::clone(&self.core);
            let events = Arc::clone(&self.events);
            // The version of the observation clients last heard, so a new version of an
            // unchanged observation still reaches the forms bound to it.
            let announced = std::sync::Mutex::new(None::<u64>);
            let view = Arc::new(move |event: Value| -> Result<()> {
                let kind = event["type"].as_str().context("environment event type")?;
                if kind == "environment_runs_changed" {
                    events.send(super::OutboundFrame::environment_runs_changed());
                    return Ok(());
                }
                let (custom_type, id, content) = if kind == "environment_delivery" {
                    let delivery = &event["delivery"];
                    let custom_type = match delivery["kind"].as_str() {
                        Some("notice") => "environment_notice",
                        Some("direct_message") => "environment_direct_message",
                        Some("status") => "environment_status",
                        _ => anyhow::bail!("invalid table event kind"),
                    };
                    (
                        custom_type,
                        delivery["event_id"].clone(),
                        delivery["message"].clone(),
                    )
                } else {
                    (
                        kind,
                        Value::Null,
                        if kind == "environment_observation" {
                            event["observation"].clone()
                        } else if kind == "environment_action" {
                            event["action"].clone()
                        } else {
                            event["run"].clone()
                        },
                    )
                };
                let row = json!({"role":"custom","customType":custom_type,"content":serde_json::to_string(&content)?,"display":true,"timestamp":crate::util::now_ms(),"details":{"event_id":id,"event":event}});
                let duplicate = {
                    let mut guard = core.lock().unwrap();
                    let store = guard
                        .store
                        .as_mut()
                        .context("host transcript unavailable")?;
                    let duplicate = if id.is_null() {
                        store
                            .entries()
                            .iter()
                            .rev()
                            .find(|entry| {
                                entry.type_ == "custom_message"
                                    && entry.fields["customType"] == custom_type
                            })
                            .is_some_and(|entry| entry.fields["content"] == row["content"])
                    } else {
                        store.entries().iter().any(|entry| {
                            entry.type_ == "custom_message"
                                && entry.fields["details"]["event_id"] == id
                        })
                    };
                    if !duplicate {
                        store.persist_entry("custom_message", json!({"customType":row["customType"],"content":row["content"],"display":true,"timestamp":crate::util::now_ms(),"details":row["details"]}))?;
                    }
                    duplicate
                };
                if duplicate {
                    // The same observation under a new version still reaches clients, so
                    // the forms bound to it can follow.
                    if kind == "environment_observation" {
                        let version = event["view_version"].as_u64();
                        let mut announced = announced
                            .lock()
                            .map_err(|_| anyhow::anyhow!("observation version lock poisoned"))?;
                        if version.is_some() && *announced != version {
                            *announced = version;
                            drop(announced);
                            emit_worker_event_with(&core, &events, event);
                        }
                    }
                    return Ok(());
                }
                emit_worker_event_with(
                    &core,
                    &events,
                    json!({"type":"message_start","message":row}),
                );
                emit_worker_event_with(&core, &events, json!({"type":"message_end","message":row}));
                if kind == "environment_observation" {
                    *announced
                        .lock()
                        .map_err(|_| anyhow::anyhow!("observation version lock poisoned"))? =
                        event["view_version"].as_u64();
                }
                if kind != "environment_delivery" {
                    emit_worker_event_with(&core, &events, event);
                }
                Ok(())
            });
            let host = crate::environment_host::EnvironmentHost::open(
                instance,
                engine
                    .config
                    .supervisor_link
                    .as_ref()
                    .context("host supervisor unavailable")?
                    .socket_path
                    .clone(),
                self.config.active_session_id.clone(),
                config.builder_token,
                config.seat_defaults,
                view,
                crate::environment_telemetry::client(&engine.config),
            )
            .await?;
            *engine.environment_host.lock().await = Some(Arc::clone(&host));
            // Keep model-less hosts supervised even when no model turn is running.
            self.record_recovery(true, "environment_host")?;
            host.start();
            return Ok(true);
        }
        engine.persist_seat_profile()?;
        Ok(false)
    }
}
