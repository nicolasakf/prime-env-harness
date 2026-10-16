use super::tests::{missed_turn_reports, waiting_host, MissedTurns};
use super::*;
use serde_json::json;

fn seat(id: &str) -> SeatId {
    SeatId::try_from(id.to_owned()).unwrap()
}

/// The builder-token parameters of a seat intervention for `agent-1`.
fn intervention(extra: serde_json::Value) -> serde_json::Value {
    let mut parameters = json!({"builder_token":"b".repeat(64),"seat":"agent-1","reason":"keeps calling unavailable actions"});
    let serde_json::Value::Object(extra) = extra else {
        panic!("intervention parameters must be an object");
    };
    for (key, value) in extra {
        parameters[key] = value;
    }
    parameters
}

async fn tick(host: &EnvironmentHost, id: &str, arguments: serde_json::Value) {
    host.instance
        .invoke(
            &seat("human-1"),
            "tick",
            arguments,
            id,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    host.table
        .materialize(&host.instance.checkpoint().await)
        .unwrap();
}

async fn phase_and_reason(host: &EnvironmentHost) -> (RunPhase, Option<String>) {
    let state = host.state.lock().await;
    (state.phase.clone(), state.reason.clone())
}

fn awaited(checkpoint: &pa_types::environments::InstanceCheckpoint) -> Vec<SeatId> {
    checkpoint.progress.as_ref().unwrap().waiting_for.clone()
}

/// A seat that cannot finish its turn used to leave the builder only pause,
/// resume, and stop. Suspended, it takes no turns and its own calls are refused;
/// the run stalls naming it; the builder takes its turn, then returns it to play.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One run walked through every intervention in order.
async fn a_suspended_seat_waits_for_the_builder_to_act_for_it_or_resume_it() {
    let root = tempfile::tempdir().unwrap();
    let host = waiting_host(root.path(), RunPhase::Running, MissedTurns::Host).await;
    let agent = seat("agent-1");
    tick(&host, "await-agent", json!({})).await;

    // A player at a table cannot intervene, and the human seat is never suspended.
    for parameters in [
        json!({"seat":"agent-1","reason":"x"}),
        intervention(json!({"builder_token":"c".repeat(64)})),
    ] {
        assert!(host
            .client_operation("builder_suspend_seat", &parameters)
            .await
            .is_err());
    }
    assert_eq!(
        host.client_operation(
            "builder_suspend_seat",
            &intervention(json!({"seat":"human-1"}))
        )
        .await
        .unwrap_err()
        .to_string(),
        "the human seat acts for itself"
    );

    // The seat's running turn is cut as the host's own abort: it does not pause the run.
    host.state
        .lock()
        .await
        .busy
        .insert(agent.clone(), crate::util::now_ms());
    host.client_operation("builder_suspend_seat", &intervention(json!({})))
        .await
        .unwrap();
    host.call(&agent, SeatCall::TurnFailed { through_seq: 1 })
        .await
        .unwrap();
    assert_eq!(
        phase_and_reason(&host).await,
        (RunPhase::Running, None),
        "a cut turn must not pause the run"
    );

    // Its own calls cannot change the run, and a racing wake starts no turn.
    let refused = host
        .call(
            &agent,
            SeatCall::Invoke {
                action: "play".into(),
                arguments: json!({}),
                operation_id: "own-play".into(),
                view_version: None,
            },
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        refused,
        "agent-1 is suspended by the run's builder and cannot act until resumed"
    );
    assert_eq!(
        host.call(&agent, SeatCall::TurnStarted { through_seq: 2 })
            .await
            .unwrap()
            .result,
        json!({"settled":true})
    );

    // Nothing else can move, so the run stalls naming the seat it waits on.
    host.drain().await.unwrap();
    assert_eq!(
        phase_and_reason(&host).await,
        (
            RunPhase::Stalled,
            Some("waiting on suspended seat agent-1: act for it or resume it".into())
        )
    );

    // The builder takes the awaited turn on the stalled run; only that seat is told.
    let rows_before = host.table.all_since(0).unwrap().len();
    let acted = host
        .client_operation(
            "builder_act_for_seat",
            &intervention(json!({"action":"play","arguments":{},"reason":"seat is stuck"})),
        )
        .await
        .unwrap();
    assert_eq!(acted["result"], json!({"played":"agent-1"}));
    let checkpoint = host.instance.checkpoint().await;
    assert!(awaited(&checkpoint).is_empty());
    let receipt = checkpoint
        .receipts
        .iter()
        .find(|(id, _)| id.starts_with("builder-"))
        .map(|(_, receipt)| receipt.request.clone())
        .unwrap();
    assert_eq!(
        (&receipt["participant"], &receipt["params"]["action"]),
        (&json!("agent-1"), &json!("play"))
    );
    let notices: Vec<_> = host
        .table
        .all_since(0)
        .unwrap()
        .into_iter()
        .skip(rows_before)
        .map(|row| (row.to, row.kind, row.wake, row.message))
        .collect();
    assert_eq!(
        notices,
        vec![(
            NoticeTarget::Seats(vec![agent.clone()]),
            DeliveryKind::Status,
            false,
            Some(
                "The run's builder took this action for you: play {}. Reason: seat is stuck".into()
            )
        )]
    );
    {
        let state = host.state.lock().await;
        assert_eq!(state.builder_actions, BTreeMap::from([(agent.clone(), 1)]));
        assert_eq!(state.phase, RunPhase::Stalled);
    }

    // Only a turn the environment awaits can be taken, and only for an agent seat.
    for (parameters, error) in [
        (
            intervention(json!({"action":"play","reason":"again"})),
            "the environment is not waiting on agent-1; act for a seat only when it is that seat's turn",
        ),
        (
            intervention(json!({"seat":"human-1","action":"tick","reason":"x"})),
            "the human seat acts for itself",
        ),
        (
            intervention(json!({"action":"play","reason":" "})),
            "an intervention needs a reason",
        ),
    ] {
        assert_eq!(
            host.client_operation("builder_act_for_seat", &parameters)
                .await
                .unwrap_err()
                .to_string(),
            error
        );
    }

    // A resumed run that stalls again is news to the builder again.
    host.state.lock().await.builder_notified = Some(RunPhase::Stalled);
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    assert_eq!(host.state.lock().await.builder_notified, None);

    // Back in play, the awaited seat is told to observe and act.
    tick(&host, "await-agent-again", json!({})).await;
    let rows_before = host.table.all_since(0).unwrap().len();
    host.client_operation("builder_resume_seat", &intervention(json!({})))
        .await
        .unwrap();
    let resumed: Vec<_> = host
        .table
        .all_since(0)
        .unwrap()
        .into_iter()
        .skip(rows_before)
        .map(|row| (row.to, row.kind, row.wake))
        .collect();
    assert_eq!(
        resumed,
        vec![(
            NoticeTarget::Seats(vec![agent.clone()]),
            DeliveryKind::Status,
            true
        )]
    );
    assert!(host.state.lock().await.suspended.is_empty());
    assert_eq!(
        host.client_operation("builder_resume_seat", &intervention(json!({})))
            .await
            .unwrap_err()
            .to_string(),
        "agent-1 is not suspended"
    );
    host.instance.leave().await.unwrap();
}

/// An environment that plays for absent seats hears about a suspended seat once
/// per revision in which it awaits it, never as a budget-spending miss.
#[tokio::test]
async fn a_suspended_awaited_seat_is_reported_to_missed_turn_once_per_revision() {
    let root = tempfile::tempdir().unwrap();
    let host = waiting_host(root.path(), RunPhase::Running, MissedTurns::Environment).await;
    let report = json!({"cause":"suspended","misses":1,"limit":2});
    tick(&host, "await-agent", json!({})).await;
    host.client_operation("builder_suspend_seat", &intervention(json!({})))
        .await
        .unwrap();

    // The environment leaves the turn open: the run stalls on the suspended seat.
    host.drain().await.unwrap();
    assert_eq!(missed_turn_reports(&host).await, vec![report.clone()]);
    assert_eq!(
        phase_and_reason(&host).await,
        (
            RunPhase::Stalled,
            Some("waiting on suspended seat agent-1: act for it or resume it".into())
        )
    );

    // The same revision is not reported twice.
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    host.drain().await.unwrap();
    assert_eq!(missed_turn_reports(&host).await, vec![report.clone()]);

    // A new revision is reported, and this time the environment ends the turn.
    tick(&host, "await-and-resolve", json!({"resolve":true})).await;
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    host.drain().await.unwrap();
    assert_eq!(
        missed_turn_reports(&host).await,
        vec![report.clone(), report]
    );
    assert!(awaited(&host.instance.checkpoint().await).is_empty());
    let state = host.state.lock().await;
    assert!(state.idle_retries.is_empty() && state.missed_turns.is_empty());
    drop(state);
    host.instance.leave().await.unwrap();
}
