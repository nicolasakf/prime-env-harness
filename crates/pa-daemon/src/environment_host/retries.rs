//! Bounded re-prompts for awaited agent seats whose turns end without progress.
use super::{
    read, Delivery, DeliveryKind, EnvironmentHost, NoticeTarget, Result, RunPhase, RunState,
    SeatId, Value,
};
use pa_types::environments::InstanceCheckpoint;
use serde_json::json;
use std::time::Duration;

/// How long an aborted timed-out turn may take to report its failure before
/// the run pauses on the turn timeout instead.
const TIMEOUT_ABORT_SETTLE_MS: u64 = 60_000;

impl EnvironmentHost {
    pub(super) fn seat_inbox(&self, seat: &SeatId) -> Result<crate::environment_seat::SeatInbox> {
        let path = self
            .instance
            .path()
            .join("seats")
            .join(seat.as_str())
            .join("inbox.json");
        if path.exists() {
            read(&path)
        } else {
            Ok(crate::environment_seat::SeatInbox::default())
        }
    }

    /// Whether the table holds a wake for `seat` that dispatch has not delivered.
    pub(super) fn has_unseen_wake(&self, seat: &SeatId) -> Result<bool> {
        let cursor = pa_environments::DeliveryCursor::load(self.instance.path(), seat)?;
        Ok(self
            .table
            .since(Some(seat), cursor.through_seq)?
            .iter()
            .any(|delivery| delivery.wake))
    }

    /// Appends a status wake addressed only to `seat`; dispatch delivers it.
    pub(super) fn wake_seat(&self, seat: &SeatId, event: &str, message: &str) -> Result<()> {
        self.table.append(Delivery {
            seq: 0,
            event_id: format!("{event}-{}", uuid::Uuid::new_v4()),
            op_seq: None,
            kind: DeliveryKind::Status,
            from: None,
            to: NoticeTarget::Seats(vec![seat.clone()]),
            message: Some(message.into()),
            wake: true,
            actionable: false,
            timestamp: 0,
        })?;
        Ok(())
    }

    /// Re-prompts an awaited seat whose settled turn left the progress revision
    /// unchanged, stalling the run once the seat's retry budget is spent.
    pub(super) fn retry_idle_turn(
        &self,
        state: &mut RunState,
        checkpoint: &InstanceCheckpoint,
        seat: &SeatId,
        through_seq: u64,
    ) -> Result<()> {
        let started = state.turn_revisions.remove(seat);
        let limit = self.instance.roster().limits.idle_turn_retries;
        let Some(progress) = checkpoint.progress.as_ref() else {
            return Ok(());
        };
        if started != Some(progress.revision) {
            state.idle_retries.remove(seat);
            return Ok(());
        }
        if limit == 0
            || state.phase != RunPhase::Running
            || checkpoint.finished.is_some()
            || !progress.waiting_for.contains(seat)
            || self.has_unseen_wake(seat)?
            || self
                .seat_inbox(seat)?
                .pending
                .is_some_and(|payload| payload.through_seq > through_seq)
        {
            return Ok(());
        }
        let attempts = state.idle_retries.get(seat).copied().unwrap_or_default();
        if attempts >= limit {
            state.phase = RunPhase::Stalled;
            state.reason = Some(format!(
                "{seat} made no progress after {attempts} re-prompts"
            ));
            crate::environment_telemetry::track(
                self.telemetry.as_ref(),
                "environment_turn_retry",
                "failed",
                None,
            );
            return Ok(());
        }
        state.idle_retries.insert(seat.clone(), attempts + 1);
        self.wake_seat(
            seat,
            "idle-retry",
            "Your turn ended without an action, and the environment is still waiting for you. Take one of your available actions now.",
        )?;
        crate::environment_telemetry::track(
            self.telemetry.as_ref(),
            "environment_turn_retry",
            "completed",
            None,
        );
        Ok(())
    }

    /// Aborts a seat's timed-out turn so its retained wake replays. Returns
    /// false when the run should pause on the timeout instead: the budget is
    /// spent, or an earlier abort never reported back.
    pub(super) async fn retry_timed_out_turn(
        &self,
        seat: &SeatId,
        session: &Value,
    ) -> Result<bool> {
        let now = crate::util::now_ms();
        {
            let mut state = self.state.lock().await;
            if let Some(aborted_at) = state.timeout_aborts.get(seat) {
                return Ok(now.saturating_sub(*aborted_at) < TIMEOUT_ABORT_SETTLE_MS);
            }
            if state.phase != RunPhase::Running {
                return Ok(false);
            }
            if state.turn_acted.contains(seat) {
                // The seat acted; only its lingering follow-up is holding the next
                // wake back. Cutting it is cleanup, not a retry: it spends no retry
                // budget and the next turn keeps its thinking.
                *state.post_action_aborts.entry(seat.clone()).or_default() += 1;
            } else {
                let attempts = state.idle_retries.get(seat).copied().unwrap_or_default();
                if attempts >= self.instance.roster().limits.idle_turn_retries {
                    return Ok(false);
                }
                state.idle_retries.insert(seat.clone(), attempts + 1);
                *state.deadline_retries.entry(seat.clone()).or_default() += 1;
            }
            state.timeout_aborts.insert(seat.clone(), now);
            self.save_state(&state)?;
        }
        self.abort_seat(seat, session).await?;
        crate::environment_telemetry::track(
            self.telemetry.as_ref(),
            "environment_turn_retry",
            "completed",
            None,
        );
        Ok(true)
    }

    pub(super) async fn abort_seat(&self, seat: &SeatId, session: &Value) -> Result<()> {
        let response = self
            .link
            .request(
                json!({"type":"abort","activeSessionId":session["activeSessionId"]}),
                Duration::from_secs(30),
            )
            .await?;
        if !response.success
            && !response
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("Unknown active session"))
        {
            anyhow::bail!("could not abort seat {seat}: {:?}", response.error);
        }
        Ok(())
    }
}
