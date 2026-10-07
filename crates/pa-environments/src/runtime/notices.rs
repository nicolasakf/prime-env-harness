//! Validate attention metadata before publishing a committed operation.
use super::{
    bail, json, ActiveInstance, Context, HumanInputResult, InstanceCheckpoint, Result, SeatId,
    Value,
};
use pa_types::environments::{
    EnvironmentFinish, EnvironmentNotice, EnvironmentProgress, NoticeTarget,
};

#[derive(Default, serde::Deserialize)]
struct Metadata {
    #[serde(default)]
    notices: Vec<EnvironmentNotice>,
    finished: Option<EnvironmentFinish>,
    progress: Option<EnvironmentProgress>,
}

pub(super) fn response_metadata(
    response: &Value,
    request: &Value,
    checkpoint: &InstanceCheckpoint,
    seats: &[SeatId],
) -> Result<(
    Vec<EnvironmentNotice>,
    Option<EnvironmentFinish>,
    Option<EnvironmentProgress>,
)> {
    let mut metadata: Metadata = serde_json::from_value(json!({
        "notices": response.get("notices").cloned().unwrap_or_else(|| json!([])),
        "finished": response.get("finished"),
        "progress": response.get("progress"),
    }))
    .context("invalid operation attention metadata")?;
    if request["method"] == "handle_input" && checkpoint.protocol_version == 1 {
        let input: HumanInputResult = serde_json::from_value(response["result"].clone())
            .context("invalid human input result")?;
        if let Some(message) = input.agent_message {
            metadata.notices.push(EnvironmentNotice {
                to: NoticeTarget::Seats(vec![SeatId::legacy_agent()]),
                message: Some(message),
                wake: true,
                actionable: false,
            });
        }
    }
    if request["method"] == "handle_input" && checkpoint.protocol_version == 2 {
        let _: pa_types::environments::SeatInputResult =
            serde_json::from_value(response["result"].clone())
                .context("invalid seat input result")?;
    }
    if metadata.notices.len() > 64 {
        bail!("at most 64 notices per operation");
    }
    for notice in &metadata.notices {
        if notice
            .message
            .as_ref()
            .is_some_and(|message| message.len() > 16 * 1024)
        {
            bail!("notice message exceeds 16 KiB");
        }
        if notice.actionable && (!notice.wake || notice.to == NoticeTarget::All) {
            bail!("an actionable notice must wake named seats");
        }
        match &notice.to {
            NoticeTarget::All => {}
            NoticeTarget::Seats(targets) => {
                if targets.is_empty()
                    || targets.len() > 16
                    || targets.iter().any(|seat| !seats.contains(seat))
                {
                    bail!("notice targets must name existing seats");
                }
                let unique: std::collections::BTreeSet<_> = targets.iter().collect();
                if unique.len() != targets.len() {
                    bail!("notice targets must be unique");
                }
            }
        }
    }
    if let Some(finished) = &metadata.finished {
        if checkpoint.protocol_version != 2 || finished.summary.len() > 16 * 1024 {
            bail!("finish requires protocol v2 and a summary of at most 16 KiB");
        }
    }
    if let Some(progress) = &metadata.progress {
        if progress.waiting_for.len() > 16
            || progress
                .waiting_for
                .iter()
                .any(|seat| !seats.contains(seat))
            || checkpoint
                .progress
                .as_ref()
                .is_some_and(|previous| progress.revision < previous.revision)
        {
            bail!("progress requires a monotonic revision and existing waiting seats");
        }
    }
    Ok((metadata.notices, metadata.finished, metadata.progress))
}

impl ActiveInstance {
    /// Subscribe to durable commits. Consumers drain persisted receipts on startup.
    #[must_use]
    pub fn commits(&self) -> tokio::sync::watch::Receiver<u64> {
        self.commits.subscribe()
    }

    /// Read the latest persisted snapshot metadata, including terminal/recovery state.
    #[tracing::instrument(skip_all)]
    pub async fn checkpoint(&self) -> InstanceCheckpoint {
        self.runtime.lock().await.checkpoint.clone()
    }

    /// Return receipt notices addressed to this seat after an operation sequence.
    #[tracing::instrument(skip_all)]
    pub async fn notices_since(
        &self,
        seat: &SeatId,
        after_sequence: u64,
    ) -> Vec<(u64, EnvironmentNotice)> {
        let runtime = self.runtime.lock().await;
        let mut notices: Vec<_> = runtime
            .checkpoint
            .receipts
            .values()
            .flat_map(|receipt| {
                receipt
                    .sequence
                    .filter(|sequence| *sequence > after_sequence)
                    .into_iter()
                    .flat_map(move |sequence| {
                        receipt
                            .notices
                            .iter()
                            .filter(move |notice| match &notice.to {
                                NoticeTarget::All => true,
                                NoticeTarget::Seats(seats) => seats.contains(seat),
                            })
                            .cloned()
                            .map(move |notice| (sequence, notice))
                    })
            })
            .collect();
        notices.sort_by_key(|(sequence, _)| *sequence);
        notices
    }
}
