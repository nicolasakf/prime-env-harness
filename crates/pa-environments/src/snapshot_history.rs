//! Append-only history of committed engine snapshots, one row per operation
//! sequence, so a finished run can be replayed state by state.
//!
//! `snapshots.jsonl` rows are either a keyframe, `{"sequence":N,"snapshot":{...}}`,
//! or a delta, `{"sequence":N,"base":M,"patch":[...]}`: an RFC 6902 subset
//! (`add`, `replace`, `remove`) applied to the state at committed sequence `M`.
//! A row is written before its checkpoint, so a row whose sequence the checkpoint
//! never committed is ignored, and a later row for the same sequence replaces it.
use crate::store::read_json;
use anyhow::{bail, Context, Result};
use pa_types::environments::InstanceCheckpoint;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const FILE: &str = "snapshots.jsonl";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum Row {
    Delta {
        sequence: u64,
        base: u64,
        patch: Vec<PatchOp>,
    },
    Keyframe {
        sequence: u64,
        snapshot: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
enum PatchOp {
    Add { path: String, value: Value },
    Replace { path: String, value: Value },
    Remove { path: String },
}

/// Writes one row per committed operation of one instance.
pub(crate) struct HistoryWriter {
    path: PathBuf,
    /// The last sequence this process wrote; only it can be a delta's base.
    last: Option<u64>,
}

impl HistoryWriter {
    pub(crate) fn new(instance: &Path) -> Self {
        Self {
            path: instance.join(FILE),
            last: None,
        }
    }

    /// Discard an incomplete final row left by a crash before appending again.
    fn repair(&self) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        let length = file.metadata()?.len();
        if length == 0 {
            return Ok(());
        }
        let mut last = [0_u8];
        file.seek(SeekFrom::End(-1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            let bytes = fs::read(&self.path)?;
            let complete = bytes
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1);
            file.set_len(complete as u64)?;
            file.sync_all()?;
        }
        Ok(())
    }

    /// Record the snapshot committed at `sequence`. `base` is the previously
    /// committed sequence and its snapshot (sequence 0 means none).
    pub(crate) fn record(
        &mut self,
        base: (u64, &Value),
        sequence: u64,
        snapshot: &Value,
    ) -> Result<()> {
        let keyframe = Row::Keyframe {
            sequence,
            snapshot: snapshot.clone(),
        };
        let row = if base.0 > 0 && self.last == Some(base.0) {
            let mut patch = Vec::new();
            diff(&mut String::new(), base.1, snapshot, &mut patch);
            let delta = Row::Delta {
                sequence,
                base: base.0,
                patch,
            };
            if serde_json::to_vec(&delta)?.len() < serde_json::to_vec(&keyframe)?.len() {
                delta
            } else {
                keyframe
            }
        } else {
            keyframe
        };
        if self.last.take().is_none() {
            self.repair()?;
        }
        append(&self.path, &row)?;
        self.last = Some(sequence);
        Ok(())
    }
}

/// Write a standalone keyframe, for state replaced outside an operation (recovery).
pub(crate) fn record_keyframe(instance: &Path, sequence: u64, snapshot: &Value) -> Result<()> {
    HistoryWriter::new(instance).record((0, &Value::Null), sequence, snapshot)
}

fn append(path: &Path, row: &Row) -> Result<()> {
    let mut bytes = serde_json::to_vec(row)?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reconstruct every committed snapshot in an instance directory, ordered by
/// operation sequence. Sequences without a recorded row (runs started before
/// history existed, or operations settled by recovery) are absent.
pub(crate) fn read(instance: &Path) -> Result<Vec<(u64, Value)>> {
    let checkpoint_path = instance.join("checkpoint.json");
    let history_path = instance.join(FILE);
    if !checkpoint_path.exists() || !history_path.exists() {
        return Ok(Vec::new());
    }
    let checkpoint: InstanceCheckpoint = read_json(&checkpoint_path)?;
    let bytes = fs::read(&history_path)?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let mut states: BTreeMap<u64, Value> = BTreeMap::new();
    for line in bytes[..complete]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        match serde_json::from_slice(line).context("corrupt snapshot history row")? {
            Row::Keyframe { sequence, snapshot } => {
                states.insert(sequence, snapshot);
            }
            Row::Delta {
                sequence,
                base,
                patch,
            } => {
                let mut state = states
                    .get(&base)
                    .with_context(|| format!("snapshot history row {sequence} has no base {base}"))?
                    .clone();
                for op in &patch {
                    apply(&mut state, op)?;
                }
                states.insert(sequence, state);
            }
        }
    }
    states.retain(|sequence, _| *sequence < checkpoint.next_sequence);
    Ok(states.into_iter().collect())
}

/// The operations (`add`, `replace`, `remove`; RFC 6902 subset) that turn `old`
/// into `new`, in the encoding snapshot history uses. Live viewers stream it.
/// # Errors
/// Reports a serialization failure.
pub fn json_patch(old: &Value, new: &Value) -> Result<Value> {
    let mut ops = Vec::new();
    diff(&mut String::new(), old, new, &mut ops);
    Ok(serde_json::to_value(ops)?)
}

fn pointer_push(path: &mut String, token: &str) {
    path.push('/');
    path.push_str(&token.replace('~', "~0").replace('/', "~1"));
}

/// Append the operations that turn `old` into `new`, recursing into objects and
/// into the common prefix of arrays so appends stay small.
fn diff(path: &mut String, old: &Value, new: &Value, ops: &mut Vec<PatchOp>) {
    let length = path.len();
    if let (Value::Object(before), Value::Object(after)) = (old, new) {
        for key in before.keys().filter(|key| !after.contains_key(*key)) {
            pointer_push(path, key);
            ops.push(PatchOp::Remove { path: path.clone() });
            path.truncate(length);
        }
        for (key, value) in after {
            pointer_push(path, key);
            match before.get(key) {
                Some(previous) => diff(path, previous, value, ops),
                None => ops.push(PatchOp::Add {
                    path: path.clone(),
                    value: value.clone(),
                }),
            }
            path.truncate(length);
        }
    } else if let (Value::Array(before), Value::Array(after)) = (old, new) {
        let common = before.len().min(after.len());
        for (index, (previous, value)) in before.iter().zip(after).enumerate() {
            pointer_push(path, &index.to_string());
            diff(path, previous, value, ops);
            path.truncate(length);
        }
        for index in (after.len()..before.len()).rev() {
            pointer_push(path, &index.to_string());
            ops.push(PatchOp::Remove { path: path.clone() });
            path.truncate(length);
        }
        for value in &after[common..] {
            pointer_push(path, "-");
            ops.push(PatchOp::Add {
                path: path.clone(),
                value: value.clone(),
            });
            path.truncate(length);
        }
    } else if old != new {
        ops.push(PatchOp::Replace {
            path: path.clone(),
            value: new.clone(),
        });
    }
}

fn apply(document: &mut Value, op: &PatchOp) -> Result<()> {
    let path = match op {
        PatchOp::Add { path, .. } | PatchOp::Replace { path, .. } | PatchOp::Remove { path } => {
            path
        }
    };
    if path.is_empty() {
        let PatchOp::Replace { value, .. } = op else {
            bail!("snapshot patch can only replace the whole document");
        };
        *document = value.clone();
        return Ok(());
    }
    let mut tokens: Vec<String> = path
        .strip_prefix('/')
        .context("snapshot patch path must start with /")?
        .split('/')
        .map(|token| token.replace("~1", "/").replace("~0", "~"))
        .collect();
    let last = tokens.pop().context("snapshot patch path")?;
    let mut parent = &mut *document;
    for token in &tokens {
        parent = match parent {
            Value::Object(fields) => fields.get_mut(token),
            Value::Array(items) => token
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get_mut(index)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
        }
        .with_context(|| format!("snapshot patch path {path} does not exist"))?;
    }
    match parent {
        Value::Object(fields) => match op {
            PatchOp::Add { value, .. } | PatchOp::Replace { value, .. } => {
                fields.insert(last, value.clone());
            }
            PatchOp::Remove { .. } => {
                fields
                    .shift_remove(&last)
                    .with_context(|| format!("snapshot patch path {path} does not exist"))?;
            }
        },
        Value::Array(items) => {
            let index = if last == "-" {
                items.len()
            } else {
                last.parse()
                    .with_context(|| format!("snapshot patch index {path}"))?
            };
            match op {
                PatchOp::Add { value, .. } if index <= items.len() => {
                    items.insert(index, value.clone());
                }
                PatchOp::Replace { value, .. } if index < items.len() => {
                    items[index] = value.clone();
                }
                PatchOp::Remove { .. } if index < items.len() => {
                    items.remove(index);
                }
                PatchOp::Add { .. } | PatchOp::Replace { .. } | PatchOp::Remove { .. } => {
                    bail!("snapshot patch index {path} is out of range");
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            bail!("snapshot patch path {path} does not name a container");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{apply, diff, HistoryWriter, PatchOp};
    use serde_json::{json, Value};

    fn roundtrip(old: &Value, new: &Value) -> Vec<PatchOp> {
        let mut ops = Vec::new();
        diff(&mut String::new(), old, new, &mut ops);
        let mut rebuilt = old.clone();
        for op in &ops {
            apply(&mut rebuilt, op).unwrap();
        }
        assert_eq!(&rebuilt, new);
        ops
    }

    #[test]
    fn patches_rebuild_the_new_snapshot() {
        let old =
            json!({"phase":"bet","log":[1,2],"seats":{"a/b":{"chips":5},"x~y":1},"gone":true});
        let new = json!({"phase":"river","log":[1,2,3,{"v":4}],"seats":{"a/b":{"chips":7},"x~y":1},"added":null});
        assert_eq!(
            roundtrip(&old, &new),
            vec![
                PatchOp::Remove {
                    path: "/gone".into()
                },
                PatchOp::Replace {
                    path: "/phase".into(),
                    value: json!("river")
                },
                PatchOp::Add {
                    path: "/log/-".into(),
                    value: json!(3)
                },
                PatchOp::Add {
                    path: "/log/-".into(),
                    value: json!({"v":4})
                },
                PatchOp::Replace {
                    path: "/seats/a~1b/chips".into(),
                    value: json!(7)
                },
                PatchOp::Add {
                    path: "/added".into(),
                    value: Value::Null
                },
            ]
        );
        roundtrip(&json!([1, 2, 3, 4]), &json!([9, 2]));
        roundtrip(&json!({"a":[1]}), &json!(["root", "changed"]));
        roundtrip(&json!({"a":{"b":1}}), &json!({"a":[1]}));
    }

    #[test]
    fn rows_after_the_committed_sequence_are_ignored_and_later_rows_win() {
        let directory = tempfile::tempdir().unwrap();
        let checkpoint = |next: u64| {
            std::fs::write(
                directory.path().join("checkpoint.json"),
                json!({"protocol_version":2,"snapshot":{},"receipts":{},"next_sequence":next})
                    .to_string(),
            )
            .unwrap();
        };
        let mut writer = HistoryWriter::new(directory.path());
        writer
            .record((0, &Value::Null), 1, &json!({"n":1}))
            .unwrap();
        writer
            .record((1, &json!({"n":1})), 2, &json!({"n":2}))
            .unwrap();
        // Sequence 3 was written but its checkpoint never committed.
        writer
            .record((2, &json!({"n":2})), 3, &json!({"n":-1}))
            .unwrap();
        checkpoint(3);
        assert_eq!(
            super::read(directory.path()).unwrap(),
            vec![(1, json!({"n":1})), (2, json!({"n":2}))]
        );
        // A later process commits sequence 3 again from the committed state.
        let mut writer = HistoryWriter::new(directory.path());
        writer
            .record((2, &json!({"n":2})), 3, &json!({"n":3}))
            .unwrap();
        writer
            .record((3, &json!({"n":3})), 4, &json!({"n":4}))
            .unwrap();
        checkpoint(5);
        assert_eq!(
            super::read(directory.path()).unwrap(),
            vec![
                (1, json!({"n":1})),
                (2, json!({"n":2})),
                (3, json!({"n":3})),
                (4, json!({"n":4}))
            ]
        );
    }

    #[test]
    fn an_incomplete_final_row_is_discarded_before_appending() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = HistoryWriter::new(directory.path());
        writer
            .record((0, &Value::Null), 1, &json!({"n":1}))
            .unwrap();
        let path = directory.path().join(super::FILE);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(br#"{"sequence":2,"snap"#);
        std::fs::write(&path, bytes).unwrap();
        let mut writer = HistoryWriter::new(directory.path());
        writer
            .record((1, &json!({"n":1})), 2, &json!({"n":2}))
            .unwrap();
        std::fs::write(
            directory.path().join("checkpoint.json"),
            json!({"protocol_version":2,"snapshot":{},"receipts":{},"next_sequence":3}).to_string(),
        )
        .unwrap();
        assert_eq!(
            super::read(directory.path()).unwrap(),
            vec![(1, json!({"n":1})), (2, json!({"n":2}))]
        );
    }
}
