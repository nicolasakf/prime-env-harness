//! Turn admission revalidates actionable tasks against the environment's wait state.
use super::*;
use pa_types::environments::{EnvironmentLanguage, ExecutionPolicy};
use serde_json::json;

/// Roster seats are `human-1` (drives the environment through `tick`) and `agent-1`.
/// `tick` publishes progress waiting on `waiting`; `task` adds an actionable wake
/// for that seat, and `note` adds an ordinary wake for `agent-1`.
async fn attention_host(root: &std::path::Path, view: ViewSink) -> Arc<EnvironmentHost> {
    let manager = pa_environments::EnvironmentManager::new(root.into(), None).unwrap();
    let draft = manager
        .store()
        .create_draft(
            "attention",
            EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    let environment = r#"from sdk import notify, progress
class E:
    def initialize(self, params):
        self.revision = 0
        return {}
    def describe_actions(self): return [{"name":"tick","description":"tick","input_schema":{"type":"object"},"output_schema":{"type":"object"},"roles":["human"]}]
    def observe(self, participant): return {"value":self.revision}
    def handle_input(self, participant, text): return {"observation":{}}
    def invoke(self, participant, action, arguments):
        self.revision += 1
        waiting = arguments["waiting"]
        progress(self.revision, waiting_for=[waiting])
        if arguments.get("task"):
            notify([waiting], "Your turn.", actionable=True)
        if arguments.get("note"):
            notify(["agent-1"], "A note.")
        return {}
    def snapshot(self): return {"revision":self.revision}
    def restore(self, snapshot):
        self.revision = snapshot["revision"]
        return {}
    def dispose(self): return {}
environment=E()
"#;
    std::fs::write(draft.join("environment.py"), environment).unwrap();
    std::fs::write(
        draft.join("checks.py"),
        "from environment import E\nenvironment = E()\nenvironment.initialize({})\nassert environment.snapshot() == {\"revision\": 0}\n",
    ).unwrap();
    assert!(
        manager
            .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
            .await
            .unwrap()
            .passed
    );
    manager.save(draft.to_str().unwrap(), "attention").unwrap();
    let definition = manager.store().definition("attention", None).unwrap().0;
    let instance = manager
        .enter_with_roster(
            "attention",
            None,
            ExecutionPolicy::default(),
            json!({}),
            pa_environments::default_roster(&definition.manifest).unwrap(),
        )
        .await
        .unwrap();
    Arc::new(EnvironmentHost {
        table: TableLog::open(instance.path()).unwrap(),
        state: Mutex::new(serde_json::from_value(json!({"phase":"running","reason":null,"operations":0,"wakes":{},"busy":{},"no_progress_turns":0,"revision":null,"started_at":crate::util::now_ms(),"settled":{}})).unwrap()),
        gate: Mutex::new(()), link: SupervisorLink::new(root.join("unused.sock")),
        builder_token: "b".repeat(64), host_id: "host".into(), telemetry: None,
        defaults: json!({}), seats: Mutex::new(BTreeMap::new()),
        python_cancellations: Mutex::new(BTreeMap::new()), changed: Notify::new(), announced_phase: std::sync::Mutex::new(None),
        shutdown: CancellationToken::new(), view, instance,
    })
}

async fn tick(host: &EnvironmentHost, id: &str, arguments: serde_json::Value) {
    host.instance
        .invoke(
            &SeatId::try_from("human-1".to_owned()).unwrap(),
            "tick",
            arguments,
            id,
            CancellationToken::new(),
        )
        .await
        .unwrap();
}

/// Persists a wake the way dispatch and admission do, for `agent-1`, carrying every
/// delivery the table holds for it, then starts its turn.
async fn start_turn(host: &Arc<EnvironmentHost>, through_seq: u64) -> SeatReply {
    let agent = SeatId::try_from("agent-1".to_owned()).unwrap();
    host.table
        .materialize(&host.instance.checkpoint().await)
        .unwrap();
    let deliveries: Vec<_> = host
        .table
        .since(Some(&agent), 0)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.seq <= through_seq)
        .collect();
    let (view_version, observation) = host.instance.observe_versioned(&agent).await.unwrap();
    write(
        &host.instance.path().join("seats/agent-1/inbox.json"),
        &crate::environment_seat::SeatInbox {
            admitted: through_seq,
            settled: 0,
            pending: Some(WakePayload {
                through_seq,
                observation,
                view_version: Some(view_version),
                deliveries,
                agent_context: AgentContext::Conversation,
                attempt: 0,
            }),
        },
    )
    .unwrap();
    host.call(&agent, SeatCall::TurnStarted { through_seq })
        .await
        .unwrap()
}

fn tasks(reply: &SeatReply) -> Vec<String> {
    reply.result["wake"]["deliveries"]
        .as_array()
        .map(|deliveries| {
            deliveries
                .iter()
                .map(|delivery| delivery["message"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_task_the_environment_no_longer_awaits_the_seat_for_starts_no_turn() {
    let root = tempfile::tempdir().unwrap();
    let host = attention_host(root.path(), Arc::new(|_| Ok(()))).await;
    let agent = SeatId::try_from("agent-1".to_owned()).unwrap();
    tick(&host, "ask", json!({"waiting":"agent-1","task":true})).await;
    // The seat's task is current: the turn runs and the wake is unchanged.
    let reply = start_turn(&host, 1).await;
    assert_eq!(reply.result, json!({}));
    assert!(host.state.lock().await.busy.contains_key(&agent));
    host.state.lock().await.busy.clear();
    // The environment moves on to the peer while the wake was queued.
    tick(&host, "moved-on", json!({"waiting":"human-1"})).await;
    let reply = start_turn(&host, 1).await;
    assert_eq!(reply.result, json!({"settled":true}));
    let state = host.state.lock().await;
    assert_eq!(state.settled.get(&agent), Some(&1));
    assert!(!state.busy.contains_key(&agent));
    drop(state);
    // The retirement is durable: starting the same wake again settles it again.
    assert_eq!(start_turn(&host, 1).await.result, json!({"settled":true}));
    host.instance.leave().await.unwrap();
}

#[tokio::test]
async fn an_obsolete_task_is_trimmed_from_a_wake_that_still_carries_a_message() {
    let root = tempfile::tempdir().unwrap();
    let host = attention_host(root.path(), Arc::new(|_| Ok(()))).await;
    tick(
        &host,
        "ask",
        json!({"waiting":"agent-1","task":true,"note":true}),
    )
    .await;
    tick(&host, "moved-on", json!({"waiting":"human-1"})).await;
    let reply = start_turn(&host, 2).await;
    assert!(reply.result.get("settled").is_none());
    assert_eq!(tasks(&reply), vec!["A note.".to_owned()]);
    host.instance.leave().await.unwrap();
}

#[tokio::test]
async fn a_newer_task_supersedes_an_older_one_and_a_lone_task_is_never_withheld() {
    let root = tempfile::tempdir().unwrap();
    let host = attention_host(root.path(), Arc::new(|_| Ok(()))).await;
    tick(&host, "first", json!({"waiting":"agent-1","task":true})).await;
    tick(&host, "second", json!({"waiting":"agent-1","task":true})).await;
    // A wake that carries only the first task: the second has its own wake.
    let reply = start_turn(&host, 1).await;
    assert_eq!(reply.result, json!({"settled":true}));
    // A wake carrying both keeps the current one.
    let reply = start_turn(&host, 2).await;
    assert_eq!(tasks(&reply), vec!["Your turn.".to_owned()]);
    assert_eq!(reply.result["wake"]["deliveries"][0]["op_seq"], 3);
    host.state.lock().await.busy.clear();
    // The environment awaits the seat again without a new task: the old one stands.
    tick(&host, "bare", json!({"waiting":"agent-1"})).await;
    let reply = start_turn(&host, 2).await;
    assert_eq!(reply.result, json!({}));
    host.instance.leave().await.unwrap();
}

#[tokio::test]
async fn a_stale_seat_call_executes_nothing_and_returns_the_current_view() {
    let root = tempfile::tempdir().unwrap();
    let host = attention_host(root.path(), Arc::new(|_| Ok(()))).await;
    let human = SeatId::try_from("human-1".to_owned()).unwrap();
    let shown = host.instance.version().await;
    tick(&host, "peer", json!({"waiting":"human-1"})).await;
    let before = host.instance.checkpoint().await;
    let call = |view_version| SeatCall::Invoke {
        action: "tick".into(),
        arguments: json!({"waiting":"human-1"}),
        operation_id: "late".into(),
        view_version: Some(view_version),
    };
    let reply = host.call(&human, call(shown)).await.unwrap();
    assert_eq!(
        reply,
        SeatReply {
            result: json!({
                "rejected": "stale_view",
                "message": "Not executed: the environment changed after the state you acted on. Reconsider against the current state and act again only if it is still right.",
            }),
            deliveries: Vec::new(),
            view: Some(json!({"value":1})),
            view_version: Some(before.next_sequence),
            stale_view: true,
        }
    );
    assert_eq!(host.instance.checkpoint().await, before);
    // Bound to the view it just received, the same action commits, and the reply
    // names the version a conversation seat's next action may build on.
    let reply = host
        .call(&human, call(reply.view_version.unwrap()))
        .await
        .unwrap();
    assert!(!reply.stale_view);
    assert_eq!(reply.view_version, Some(before.next_sequence + 1));
    host.instance.leave().await.unwrap();
}

/// A form is bound to the state it displayed: a stale submission is rejected, the
/// refreshed state is published for the human to review, and nothing is rebound.
#[tokio::test]
async fn a_human_form_bound_to_a_stale_display_is_rejected_and_the_state_refreshed() {
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let host = attention_host(
        root.path(),
        Arc::new(move |event| {
            sink.lock().unwrap().push(event);
            Ok(())
        }),
    )
    .await;
    let displayed = host.status().await.unwrap()["view_version"]
        .as_u64()
        .unwrap();
    tick(&host, "peer", json!({"waiting":"human-1"})).await;
    let before = host.instance.checkpoint().await;
    let form = |view_version: u64| json!({"action":"tick","arguments":{"waiting":"human-1"},"operation_id":"form","view_version":view_version});
    let error = host
        .client_operation("act", &form(displayed))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("submit again"), "{error:#}");
    assert_eq!(host.instance.checkpoint().await, before);
    assert_eq!(
        events.lock().unwrap().last().unwrap(),
        &json!({"type":"environment_observation","observation":{"value":1},"view_version":before.next_sequence})
    );
    // Reviewed and submitted again against the refreshed display, it commits.
    let reply = host
        .client_operation("act", &form(before.next_sequence))
        .await
        .unwrap();
    assert_eq!(reply["view_version"], before.next_sequence + 1);
    host.instance.leave().await.unwrap();
}
