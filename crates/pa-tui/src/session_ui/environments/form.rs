//! Schema-driven environment input and submission failures.
use super::{json, AgentView, EnvironmentRequestFailure, Result, SessionUi};

impl SessionUi {
    pub(crate) fn apply_environment_request_failure(
        &mut self,
        failure: &EnvironmentRequestFailure,
        view: &mut AgentView,
    ) {
        if self.environment_pending.as_deref() != Some(failure.origin.as_str()) {
            return;
        }
        if let Some(panel) = view
            .environment_panel
            .as_mut()
            .filter(|panel| panel.pending)
        {
            panel.fail(failure.error.clone());
            panel.uncertain = true;
        }
        self.error_row(&failure.error, view);
        self.dirty = true;
    }

    pub(crate) fn open_environment_actions(&mut self, view: &mut AgentView) {
        self.track_feature_outcome("environment_action_form", "initiated", None);
        view.editor.cancel_autocomplete();
        let mut panel = crate::environment_panel::EnvironmentPanel::actions(
            &self.environment_actions,
            view.editor.keybindings(),
        );
        panel.view_version = self.environment_status["view_version"].as_u64();
        view.environment_panel = Some(panel);
        self.dirty = true;
    }

    pub(crate) fn handle_environment_panel_outcome(
        &mut self,
        outcome: crate::environment_panel::PanelOutcome,
        view: &mut AgentView,
    ) -> Result<()> {
        use crate::environment_panel::{Intent, PanelOutcome};
        match outcome {
            PanelOutcome::None => {}
            PanelOutcome::Cancel => {
                if let Some(panel) = &view.environment_panel {
                    self.track_feature_outcome(panel.feature(), "canceled", None);
                }
                view.environment_panel = None;
            }
            PanelOutcome::Submit { intent, arguments } => {
                let (operation, parameters) = match intent {
                    Intent::Act(action) => (
                        "act",
                        json!({"action":action,"arguments":arguments,"operation_id":format!("human-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()),"view_version":view.environment_panel.as_ref().and_then(|panel| panel.view_version)}),
                    ),
                    Intent::Enter { definition, policy } => (
                        "enter",
                        json!({"id":definition,"policy":policy.unwrap_or_else(|| json!({"backend":arguments["backend"]})),"setup_arguments":arguments["parameters"]}),
                    ),
                };
                if let Err(error) = self.queue_environment_request(operation, parameters, view) {
                    if let Some(panel) = &mut view.environment_panel {
                        panel.fail(error.to_string());
                    }
                }
            }
        }
        self.dirty = true;
        Ok(())
    }
}
