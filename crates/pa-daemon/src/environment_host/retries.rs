//! Bounded re-prompts for awaited agent seats whose turns end without progress.
use super::{
    read, Delivery, DeliveryKind, EnvironmentHost, MissedTurnCause, NoticeTarget, Result, RunPhase,
    RunState, SeatId, Value,
};
use pa_types::environments::{InstanceCheckpoint, MissedTurn};
use serde_json::json;
use std::time::Duration;

/// How long an aborted timed-out turn may take to report its failure before
/// the run pauses on the turn timeout instead.
const TIMEOUT_ABORT_SETTLE_MS: u64 = 60_000;

const IDLE_RETRY_MESSAGE: &str = "Your turn ended without an action, and the environment is still waiting for you. Take one of your available actions now.";

/// Whether the environment still awaits `seat` in an unfinished run.
fn awaits(checkpoint: &InstanceCheckpoint, seat: &SeatId) -> bool {
    checkpoint.finished.is_none()
        && checkpoint
            .progress
            .as_ref()
            .is_some_and(|progress| progress.waiting_for.contains(seat))
}

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

    /// Re-prompts an awaited seat whose settled turn did not move the run: either
    /// the progress revision is unchanged, or other seats advanced it while this
    /// seat took no action (a simultaneous round still waiting on it). Stalls the
    /// run once the seat's retry budget is spent.
    pub(super) fn retry_idle_turn(
        &self,
        state: &mut RunState,
        checkpoint: &InstanceCheckpoint,
        seat: &SeatId,
        through_seq: u64,
    ) -> Result<()> {
        let started = state.turn_revisions.remove(seat);
        let limit = self.instance.roster().limits.idle_turn_retries;
        let reports = self.instance.definition.manifest.missed_turns;
        let Some(progress) = checkpoint.progress.as_ref() else {
            return Ok(());
        };
        if started != Some(progress.revision) {
            // Progress refunds the retry budget. A seat that acted moved the run
            // itself, and any further task reaches it by its own wake; one that did
            // not act may still owe its part of a round other seats advanced.
            state.idle_retries.remove(seat);
            if started.is_none() || state.turn_acted.contains(seat) {
                return Ok(());
            }
        }
        if (limit == 0 && !reports)
            || state.phase != RunPhase::Running
            || state.suspended.contains_key(seat)
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
        if reports {
            // The environment hears about the miss before any re-prompt; dispatch
            // reports it and re-prompts or stalls with the outcome.
            state.idle_retries.insert(seat.clone(), attempts + 1);
            state
                .missed_turns
                .insert(seat.clone(), MissedTurnCause::Idle);
            return Ok(());
        }
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
        self.wake_seat(seat, "idle-retry", IDLE_RETRY_MESSAGE)?;
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
                let reports = self.instance.definition.manifest.missed_turns;
                let exhausted = attempts >= self.instance.roster().limits.idle_turn_retries;
                if exhausted && !reports {
                    return Ok(false);
                }
                state.idle_retries.insert(seat.clone(), attempts + 1);
                if reports {
                    // Even a spent budget is reported: the environment may resolve
                    // the turn, and only otherwise does the run pause.
                    state
                        .missed_turns
                        .insert(seat.clone(), MissedTurnCause::Deadline);
                }
                if !exhausted {
                    *state.deadline_retries.entry(seat.clone()).or_default() += 1;
                }
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

    /// Hands each queued missed turn to the environment's `missed_turn`, then
    /// settles it: a seat the environment no longer awaits needs nothing more; one
    /// whose run advanced gets its budget back; one past its budget stops the run
    /// (an idle miss stalls it, a deadline miss pauses it); otherwise an idle seat
    /// the environment did not wake gets the generic re-prompt, and a timed-out
    /// seat replays its retained wake. A suspended seat is only reported: it is
    /// never re-prompted and never stops the run here. Returns whether any report
    /// was settled.
    pub(super) async fn settle_missed_turns(&self) -> Result<bool> {
        let limit = self.instance.roster().limits.idle_turn_retries;
        let mut settled = false;
        loop {
            let _gate = self.gate.lock().await;
            let (seat, cause, misses) = {
                let state = self.state.lock().await;
                if state.phase != RunPhase::Running {
                    return Ok(settled);
                }
                let Some((seat, cause)) = state.missed_turns.first_key_value() else {
                    return Ok(settled);
                };
                // A resume clears the budget; the queued miss then counts as the first.
                // A suspended seat spends no budget: each report is its only miss.
                let misses = match cause {
                    MissedTurnCause::Idle | MissedTurnCause::Deadline => {
                        state.idle_retries.get(seat).copied().unwrap_or_default()
                    }
                    MissedTurnCause::Suspended => 1,
                };
                (seat.clone(), *cause, misses.max(1))
            };
            settled = true;
            let before = self.instance.checkpoint().await;
            if awaits(&before, &seat) {
                let id = format!("missed-turn-{}", uuid::Uuid::new_v4());
                let report = MissedTurn {
                    cause,
                    misses,
                    limit,
                };
                let outcome = match self.require_mutation(&id).await {
                    Ok(()) => self.instance.missed_turn(&seat, report, &id).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = outcome {
                    if self.instance.checkpoint().await.requires_recovery {
                        return Err(error);
                    }
                    // A refused report leaves the turn to the host's own handling.
                    eprintln!("environment missed_turn for {seat} was refused: {error:#}");
                }
                crate::environment_telemetry::track(
                    self.telemetry.as_ref(),
                    "environment_missed_turn",
                    "completed",
                    Some(match cause {
                        MissedTurnCause::Idle => "idle",
                        MissedTurnCause::Deadline => "deadline",
                        MissedTurnCause::Suspended => "suspended",
                    }),
                );
            }
            let after = self.instance.checkpoint().await;
            self.table.materialize(&after)?;
            let mut state = self.state.lock().await;
            state.missed_turns.remove(&seat);
            let advanced = after.progress.as_ref().map(|progress| progress.revision)
                != before.progress.as_ref().map(|progress| progress.revision);
            if !awaits(&after, &seat) || advanced {
                state.idle_retries.remove(&seat);
            } else if let (true, Some((phase, reason))) = (
                misses > limit,
                match cause {
                    MissedTurnCause::Idle => Some((
                        RunPhase::Stalled,
                        format!("{seat} made no progress after {limit} re-prompts"),
                    )),
                    MissedTurnCause::Deadline => {
                        Some((RunPhase::Paused, String::from("seat turn timeout")))
                    }
                    // A suspended seat has no budget to spend; the run waits on the builder.
                    MissedTurnCause::Suspended => None,
                },
            ) {
                state.phase = phase;
                state.reason = Some(reason);
                self.save_state(&state)?;
                (self.view)(json!({"type":"environment_status","run":*state}))?;
                crate::environment_telemetry::track(
                    self.telemetry.as_ref(),
                    "environment_turn_retry",
                    "failed",
                    None,
                );
                return Ok(settled);
            }
            if awaits(&after, &seat)
                && cause == MissedTurnCause::Idle
                && !self.has_unseen_wake(&seat)?
            {
                self.wake_seat(&seat, "idle-retry", IDLE_RETRY_MESSAGE)?;
                crate::environment_telemetry::track(
                    self.telemetry.as_ref(),
                    "environment_turn_retry",
                    "completed",
                    None,
                );
            }
            self.save_state(&state)?;
        }
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
