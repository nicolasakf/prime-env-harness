//! The Environments column's data feed: every environment run, live and
//! ended, fetched over the roster connection and re-fetched whenever the
//! daemon announces a run opened or changed phase.
use std::time::Duration;

use pa_types::daemon::DaemonCommand;
use pa_types::environments::{InstanceRun, ENVIRONMENT_CAPABILITY};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::status::StatusTone;
use super::{AgentsViewMode, DaemonClient, UiInput};

/// Cap on one `controls` fetch.
const ENVIRONMENT_RUNS_TIMEOUT_MS: u64 = 10_000;

/// Fetch bookkeeping: at most one fetch in flight, with one trailing
/// fetch queued behind it so a burst of announcements never stacks
/// requests.
#[derive(Debug, Default)]
pub(super) struct EnvironmentRunsFeed {
    /// A fetch has been issued at least once, so a roster push only
    /// triggers the first one.
    pub(super) requested: bool,
    in_flight: bool,
    queued: bool,
}

impl AgentsViewMode {
    /// Fetch the runs, or queue the fetch behind the one in flight. The
    /// daemon lists every run whichever session answers, so the
    /// request rides a live session that is not itself an environment host
    /// or seat; with none on the roster yet the fetch waits for the next
    /// roster push.
    pub(super) fn refresh_environment_runs(
        &mut self,
        client: &DaemonClient,
        ui_tx: &mpsc::UnboundedSender<UiInput>,
    ) {
        if !client.supports_server_capability(ENVIRONMENT_CAPABILITY) {
            return;
        }
        if self.environment_feed.in_flight {
            self.environment_feed.queued = true;
            return;
        }
        let Some(active_session_id) = self.roster.iter().find_map(|entry| {
            let summary = entry.get("summary")?;
            let live = summary.get("lifecycle").and_then(Value::as_str) == Some("live");
            let participant = summary
                .get("environmentRole")
                .and_then(Value::as_str)
                .is_some();
            summary
                .get("activeSessionId")
                .and_then(Value::as_str)
                .filter(|_| live && !participant)
        }) else {
            return;
        };
        let active_session_id = active_session_id.to_owned();
        self.environment_feed.requested = true;
        self.environment_feed.in_flight = true;
        let client = client.clone();
        let ui_tx = ui_tx.clone();
        tokio::spawn(async move {
            let fetched = tokio::time::timeout(
                Duration::from_millis(ENVIRONMENT_RUNS_TIMEOUT_MS),
                client.request_ok(DaemonCommand::Env {
                    id: None,
                    active_session_id,
                    operation: "controls".into(),
                    parameters: json!({"include_ended": true}),
                    rest: serde_json::Map::default(),
                }),
            )
            .await;
            let runs = match fetched {
                Ok(Ok(data)) => serde_json::from_value(data["runs"].clone())
                    .map_err(|error| format!("{error:#}")),
                Ok(Err(error)) => Err(format!("{error:#}")),
                Err(_) => Err("timed out waiting for the Prime Env daemon response".to_owned()),
            };
            let _ = ui_tx.send(UiInput::EnvironmentRuns { runs });
        });
    }

    /// Fold a landed fetch in: a failure keeps the last list and reports on
    /// the status line. Returns whether a queued fetch is due.
    pub(super) fn apply_environment_runs(
        &mut self,
        runs: Result<Vec<InstanceRun>, String>,
    ) -> bool {
        self.environment_feed.in_flight = false;
        match runs {
            Ok(runs) => self.environment_runs = runs,
            Err(error) => self.set_status_tone(
                &format!("Environment runs unavailable: {error}"),
                StatusTone::Error,
            ),
        }
        std::mem::take(&mut self.environment_feed.queued)
    }
}
