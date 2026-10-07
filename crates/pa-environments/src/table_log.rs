//! Durable delivery order, independent of model transcripts and admission cursors.
use anyhow::{bail, Context, Result};
use pa_types::environments::{Delivery, DeliveryKind, InstanceCheckpoint, NoticeTarget, SeatId};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

pub struct TableLog {
    path: PathBuf,
    rows: Mutex<Vec<Delivery>>,
    failed: AtomicBool,
}

impl TableLog {
    /// Open a table and discard only an incomplete final line left by a crash.
    /// # Errors
    /// Rejects corrupt complete rows or a broken delivery sequence.
    pub fn open(instance: &Path) -> Result<Self> {
        let path = instance.join("table.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        let bytes = std::fs::read(&path)?;
        let complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let mut rows: Vec<Delivery> = Vec::new();
        let mut ids = BTreeSet::new();
        for line in bytes[..complete]
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let row: Delivery = serde_json::from_slice(line).context("corrupt table row")?;
            if row.seq != rows.len() as u64 + 1 || !ids.insert(row.event_id.clone()) {
                bail!("invalid table delivery sequence or duplicate event");
            }
            rows.push(row);
        }
        if complete != bytes.len() {
            file.set_len(complete as u64)?;
            file.sync_all()?;
        }
        Ok(Self {
            path,
            rows: Mutex::new(rows),
            failed: AtomicBool::new(false),
        })
    }

    /// Rebuild missing notices by stable (operation sequence, notice index) IDs.
    /// # Errors
    /// Returns persistence failures; retrying never duplicates a complete row.
    pub fn materialize(&self, checkpoint: &InstanceCheckpoint) -> Result<()> {
        let mut receipts: Vec<_> = checkpoint
            .receipts
            .values()
            .filter_map(|receipt| receipt.sequence.map(|seq| (seq, receipt)))
            .collect();
        receipts.sort_by_key(|(seq, _)| *seq);
        for (seq, receipt) in receipts {
            for (index, notice) in receipt.notices.iter().enumerate() {
                self.append(Delivery {
                    seq: 0,
                    event_id: format!("operation-{seq}-notice-{index}"),
                    op_seq: Some(seq),
                    kind: DeliveryKind::Notice,
                    from: None,
                    to: notice.to.clone(),
                    message: notice.message.clone(),
                    wake: notice.wake,
                    actionable: notice.actionable,
                    timestamp: 0,
                })?;
            }
            if let Some(finished) = &receipt.finished {
                self.append(Delivery {
                    seq: 0,
                    event_id: format!("operation-{seq}-finished"),
                    op_seq: Some(seq),
                    kind: DeliveryKind::Status,
                    from: None,
                    to: NoticeTarget::All,
                    message: Some(finished.summary.clone()),
                    wake: false,
                    actionable: false,
                    timestamp: 0,
                })?;
            }
        }
        Ok(())
    }

    /// Append once and fsync before returning. Reused IDs must describe the same event.
    /// # Errors
    /// Rejects ID reuse with different content and reports I/O errors.
    pub fn append(&self, mut row: Delivery) -> Result<Delivery> {
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| anyhow::anyhow!("table lock poisoned"))?;
        if self.failed.load(Ordering::Acquire) {
            bail!("table append failed; reopen before retrying");
        }
        if let Some(previous) = rows
            .iter()
            .find(|previous| previous.event_id == row.event_id)
        {
            row.seq = previous.seq;
            row.timestamp = previous.timestamp;
            if &row != previous {
                bail!("table event ID reused with different content");
            }
            return Ok(previous.clone());
        }
        row.seq = rows.len() as u64 + 1;
        row.timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        let mut bytes = serde_json::to_vec(&row)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            self.failed.store(true, Ordering::Release);
            return Err(error.into());
        }
        rows.push(row.clone());
        Ok(row)
    }

    /// Read events for one seat, or public events for a spectator.
    /// # Errors
    /// Reports a poisoned lock.
    pub fn since(&self, seat: Option<&SeatId>, after: u64) -> Result<Vec<Delivery>> {
        let rows = self
            .rows
            .lock()
            .map_err(|_| anyhow::anyhow!("table lock poisoned"))?;
        Ok(rows
            .iter()
            .filter(|row| {
                row.seq > after
                    && seat.map_or(matches!(row.to, NoticeTarget::All), |seat| {
                        row.visible_to(seat)
                    })
            })
            .cloned()
            .collect())
    }

    /// Privileged read of the full table, including targeted events.
    /// # Errors
    /// Reports a poisoned lock.
    pub fn all_since(&self, after: u64) -> Result<Vec<Delivery>> {
        Ok(self
            .rows
            .lock()
            .map_err(|_| anyhow::anyhow!("table lock poisoned"))?
            .iter()
            .filter(|row| row.seq > after)
            .cloned()
            .collect())
    }
}
