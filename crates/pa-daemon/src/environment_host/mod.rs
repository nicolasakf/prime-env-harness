//! Model-less supervised host: owns execution, delivery, and run controls.
mod calls;
mod dispatch;
mod files;
mod interventions;
mod live_view;
mod retries;
mod seats;
use crate::supervisor_link::SupervisorLink;
use anyhow::{Context, Result};
pub(crate) use files::{authenticate, inbox_lock, read, write};
pub(crate) use interventions::Requester;
use pa_environments::{ActiveInstance, TableLog};
use pa_types::environments::{
    AgentContext, Delivery, DeliveryKind, MissedTurnCause, NoticeTarget, OccupantKind, RoleName,
    Seat, SeatCall, SeatId, SeatReply, WakePayload,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

type ViewSink = Arc<dyn Fn(Value) -> Result<()> + Send + Sync>;

pub(crate) use pa_types::environments::RunPhase;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RunState {
    pub phase: RunPhase,
    pub reason: Option<String>,
    pub operations: u64,
    pub wakes: BTreeMap<SeatId, u64>,
    pub busy: BTreeMap<SeatId, u64>,
    pub no_progress_turns: u64,
    pub revision: Option<u64>,
    pub started_at: u64,
    pub settled: BTreeMap<SeatId, u64>,
    #[serde(default)]
    pub turn_sequences: BTreeMap<SeatId, u64>,
    #[serde(default)]
    pub direct_messages: BTreeMap<SeatId, u64>,
    #[serde(default)]
    pub wake_sequences: BTreeMap<SeatId, u64>,
    #[serde(default)]
    pub builder_notified: Option<RunPhase>,
    /// Set when a run bound paused the run, as opposed to a requester's pause or stop;
    /// only an automatic pause is news to the builder.
    #[serde(default)]
    pub paused_automatically: bool,
    /// Progress revision observed when each seat's current turn started.
    #[serde(default)]
    pub turn_revisions: BTreeMap<SeatId, u64>,
    /// Consecutive re-prompts spent per seat since it last advanced the run.
    #[serde(default)]
    pub idle_retries: BTreeMap<SeatId, u64>,
    /// Missed turns queued for the environment's `missed_turn`, with their cause.
    #[serde(default)]
    pub missed_turns: BTreeMap<SeatId, MissedTurnCause>,
    /// Seats whose timed-out turn is being aborted for a replay, with the abort time.
    #[serde(default)]
    pub timeout_aborts: BTreeMap<SeatId, u64>,
    /// Turns that ran past the deadline and were aborted for a retry, per seat.
    #[serde(default)]
    pub deadline_retries: BTreeMap<SeatId, u64>,
    /// Turns cut after the seat had already acted, to let its next wake through.
    #[serde(default)]
    pub post_action_aborts: BTreeMap<SeatId, u64>,
    /// Turn start to first action, per seat: how long the seat deliberated.
    #[serde(default)]
    pub time_to_action: BTreeMap<SeatId, ActionTiming>,
    /// Seats that already acted in their current turn.
    #[serde(default)]
    pub turn_acted: std::collections::BTreeSet<SeatId>,
    /// Agent seats the builder took out of play: they get no wakes or turns, and
    /// their own calls cannot change the run, until the builder resumes them.
    #[serde(default)]
    pub suspended: BTreeMap<SeatId, SuspendedSeat>,
    /// Actions the builder committed on each seat's behalf.
    #[serde(default)]
    pub builder_actions: BTreeMap<SeatId, u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SuspendedSeat {
    pub reason: String,
    /// The progress revision in which the environment last heard, through
    /// `missed_turn`, that it awaits this suspended seat.
    #[serde(default)]
    pub reported_revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ActionTiming {
    pub count: u64,
    pub total_ms: u64,
    pub max_ms: u64,
}

pub(crate) struct EnvironmentHost {
    pub instance: Arc<ActiveInstance>,
    pub table: TableLog,
    pub state: Mutex<RunState>,
    pub gate: Mutex<()>,
    pub link: SupervisorLink,
    pub builder_token: String,
    pub host_id: String,
    pub telemetry: Option<pa_telemetry::TelemetryClient>,
    pub defaults: Value,
    pub seats: Mutex<BTreeMap<SeatId, Value>>,
    pub python_cancellations: Mutex<BTreeMap<SeatId, (String, CancellationToken)>>,
    pub changed: Notify,
    pub shutdown: CancellationToken,
    pub view: ViewSink,
    /// The live-view server, started by the first `view` request.
    live: Mutex<Option<Arc<live_view::LiveViews>>>,
    /// The last phase announced as an `environment_runs_changed` event, so a
    /// save that keeps the phase does not wake every client's dock. Unset
    /// until the first save, which announces the run itself.
    announced_phase: std::sync::Mutex<Option<RunPhase>>,
}

impl EnvironmentHost {
    pub async fn open(
        instance: Arc<ActiveInstance>,
        socket: std::path::PathBuf,
        host_id: String,
        builder_token: String,
        defaults: Value,
        view: ViewSink,
        telemetry: Option<pa_telemetry::TelemetryClient>,
    ) -> Result<Arc<Self>> {
        let path = instance.path();
        let table = TableLog::open(path)?;
        table.materialize(&instance.checkpoint().await)?;
        let state_path = path.join("run.json");
        let mut state = if state_path.exists() {
            read(&state_path)?
        } else {
            RunState {
                phase: RunPhase::Running,
                reason: None,
                operations: 0,
                wakes: BTreeMap::new(),
                busy: BTreeMap::new(),
                no_progress_turns: 0,
                revision: None,
                started_at: crate::util::now_ms(),
                settled: BTreeMap::new(),
                turn_sequences: BTreeMap::new(),
                direct_messages: BTreeMap::new(),
                wake_sequences: BTreeMap::new(),
                builder_notified: None,
                paused_automatically: false,
                turn_revisions: BTreeMap::new(),
                idle_retries: BTreeMap::new(),
                missed_turns: BTreeMap::new(),
                timeout_aborts: BTreeMap::new(),
                deadline_retries: BTreeMap::new(),
                post_action_aborts: BTreeMap::new(),
                time_to_action: BTreeMap::new(),
                turn_acted: std::collections::BTreeSet::new(),
                suspended: BTreeMap::new(),
                builder_actions: BTreeMap::new(),
            }
        };
        if instance.checkpoint().await.finished.is_some() {
            state.phase = RunPhase::Finished;
        }
        // A surviving seat retains its durable inbox; replay reconciles any turn
        // whose completion was interrupted by the host crash.
        state.busy.clear();
        state.timeout_aborts.clear();
        write(&state_path, &state)?;
        write(
            &path.join("host-session.json"),
            &serde_json::json!({"activeSessionId":host_id}),
        )?;
        let host = Arc::new(Self {
            link: SupervisorLink::owned(socket, format!("env-host-{}", instance.instance.id)),
            instance,
            table,
            state: Mutex::new(state),
            gate: Mutex::new(()),
            builder_token,
            host_id,
            telemetry,
            defaults,
            seats: Mutex::new(BTreeMap::new()),
            python_cancellations: Mutex::new(BTreeMap::new()),
            changed: Notify::new(),
            shutdown: CancellationToken::new(),
            view,
            live: Mutex::new(None),
            announced_phase: std::sync::Mutex::new(None),
        });
        host.provision().await?;
        Ok(host)
    }

    pub fn save_state(&self, state: &RunState) -> Result<()> {
        write(&self.instance.path().join("run.json"), state)?;
        let changed = {
            let mut announced = self
                .announced_phase
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let changed = announced.as_ref() != Some(&state.phase);
            *announced = Some(state.phase.clone());
            changed
        };
        if changed {
            (self.view)(serde_json::json!({"type":"environment_runs_changed"}))?;
        }
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        self.shutdown.cancel();
        let mut state = self.state.lock().await;
        if state.phase == RunPhase::Running && self.human().is_some() {
            state.phase = RunPhase::Paused;
            state.reason = Some("human detached".into());
            self.save_state(&state)?;
        }
        drop(state);
        self.stop_seats().await?;
        self.instance.leave().await
    }

    pub fn human(&self) -> Option<SeatId> {
        self.instance
            .roster()
            .seats
            .into_iter()
            .find(|seat| seat.occupant == OccupantKind::Human)
            .map(|seat| seat.id)
    }

    pub fn seat_token(&self, seat: &SeatId) -> Result<String> {
        read(
            &self
                .instance
                .path()
                .join("seats")
                .join(seat.as_str())
                .join("token.json"),
        )
    }

    pub fn check_seat(&self, seat: &SeatId, token: &str) -> Result<()> {
        self.instance
            .roster()
            .seats
            .iter()
            .find(|entry| &entry.id == seat && entry.occupant == OccupantKind::Agent)
            .context("unknown agent seat")?;
        authenticate(&self.seat_token(seat)?, token)
    }
}

#[cfg(test)]
mod attention_tests;
#[cfg(test)]
mod builder_notice_tests;
#[cfg(test)]
mod intervention_tests;
#[cfg(test)]
mod live_view_tests;
#[cfg(test)]
mod tests;
