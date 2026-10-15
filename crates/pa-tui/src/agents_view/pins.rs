//! Pinned sessions: the settings seam the view persists pins through and
//! the pin key's toggle. A pinned agent lists in its own block above the
//! status sections and stays listed while it holds no messages.
use std::sync::Arc;

use serde_json::Value;

use super::AgentsViewMode;
use crate::agents_view_forest::RowKind;
use crate::client_settings::ClientSettings;

/// The settings the view reads and writes its pins through
/// (`pinnedSessionIds`, global scope).
#[derive(Clone)]
pub struct AgentsViewSettings(pub Arc<dyn ClientSettings>);

impl std::fmt::Debug for AgentsViewSettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentsViewSettings")
    }
}

impl AgentsViewMode {
    /// The selected agent row's session id: the row the pin key acts on.
    /// Only top-level agent rows with a persisted session pin.
    pub(super) fn pin_target(&self) -> Option<&str> {
        self.rows
            .get(self.selected)
            .filter(|row| row.kind == RowKind::Agent)
            .and_then(|row| row.summary.get("sessionId").and_then(Value::as_str))
            .filter(|session_id| !session_id.is_empty())
    }

    /// Toggle the selected agent's pin (TS `togglePinForSelected`): the
    /// seam persists it, the rows rebuild so the agent moves between the
    /// pinned block and its status section, and the selection follows it.
    pub(super) fn toggle_pin_for_selected(&mut self) {
        let Some(settings) = self.options.client_settings.clone() else {
            return;
        };
        let Some(session_id) = self.pin_target().map(str::to_string) else {
            return;
        };
        let title = self
            .rows
            .get(self.selected)
            .map(|row| row.title.clone())
            .unwrap_or_default();
        match settings.0.toggle_session_pinned(&session_id) {
            Ok(pinned) => {
                if pinned {
                    self.pinned_session_ids.insert(session_id);
                } else {
                    self.pinned_session_ids.remove(&session_id);
                }
                self.rebuild_rows();
                self.set_status(&format!(
                    "{} {title}",
                    if pinned { "Pinned" } else { "Unpinned" }
                ));
                self.actions
                    .push(if pinned { "pinned" } else { "unpinned" });
            }
            Err(error) => self.set_status(&format!("Failed to update pin: {error:#}")),
        }
    }
}
