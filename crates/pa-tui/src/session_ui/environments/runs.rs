//! The Environments dock group: the runs this builder session provisioned,
//! their background refresh, and the runs panel's open and stop hand-offs.
use super::super::{picker_viewport_rows, UI_REQUEST_TIMEOUT_MS};
use super::{json, AgentView, DaemonCommand, Duration, Map, Result, SessionUi, Value};
use crate::chrome::ActivityGroup;
use crate::environment_runs_view::{
    is_agents_only, EnvironmentRunsAction, EnvironmentRunsView, RunOwners, ScopedRuns,
};
use crate::subagents::{descendant_entries, SessionIdentity};
use crossterm::event::KeyEvent;
use pa_types::environments::InstanceRun;

/// A landed background fetch for the Environments group.
pub(crate) enum EnvironmentRunsUpdate {
    /// A `controls` fetch: every run, live and ended, or the fetch error
    /// that keeps the last list (stale-while-revalidate).
    Runs(Result<Vec<InstanceRun>, String>),
    /// A `transcript` fetch for the ended run `id`: its saved messages.
    Transcript {
        id: String,
        messages: Result<Vec<Value>, String>,
    },
}

impl SessionUi {
    /// The runs this session's Environments group shows: the ones this
    /// session provisioned plus those its subagents did, matched on the
    /// builder identity each run persists (its saved session id, which
    /// survives restarts, or for older runs its active id), and the older
    /// runs no session can be matched to. A run stays with the builder that
    /// created it, whichever session later resumed or stopped it.
    pub(crate) fn scoped_environment_runs(&self) -> ScopedRuns {
        let identity = SessionIdentity::new(
            (!self.active_session_id.is_empty()).then(|| self.active_session_id.clone()),
            (!self.session_id.is_empty()).then(|| self.session_id.clone()),
            self.session_file.clone(),
        );
        let descendants = descendant_entries(&self.roster, &identity);
        let summary_ids = |key: &str| {
            descendants
                .iter()
                .filter_map(|entry| entry["summary"][key].as_str())
                .collect::<Vec<_>>()
        };
        let mut owners = RunOwners {
            saved_session_ids: summary_ids("sessionId"),
            active_session_ids: summary_ids("activeSessionId"),
        };
        owners
            .saved_session_ids
            .extend(Some(self.session_id.as_str()).filter(|id| !id.is_empty()));
        owners
            .active_session_ids
            .extend(Some(self.active_session_id.as_str()).filter(|id| !id.is_empty()));
        ScopedRuns::of(&self.environment_runs, &owners)
    }

    /// Fire a background refresh of the run list (`controls`). At most one
    /// refresh runs in flight with one queued trailing refresh, so a burst of
    /// `environment_runs_changed` broadcasts never stacks requests.
    pub(crate) fn spawn_environment_runs_refresh(&mut self) {
        if self.environment_profile.is_some()
            || self.active_session_id.is_empty()
            || !self
                .client
                .supports_server_capability(pa_types::environments::ENVIRONMENT_CAPABILITY)
        {
            return;
        }
        if self.environment_refresh_in_flight {
            self.environment_refresh_queued = true;
            return;
        }
        self.environment_refresh_in_flight = true;
        let updates = self.environment_run_updates.clone();
        let client = self.client.clone();
        let active_session_id = self.active_session_id.clone();
        tokio::spawn(async move {
            let fetched = tokio::time::timeout(
                Duration::from_millis(UI_REQUEST_TIMEOUT_MS),
                client.request_ok(DaemonCommand::Env {
                    id: None,
                    active_session_id,
                    operation: "controls".into(),
                    parameters: json!({"include_ended": true}),
                    rest: Map::default(),
                }),
            )
            .await;
            let runs = match fetched {
                Ok(Ok(data)) => serde_json::from_value(data["runs"].clone())
                    .map_err(|error| format!("{error:#}")),
                Ok(Err(error)) => Err(format!("{error:#}")),
                Err(_) => Err("timed out waiting for the Prime Env daemon response".to_owned()),
            };
            let _ = updates.send(EnvironmentRunsUpdate::Runs(runs));
        });
    }

    /// Fold a landed fetch in. A run list: the dock follows it with the
    /// panel closed, an open panel keeps its selection, and a failure
    /// surfaces only inside the open panel. A transcript: shown when its run
    /// is still the one loading.
    pub(crate) fn apply_environment_runs_update(
        &mut self,
        update: EnvironmentRunsUpdate,
        view: &mut AgentView,
    ) {
        let runs = match update {
            EnvironmentRunsUpdate::Runs(runs) => runs,
            EnvironmentRunsUpdate::Transcript { id, messages } => {
                self.apply_environment_run_transcript(&id, messages, view);
                return;
            }
        };
        self.environment_refresh_in_flight = false;
        let queued = std::mem::take(&mut self.environment_refresh_queued);
        match runs {
            Ok(runs) => {
                self.environment_runs = runs;
                let scoped = self.scoped_environment_runs();
                if let Some(runs_view) = view.environment_runs_view.as_mut() {
                    runs_view.apply_runs(scoped);
                    runs_view.set_error(None);
                }
            }
            Err(error) => {
                if let Some(runs_view) = view.environment_runs_view.as_mut() {
                    runs_view.set_error(Some(error));
                }
            }
        }
        self.sync_activity_dock(view);
        self.dirty = true;
        if queued {
            self.spawn_environment_runs_refresh();
        }
    }

    /// Open the runs panel over the cached list at once; a refresh lands
    /// through the update channel.
    pub(crate) fn open_environment_runs_view(&mut self, view: &mut AgentView) {
        self.activity_group = ActivityGroup::Environments;
        self.subagents_focused = false;
        view.environment_runs_view = Some(EnvironmentRunsView::new(
            self.scoped_environment_runs(),
            picker_viewport_rows(view.terminal_rows()),
        ));
        self.spawn_environment_runs_refresh();
        self.track_feature_outcome("environment_runs_panel", "completed", None);
        self.update_subagent_summary(view);
        self.dirty = true;
    }

    /// One key press while the runs panel is open: Esc, left and Ctrl+C
    /// close it back onto the dock item; Enter attaches to the selected run;
    /// `x` stops it.
    pub(crate) async fn handle_environment_runs_key(
        &mut self,
        key: KeyEvent,
        view: &mut AgentView,
    ) -> Result<()> {
        let Some(id) = crate::keys::key_event_to_id(&key) else {
            return Ok(());
        };
        if id == "ctrl+c" {
            self.exit_guard.note_ctrl_c_handled();
        }
        let action = view
            .environment_runs_view
            .as_mut()
            .map(|runs_view| runs_view.handle_key(&id, view.editor.keybindings()));
        match action {
            Some(EnvironmentRunsAction::None) => self.dirty = true,
            Some(EnvironmentRunsAction::Close) => {
                view.environment_runs_view = None;
                self.focus_activity_dock(view);
                self.dirty = true;
            }
            Some(EnvironmentRunsAction::Open(run)) => {
                view.environment_runs_view = None;
                let operation = if is_agents_only(&run) {
                    "watch"
                } else {
                    "resume"
                };
                if let Err(error) =
                    self.queue_environment_request(operation, json!({"id": run.instance.id}), view)
                {
                    self.note(&format!("{error:#}"), view);
                    self.focus_activity_dock(view);
                }
                self.dirty = true;
            }
            Some(EnvironmentRunsAction::View(run)) => {
                view.environment_runs_view = None;
                self.open_environment_run_transcript(run, view);
            }
            Some(EnvironmentRunsAction::Stop(run)) => {
                match self
                    .environment_request("stop", json!({"id": run.instance.id}))
                    .await
                {
                    Ok(_) => {
                        self.environment_runs
                            .retain(|existing| existing.instance.id != run.instance.id);
                        let scoped = self.scoped_environment_runs();
                        if let Some(runs_view) = view.environment_runs_view.as_mut() {
                            runs_view.apply_runs(scoped);
                            runs_view.set_error(None);
                        }
                        self.sync_activity_dock(view);
                        self.spawn_environment_runs_refresh();
                    }
                    Err(error) => {
                        if let Some(runs_view) = view.environment_runs_view.as_mut() {
                            runs_view.set_error(Some(format!("stop failed: {error:#}")));
                        }
                    }
                }
                self.dirty = true;
            }
            None => {}
        }
        Ok(())
    }
}
