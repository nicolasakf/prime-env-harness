//! Apply public host status and environment request results.
use super::{AgentView, SessionUi, Value};

impl SessionUi {
    pub(crate) fn apply_environment_event(&mut self, event: &Value, view: &mut AgentView) {
        if event["type"] == "environment_status" || event["type"] == "environment_seat_state" {
            self.environment_status["run"] = event["run"].clone();
        }
        // What the human is shown, and the version their submissions are bound to.
        if event["type"] == "environment_observation"
            && self.environment_profile.is_some()
            && event["view_version"].is_u64()
        {
            self.environment_status["observation"] = event["observation"].clone();
            self.environment_status["view_version"] = event["view_version"].clone();
        }
        if event["type"] == "environment_update" {
            if self.environment_profile.is_some()
                && event["phase"] == "ready"
                && event["data"].get("run").is_some()
            {
                self.environment_status = event["data"].clone();
            }
            if event["ui_origin"]
                .as_str()
                .is_some_and(|origin| self.environment_pending.as_deref() == Some(origin))
                && matches!(event["phase"].as_str(), Some("ready" | "failed"))
            {
                self.environment_pending = None;
                if view
                    .environment_panel
                    .as_ref()
                    .is_some_and(|panel| panel.pending)
                {
                    if let Some(panel) = &view.environment_panel {
                        self.track_feature_outcome(
                            panel.feature(),
                            if event["phase"] == "ready" {
                                "completed"
                            } else {
                                "failed"
                            },
                            None,
                        );
                    }
                    if event["phase"] == "ready" {
                        view.environment_panel = None;
                    } else if let Some(panel) = &mut view.environment_panel {
                        panel.fail(
                            event["error"]
                                .as_str()
                                .unwrap_or("Environment request failed")
                                .to_owned(),
                        );
                        // A refused submission was followed by the refreshed state; the
                        // next submit is bound to that, once the human has seen it.
                        panel.view_version = self.environment_status["view_version"].as_u64();
                    }
                }
                self.track_feature_outcome(
                    "environment",
                    if event["phase"] == "ready" {
                        "completed"
                    } else {
                        "failed"
                    },
                    None,
                );
                if event["phase"] == "ready" {
                    self.pending_rebind = event["data"]["session"]["activeSessionId"]
                        .as_str()
                        .or_else(|| event["data"]["session"]["session"]["activeSessionId"].as_str())
                        .or_else(|| event["data"]["builder_session_id"].as_str())
                        .map(str::to_owned);
                    self.plain_row(
                        &serde_json::to_string_pretty(&event["data"])
                            .unwrap_or_else(|_| event["data"].to_string()),
                        view,
                    );
                }
            }
            self.note(
                &format!(
                    "Environment {}: {}{}{}",
                    event["operation"].as_str().unwrap_or("operation"),
                    event["phase"].as_str().unwrap_or("update"),
                    event
                        .get("error")
                        .map_or(String::new(), |error| format!(" — {error}")),
                    if event["operation"] == "validate" && event["phase"] == "ready" {
                        if event["data"]["passed"] == true {
                            " — Validation passed"
                        } else {
                            " — Validation failed"
                        }
                    } else {
                        ""
                    }
                ),
                view,
            );
        }
    }
}
