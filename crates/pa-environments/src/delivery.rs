//! Durable cursors. Advance only after the consumer acknowledges durable admission.
use crate::store::{read_json, write_json};
use anyhow::{bail, Result};
use pa_types::environments::SeatId;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryCursor {
    pub through_seq: u64,
}

impl DeliveryCursor {
    /// Read a previously acknowledged delivery position.
    /// # Errors
    /// Returns corrupt cursor or filesystem errors.
    pub fn load(instance: &Path, seat: &SeatId) -> Result<Self> {
        let path = instance.join("deliveries").join(format!("{seat}.json"));
        if path.exists() {
            read_json(&path)
        } else {
            Ok(Self::default())
        }
    }

    /// Persist a monotonic acknowledgment after durable admission.
    /// # Errors
    /// Rejects cursor regression and returns persistence failures.
    pub fn acknowledge(&mut self, instance: &Path, seat: &SeatId, through_seq: u64) -> Result<()> {
        if through_seq < self.through_seq {
            bail!("delivery acknowledgment regressed");
        }
        let next = Self { through_seq };
        write_json(
            &instance.join("deliveries").join(format!("{seat}.json")),
            &next,
        )?;
        *self = next;
        Ok(())
    }
}
