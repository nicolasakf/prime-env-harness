//! Human-facing projections of participant history. The model and durable
//! transcript retain the originals; only data leaving the worker is projected.
use serde_json::{json, Value};

use super::Worker;

pub(super) const PRIVATE_NOTICE: &str = "Private environment response delivered to agent.";

impl Worker {
    pub(super) fn has_environment_participant(&self) -> bool {
        self.agent_engine.as_ref().is_some_and(|engine| {
            !engine.is_environment_host().unwrap_or(false)
                && engine
                    .participant_binding()
                    .map_or(true, |binding| binding.is_some())
        })
    }
}

/// Applies only to a durably bound environment participant, never a builder.
/// Walks nested run/turn events and attach snapshots as well as individual rows.
pub(super) fn project(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(project),
        Value::Object(object) => {
            if matches!(
                object.get("type").and_then(Value::as_str),
                Some("environment_observation" | "environment_update")
            ) {
                return;
            }
            if object.get("firstMessage").is_some_and(Value::is_string) {
                object.insert("firstMessage".into(), json!(PRIVATE_NOTICE));
            }
            let private_row = object
                .get("customType")
                .and_then(Value::as_str)
                .is_some_and(|kind| matches!(kind, "environment_private" | "environment_wake"))
                || matches!(
                    object.get("role").and_then(Value::as_str),
                    Some("user" | "toolResult" | "assistant")
                );
            if private_row {
                // A whitelist prevents details, images, or unknown extension
                // fields from carrying a second copy of private content.
                object.retain(|key, _| {
                    matches!(
                        key.as_str(),
                        "role"
                            | "customType"
                            | "display"
                            | "timestamp"
                            | "toolCallId"
                            | "toolName"
                            | "isError"
                            | "api"
                            | "provider"
                            | "model"
                            | "stopReason"
                            | "usage"
                    )
                });
                if object.get("role").and_then(Value::as_str) == Some("user") {
                    object.insert("role".into(), json!("custom"));
                    object.insert("customType".into(), json!("environment_private"));
                    object.insert("display".into(), json!(true));
                }
                object.insert(
                    "content".into(),
                    if matches!(
                        object.get("role").and_then(Value::as_str),
                        Some("toolResult" | "assistant")
                    ) {
                        json!([{"type":"text","text":PRIVATE_NOTICE}])
                    } else {
                        json!(PRIVATE_NOTICE)
                    },
                );
            }
            let kind = object.get("type").and_then(Value::as_str);
            if kind == Some("tool_execution_end") {
                object.insert(
                    "result".into(),
                    json!({"content":[{"type":"text","text":PRIVATE_NOTICE}],"details":null}),
                );
            } else if kind == Some("tool_execution_update") {
                object.insert(
                    "partialResult".into(),
                    json!({"content":[{"type":"text","text":PRIVATE_NOTICE}],"details":null}),
                );
            } else if kind == Some("tool_execution_start") {
                object.insert("args".into(), json!({}));
            } else if kind == Some("toolCall") {
                object.insert("arguments".into(), json!({}));
            } else if matches!(
                kind,
                Some(
                    "toolcall_delta" | "compaction_summary_delta" | "text_delta" | "thinking_delta"
                )
            ) {
                object.insert("delta".into(), json!(""));
            } else if kind == Some("thinking") {
                object.retain(|key, _| key == "type");
                object.insert("thinking".into(), json!(PRIVATE_NOTICE));
            }
            object.remove("thinkingSignature");
            // Compaction summaries are derived from the private transcript.
            if object.contains_key("summary")
                && (object.contains_key("firstKeptEntryId")
                    || object.get("type").and_then(Value::as_str) == Some("compaction")
                    || object.get("role").and_then(Value::as_str) == Some("compactionSummary"))
            {
                object.retain(|key, _| {
                    matches!(
                        key.as_str(),
                        "role"
                            | "type"
                            | "id"
                            | "timestamp"
                            | "firstKeptEntryId"
                            | "tokensBefore"
                            | "retainedMessageCount"
                            | "parentId"
                    )
                });
                object.insert("summary".into(), json!(PRIVATE_NOTICE));
            }
            object.values_mut().for_each(project);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_content_is_projected_in_live_events_and_nested_history() {
        let private = json!({"role":"custom","customType":"environment_private","content":"ace_of_spades","display":true,"details":{"hand":"ace_of_spades"},"timestamp":1});
        let tool = json!({"role":"toolResult","toolName":"observe_environment","toolCallId":"t1","content":[{"type":"text","text":"ace_of_spades"}],"details":{"secret":"ace_of_spades"},"isError":false});
        let mut frames = json!([
            {"type":"message_end","message":private},
            {"type":"agent_end","messages":[private,tool]},
            {"snapshot":{"messages":[private,tool]}},
            {"type":"tool_execution_end","result":{"content":[{"type":"text","text":"ace_of_spades"}],"details":{"secret":"ace_of_spades"}}},
            {"type":"tool_execution_update","partialResult":{"content":[{"type":"text","text":"ace_of_spades"}]}},
            {"role":"user","content":"legacy Environment message: ace_of_spades"},
            {"role":"compactionSummary","summary":"ace_of_spades","harnessDigest":"ace_of_spades","timestamp":1},
            {"type":"compaction_summary_delta","delta":"ace_of_spades"},
            {"type":"toolcall_delta","delta":"ace_of_spades"},
            {"role":"assistant","content":[{"type":"text","text":"ace_of_spades"},{"type":"thinking","thinking":"ace_of_spades","thinkingSignature":"ace_of_spades"}]},
            {"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"ace_of_spades"}},
            {"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"ace_of_spades"}},
            {"role":"assistant","content":[{"type":"toolCall","arguments":{"card":"ace_of_spades"}}]}
        ]);
        project(&mut frames);
        assert!(!frames.to_string().contains("ace_of_spades"));
        assert_eq!(
            frames[0]["message"],
            json!({"role":"custom","customType":"environment_private","content":PRIVATE_NOTICE,"display":true,"timestamp":1})
        );
        assert_eq!(
            frames[1]["messages"][1],
            json!({"role":"toolResult","toolName":"observe_environment","toolCallId":"t1","content":[{"type":"text","text":PRIVATE_NOTICE}],"isError":false})
        );
        let once = frames.clone();
        project(&mut frames);
        assert_eq!(frames, once);
    }

    #[test]
    fn human_observations_remain_visible() {
        let mut frames = json!([
            {"type":"environment_observation","participant":"human","observation":{"hand":["seven_of_hearts"]}},
            {"type":"environment_update","operation":"act","data":{"result":{"role":"user","content":"public domain result"}}}
        ]);
        let original = frames.clone();
        project(&mut frames);
        assert_eq!(frames, original);
    }
}
