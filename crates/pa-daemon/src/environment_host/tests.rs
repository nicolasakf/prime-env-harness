use super::*;
use pa_telemetry::{MockSink, TelemetryClient, TelemetryClientConfig};
use pa_types::environments::{EnvironmentLanguage, ExecutionPolicy};
use serde_json::json;

#[tokio::test]
// Exercise controls, limits and emitted telemetry against one authenticated host.
#[allow(clippy::too_many_lines)]
async fn capabilities_direct_messages_controls_and_emitted_telemetry() {
    let root = tempfile::tempdir().unwrap();
    let manager = pa_environments::EnvironmentManager::new(root.path().into(), None).unwrap();
    let draft = manager
        .store()
        .create_draft(
            "host boundaries",
            EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    assert!(
        manager
            .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
            .await
            .unwrap()
            .passed
    );
    manager.save(draft.to_str().unwrap(), "counter").unwrap();
    let definition = manager.store().definition("counter", None).unwrap().0;
    let mut roster = pa_environments::default_roster(&definition.manifest).unwrap();
    roster.limits.max_operations = 1;
    let instance = manager
        .enter_with_roster(
            "counter",
            None,
            ExecutionPolicy::default(),
            json!({}),
            roster,
        )
        .await
        .unwrap();
    let sink = Arc::new(MockSink::new());
    let mut telemetry_config = TelemetryClientConfig::new("test");
    telemetry_config.sinks = vec![sink.clone()];
    let telemetry = TelemetryClient::spawn(telemetry_config).unwrap();
    let host = Arc::new(EnvironmentHost { table: TableLog::open(instance.path()).unwrap(), state: Mutex::new(serde_json::from_value(json!({"phase":"running","reason":null,"operations":0,"wakes":{},"busy":{},"no_progress_turns":0,"revision":null,"started_at":crate::util::now_ms(),"settled":{}})).unwrap()), gate: Mutex::new(()), link: SupervisorLink::new(root.path().join("unused.sock")), builder_token: "b".repeat(64), host_id: "host".into(), telemetry: Some(telemetry.clone()), defaults: json!({}), seats: Mutex::new(BTreeMap::new()), python_cancellations: Mutex::new(BTreeMap::new()), changed: Notify::new(), live: Mutex::new(None), announced_phase: std::sync::Mutex::new(None), shutdown: CancellationToken::new(), view: Arc::new(|_| Ok(())), instance });
    let agent = SeatId::try_from("agent-1".to_owned()).unwrap();
    let human = SeatId::try_from("human-1".to_owned()).unwrap();
    write(
        &host.instance.path().join("seats/agent-1/token.json"),
        &"a".repeat(64),
    )
    .unwrap();
    assert!(host
        .seat_call(&agent, &"b".repeat(64), SeatCall::Observe)
        .await
        .is_err());
    assert!(host
        .client_operation("snapshot", &json!({"builder_token":"wrong"}))
        .await
        .is_err());
    host.seat_call(&agent, &"a".repeat(64), SeatCall::Observe)
        .await
        .unwrap();
    let first = host
        .call(
            &agent,
            SeatCall::Message {
                to: human.clone(),
                text: "recipient secret".into(),
                message_id: "message-0".into(),
            },
        )
        .await
        .unwrap();
    let replay = host
        .call(
            &agent,
            SeatCall::Message {
                to: human.clone(),
                text: "recipient secret".into(),
                message_id: "message-0".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert!(host.table.since(None, 0).unwrap().is_empty());
    assert_eq!(host.table.since(Some(&human), 0).unwrap().len(), 1);
    assert!(host
        .call(
            &agent,
            SeatCall::Message {
                to: human.clone(),
                text: "different content".into(),
                message_id: "message-0".into()
            }
        )
        .await
        .is_err());
    for index in 1..32 {
        host.call(
            &agent,
            SeatCall::Message {
                to: human.clone(),
                text: "text".into(),
                message_id: format!("message-{index}"),
            },
        )
        .await
        .unwrap();
    }
    assert!(host
        .call(
            &agent,
            SeatCall::Message {
                to: human.clone(),
                text: "too many".into(),
                message_id: "message-32".into()
            }
        )
        .await
        .is_err());
    host.client_operation("pause", &json!({})).await.unwrap();
    assert!(host
        .call(
            &agent,
            SeatCall::Invoke {
                action: "increment".into(),
                arguments: json!({"amount":1}),
                operation_id: "paused".into(),
                view_version: None
            }
        )
        .await
        .is_err());
    host.call(&agent, SeatCall::Observe).await.unwrap();
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    let builder_token = json!({"builder_token":"b".repeat(64)});
    assert!(host
        .client_operation("builder_pause", &json!({"builder_token":"wrong"}))
        .await
        .is_err());
    assert!(host
        .client_operation("builder_resume_run", &json!({"builder_token":"wrong"}))
        .await
        .is_err());
    assert_eq!(host.state.lock().await.phase, RunPhase::Running);
    host.client_operation("builder_pause", &builder_token)
        .await
        .unwrap();
    {
        let state = host.state.lock().await;
        assert_eq!(
            (state.phase.clone(), state.reason.as_deref()),
            (RunPhase::Paused, Some("builder requested pause"))
        );
    }
    host.client_operation("builder_resume_run", &builder_token)
        .await
        .unwrap();
    assert_eq!(host.state.lock().await.phase, RunPhase::Running);
    assert_eq!(
        host.state.lock().await.reason.as_deref(),
        Some("builder requested resume_run")
    );
    host.state.lock().await.no_progress_turns = 4;
    host.drain().await.unwrap();
    assert_eq!(host.state.lock().await.phase, RunPhase::Stalled);
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    host.call(
        &agent,
        SeatCall::Say {
            text: "environment input".into(),
            operation_id: "say".into(),
            view_version: None,
        },
    )
    .await
    .unwrap();
    assert!(host
        .call(
            &agent,
            SeatCall::Invoke {
                action: "increment".into(),
                arguments: json!({"amount":1}),
                operation_id: "over-limit".into(),
                view_version: None
            }
        )
        .await
        .is_err());
    host.drain().await.unwrap();
    assert_eq!(host.state.lock().await.phase, RunPhase::Paused);
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    host.state.lock().await.no_progress_turns = 4;
    host.drain().await.unwrap();
    // The operation bound takes precedence over the no-progress bound.
    assert_eq!(
        host.state.lock().await.reason.as_deref(),
        Some("operation limit")
    );
    host.client_operation("stop", &json!({})).await.unwrap();
    assert!(host
        .client_operation("resume_run", &json!({}))
        .await
        .is_err());
    telemetry.flush().await.unwrap();
    let events = sink.events();
    assert!(events
        .iter()
        .any(|event| event.properties.get("feature_name")
            == Some(&json!("environment_direct_message"))));
    assert!(events
        .iter()
        .any(|event| event.properties.get("feature_name") == Some(&json!("environment_say"))));
    assert!(
        events
            .iter()
            .any(|event| event.properties.get("feature_name")
                == Some(&json!("environment_run_limit")))
    );
    assert!(events
        .iter()
        .any(|event| event.properties.get("configuration_choice") == Some(&json!("stop"))));
    assert!(
        events
            .iter()
            .any(|event| event.properties.get("configuration_choice")
                == Some(&json!("builder_pause")))
    );
    for event in events {
        assert_ne!(
            event.properties.get("feature_name"),
            Some(&json!("unknown"))
        );
        assert!(!serde_json::to_string(&event)
            .unwrap()
            .contains("recipient secret"));
    }
    telemetry.shutdown().await.unwrap();
    host.instance.leave().await.unwrap();
}

/// Who answers an awaited seat's missed turn in [`waiting_host`].
#[derive(Clone, Copy)]
pub(super) enum MissedTurns {
    Host,
    /// The manifest declares `missed_turns`: the first miss warns the seat, the
    /// second is left to the host, and a later one ends the seat's turn once a
    /// `tick` set `resolve`. A suspended seat's turn is ended the same way, at
    /// once, when `resolve` is set.
    Environment,
}

/// A host whose `tick` action (human-only) reports progress waiting on the seat
/// named by `waiting` (default `agent-1`), or finishes when `finished` is set.
/// Its `play` action (agent-only) ends the wait.
pub(super) async fn waiting_host(
    root: &std::path::Path,
    phase: RunPhase,
    missed_turns: MissedTurns,
) -> Arc<EnvironmentHost> {
    let manager = pa_environments::EnvironmentManager::new(root.into(), None).unwrap();
    let draft = manager
        .store()
        .create_draft(
            "resume wake",
            EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    let environment = r#"from sdk import notify, progress, finish
class E:
    def initialize(self, params):
        self.revision = 0
        self.resolve = False
        return {}
    def missed_turn(self, participant, report):
        if report["cause"] == "suspended":
            if self.resolve:
                self.revision += 1
                progress(self.revision, waiting_for=[])
            return {}
        if report["misses"] == 1:
            notify([participant], "Missed once; act now.", wake=True)
        elif report["misses"] > report["limit"] and self.resolve:
            self.revision += 1
            progress(self.revision, waiting_for=[])
        return {}
    def describe_actions(self): return [{"name":"tick","description":"tick","input_schema":{"type":"object"},"output_schema":{"type":"object"},"roles":["human"]},{"name":"play","description":"play","input_schema":{"type":"object"},"output_schema":{"type":"object"},"roles":["agent"]}]
    def observe(self, participant): return {"value":0}
    def handle_input(self, participant, text): return {"observation":{}}
    def invoke(self, participant, action, arguments):
        if action == "play":
            self.revision += 1
            progress(self.revision, waiting_for=[])
            return {"played": participant}
        if arguments.get("finished"):
            finish("done")
        self.resolve = arguments.get("resolve", False)
        self.revision += 1
        progress(self.revision, waiting_for=[arguments.get("waiting", "agent-1")])
        return {}
    def snapshot(self): return {"revision":self.revision,"resolve":self.resolve}
    def restore(self, snapshot):
        self.revision = snapshot["revision"]
        self.resolve = snapshot["resolve"]
        return {}
    def dispose(self): return {}
environment=E()
"#;
    std::fs::write(draft.join("environment.py"), environment).unwrap();
    std::fs::write(
        draft.join("checks.py"),
        "from environment import E\nenvironment = E()\nenvironment.initialize({})\nassert environment.snapshot() == {\"revision\": 0, \"resolve\": False}\n",
    ).unwrap();
    if let MissedTurns::Environment = missed_turns {
        let path = draft.join("environment.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["protocol_version"] = json!(2);
        manifest["missed_turns"] = json!(true);
        std::fs::write(&path, manifest.to_string()).unwrap();
    }
    let validation = manager
        .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
        .await
        .unwrap();
    assert!(validation.passed, "{validation:?}");
    manager
        .save(draft.to_str().unwrap(), "resume-wake")
        .unwrap();
    let definition = manager.store().definition("resume-wake", None).unwrap().0;
    let instance = manager
        .enter_with_roster(
            "resume-wake",
            None,
            ExecutionPolicy::default(),
            json!({}),
            pa_environments::default_roster(&definition.manifest).unwrap(),
        )
        .await
        .unwrap();
    Arc::new(EnvironmentHost {
        table: TableLog::open(instance.path()).unwrap(),
        state: Mutex::new(serde_json::from_value(json!({"phase":phase,"reason":null,"operations":0,"wakes":{},"busy":{},"no_progress_turns":0,"revision":null,"started_at":crate::util::now_ms(),"settled":{}})).unwrap()),
        gate: Mutex::new(()), link: SupervisorLink::new(root.join("unused.sock")),
        builder_token: "b".repeat(64), host_id: "host".into(), telemetry: None,
        defaults: json!({}), seats: Mutex::new(BTreeMap::new()),
        python_cancellations: Mutex::new(BTreeMap::new()), changed: Notify::new(), live: Mutex::new(None), announced_phase: std::sync::Mutex::new(None),
        shutdown: CancellationToken::new(), view: Arc::new(|_| Ok(())), instance,
    })
}

#[tokio::test]
async fn stalled_resume_wakes_waiting_agent_only_when_no_wake_is_unseen() {
    let root = tempfile::tempdir().unwrap();
    let host = waiting_host(root.path(), RunPhase::Stalled, MissedTurns::Host).await;
    let agent = SeatId::try_from("agent-1".to_owned()).unwrap();
    let human = SeatId::try_from("human-1".to_owned()).unwrap();
    host.instance
        .invoke(&human, "tick", json!({}), "tick", CancellationToken::new())
        .await
        .unwrap();
    let checkpoint = host.instance.checkpoint().await;
    host.table.materialize(&checkpoint).unwrap();
    let old_snapshot = checkpoint.snapshot.clone();
    host.table
        .append(Delivery {
            seq: 0,
            event_id: "old-consumed".into(),
            op_seq: None,
            kind: DeliveryKind::Notice,
            from: None,
            to: NoticeTarget::Seats(vec![agent.clone()]),
            message: Some("old wake".into()),
            wake: true,
            actionable: false,
            timestamp: 0,
        })
        .unwrap();
    let last = host.table.all_since(0).unwrap().last().unwrap().seq;
    pa_environments::DeliveryCursor::default()
        .acknowledge(host.instance.path(), &agent, last)
        .unwrap();
    write(
        &host.instance.path().join("seats/agent-1/inbox.json"),
        &crate::environment_seat::SeatInbox {
            admitted: last,
            settled: last,
            pending: None,
        },
    )
    .unwrap();
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    let rows = host.table.all_since(0).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].to, NoticeTarget::Seats(vec![agent.clone()]));
    assert!(rows[1].wake);
    assert!(rows[1].seq > last);
    assert_eq!(host.instance.checkpoint().await.snapshot, old_snapshot);
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    assert_eq!(host.table.all_since(0).unwrap().len(), 2);
    host.state.lock().await.phase = RunPhase::Stalled;
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    assert_eq!(host.table.all_since(0).unwrap().len(), 2);
    pa_environments::DeliveryCursor::load(host.instance.path(), &agent)
        .unwrap()
        .acknowledge(host.instance.path(), &agent, rows[1].seq)
        .unwrap();
    host.client_operation("pause", &json!({})).await.unwrap();
    host.table
        .append(Delivery {
            seq: 0,
            event_id: "retained-unseen".into(),
            op_seq: None,
            kind: DeliveryKind::Notice,
            from: None,
            to: NoticeTarget::Seats(vec![agent.clone()]),
            message: Some("retained".into()),
            wake: true,
            actionable: false,
            timestamp: 0,
        })
        .unwrap();
    host.state.lock().await.phase = RunPhase::Stalled;
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    assert_eq!(host.table.all_since(0).unwrap().len(), 3);
    let retained = host.table.all_since(0).unwrap().last().unwrap().seq;
    pa_environments::DeliveryCursor::load(host.instance.path(), &agent)
        .unwrap()
        .acknowledge(host.instance.path(), &agent, retained)
        .unwrap();
    host.instance
        .invoke(
            &human,
            "tick",
            json!({"waiting":"human-1"}),
            "human-wait",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    host.state.lock().await.phase = RunPhase::Stalled;
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    assert_eq!(host.table.all_since(0).unwrap().len(), 3);
    host.instance
        .invoke(
            &human,
            "tick",
            json!({"finished":true}),
            "finish",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    host.state.lock().await.phase = RunPhase::Stalled;
    let before_resume = host.instance.checkpoint().await;
    assert!(host
        .client_operation("resume_run", &json!({}))
        .await
        .is_err());
    assert_eq!(host.state.lock().await.phase, RunPhase::Stalled);
    assert_eq!(
        host.instance.checkpoint().await.snapshot,
        before_resume.snapshot
    );
    host.instance.leave().await.unwrap();
}

/// Runs one agent turn that takes no action: dispatch has delivered every wake,
/// and the turn settles without invoking anything.
async fn idle_turn(host: &Arc<EnvironmentHost>, agent: &SeatId, through_seq: u64) {
    if let Some(last) = host.table.all_since(0).unwrap().last() {
        let mut cursor =
            pa_environments::DeliveryCursor::load(host.instance.path(), agent).unwrap();
        cursor
            .acknowledge(
                host.instance.path(),
                agent,
                last.seq.max(cursor.through_seq),
            )
            .unwrap();
    }
    host.call(agent, SeatCall::TurnStarted { through_seq })
        .await
        .unwrap();
    host.call(agent, SeatCall::TurnSettled { through_seq })
        .await
        .unwrap();
}

fn retry_wakes(host: &EnvironmentHost) -> Vec<(NoticeTarget, DeliveryKind, bool)> {
    host.table
        .all_since(0)
        .unwrap()
        .into_iter()
        .filter(|row| row.event_id.starts_with("idle-retry-"))
        .map(|row| (row.to, row.kind, row.wake))
        .collect()
}

/// In a simultaneous round other seats advance the revision without waking the
/// seat still owed a submission. Its idle turn must still be re-prompted, or the
/// round waits on it forever.
#[tokio::test]
async fn awaited_agent_is_reprompted_when_others_advanced_the_round_during_its_idle_turn() {
    let root = tempfile::tempdir().unwrap();
    let host = waiting_host(root.path(), RunPhase::Running, MissedTurns::Host).await;
    let agent = SeatId::try_from("agent-1".to_owned()).unwrap();
    let human = SeatId::try_from("human-1".to_owned()).unwrap();
    for id in ["round-opens", "peer-submits"] {
        if id == "peer-submits" {
            if let Some(last) = host.table.all_since(0).unwrap().last() {
                pa_environments::DeliveryCursor::load(host.instance.path(), &agent)
                    .unwrap()
                    .acknowledge(host.instance.path(), &agent, last.seq)
                    .unwrap();
            }
            host.call(&agent, SeatCall::TurnStarted { through_seq: 1 })
                .await
                .unwrap();
        }
        host.instance
            .invoke(
                &human,
                "tick",
                json!({"waiting":"agent-1"}),
                id,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        host.table
            .materialize(&host.instance.checkpoint().await)
            .unwrap();
    }
    host.call(&agent, SeatCall::TurnSettled { through_seq: 1 })
        .await
        .unwrap();
    assert_eq!(
        retry_wakes(&host),
        vec![(
            NoticeTarget::Seats(vec![agent.clone()]),
            DeliveryKind::Status,
            true
        )]
    );
    assert_eq!(host.state.lock().await.idle_retries.get(&agent), Some(&1));
    host.instance.leave().await.unwrap();
}

#[tokio::test]
async fn awaited_agent_is_reprompted_after_idle_turns_until_the_budget_stalls_the_run() {
    let root = tempfile::tempdir().unwrap();
    let host = waiting_host(root.path(), RunPhase::Running, MissedTurns::Host).await;
    let agent = SeatId::try_from("agent-1".to_owned()).unwrap();
    let human = SeatId::try_from("human-1".to_owned()).unwrap();
    let retry = (
        NoticeTarget::Seats(vec![agent.clone()]),
        DeliveryKind::Status,
        true,
    );
    let tick = |waiting: &'static str, id: &'static str| {
        let host = Arc::clone(&host);
        let human = human.clone();
        async move {
            host.instance
                .invoke(
                    &human,
                    "tick",
                    json!({"waiting":waiting}),
                    id,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            host.table
                .materialize(&host.instance.checkpoint().await)
                .unwrap();
        }
    };
    tick("agent-1", "await-agent").await;
    idle_turn(&host, &agent, 1).await;
    assert_eq!(retry_wakes(&host), vec![retry.clone()]);

    // Progress during the turn refunds the budget; nothing re-prompts.
    let mut cursor = pa_environments::DeliveryCursor::load(host.instance.path(), &agent).unwrap();
    let last = host.table.all_since(0).unwrap().last().unwrap().seq;
    cursor
        .acknowledge(host.instance.path(), &agent, last)
        .unwrap();
    host.call(&agent, SeatCall::TurnStarted { through_seq: 2 })
        .await
        .unwrap();
    tick("human-1", "await-human").await;
    host.call(&agent, SeatCall::TurnSettled { through_seq: 2 })
        .await
        .unwrap();
    assert!(host.state.lock().await.idle_retries.is_empty());

    // A stale wake for a seat the environment is not awaiting is a legal no-op.
    idle_turn(&host, &agent, 3).await;
    assert_eq!(retry_wakes(&host).len(), 1);

    // A turn the host aborted on timeout replays instead of pausing the run;
    // any other failed turn still pauses it.
    host.call(&agent, SeatCall::TurnStarted { through_seq: 4 })
        .await
        .unwrap();
    host.state
        .lock()
        .await
        .timeout_aborts
        .insert(agent.clone(), crate::util::now_ms());
    host.call(&agent, SeatCall::TurnFailed { through_seq: 4 })
        .await
        .unwrap();
    assert_eq!(host.state.lock().await.phase, RunPhase::Running);
    assert!(host.state.lock().await.timeout_aborts.is_empty());
    host.call(&agent, SeatCall::TurnStarted { through_seq: 4 })
        .await
        .unwrap();
    host.call(&agent, SeatCall::TurnFailed { through_seq: 4 })
        .await
        .unwrap();
    assert_eq!(host.state.lock().await.phase, RunPhase::Paused);
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();

    tick("agent-1", "await-agent-again").await;
    idle_turn(&host, &agent, 5).await;
    idle_turn(&host, &agent, 6).await;
    assert_eq!(
        retry_wakes(&host),
        vec![retry.clone(), retry.clone(), retry]
    );
    assert_eq!(host.state.lock().await.phase, RunPhase::Running);
    idle_turn(&host, &agent, 7).await;
    {
        let state = host.state.lock().await;
        assert_eq!(
            (state.phase.clone(), state.reason.as_deref()),
            (
                RunPhase::Stalled,
                Some("agent-1 made no progress after 2 re-prompts")
            )
        );
    }
    assert_eq!(retry_wakes(&host).len(), 3);
    host.instance.leave().await.unwrap();
}

/// The reports the environment's `missed_turn` received, in commit order.
pub(super) async fn missed_turn_reports(host: &EnvironmentHost) -> Vec<serde_json::Value> {
    let mut receipts: Vec<_> = host
        .instance
        .checkpoint()
        .await
        .receipts
        .into_values()
        .filter(|receipt| receipt.request["method"] == "missed_turn")
        .collect();
    receipts.sort_by_key(|receipt| receipt.sequence);
    receipts
        .into_iter()
        .map(|receipt| receipt.request["params"].clone())
        .collect()
}

#[tokio::test]
async fn an_environment_answers_missed_turns_before_the_host_reprompts_or_stalls() {
    let root = tempfile::tempdir().unwrap();
    let host = waiting_host(root.path(), RunPhase::Running, MissedTurns::Environment).await;
    let agent = SeatId::try_from("agent-1".to_owned()).unwrap();
    let human = SeatId::try_from("human-1".to_owned()).unwrap();
    let tick = |id: &'static str, arguments: serde_json::Value| {
        let host = Arc::clone(&host);
        let human = human.clone();
        async move {
            host.instance
                .invoke(&human, "tick", arguments, id, CancellationToken::new())
                .await
                .unwrap();
            host.table
                .materialize(&host.instance.checkpoint().await)
                .unwrap();
        }
    };
    let warnings = |host: &EnvironmentHost| {
        host.table
            .all_since(0)
            .unwrap()
            .into_iter()
            .filter(|row| row.message.as_deref() == Some("Missed once; act now."))
            .map(|row| (row.to, row.wake))
            .collect::<Vec<_>>()
    };
    let warning = (NoticeTarget::Seats(vec![agent.clone()]), true);
    let retry = (
        NoticeTarget::Seats(vec![agent.clone()]),
        DeliveryKind::Status,
        true,
    );
    let report = |misses: u64| json!({"cause":"idle","misses":misses,"limit":2});

    // First miss: the environment's own warning replaces the generic re-prompt.
    tick("await-agent", json!({})).await;
    idle_turn(&host, &agent, 1).await;
    assert!(host.settle_missed_turns().await.unwrap());
    assert_eq!(warnings(&host), vec![warning.clone()]);
    assert!(retry_wakes(&host).is_empty());

    // Second miss: the environment does nothing, so the host re-prompts.
    idle_turn(&host, &agent, 2).await;
    host.settle_missed_turns().await.unwrap();
    assert_eq!(retry_wakes(&host), vec![retry.clone()]);

    // Past the budget with the turn unresolved, the run stalls as before.
    idle_turn(&host, &agent, 3).await;
    host.settle_missed_turns().await.unwrap();
    {
        let state = host.state.lock().await;
        assert_eq!(
            (state.phase.clone(), state.reason.as_deref()),
            (
                RunPhase::Stalled,
                Some("agent-1 made no progress after 2 re-prompts")
            )
        );
    }
    assert_eq!(
        missed_turn_reports(&host).await,
        vec![report(1), report(2), report(3)]
    );

    // Resuming restores the budget; past it, the environment ends the turn itself.
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    tick("await-and-resolve", json!({"resolve":true})).await;
    for through_seq in 4..=6 {
        idle_turn(&host, &agent, through_seq).await;
        host.settle_missed_turns().await.unwrap();
    }
    {
        let state = host.state.lock().await;
        assert_eq!(state.phase, RunPhase::Running);
        assert!(state.idle_retries.is_empty());
        assert!(state.missed_turns.is_empty());
    }
    assert_eq!(warnings(&host), vec![warning.clone(), warning]);
    assert_eq!(retry_wakes(&host), vec![retry.clone(), retry]);
    assert_eq!(
        missed_turn_reports(&host).await,
        vec![
            report(1),
            report(2),
            report(3),
            report(1),
            report(2),
            report(3)
        ]
    );
    assert!(host
        .instance
        .checkpoint()
        .await
        .progress
        .unwrap()
        .waiting_for
        .is_empty());
    host.instance.leave().await.unwrap();
}
