//! An ended environment run's read-only transcript: opened from the runs
//! panel, it fetches the run's saved conversation in the background and then
//! swaps it in for the chat; its close re-attaches the builder session,
//! whose snapshot rebuilds the chat and whatever the held events changed
//! meanwhile.
use super::super::{DockFold, RebuildKind};
use super::{json, AgentView, DaemonCommand, Duration, Map, Result, SessionUi, Value};
use crate::environment_run_transcript::{EnvironmentRunTranscript, EnvironmentRunTranscriptAction};
use crate::session_ui::EnvironmentRunsUpdate;
use crossterm::event::KeyEvent;
use pa_types::environments::InstanceRun;

impl SessionUi {
    /// Mount the transcript view in its loading state and fetch the run's
    /// saved conversation off the UI loop: a long run's transcript can take
    /// seconds to read.
    pub(crate) fn open_environment_run_transcript(
        &mut self,
        run: InstanceRun,
        view: &mut AgentView,
    ) {
        let id = run.instance.id.clone();
        view.environment_run_transcript = Some(EnvironmentRunTranscript::new(run));
        self.subagents_focused = false;
        self.dirty = true;
        let updates = self.environment_run_updates.clone();
        let client = self.client.clone();
        let active_session_id = self.active_session_id.clone();
        tokio::spawn(async move {
            let fetched = tokio::time::timeout(
                Duration::from_mins(2),
                client.request_ok(DaemonCommand::Env {
                    id: None,
                    active_session_id,
                    operation: "transcript".into(),
                    parameters: json!({"id": id}),
                    rest: Map::default(),
                }),
            )
            .await;
            let messages = match fetched {
                Ok(Ok(mut data)) => match data["messages"].take() {
                    Value::Array(messages) => Ok(messages),
                    _ => Err("the daemon returned no transcript".to_owned()),
                },
                Ok(Err(error)) => Err(format!("{error:#}")),
                Err(_) => Err("timed out waiting for the Prime Env daemon response".to_owned()),
            };
            let _ = updates.send(EnvironmentRunsUpdate::Transcript { id, messages });
        });
    }

    /// A landed transcript fetch: it replaces the chat when its run is still
    /// the one loading (a closed or replaced view drops it), and a failure
    /// returns to the runs panel with the error.
    pub(super) fn apply_environment_run_transcript(
        &mut self,
        id: &str,
        messages: Result<Vec<Value>, String>,
        view: &mut AgentView,
    ) {
        let Some(transcript) = view
            .environment_run_transcript
            .as_mut()
            .filter(|transcript| !transcript.is_shown() && transcript.run().instance.id == id)
        else {
            return;
        };
        let messages = match messages {
            Ok(messages) => messages,
            Err(error) => {
                view.environment_run_transcript = None;
                self.open_environment_runs_view(view);
                if let Some(runs_view) = view.environment_runs_view.as_mut() {
                    runs_view.select_run(id);
                    runs_view.set_error(Some(format!("transcript unavailable: {error}")));
                }
                self.track_feature_outcome("environment_run_transcript", "failed", None);
                return;
            }
        };
        transcript.mark_shown();
        view.clear_chat();
        // The builder's live loaders belong to its own transcript; the
        // close's re-attach restores them.
        view.working = None;
        view.working_since = None;
        view.compaction = None;
        view.retry = None;
        self.last_status_index = None;
        self.pressed_click = None;
        for entry in crate::snapshot::transcript_to_entries(&messages) {
            view.push_entry(entry);
        }
        view.follow();
        self.track_feature_outcome("environment_run_transcript", "completed", None);
        self.dirty = true;
    }

    /// One key press while the transcript view is up: the viewport keys
    /// scroll it, the detail key cycles its cards, the close keys return to
    /// the runs panel on the same run, and every other key does nothing.
    pub(crate) async fn handle_environment_run_transcript_key(
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
        let Some(action) = view
            .environment_run_transcript
            .as_ref()
            .map(|transcript| transcript.handle_key(&id, view.editor.keybindings()))
        else {
            return Ok(());
        };
        match action {
            EnvironmentRunTranscriptAction::None => {}
            EnvironmentRunTranscriptAction::ScrollLines(lines) => view.scroll_by(lines),
            EnvironmentRunTranscriptAction::PageUp => {
                view.scroll_by(-(view.page_size() as isize));
            }
            EnvironmentRunTranscriptAction::PageDown => {
                view.scroll_by(view.page_size() as isize);
            }
            EnvironmentRunTranscriptAction::Top => view.scroll_to_top(),
            EnvironmentRunTranscriptAction::Follow => view.scroll_to_bottom(),
            EnvironmentRunTranscriptAction::CycleDetail => self.cycle_detail(view),
            EnvironmentRunTranscriptAction::Close => {
                let Some(transcript) = view.environment_run_transcript.take() else {
                    return Ok(());
                };
                // Only a shown transcript replaced the chat and held the
                // session's events; a loading one left both untouched.
                if transcript.is_shown() {
                    let session = self.active_session_id.clone();
                    match self.attach_session(&session, DockFold::Held).await {
                        Ok(()) => self.rebuild_view(view, &RebuildKind::Rebind),
                        // The chat still shows the run: put the session's own
                        // messages back even without the live state.
                        Err(error) => {
                            self.rebuild_transcript(view).await;
                            self.note(&format!("session rebind failed: {error:#}"), view);
                        }
                    }
                }
                self.open_environment_runs_view(view);
                if let Some(runs_view) = view.environment_runs_view.as_mut() {
                    runs_view.select_run(&transcript.run().instance.id);
                }
            }
        }
        self.dirty = true;
        Ok(())
    }
}
