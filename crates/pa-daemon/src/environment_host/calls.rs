//! Authenticated seat calls and human/builder control surfaces.
use super::interventions::Requester;
use super::{
    authenticate, read, AgentContext, Arc, Context, Delivery, DeliveryKind, EnvironmentHost,
    NoticeTarget, OccupantKind, Result, RoleName, RunPhase, SeatCall, SeatId, SeatReply, Value,
    WakePayload,
};
use anyhow::bail;
use pa_environments::{StaleView, ViewGuard};
use serde_json::json;
use tokio_util::sync::CancellationToken;

impl EnvironmentHost {
    pub async fn seat_call(
        self: &Arc<Self>,
        seat: &SeatId,
        token: &str,
        call: SeatCall,
    ) -> Result<SeatReply> {
        self.check_seat(seat, token)?;
        self.call(seat, call).await
    }

    pub async fn call(self: &Arc<Self>, seat: &SeatId, call: SeatCall) -> Result<SeatReply> {
        if let SeatCall::CancelPython { operation_id } = &call {
            let id = scoped_id(seat, operation_id)?;
            if let Some((active_id, signal)) = self.python_cancellations.lock().await.get(seat) {
                if active_id == &id {
                    signal.cancel();
                }
            }
            return Ok(SeatReply {
                result: json!({}),
                deliveries: Vec::new(),
                view: None,
                view_version: None,
                stale_view: false,
            });
        }
        // Completion reports must remain available while stop waits for workers
        // to settle; they update counters, not environment state.
        let gate = if matches!(
            &call,
            SeatCall::TurnSettled { .. } | SeatCall::TurnFailed { .. }
        ) {
            None
        } else {
            Some(self.gate.lock().await)
        };
        // A suspended seat's own calls cannot change the run until the builder
        // resumes it; reads stay available.
        if matches!(
            call,
            SeatCall::Invoke { .. }
                | SeatCall::Say { .. }
                | SeatCall::Python { .. }
                | SeatCall::Message { .. }
        ) && self.state.lock().await.suspended.contains_key(seat)
        {
            bail!("{seat} is suspended by the run's builder and cannot act until resumed");
        }
        let feature = match &call {
            SeatCall::Say { .. } => Some("environment_say"),
            SeatCall::Message { .. } => Some("environment_direct_message"),
            SeatCall::Observe
            | SeatCall::AgentView
            | SeatCall::Invoke { .. }
            | SeatCall::Python { .. }
            | SeatCall::CancelPython { .. }
            | SeatCall::TurnStarted { .. }
            | SeatCall::TurnSettled { .. }
            | SeatCall::TurnFailed { .. } => None,
        };
        // A stateless seat is handed a fresh view with each action it takes and when
        // each turn starts, since a wake may have waited behind its previous turn.
        let refreshes_view = matches!(
            call,
            SeatCall::Invoke { .. } | SeatCall::Say { .. } | SeatCall::TurnStarted { .. }
        );
        let acted = if matches!(call, SeatCall::Invoke { .. } | SeatCall::Say { .. }) {
            Acted::Operation
        } else {
            Acted::Other
        };
        let stateless = self.instance.definition.manifest.agent_context == AgentContext::Stateless;
        let mut view_version = None;
        let result = match call {
            SeatCall::Observe => {
                let (version, observation) = self.instance.observe_versioned(seat).await?;
                view_version = Some(version);
                observation
            }
            SeatCall::AgentView => {
                let (version, view) = self.instance.agent_view_versioned(seat).await?;
                view_version = Some(version);
                view
            }
            SeatCall::TurnStarted { through_seq } => {
                let checkpoint = self.instance.checkpoint().await;
                let revision = checkpoint
                    .progress
                    .as_ref()
                    .map(|progress| progress.revision);
                let mut state = self.state.lock().await;
                if state.phase != RunPhase::Running {
                    bail!("environment run is not running");
                }
                // A wake that raced a suspension never starts a model turn.
                if state.suspended.contains_key(seat)
                    || state
                        .settled
                        .get(seat)
                        .is_some_and(|seq| *seq >= through_seq)
                {
                    return Ok(SeatReply {
                        result: json!({"settled":true}),
                        deliveries: Vec::new(),
                        view: None,
                        view_version: None,
                        stale_view: false,
                    });
                }
                // The environment decides what a seat is asked to do; the host only
                // revalidates it. An actionable task the environment no longer awaits
                // this seat for is retired here, so the model never reads "your turn"
                // beside a state in which it has none. Messages and ordinary notices
                // are never retired, and the seat's current task is never withheld:
                // a newer one is delivered by its own wake.
                let mut wake = None;
                if let Some(pending) = self
                    .seat_inbox(seat)?
                    .pending
                    .filter(|pending| pending.through_seq == through_seq)
                {
                    self.table.materialize(&checkpoint)?;
                    let awaited = checkpoint
                        .progress
                        .as_ref()
                        .is_none_or(|progress| progress.waiting_for.contains(seat));
                    let current = checkpoint.progress_sequence;
                    let superseded = self.table.since(Some(seat), 0)?.iter().any(|delivery| {
                        delivery.actionable && delivery.op_seq.unwrap_or(u64::MAX) >= current
                    });
                    let (kept, retired): (Vec<_>, Vec<_>) =
                        pending.deliveries.iter().cloned().partition(|delivery| {
                            !delivery.actionable
                                || (awaited
                                    && (delivery.op_seq.unwrap_or(u64::MAX) >= current
                                        || !superseded))
                        });
                    if !retired.is_empty() {
                        if kept.iter().any(|delivery| delivery.wake) {
                            wake = Some(WakePayload {
                                deliveries: kept,
                                ..pending
                            });
                        } else {
                            state.settled.insert(seat.clone(), through_seq);
                            self.save_state(&state)?;
                            crate::environment_telemetry::track(
                                self.telemetry.as_ref(),
                                "environment_obsolete_wake",
                                "completed",
                                None,
                            );
                            return Ok(SeatReply {
                                result: json!({"settled":true}),
                                deliveries: Vec::new(),
                                view: None,
                                view_version: None,
                                stale_view: false,
                            });
                        }
                    }
                }
                if state.turn_sequences.get(seat).copied().unwrap_or_default() < through_seq {
                    state.turn_sequences.insert(seat.clone(), through_seq);
                    state.direct_messages.insert(seat.clone(), 0);
                }
                state.busy.insert(seat.clone(), crate::util::now_ms());
                state.turn_acted.remove(seat);
                match revision {
                    Some(revision) => state.turn_revisions.insert(seat.clone(), revision),
                    None => state.turn_revisions.remove(seat),
                };
                self.save_state(&state)?;
                (self.view)(json!({"type":"environment_seat_state","run":*state}))?;
                // A trimmed wake travels back so the seat renders what remains of it.
                wake.map_or_else(|| json!({}), |wake| json!({"wake":wake}))
            }
            SeatCall::TurnSettled { through_seq } => {
                let checkpoint = self.instance.checkpoint().await;
                let mut state = self.state.lock().await;
                if state.settled.get(seat).is_none_or(|seq| *seq < through_seq) {
                    let revision = checkpoint
                        .progress
                        .as_ref()
                        .map(|progress| progress.revision);
                    if revision == state.revision {
                        state.no_progress_turns += 1;
                    } else {
                        state.no_progress_turns = 0;
                        state.revision = revision;
                    }
                    state.settled.insert(seat.clone(), through_seq);
                    self.retry_idle_turn(&mut state, &checkpoint, seat, through_seq)?;
                }
                state.timeout_aborts.remove(seat);
                state.busy.remove(seat);
                self.save_state(&state)?;
                (self.view)(json!({"type":"environment_seat_state","run":*state}))?;
                self.changed.notify_one();
                json!({})
            }
            SeatCall::TurnFailed { through_seq: _ } => {
                let mut state = self.state.lock().await;
                state.busy.remove(seat);
                state.turn_revisions.remove(seat);
                // A turn the host aborted on timeout keeps running: dispatch
                // replays its retained wake.
                if state.timeout_aborts.remove(seat).is_none() && state.phase == RunPhase::Running {
                    state.phase = RunPhase::Paused;
                    state.reason = Some(format!(
                        "model turn failed for {seat}; pending wake retained"
                    ));
                }
                self.save_state(&state)?;
                (self.view)(json!({"type":"environment_seat_state","run":*state}))?;
                self.changed.notify_one();
                json!({})
            }
            SeatCall::Invoke {
                action,
                arguments,
                operation_id,
                view_version: bound,
            } => {
                let operation_id = scoped_id(seat, &operation_id)?;
                self.require_mutation(&operation_id).await?;
                let result = match self
                    .instance
                    .invoke_guarded(
                        seat,
                        &action,
                        arguments,
                        &operation_id,
                        CancellationToken::new(),
                        ViewGuard::from(bound),
                    )
                    .await
                {
                    Ok(result) => result,
                    Err(error) if error.is::<StaleView>() => return self.stale_reply(seat).await,
                    Err(error) => return Err(error),
                };
                let mut state = self.state.lock().await;
                let now = crate::util::now_ms();
                if let Some(started) = state.busy.get(seat).copied() {
                    // The first action of a turn measures how long the seat took to act.
                    if state.turn_acted.insert(seat.clone()) {
                        let elapsed = now.saturating_sub(started);
                        let timing = state.time_to_action.entry(seat.clone()).or_default();
                        timing.count += 1;
                        timing.total_ms += elapsed;
                        timing.max_ms = timing.max_ms.max(elapsed);
                    }
                    // Acting restarts the deadline clock, and the seat has advanced the
                    // run, so its next wake is a fresh turn rather than a retry.
                    state.busy.insert(seat.clone(), now);
                    state.idle_retries.remove(seat);
                    self.save_state(&state)?;
                }
                drop(state);
                result
            }
            SeatCall::Say {
                text,
                operation_id,
                view_version: bound,
            } => {
                let operation_id = scoped_id(seat, &operation_id)?;
                self.require_mutation(&operation_id).await?;
                bounded_text(&text)?;
                match self
                    .instance
                    .input_seat_guarded(seat, &text, &operation_id, ViewGuard::from(bound))
                    .await
                {
                    Ok(result) => result,
                    Err(error) if error.is::<StaleView>() => return self.stale_reply(seat).await,
                    Err(error) => return Err(error),
                }
            }
            SeatCall::Python {
                code,
                operation_id,
                view_version: bound,
            } => {
                let operation_id = scoped_id(seat, &operation_id)?;
                self.require_mutation(&operation_id).await?;
                let signal = CancellationToken::new();
                self.python_cancellations
                    .lock()
                    .await
                    .insert(seat.clone(), (operation_id.clone(), signal.clone()));
                drop(gate);
                // Bridge admission stays in-process and retains the host weakly;
                // cells cannot keep a closed host alive or bypass its mutation gate.
                let host = Arc::downgrade(self);
                let bound_seat = seat.clone();
                // The cell is bound to the view that wrote it. It never rebinds: once
                // any operation commits, including this cell's own, further
                // `env.invoke` calls are refused and need a fresh model request.
                let guard = ViewGuard::from(bound);
                let bridge: pa_environments::HostCallHandler = Arc::new(move |request| {
                    let host = host.clone();
                    let seat = bound_seat.clone();
                    Box::pin(async move {
                        let host = host.upgrade().context("environment host closed")?;
                        match request.method.as_str() {
                            "environment.observe" => host.instance.observe(&seat).await,
                            "environment.invoke" => {
                                let _gate = host.gate.lock().await;
                                let id = scoped_id(&seat, &uuid::Uuid::new_v4().to_string())?;
                                host.require_mutation(&id).await?;
                                host.instance
                                    .invoke_guarded(
                                        &seat,
                                        request.params["action"]
                                            .as_str()
                                            .context("action required")?,
                                        request.params["arguments"].clone(),
                                        &id,
                                        CancellationToken::new(),
                                        guard,
                                    )
                                    .await
                            }
                            _ => bail!("unsupported Python host call"),
                        }
                    })
                });
                let result = self
                    .instance
                    .execute_python_seat_with_host(seat, &code, &operation_id, signal, bridge)
                    .await;
                self.python_cancellations.lock().await.remove(seat);
                result?
            }
            SeatCall::CancelPython { .. } => unreachable!("handled before mutation gate"),
            SeatCall::Message {
                to,
                text,
                message_id,
            } => {
                bounded_text(&text)?;
                if !self.instance.roster().direct_messages {
                    bail!("direct seat messages are disabled");
                }
                if &to == seat
                    || !self
                        .instance
                        .roster()
                        .seats
                        .iter()
                        .any(|entry| entry.id == to)
                {
                    bail!("message must target another existing seat");
                }
                if message_id.is_empty() || message_id.len() > 128 {
                    bail!("invalid message ID");
                }
                let event_id = format!("message-{}", scoped_id(seat, &message_id)?);
                let existing = self
                    .table
                    .all_since(0)?
                    .iter()
                    .any(|row| row.event_id == event_id);
                if !existing {
                    self.require_running().await?;
                }
                let mut state = self.state.lock().await;
                if !existing && state.direct_messages.get(seat).copied().unwrap_or_default() >= 32 {
                    bail!("direct-message turn limit reached");
                }
                let delivery = self.table.append(Delivery {
                    seq: 0,
                    event_id,
                    op_seq: None,
                    kind: DeliveryKind::DirectMessage,
                    from: Some(seat.clone()),
                    to: NoticeTarget::Seats(vec![to]),
                    message: Some(text),
                    wake: true,
                    actionable: false,
                    timestamp: 0,
                })?;
                if !existing {
                    *state.direct_messages.entry(seat.clone()).or_default() += 1;
                    self.save_state(&state)?;
                }
                self.changed.notify_one();
                json!({"seq":delivery.seq})
            }
        };
        let checkpoint = self.instance.checkpoint().await;
        self.table.materialize(&checkpoint)?;
        self.changed.notify_one();
        if let Some(feature) = feature {
            crate::environment_telemetry::track(
                self.telemetry.as_ref(),
                feature,
                "completed",
                None,
            );
        }
        // In stateless context a seat cannot re-observe, so an action returns the
        // refreshed view and the replayed table events are redundant.
        let view = if stateless && refreshes_view {
            let (version, view) = self.instance.agent_view_versioned(seat).await?;
            view_version = Some(version);
            Some(view)
        } else {
            None
        };
        // A conversation seat's reply lists the deliveries addressed to it through
        // this commit, so its next action may build on the version just committed.
        if !stateless && matches!(acted, Acted::Operation) {
            view_version = Some(checkpoint.next_sequence);
        }
        Ok(SeatReply {
            result,
            deliveries: if stateless {
                Vec::new()
            } else {
                self.table.since(
                    Some(seat),
                    pa_environments::DeliveryCursor::load(self.instance.path(), seat)?.through_seq,
                )?
            },
            view,
            view_version,
            stale_view: false,
        })
    }

    /// The reply to a mutation refused for a stale view: nothing ran, and the seat
    /// gets its current state, with that state's version, to reconsider. The refused
    /// call is never replayed; the caller must make a new one.
    async fn stale_reply(&self, seat: &SeatId) -> Result<SeatReply> {
        let (version, view) = match self.instance.definition.manifest.agent_context {
            AgentContext::Stateless if self.human().as_ref() != Some(seat) => {
                self.instance.agent_view_versioned(seat).await?
            }
            AgentContext::Stateless | AgentContext::Conversation => {
                self.instance.observe_versioned(seat).await?
            }
        };
        crate::environment_telemetry::track(
            self.telemetry.as_ref(),
            "environment_stale_view",
            "failed",
            None,
        );
        Ok(SeatReply {
            result: json!({
                "rejected": "stale_view",
                "message": "Not executed: the environment changed after the state you acted on. Reconsider against the current state and act again only if it is still right.",
            }),
            deliveries: Vec::new(),
            view: Some(view),
            view_version: Some(version),
            stale_view: true,
        })
    }

    pub(super) async fn require_mutation(&self, id: &str) -> Result<()> {
        let checkpoint = self.instance.checkpoint().await;
        if checkpoint.receipts.contains_key(id) {
            return Ok(());
        }
        self.require_running().await?;
        self.within_operation_limit(&checkpoint)
    }

    pub(super) fn within_operation_limit(
        &self,
        checkpoint: &pa_types::environments::InstanceCheckpoint,
    ) -> Result<()> {
        if checkpoint.receipts.len().saturating_sub(1) as u64
            >= self.instance.roster().limits.max_operations
        {
            anyhow::bail!("operation limit reached");
        }
        Ok(())
    }

    async fn require_running(&self) -> Result<()> {
        let state = self.state.lock().await;
        if state.phase != RunPhase::Running {
            bail!(
                "environment run is {:?}: {}",
                state.phase,
                state.reason.as_deref().unwrap_or_default()
            );
        }
        if self.instance.checkpoint().await.finished.is_some() {
            bail!("environment has finished");
        }
        Ok(())
    }

    pub async fn client_operation(
        self: &Arc<Self>,
        operation: &str,
        parameters: &Value,
    ) -> Result<Value> {
        match operation {
            "seat_call" => {
                let seat: SeatId = serde_json::from_value(parameters["seat"].clone())?;
                let call: SeatCall = serde_json::from_value(parameters["call"].clone())?;
                Ok(serde_json::to_value(
                    self.seat_call(
                        &seat,
                        parameters["token"]
                            .as_str()
                            .context("seat capability required")?,
                        call,
                    )
                    .await?,
                )?)
            }
            "status" | "observe" => self.status().await,
            "view" => self.live_view(parameters).await,
            "act" | "say" | "msg" => {
                let seat = self
                    .human()
                    .context("spectator cannot act or send messages")?;
                let id = parameters["operation_id"]
                    .as_str()
                    .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
                let call = match operation {
                    "act" => SeatCall::Invoke {
                        action: parameters["action"]
                            .as_str()
                            .context("action required")?
                            .into(),
                        arguments: parameters["arguments"].clone(),
                        operation_id: id,
                        view_version: parameters["view_version"].as_u64(),
                    },
                    "say" => SeatCall::Say {
                        text: parameters["text"].as_str().context("text required")?.into(),
                        operation_id: id,
                        view_version: parameters["view_version"].as_u64(),
                    },
                    "msg" => SeatCall::Message {
                        to: serde_json::from_value(parameters["to"].clone())?,
                        text: parameters["text"].as_str().context("text required")?.into(),
                        message_id: id,
                    },
                    _ => unreachable!("matched human operations"),
                };
                let reply = match self.call(&seat, call).await {
                    Ok(reply) => reply,
                    Err(error) => {
                        // A rejected action still committed a receipt, so the version the
                        // form was bound to is spent. Show the human the current state, and
                        // its version, so they can submit again.
                        if let Ok(status) = self.status().await {
                            (self.view)(
                                json!({"type":"environment_observation","observation":status["observation"],"view_version":status["view_version"]}),
                            )?;
                        }
                        return Err(error);
                    }
                };
                if reply.stale_view {
                    // The form was bound to the state it displayed. Show the refreshed
                    // state and make the human submit again; arguments are never
                    // rebound to it here.
                    (self.view)(
                        json!({"type":"environment_observation","observation":reply.view,"view_version":reply.view_version}),
                    )?;
                    bail!("The environment changed after the state you were viewing, so nothing was executed. The refreshed state is shown; review it and submit again.");
                }
                if operation == "act" {
                    (self.view)(
                        json!({"type":"environment_action","action":{"seat":seat,"name":parameters["action"],"result":reply.result}}),
                    )?;
                }
                let (view_version, observation) = self.instance.observe_versioned(&seat).await?;
                Ok(
                    json!({"result":reply.result,"observation":observation,"view_version":view_version}),
                )
            }
            "pause" | "resume_run" | "stop" => self.run_control(operation, "human").await,
            // Interventions belong to the run's builder, so even the human at the
            // builder prompt goes through the builder token; a player at the table
            // can never act for another seat.
            "builder_act_for_seat" | "builder_suspend_seat" | "builder_resume_seat" => {
                authenticate(
                    &self.builder_token,
                    parameters["builder_token"].as_str().unwrap_or_default(),
                )?;
                let requester = if parameters["requester"] == "human" {
                    Requester::Human
                } else {
                    Requester::Builder
                };
                self.intervene(
                    operation.trim_start_matches("builder_"),
                    parameters,
                    requester,
                )
                .await
            }
            "builder_pause" | "builder_resume_run" | "builder_stop" => {
                authenticate(
                    &self.builder_token,
                    parameters["builder_token"].as_str().unwrap_or_default(),
                )?;
                self.run_control(operation.trim_start_matches("builder_"), "builder")
                    .await
            }
            "snapshot" | "receipts" | "table_log" | "seat_transcript" | "run_state"
            | "privileged_observe" => {
                authenticate(
                    &self.builder_token,
                    parameters["builder_token"].as_str().unwrap_or_default(),
                )?;
                match operation {
                    "snapshot" => Ok(self.instance.checkpoint().await.snapshot),
                    "receipts" => Ok(json!(self.instance.checkpoint().await.receipts)),
                    "table_log" => Ok(json!(self
                        .table
                        .all_since(parameters["since_seq"].as_u64().unwrap_or_default())?)),
                    "privileged_observe" => {
                        self.instance
                            .observe(&serde_json::from_value(parameters["seat"].clone())?)
                            .await
                    }
                    "run_state" => Ok(json!(*self.state.lock().await)),
                    "seat_transcript" => {
                        let seat: SeatId = serde_json::from_value(parameters["seat"].clone())?;
                        let entry = self
                            .instance
                            .roster()
                            .seats
                            .into_iter()
                            .find(|entry| entry.id == seat && entry.occupant == OccupantKind::Agent)
                            .context("unknown agent seat")?;
                        let path = if self.instance.definition.manifest.protocol_version == 1 {
                            self.instance.path().join("conversation.jsonl")
                        } else {
                            self.instance
                                .path()
                                .join("seats")
                                .join(entry.id.as_str())
                                .join("conversation.jsonl")
                        };
                        Ok(
                            json!({"transcript":std::fs::read_to_string(path)?.lines().skip(parameters["since"].as_u64().unwrap_or_default() as usize).collect::<Vec<_>>().join("\n")}),
                        )
                    }
                    _ => unreachable!("matched privileged reads"),
                }
            }
            _ => bail!("unknown environment host operation"),
        }
    }

    /// Routes a seat intervention's wire parameters to its handler.
    async fn intervene(
        self: &Arc<Self>,
        operation: &str,
        parameters: &Value,
        requester: Requester,
    ) -> Result<Value> {
        let seat: SeatId =
            serde_json::from_value(parameters["seat"].clone()).context("seat required")?;
        let reason = parameters["reason"].as_str().unwrap_or_default();
        match operation {
            "act_for_seat" => {
                self.act_for_seat(
                    &seat,
                    parameters["action"].as_str().context("action required")?,
                    parameters
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                    reason,
                    requester,
                )
                .await
            }
            "suspend_seat" => self.suspend_seat(&seat, reason, requester).await,
            "resume_seat" => self.resume_seat(&seat, requester).await,
            _ => bail!("unknown seat intervention"),
        }
    }

    /// Applies a run control for `requester` ("human" at the table, or "builder"
    /// through the token-authenticated builder operations).
    async fn run_control(self: &Arc<Self>, operation: &str, requester: &str) -> Result<Value> {
        let _gate = self.gate.lock().await;
        let checkpoint = self.instance.checkpoint().await;
        self.table.materialize(&checkpoint)?;
        let mut state = self.state.lock().await;
        if state.phase.is_terminal()
            || checkpoint.finished.is_some()
            || checkpoint.requires_recovery
        {
            bail!("terminal environment cannot resume");
        }
        if let (true, Some(progress)) = (
            operation == "resume_run" && state.phase == RunPhase::Stalled,
            checkpoint.progress.as_ref(),
        ) {
            for seat_id in progress.waiting_for.iter().filter(|id| {
                !state.suspended.contains_key(*id)
                    && self
                        .instance
                        .roster()
                        .seats
                        .iter()
                        .any(|seat| seat.id == **id && seat.occupant == OccupantKind::Agent)
            }) {
                if self.seat_inbox(seat_id)?.pending.is_none() && !self.has_unseen_wake(seat_id)? {
                    self.wake_seat(
                        seat_id,
                        "resume",
                        "The run was resumed. Observe the current environment, then act only if this is a legal turn for you.",
                    )?;
                }
            }
        }
        state.phase = match operation {
            "pause" => RunPhase::Paused,
            "resume_run" => RunPhase::Running,
            "stop" => RunPhase::Stopped,
            _ => unreachable!("matched controls"),
        };
        state.reason = Some(format!("{requester} requested {operation}"));
        state.paused_automatically = false;
        if operation == "resume_run" {
            // A resumed run that stops again is news to the builder again.
            state.builder_notified = None;
            state.no_progress_turns = 0;
            state.busy.clear();
            state.idle_retries.clear();
            state.timeout_aborts.clear();
        }
        let name = if operation == "resume_run" {
            "resume"
        } else {
            operation
        };
        let choice = if requester == "human" {
            name.to_owned()
        } else {
            format!("{requester}_{name}")
        };
        crate::environment_telemetry::track(
            self.telemetry.as_ref(),
            "environment_run_control",
            "completed",
            Some(&choice),
        );
        self.save_state(&state)?;
        (self.view)(json!({"type":"environment_status","run":*state}))?;
        drop(state);
        if operation != "resume_run" {
            self.abort_seats().await?;
        }
        if operation == "resume_run" {
            let suspended = self.state.lock().await.suspended.clone();
            for seat in self.instance.roster().seats.into_iter().filter(|seat| {
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
                    if let Some(payload) = inbox.pending {
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
        if operation == "stop" {
            self.stop_seats().await?;
        }
        self.changed.notify_one();
        self.status().await
    }

    pub async fn status(&self) -> Result<Value> {
        let human = self.human();
        let (view_version, observation) = match &human {
            Some(seat) => self.instance.observe_versioned(seat).await?,
            None => (
                self.instance.version().await,
                json!({"spectator":true,"public_deliveries":self.table.since(None,0)?}),
            ),
        };
        let mut actions = self.instance.actions.clone();
        // The UI's v1 action selector remains compatible while the wire exposes
        // the actual roster and role. Only actions available to its human are sent.
        if let Some(seat) = &human {
            let role = self
                .instance
                .roster()
                .seats
                .into_iter()
                .find(|entry| &entry.id == seat)
                .context("human seat")?
                .role;
            actions.retain(|action| action.allowed_roles().contains(&role));
            for action in &mut actions {
                action.participants = vec![RoleName::legacy_human()];
            }
        } else {
            actions.clear();
        }
        Ok(
            json!({"instance":self.instance.instance,"definition":self.instance.definition,"actions":actions,"observation":observation,"view_version":view_version,"human_seat":human,"roster":self.instance.roster(),"run":*self.state.lock().await}),
        )
    }
}
/// Whether a seat call committed an engine operation.
enum Acted {
    Operation,
    Other,
}

fn bounded_text(text: &str) -> Result<()> {
    if text.len() > 16 * 1024 {
        anyhow::bail!("message exceeds 16 KiB");
    }
    Ok(())
}

fn scoped_id(seat: &SeatId, id: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    if id.is_empty() || id.len() > 128 {
        anyhow::bail!("invalid operation ID");
    }
    Ok(format!(
        "seat-{:x}",
        Sha256::digest(format!("{seat}\0{id}").as_bytes())
    ))
}
