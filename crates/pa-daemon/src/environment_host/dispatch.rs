//! Drain durable deliveries; admission acknowledgments precede host cursor updates.
use super::{
    read, write, AgentContext, Arc, Context, EnvironmentHost, MissedTurnCause, OccupantKind,
    Result, RunPhase, SeatId, WakePayload,
};
use pa_environments::DeliveryCursor;
use serde_json::json;
use std::time::Duration;

impl EnvironmentHost {
    pub fn start(self: &Arc<Self>) {
        let host = Arc::clone(self);
        tokio::spawn(async move {
            let mut commits = host.instance.commits();
            let mut timer = tokio::time::interval(Duration::from_secs(1));
            loop {
                if let Err(error) = host.drain().await {
                    if error
                        .downcast_ref::<super::seats::DeliveryUnavailable>()
                        .is_none()
                    {
                        let mut state = host.state.lock().await;
                        state.phase = if host.instance.checkpoint().await.requires_recovery {
                            RunPhase::NeedsRecovery
                        } else {
                            RunPhase::Paused
                        };
                        state.reason = Some("delivery failed; builder inspection required".into());
                        eprintln!("environment delivery failed: {error:#}");
                        if let Err(error) = host.save_state(&state) {
                            eprintln!("environment run checkpoint failed: {error:#}");
                        }
                        drop(state);
                        if let Err(error) = host.abort_seats().await {
                            eprintln!("environment seat abort failed: {error:#}");
                        }
                        if let Err(error) = (host.view)(
                            json!({"type":"environment_status","run":*host.state.lock().await}),
                        ) {
                            eprintln!("environment status persistence failed: {error:#}");
                        }
                    }
                }
                tokio::select! {
                    () = host.shutdown.cancelled() => break,
                    changed = commits.changed() => if changed.is_err() { break; },
                    () = host.changed.notified() => {},
                    _ = timer.tick() => {},
                }
            }
        });
    }

    pub async fn abort_seats(&self) -> Result<()> {
        for (_, signal) in self.python_cancellations.lock().await.values() {
            signal.cancel();
        }
        for (seat, session) in self.seats.lock().await.clone() {
            self.abort_seat(&seat, &session).await?;
        }
        Ok(())
    }

    pub(super) async fn drain(&self) -> Result<()> {
        if let Err(error) = self.notify_builder().await {
            eprintln!("environment builder notification deferred: {error:#}");
        }
        let checkpoint = self.instance.checkpoint().await;
        self.table.materialize(&checkpoint)?;
        let roster = self.instance.roster();
        let agents = roster
            .seats
            .iter()
            .filter(|seat| seat.occupant == OccupantKind::Agent)
            .count() as u64;
        // The deadline bounds time to action: how long a seat the environment is
        // waiting on may take since its turn started or it last acted. It also bounds
        // the follow-up of a seat that acted and is no longer awaited: such a seat has
        // nothing left to do, and a model can keep calling tools for many requests,
        // each one only capped in output. Cutting that follow-up is cleanup that spends
        // no retry budget, and its retained wake, a task the seat no longer has, is
        // retired when it replays.
        let timed_out = {
            let state = self.state.lock().await;
            let awaited = checkpoint
                .progress
                .as_ref()
                .map(|progress| &progress.waiting_for);
            state
                .busy
                .iter()
                .filter(|(seat, started)| {
                    (awaited.is_none_or(|awaited| awaited.contains(seat))
                        || state.turn_acted.contains(*seat))
                        && crate::util::now_ms().saturating_sub(**started) / 1000
                            >= roster.limits.seat_turn_timeout_seconds
                })
                .map(|(seat, _)| seat.clone())
                .collect::<Vec<_>>()
        };
        let mut confirmed_timeout = false;
        for seat in timed_out {
            if let Some(session) = self.seats.lock().await.get(&seat).cloned() {
                let reply = self.link.request(json!({"type":"get_state","activeSessionId":session["activeSessionId"],"environmentSeatCapability":self.seat_token(&seat)?}), Duration::from_secs(30)).await?;
                if reply.success
                    && reply
                        .data
                        .as_ref()
                        .is_some_and(|state| state["isStreaming"] == true)
                    && !self.retry_timed_out_turn(&seat, &session).await?
                {
                    confirmed_timeout = true;
                }
            }
        }
        // A suspended seat the environment awaits is reported once per progress
        // revision, so the environment may play for it; without `missed_turn` the
        // run goes quiescent and the builder is told which seat it waits on.
        if self.instance.definition.manifest.missed_turns {
            if let Some(progress) = checkpoint.progress.as_ref() {
                let mut state = self.state.lock().await;
                let state = &mut *state;
                if state.phase == RunPhase::Running && checkpoint.finished.is_none() {
                    let mut queued = false;
                    for (seat, suspended) in &mut state.suspended {
                        if progress.waiting_for.contains(seat)
                            && suspended.reported_revision != Some(progress.revision)
                        {
                            suspended.reported_revision = Some(progress.revision);
                            state
                                .missed_turns
                                .insert(seat.clone(), MissedTurnCause::Suspended);
                            queued = true;
                        }
                    }
                    if queued {
                        self.save_state(state)?;
                    }
                }
            }
        }
        // The environment's answer to a missed turn may commit notices or progress
        // that the rest of this pass must see.
        let checkpoint = if self.settle_missed_turns().await? {
            self.instance.checkpoint().await
        } else {
            checkpoint
        };
        let mut state = self.state.lock().await;
        state.operations = checkpoint.receipts.len().saturating_sub(1) as u64;
        let old_phase = state.phase.clone();
        if checkpoint.requires_recovery {
            state.phase = RunPhase::NeedsRecovery;
            state.reason = Some("explicit environment recovery required".into());
        }
        if checkpoint.finished.is_some() {
            state.phase = RunPhase::Finished;
            state.reason = checkpoint
                .finished
                .as_ref()
                .map(|finished| finished.summary.clone());
        }
        if state.phase == RunPhase::Running {
            let now = crate::util::now_ms();
            let stall = if roster.limits.stall_turns == 0 {
                2 * agents + 2
            } else {
                roster.limits.stall_turns
            };
            let reason = if state.operations >= roster.limits.max_operations {
                Some("operation limit")
            } else if now.saturating_sub(state.started_at) / 1000
                >= roster.limits.max_wall_clock_seconds
            {
                Some("wall-clock limit")
            } else if confirmed_timeout {
                Some("seat turn timeout")
            } else if state.no_progress_turns >= stall {
                Some("no environment progress")
            } else {
                None
            };
            if let Some(reason) = reason {
                state.phase = if reason == "no environment progress" {
                    RunPhase::Stalled
                } else {
                    state.paused_automatically = true;
                    RunPhase::Paused
                };
                state.reason = Some(reason.into());
            }
        }
        self.save_state(&state)?;
        let phase = state.phase.clone();
        drop(state);
        if phase != old_phase {
            if matches!(phase, RunPhase::Paused | RunPhase::Stalled) {
                crate::environment_telemetry::track(
                    self.telemetry.as_ref(),
                    "environment_run_limit",
                    "completed",
                    None,
                );
            }
            if let Err(error) = self.notify_builder().await {
                eprintln!("environment builder notification deferred: {error:#}");
            }
            self.abort_seats().await?;
            if phase == RunPhase::Finished {
                self.stop_seats().await?;
            }
            (self.view)(json!({"type":"environment_status","run":*self.state.lock().await}))?;
        }
        // Public/human delivery never depends on whether models may currently run.
        let view_path = self.instance.path().join("view-cursor.json");
        let mut cursor: DeliveryCursor = if view_path.exists() {
            read(&view_path)?
        } else {
            DeliveryCursor::default()
        };
        let deliveries = self
            .table
            .since(self.human().as_ref(), cursor.through_seq)?;
        for delivery in deliveries {
            (self.view)(json!({"type":"environment_delivery","delivery":delivery}))?;
            cursor.through_seq = delivery.seq;
            write(&view_path, &cursor)?;
        }
        let status = self.status().await?;
        (self.view)(
            json!({"type":"environment_observation","observation":status["observation"],"view_version":status["view_version"]}),
        )?;
        if phase != RunPhase::Running {
            return Ok(());
        }
        for seat in roster
            .seats
            .iter()
            .filter(|seat| seat.occupant == OccupantKind::Agent)
        {
            // A suspended seat's deliveries wait, unacknowledged, until it resumes.
            if self.state.lock().await.suspended.contains_key(&seat.id) {
                continue;
            }
            let mut cursor = DeliveryCursor::load(self.instance.path(), &seat.id)?;
            let deliveries = self.table.since(Some(&seat.id), cursor.through_seq)?;
            if !deliveries.iter().any(|delivery| delivery.wake) {
                continue;
            }
            let through_seq = deliveries.last().context("wake deliveries")?.seq;
            let mut state = self.state.lock().await;
            let count = state.wakes.get(&seat.id).copied().unwrap_or_default();
            if state
                .wake_sequences
                .get(&seat.id)
                .copied()
                .unwrap_or_default()
                < through_seq
                && count >= roster.limits.max_wakes_per_seat
            {
                state.phase = RunPhase::Paused;
                state.reason = Some(format!("wake limit for {}", seat.id));
                self.save_state(&state)?;
                (self.view)(json!({"type":"environment_status","run":*state}))?;
                crate::environment_telemetry::track(
                    self.telemetry.as_ref(),
                    "environment_run_limit",
                    "completed",
                    None,
                );
                drop(state);
                self.abort_seats().await?;
                return Ok(());
            }
            if state
                .wake_sequences
                .get(&seat.id)
                .copied()
                .unwrap_or_default()
                < through_seq
            {
                state.wakes.insert(seat.id.clone(), count + 1);
                state.wake_sequences.insert(seat.id.clone(), through_seq);
                self.save_state(&state)?;
            }
            // Re-prompts spent since the seat last advanced the run: a retried turn
            // runs with thinking off.
            let attempt = state
                .idle_retries
                .get(&seat.id)
                .copied()
                .unwrap_or_default();
            drop(state);
            let _gate = self.gate.lock().await;
            if self.state.lock().await.phase != RunPhase::Running
                || self.instance.checkpoint().await.finished.is_some()
            {
                return Ok(());
            }
            let agent_context = self.instance.definition.manifest.agent_context;
            let (view_version, observation) = match agent_context {
                AgentContext::Conversation => self.instance.observe_versioned(&seat.id).await?,
                AgentContext::Stateless => self.instance.agent_view_versioned(&seat.id).await?,
            };
            let payload = WakePayload {
                through_seq,
                observation,
                view_version: Some(view_version),
                deliveries,
                agent_context,
                attempt,
            };
            let token = self.seat_token(&seat.id)?;
            self.send_seat(
                &seat.id,
                "seat_wake",
                json!({"token":token,"payload":payload}),
            )
            .await?;
            cursor.acknowledge(self.instance.path(), &seat.id, through_seq)?;
            crate::environment_telemetry::track(
                self.telemetry.as_ref(),
                "environment_wake",
                "completed",
                None,
            );
        }
        let mut pending = false;
        let suspended = self.state.lock().await.suspended.clone();
        for seat in roster.seats.iter().filter(|seat| {
            seat.occupant == OccupantKind::Agent && !suspended.contains_key(&seat.id)
        }) {
            let path = self
                .instance
                .path()
                .join("seats")
                .join(seat.id.as_str())
                .join("inbox.json");
            if path.exists() {
                let inbox: crate::environment_seat::SeatInbox = read(&path)?;
                if let Some(payload) = inbox
                    .pending
                    .filter(|payload| payload.through_seq > inbox.settled)
                {
                    pending = true;
                    let (should_replay, attempt) = {
                        let state = self.state.lock().await;
                        (
                            !state.busy.contains_key(&seat.id) && state.phase == RunPhase::Running,
                            state
                                .idle_retries
                                .get(&seat.id)
                                .copied()
                                .unwrap_or_default(),
                        )
                    };
                    if should_replay {
                        // The stored payload predates any retry; stamp the current attempt.
                        let payload = WakePayload { attempt, ..payload };
                        self.send_seat(
                            &seat.id,
                            "seat_replay",
                            json!({"token":self.seat_token(&seat.id)?,"payload":payload}),
                        )
                        .await?;
                    }
                }
            }
        }
        let human_waiting = self.human().is_some_and(|seat| {
            checkpoint
                .progress
                .as_ref()
                .is_none_or(|progress| progress.waiting_for.contains(&seat))
        });
        let mut state = self.state.lock().await;
        let mut quiescent = state.phase == RunPhase::Running
            && state.busy.is_empty()
            && state.missed_turns.is_empty()
            && !pending
            && !human_waiting;
        // A settling turn appends its re-prompt or queues its missed turn under the
        // state lock, so either landing after the dispatch pass above counts as pending.
        // Held-back wakes of suspended seats are not pending: nothing will run them.
        for seat in roster.seats.iter().filter(|seat| {
            seat.occupant == OccupantKind::Agent && !state.suspended.contains_key(&seat.id)
        }) {
            if !quiescent {
                break;
            }
            quiescent = !self.has_unseen_wake(&seat.id)?;
        }
        if quiescent {
            let waiting_on_suspended: Vec<_> = checkpoint
                .progress
                .as_ref()
                .map(|progress| {
                    progress
                        .waiting_for
                        .iter()
                        .filter(|seat| state.suspended.contains_key(*seat))
                        .map(SeatId::as_str)
                        .collect()
                })
                .unwrap_or_default();
            state.phase = RunPhase::Stalled;
            state.reason = Some(if waiting_on_suspended.is_empty() {
                "quiescent: no pending wake or human turn".into()
            } else {
                format!(
                    "waiting on suspended seat {}: act for it or resume it",
                    waiting_on_suspended.join(", ")
                )
            });
            crate::environment_telemetry::track(
                self.telemetry.as_ref(),
                "environment_run_limit",
                "completed",
                None,
            );
            self.save_state(&state)?;
            (self.view)(json!({"type":"environment_status","run":*state}))?;
        }
        Ok(())
    }
    async fn notify_builder(&self) -> Result<()> {
        let (phase, reason) = {
            let state = self.state.lock().await;
            let news = match state.phase {
                RunPhase::Finished | RunPhase::Stalled | RunPhase::NeedsRecovery => true,
                RunPhase::Paused => state.paused_automatically,
                RunPhase::Running | RunPhase::Stopped => false,
            };
            if !news || state.builder_notified.as_ref() == Some(&state.phase) {
                return Ok(());
            }
            (
                state.phase.clone(),
                state.reason.clone().unwrap_or_default(),
            )
        };
        let next = match phase {
            RunPhase::Stalled => " For a seat that is stuck, environments.act_for_seat takes its turn, and suspend_seat takes it out of play; then resume the run with environments.resume_instance.",
            RunPhase::Paused => " The run paused itself and stays paused until it is resumed with environments.resume_instance.",
            RunPhase::Running | RunPhase::NeedsRecovery | RunPhase::Stopped | RunPhase::Finished => "",
        };
        let parent = self.instance.path().join("builder-session.json");
        if !parent.exists() {
            return Ok(());
        }
        let parent: String = read(&parent)?;
        let response = self.link.request(serde_json::json!({"type":"send_message","targetActiveSessionId":parent,"fromActiveSessionId":self.host_id,"message":format!("Environment {} is {:?}: {reason}. Use environments.instance_status and privileged read APIs to inspect it.{next}", self.instance.instance.id, phase)}), Duration::from_secs(30)).await?;
        if !response.success {
            eprintln!(
                "environment builder notification failed: {:?}",
                response.error
            );
            return Ok(());
        }
        let mut state = self.state.lock().await;
        state.builder_notified = Some(phase);
        self.save_state(&state)
    }
}
