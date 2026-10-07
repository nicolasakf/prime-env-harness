//! Environment routing, kept separate from upstream command dispatch.
use super::{json, response_failure, response_success, Value, Worker};
impl Worker {
    pub(super) async fn dispatch_environment(
        &self,
        command_type: &str,
        payload: &Value,
    ) -> Option<crate::protocol::DaemonResponse> {
        let participant = self.has_environment_participant()
            || self
                .agent_engine
                .as_ref()
                .is_some_and(|engine| engine.is_environment_host().unwrap_or(false));
        if command_type == "env" {
            if matches!(
                payload["operation"].as_str(),
                Some("seat_wake" | "seat_replay")
            ) {
                return Some(
                    match self.admit_environment_wake(
                        &payload["parameters"],
                        /*replay*/ payload["operation"] == "seat_replay",
                    ) {
                        Ok(data) => response_success(None, "env", Some(data)),
                        Err(error) => response_failure(None, "env", &format!("{error:#}"), None),
                    },
                );
            }
            let Some(engine) = &self.agent_engine else {
                return Some(response_failure(
                    None,
                    "env",
                    "environment commands require the Prime session engine",
                    None,
                ));
            };
            let operation = payload["operation"].as_str().unwrap_or_default();
            if matches!(
                operation,
                "seat_call"
                    | "snapshot"
                    | "receipts"
                    | "table_log"
                    | "seat_transcript"
                    | "run_state"
                    | "privileged_observe"
            ) {
                return Some(
                    match engine
                        .environment_command(operation, &payload["parameters"])
                        .await
                    {
                        Ok(data) => response_success(None, "env", Some(data)),
                        Err(error) => response_failure(None, "env", &format!("{error:#}"), None),
                    },
                );
            }
            let origin = payload["parameters"]
                .get("ui_origin")
                .cloned()
                .unwrap_or(Value::Null);
            if operation == "leave" {
                self.request_abort();
                self.await_session_work_settled().await;
            }
            self.emit_worker_event(
                json!({"type":"environment_update","operation":operation,"phase":"starting","ui_origin":origin}),
            );
            return Some(
                match engine
                    .environment_command(operation, &payload["parameters"])
                    .await
                {
                    Ok(data) => {
                        if operation == "leave"
                            && engine.close_environment_on_detach().unwrap_or(true)
                        {
                            let mut core = self.core.lock().unwrap();
                            core.created = false;
                            core.steering.clear();
                            core.follow_up.clear();
                            let lease = core.store.as_mut().and_then(|store| store.lease.take());
                            if let Some(lease) = &lease {
                                lease.release();
                            }
                            drop(lease);
                        }
                        self.emit_worker_event(json!({"type":"environment_update","operation":operation,"phase":"ready","data":data,"ui_origin":origin}));
                        response_success(None, "env", Some(data))
                    }
                    Err(error) => {
                        self.emit_worker_event(json!({"type":"environment_update","operation":operation,"phase":"failed","error":format!("{error:#}"),"ui_origin":origin}));
                        response_failure(None, "env", &format!("{error:#}"), None)
                    }
                },
            );
        }
        if participant && matches!(command_type, "prompt" | "prompt_and_wait") {
            let engine = self
                .agent_engine
                .as_ref()
                .expect("participant has agent engine");
            if engine.is_environment_host().unwrap_or(false) {
                let parameters = json!({"text":payload["message"],"operation_id":payload["id"].as_str().map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned)});
                return Some(match engine.environment_command("say", &parameters).await {
                    Ok(mut data) => {
                        data["forwarded"] = json!(false);
                        response_success(None, command_type, Some(data))
                    }
                    Err(error) => response_failure(None, command_type, &format!("{error:#}"), None),
                });
            }
            let result = async {
                let instance = engine.active_environment().await?.ok_or_else(|| anyhow::anyhow!("participant binding unavailable"))?;
                let raw = payload["message"].as_str().ok_or_else(|| anyhow::anyhow!("human input must be text"))?;
                let operation_id = payload["id"].as_str().map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
                let input = instance.input(raw, &operation_id).await?;
                // Human observations must never enter the model transcript, including
                // after compaction or reconnect. The operation receipt keeps them
                // separately from conversation history.
                self.emit_worker_event(json!({"type":"environment_observation","participant":"human","observation":input.observation}));
                let Some(message) = input.agent_message else { return Ok(response_success(None, command_type, Some(json!({"observation":input.observation,"forwarded":false})))); };
                // Only the environment-selected message and agent observation
                // enter the model. Client prefixes, images, and slash expansion
                // never cross this boundary.
                let observation = instance.observe(&pa_types::environments::SeatId::legacy_agent()).await?;
                let mut filtered = json!({"message":format!("Environment message:\n{message}\nObservation: {observation}"),"expandPromptTemplates":false});
                // Queue/admission metadata is host control, never model content.
                // It must survive filtering so input can resume after compaction.
                for key in ["streamingBehavior", "admissionId"] {
                    if let Some(value) = payload.get(key) {
                        filtered[key] = value.clone();
                    }
                }
                Ok::<_, anyhow::Error>(self.handle_prompt(&filtered, command_type == "prompt_and_wait").await)
            }.await;
            return Some(result.unwrap_or_else(|error| {
                response_failure(None, command_type, &format!("{error:#}"), None)
            }));
        }
        None
    }
}
