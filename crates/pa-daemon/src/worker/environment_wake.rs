//! Durable wake inbox and atomic queue admission; ACK never outruns persistence.
use super::{json, queue, QueuePriority, QueuedItem, Result, Value, Worker};
use crate::environment_host::{authenticate, read, write};
use crate::environment_seat::SeatInbox;
use pa_types::environments::{AgentContext, DeliveryKind, NoticeTarget, WakePayload};

/// What the host tells a seat when its turn starts, to bring a queued wake up to date.
#[derive(Default)]
pub(super) struct WakeRefresh {
    /// A stateless seat's view as of now.
    pub view: Option<Value>,
    /// Committed-state version of `view`; the turn's tool calls are bound to it.
    pub view_version: Option<u64>,
    /// The wake without the tasks the environment no longer asks of the seat.
    pub trimmed: Option<WakePayload>,
}

/// The transcript text of a wake: for a conversation seat the whole payload, for a
/// stateless seat its view plus only what was addressed to it, since broadcast
/// events live in the view, not in the model's context.
fn wake_text(wake: &WakePayload) -> Result<String> {
    Ok(match wake.agent_context {
        AgentContext::Conversation => {
            format!("Environment wake:\n{}", serde_json::to_string(wake)?)
        }
        AgentContext::Stateless => {
            let messages: Vec<String> = wake
                .deliveries
                .iter()
                .filter(|delivery| delivery.to != NoticeTarget::All)
                .filter_map(|delivery| {
                    let message = delivery.message.as_deref()?;
                    Some(match (&delivery.kind, &delivery.from) {
                        (DeliveryKind::DirectMessage, Some(from)) => {
                            format!("{from} says: {message}")
                        }
                        _ => message.to_owned(),
                    })
                })
                .collect();
            let mut view = json!({"state":wake.observation,"messages":messages});
            if wake.attempt > 0 {
                view["retry"] = json!(wake.attempt);
            }
            format!("Environment wake:\n{}", serde_json::to_string(&view)?)
        }
    })
}

/// Bring a queued wake up to date when its turn starts: the current view and its
/// version, in the prompt and in the transcript row alike, so the model never reads
/// state that has since moved on, and without tasks the environment retired.
pub(super) fn refresh_wake(item: &mut QueuedItem, refresh: WakeRefresh) -> Result<()> {
    let WakeRefresh {
        view,
        view_version,
        trimmed,
    } = refresh;
    let text = if let Some(mut trimmed) = trimmed {
        if let Some(view) = view {
            trimmed.observation = view;
        }
        Some(wake_text(&trimmed)?)
    } else {
        view.and_then(|view| {
            let rest = item.message.strip_prefix("Environment wake:\n")?;
            let mut wake = serde_json::from_str::<Value>(rest).ok()?;
            wake["state"] = view;
            Some(format!("Environment wake:\n{wake}"))
        })
    };
    if let Some(row) = item.custom_message.as_mut() {
        if let Some(text) = &text {
            row["content"] = json!(text);
        }
        if let Some(version) = view_version {
            row["details"]["view_version"] = json!(version);
        }
    }
    if let Some(text) = text {
        item.message = text;
    }
    Ok(())
}

impl Worker {
    pub(super) fn admit_environment_wake(&self, parameters: &Value, replay: bool) -> Result<Value> {
        let engine = self
            .agent_engine
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("seat engine required"))?;
        let config = engine
            .create_resources
            .read()
            .map_err(|_| anyhow::anyhow!("seat config lock poisoned"))?
            .environment_seat
            .clone()
            .ok_or_else(|| anyhow::anyhow!("not an environment seat"))?;
        authenticate(
            &config.token,
            parameters["token"].as_str().unwrap_or_default(),
        )?;
        let payload: WakePayload = serde_json::from_value(parameters["payload"].clone())?;
        let path = engine
            .environment_manager()?
            .store()
            .instance_path(&config.instance)?
            .join("seats")
            .join(config.seat.as_str())
            .join("inbox.json");
        // The recovery -> core lock order also serializes concurrent wake admissions.
        let mut recovery = self.recovery.lock().unwrap();
        let mut core = self.core.lock().unwrap();
        if engine.session_is_closed() || !core.created {
            anyhow::bail!("environment seat is closed");
        }
        let _lease = crate::environment_host::inbox_lock(&path)?;
        let mut inbox: SeatInbox = if path.exists() {
            read(&path)?
        } else {
            SeatInbox::default()
        };
        if payload.through_seq <= inbox.settled
            || (!replay && payload.through_seq <= inbox.admitted)
        {
            return Ok(json!({"admitted":inbox.admitted}));
        }
        if core.follow_up.iter().any(|item| {
            item.custom_message
                .as_ref()
                .and_then(|row| row["details"]["through_seq"].as_u64())
                .is_some_and(|seq| seq >= payload.through_seq)
        }) {
            inbox.admitted = inbox.admitted.max(payload.through_seq);
            write(&path, &inbox)?;
            return Ok(json!({"admitted":payload.through_seq}));
        }
        // Only a wake still waiting in the queue is replaced by this one, so only its
        // deliveries are carried over. A wake already handed to the model is in the
        // transcript; its events must not be sent again.
        let queued = core
            .follow_up
            .iter()
            .any(|item| item.queue_key.as_deref() == Some("environment_wake"));
        let mut merged = payload;
        if let (true, Some(pending)) = (queued, &inbox.pending) {
            for delivery in &pending.deliveries {
                if !merged
                    .deliveries
                    .iter()
                    .any(|entry| entry.seq == delivery.seq)
                {
                    merged.deliveries.push(delivery.clone());
                }
            }
            merged.deliveries.sort_by_key(|delivery| delivery.seq);
        }
        inbox.pending = Some(merged.clone());
        // Pending intent is durable before admitting any model work.
        write(&path, &inbox)?;
        // Stateless seats get the environment's view plus only what was addressed to
        // them; broadcast events live in the view, not in the model's context.
        let text = wake_text(&merged)?;
        let row = json!({"role":"custom","customType":"environment_wake","content":text,"display":false,"timestamp":crate::util::now_ms(),"details":{"through_seq":merged.through_seq,"attempt":merged.attempt,"view_version":merged.view_version}});
        let journal = recovery
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("seat wake requires a durable recovery journal"))?;
        let store = core
            .store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("seat wake requires a durable transcript"))?;
        let session_id = store.session_id().to_owned();
        let session_path = store.path.to_string_lossy().into_owned();
        let policy = if core.busy {
            queue::TurnPolicy::Queued
        } else {
            queue::TurnPolicy::Injected
        };
        let suspended = core.queued_input_suspended;
        let aborted = core.abort_requested;
        core.queued_input_suspended = false;
        core.abort_requested = false;
        let item = QueuedItem {
            message: text,
            priority: QueuePriority::Background,
            preview: None,
            custom_message: Some(row),
            agent_message: None,
            queue_key: Some("environment_wake".into()),
            admission_id: None,
            images: Vec::new(),
            done: None,
            queue_visible: false,
            policy,
            forced_batch: false,
        };
        let previous = core
            .follow_up
            .iter()
            .position(|item| item.queue_key.as_deref() == Some("environment_wake"))
            .and_then(|index| core.follow_up.remove(index));
        queue::enqueue_priority(&mut core.follow_up, item);
        let lanes = queue::queue_lanes(&core);
        // Unlike the generic best-effort checkpoint helper, failure is returned.
        if let Err(error) = journal.record_queue_checkpoint(
            &core.active_session_id,
            &session_id,
            Some(&session_path),
            true,
            "environment_wake",
            &lanes.steering,
            &lanes.follow_up,
        ) {
            if let Some(index) = core
                .follow_up
                .iter()
                .position(|item| item.queue_key.as_deref() == Some("environment_wake"))
            {
                core.follow_up.remove(index);
            }
            if let Some(previous) = previous {
                queue::enqueue_priority(&mut core.follow_up, previous);
            }
            core.queued_input_suspended = suspended;
            core.abort_requested = aborted;
            return Err(error);
        }
        inbox.admitted = inbox.admitted.max(merged.through_seq);
        write(&path, &inbox)?;
        drop(core);
        drop(recovery);
        self.work_notify.notify_one();
        Ok(json!({"admitted":merged.through_seq}))
    }

    pub(super) fn restore_environment_inbox(&self) -> Result<()> {
        let Some(engine) = &self.agent_engine else {
            return Ok(());
        };
        let config = engine
            .create_resources
            .read()
            .map_err(|_| anyhow::anyhow!("seat config lock poisoned"))?
            .environment_seat
            .clone();
        let Some(config) = config else {
            return Ok(());
        };
        let path = engine
            .environment_manager()?
            .store()
            .instance_path(&config.instance)?
            .join("seats")
            .join(config.seat.as_str())
            .join("inbox.json");
        if path.exists() {
            let inbox: SeatInbox = read(&path)?;
            if let Some(payload) = inbox
                .pending
                .filter(|payload| payload.through_seq > inbox.settled)
            {
                self.admit_environment_wake(
                    &json!({"token":config.token,"payload":payload}),
                    /*replay*/ true,
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::WorkerConfig;
    use pa_environments::EnvironmentManager;
    use pa_types::environments::{EnvironmentLanguage, ExecutionPolicy};
    use std::sync::Arc;

    #[tokio::test]
    async fn wake_admission_coalesces_and_failed_journaling_cannot_ack_or_run() {
        let root = tempfile::tempdir().unwrap();
        let manager =
            EnvironmentManager::new(root.path().join("agent/environments"), None).unwrap();
        let draft = manager
            .store()
            .create_draft(
                "wake admission",
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
        let active = manager
            .enter("counter", None, ExecutionPolicy::default())
            .await
            .unwrap();
        let seat_path = active.path().join("seats/agent-1");
        std::fs::create_dir_all(&seat_path).unwrap();
        let token = "a".repeat(64);
        write(&seat_path.join("token.json"), &token).unwrap();
        let config = WorkerConfig {
            socket_path: root.path().join("worker.sock"),
            supervisor_socket_path: root.path().join("supervisor.sock"),
            token: "worker-token".into(),
            worker_instance_id: String::new(),
            active_session_id: "seat-test".into(),
            agent_dir: root.path().join("agent"),
            recovery_journal_path: root.path().join("recovery.jsonl"),
            telemetry_disabled: Some(true),
            script: Some(json!({"engine":"faux","responses":["unused"]})),
        };
        let worker = Arc::new(Worker::new(config, None));
        *worker.recovery.lock().unwrap() = Some(
            crate::journal::WorkerRecoveryJournal::open(&worker.config.recovery_journal_path)
                .unwrap(),
        );
        let created = worker.dispatch("create", &json!({"cwd":root.path(),"sessionPath":seat_path.join("conversation.jsonl"),"environmentSeat":{"instance":active.instance.id,"seat":"agent-1","token":token}})).await;
        assert!(created.success, "{created:?}");
        worker.core.lock().unwrap().busy = true;
        let payload = |seq| json!({"token":token,"payload":{"through_seq":seq,"observation":{"value":seq},"deliveries":[]}});
        worker
            .admit_environment_wake(&payload(1), /*replay*/ false)
            .unwrap();
        worker
            .admit_environment_wake(&payload(2), /*replay*/ false)
            .unwrap();
        assert_eq!(worker.core.lock().unwrap().follow_up.len(), 1);
        let pending: SeatInbox = read(&seat_path.join("inbox.json")).unwrap();
        assert_eq!(pending.admitted, 2);
        let queued = worker
            .core
            .lock()
            .unwrap()
            .follow_up
            .front()
            .unwrap()
            .message
            .clone();
        std::fs::remove_file(&worker.config.recovery_journal_path).unwrap();
        std::fs::create_dir(&worker.config.recovery_journal_path).unwrap();
        assert!(worker
            .admit_environment_wake(&payload(3), /*replay*/ false)
            .is_err());
        assert_eq!(
            worker
                .core
                .lock()
                .unwrap()
                .follow_up
                .front()
                .unwrap()
                .message,
            queued
        );
        let pending: SeatInbox = read(&seat_path.join("inbox.json")).unwrap();
        assert_eq!(pending.admitted, 2);
        assert_eq!(pending.pending.unwrap().through_seq, 3);
        std::fs::remove_dir(&worker.config.recovery_journal_path).unwrap();
        worker
            .admit_environment_wake(&payload(3), /*replay*/ false)
            .unwrap();
        assert_eq!(worker.core.lock().unwrap().follow_up.len(), 1);
        let delivery = |seq: u64| json!({"seq":seq,"event_id":format!("event-{seq}"),"kind":"notice","to":"*","message":"moved","wake":true,"timestamp":0});
        let with_delivery = |seq: u64| json!({"token":token,"payload":{"through_seq":seq,"observation":{"value":seq},"deliveries":[delivery(seq)]}});
        let pending_deliveries = || {
            let inbox: SeatInbox = read(&seat_path.join("inbox.json")).unwrap();
            serde_json::to_value(inbox.pending.unwrap().deliveries).unwrap()
        };
        // A wake still waiting in the queue is replaced, keeping its events.
        worker
            .admit_environment_wake(&with_delivery(4), /*replay*/ false)
            .unwrap();
        worker
            .admit_environment_wake(&with_delivery(5), /*replay*/ false)
            .unwrap();
        assert_eq!(pending_deliveries(), json!([delivery(4), delivery(5)]));
        // A wake the model already consumed is not sent again with the next one.
        worker.core.lock().unwrap().follow_up.clear();
        worker
            .admit_environment_wake(&with_delivery(6), /*replay*/ false)
            .unwrap();
        assert_eq!(pending_deliveries(), json!([delivery(6)]));
        worker.core.lock().unwrap().follow_up.clear();
        worker.core.lock().unwrap().busy = false;
        active.leave().await.unwrap();
    }

    #[test]
    fn refreshing_a_wake_replaces_its_state_in_prompt_and_transcript_row_only() {
        let wake = |state: Value| {
            format!(
                "Environment wake:\n{}",
                json!({"state":state,"messages":["Your turn."],"retry":1})
            )
        };
        let mut item = QueuedItem {
            message: wake(json!({"turn":1})),
            priority: QueuePriority::Background,
            preview: None,
            custom_message: Some(
                json!({"customType":"environment_wake","content":wake(json!({"turn":1})),"details":{"through_seq":4}}),
            ),
            agent_message: None,
            queue_key: Some("environment_wake".into()),
            admission_id: None,
            images: Vec::new(),
            done: None,
            queue_visible: false,
            policy: queue::TurnPolicy::Queued,
            forced_batch: false,
        };
        refresh_wake(
            &mut item,
            WakeRefresh {
                view: Some(json!({"turn":3})),
                view_version: Some(9),
                trimmed: None,
            },
        )
        .unwrap();
        assert_eq!(item.message, wake(json!({"turn":3})));
        assert_eq!(
            item.custom_message.unwrap(),
            json!({"customType":"environment_wake","content":wake(json!({"turn":3})),"details":{"through_seq":4,"view_version":9}})
        );
    }
}
