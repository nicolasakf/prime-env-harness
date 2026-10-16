//! Builder interventions in a stuck run: acting for an awaited agent seat, and
//! suspending or resuming a seat. They are run controls, not game logic: an
//! action taken for a seat goes through the environment's own validation like
//! any other, and what a suspended seat's absence means is the environment's to
//! decide through `missed_turn`.
use super::{
    Arc, Context, Delivery, DeliveryKind, EnvironmentHost, NoticeTarget, OccupantKind, Result,
    RunPhase, SeatId, SuspendedSeat, Value,
};
use anyhow::bail;
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Longest reason the builder may attach to an intervention.
const MAX_REASON_BYTES: usize = 1024;

const RESUMED_SEAT_MESSAGE: &str = "The builder resumed your seat. Observe the current environment, then act only if this is a legal turn for you.";

/// Who asked for an intervention: the builder agent through its API, or the
/// human at the builder prompt through `/env`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Requester {
    Builder,
    Human,
}

impl Requester {
    fn telemetry_choice(self, intervention: &str) -> String {
        match self {
            Self::Builder => format!("builder_{intervention}"),
            Self::Human => intervention.to_owned(),
        }
    }
}

fn checked_reason(reason: &str) -> Result<&str> {
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("an intervention needs a reason");
    }
    if reason.len() > MAX_REASON_BYTES {
        bail!("the reason exceeds {MAX_REASON_BYTES} bytes");
    }
    Ok(reason)
}

impl EnvironmentHost {
    /// Fails unless `seat` is an agent seat: the human at a table acts for themself.
    fn intervention_seat(&self, seat: &SeatId) -> Result<()> {
        let entry = self
            .instance
            .roster()
            .seats
            .into_iter()
            .find(|entry| &entry.id == seat)
            .context("unknown seat")?;
        match entry.occupant {
            OccupantKind::Agent => Ok(()),
            OccupantKind::Human => bail!("the human seat acts for itself"),
        }
    }

    /// Fails unless the run can still change: not ended and not awaiting recovery.
    /// Paused and stalled runs accept interventions, since resolving a stuck seat
    /// is usually why the builder stopped the run.
    async fn require_open_run(&self) -> Result<()> {
        let checkpoint = self.instance.checkpoint().await;
        let phase = self.state.lock().await.phase.clone();
        if phase.is_terminal()
            || phase == RunPhase::NeedsRecovery
            || checkpoint.finished.is_some()
            || checkpoint.requires_recovery
        {
            bail!("the run has ended or needs recovery; it takes no interventions");
        }
        Ok(())
    }

    /// Aborts `seat`'s model turn if one is running. The abort is marked as the
    /// host's own, so its failure report neither pauses the run nor spends retries.
    async fn cut_turn(&self, seat: &SeatId) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            if !state.busy.contains_key(seat) {
                return Ok(());
            }
            state
                .timeout_aborts
                .insert(seat.clone(), crate::util::now_ms());
            self.save_state(&state)?;
        }
        if let Some(session) = self.seats.lock().await.get(seat).cloned() {
            self.abort_seat(seat, &session).await?;
        }
        Ok(())
    }

    /// Commits `action` for an agent seat the environment is waiting on, as if the
    /// seat had taken it. The environment validates it like any other action, and
    /// only the seat itself is told that the builder acted for it and why. A paused
    /// or stalled run stays so; its new wakes go out when it resumes.
    pub(crate) async fn act_for_seat(
        self: &Arc<Self>,
        seat: &SeatId,
        action: &str,
        arguments: Value,
        reason: &str,
        requester: Requester,
    ) -> Result<Value> {
        self.intervention_seat(seat)?;
        let reason = checked_reason(reason)?;
        let _gate = self.gate.lock().await;
        self.require_open_run().await?;
        let checkpoint = self.instance.checkpoint().await;
        if checkpoint
            .progress
            .as_ref()
            .is_some_and(|progress| !progress.waiting_for.contains(seat))
        {
            bail!("the environment is not waiting on {seat}; act for a seat only when it is that seat's turn");
        }
        self.within_operation_limit(&checkpoint)?;
        self.cut_turn(seat).await?;
        let operation_id = format!("builder-{}", uuid::Uuid::new_v4());
        let result = self
            .instance
            .invoke(
                seat,
                action,
                arguments.clone(),
                &operation_id,
                CancellationToken::new(),
            )
            .await;
        crate::environment_telemetry::track(
            self.telemetry.as_ref(),
            "environment_seat_intervention",
            if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            Some(&requester.telemetry_choice("act")),
        );
        let result = result?;
        self.table.materialize(&self.instance.checkpoint().await)?;
        self.table.append(Delivery {
            seq: 0,
            event_id: format!("builder-act-{operation_id}"),
            op_seq: None,
            kind: DeliveryKind::Status,
            from: None,
            to: NoticeTarget::Seats(vec![seat.clone()]),
            message: Some(format!(
                "The run's builder took this action for you: {action} {arguments}. Reason: {reason}"
            )),
            wake: false,
            actionable: false,
            timestamp: 0,
        })?;
        let mut state = self.state.lock().await;
        *state.builder_actions.entry(seat.clone()).or_default() += 1;
        state.idle_retries.remove(seat);
        state.missed_turns.remove(seat);
        self.save_state(&state)?;
        (self.view)(json!({"type":"environment_status","run":*state}))?;
        let run = json!(*state);
        drop(state);
        self.changed.notify_one();
        Ok(
            json!({"instance_id":self.instance.instance.id,"seat":seat,"action":action,"result":result,"run":run}),
        )
    }

    /// Takes an agent seat out of play: its running turn is cut, it gets no wakes
    /// or turns, and its own calls cannot change the run. While the environment
    /// waits on it, the environment hears so through `missed_turn` once per
    /// progress revision; otherwise the run stalls until the builder acts for the
    /// seat or resumes it. Suspending a suspended seat updates its reason.
    pub(crate) async fn suspend_seat(
        self: &Arc<Self>,
        seat: &SeatId,
        reason: &str,
        requester: Requester,
    ) -> Result<Value> {
        self.intervention_seat(seat)?;
        let reason = checked_reason(reason)?;
        let _gate = self.gate.lock().await;
        self.require_open_run().await?;
        {
            let mut state = self.state.lock().await;
            state.suspended.insert(
                seat.clone(),
                SuspendedSeat {
                    reason: reason.to_owned(),
                    reported_revision: None,
                },
            );
            state.idle_retries.remove(seat);
            state.missed_turns.remove(seat);
            state.turn_revisions.remove(seat);
            self.save_state(&state)?;
        }
        self.cut_turn(seat).await?;
        crate::environment_telemetry::track(
            self.telemetry.as_ref(),
            "environment_seat_intervention",
            "completed",
            Some(&requester.telemetry_choice("suspend")),
        );
        let state = self.state.lock().await;
        (self.view)(json!({"type":"environment_status","run":*state}))?;
        let run = json!(*state);
        drop(state);
        self.changed.notify_one();
        Ok(json!({"instance_id":self.instance.instance.id,"seat":seat,"suspended":true,"run":run}))
    }

    /// Returns a suspended seat to play. Wakes it held back are delivered by
    /// dispatch; a seat the environment awaits with nothing left to deliver is
    /// told to observe and act. A stalled or paused run stays so until resumed.
    pub(crate) async fn resume_seat(
        self: &Arc<Self>,
        seat: &SeatId,
        requester: Requester,
    ) -> Result<Value> {
        self.intervention_seat(seat)?;
        let _gate = self.gate.lock().await;
        self.require_open_run().await?;
        let checkpoint = self.instance.checkpoint().await;
        let mut state = self.state.lock().await;
        if state.suspended.remove(seat).is_none() {
            bail!("{seat} is not suspended");
        }
        let awaited = checkpoint
            .progress
            .as_ref()
            .is_some_and(|progress| progress.waiting_for.contains(seat));
        if awaited && self.seat_inbox(seat)?.pending.is_none() && !self.has_unseen_wake(seat)? {
            self.wake_seat(seat, "resume-seat", RESUMED_SEAT_MESSAGE)?;
        }
        self.save_state(&state)?;
        crate::environment_telemetry::track(
            self.telemetry.as_ref(),
            "environment_seat_intervention",
            "completed",
            Some(&requester.telemetry_choice("resume")),
        );
        (self.view)(json!({"type":"environment_status","run":*state}))?;
        let run = json!(*state);
        drop(state);
        self.changed.notify_one();
        Ok(json!({"instance_id":self.instance.instance.id,"seat":seat,"suspended":false,"run":run}))
    }
}
