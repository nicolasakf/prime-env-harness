//! Restricted seat tools. Execution and identity are owned by the daemon port.
use super::provider_adapter::{map_thinking_level, model_thinking_level};
use crate::tools::tool_definition::{ExecutionMode, ToolDefinition, ToolExecutionResult};
use anyhow::{Context, Result};
use pa_agent::agent_loop::TransformContextFn;
use pa_agent::stream::StreamFn;
use pa_agent::types::{AgentMessage, AgentTool, ThinkingLevel};
use pa_types::environments::{
    AgentContext, SeatBinding, SeatCall, SeatId, SeatReply, ENVIRONMENT_WAKE_CUSTOM_TYPE,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{future::Future, pin::Pin, sync::Arc};

/// An authenticated, seat-bound connection to the authoritative environment host.
/// Implementations serialize mutations, persist before replying, and never accept
/// identity from model arguments. Dropping a call must not cancel a committed action.
/// Boxed futures keep the port object-safe for the core's injected `Arc<dyn SeatPort>`.
pub trait SeatPort: Send + Sync {
    fn call(&self, call: SeatCall) -> Pin<Box<dyn Future<Output = Result<SeatReply>> + Send + '_>>;
}

/// Binds each tool call to the model request that generated it.
///
/// The environment host refuses a mutation whose version is older than its
/// committed state. The version must be the one behind the state the model was
/// actually shown when it wrote the call, so it is frozen per request by
/// [`seat_context`] and never taken from a seat-wide latest: a state a tool
/// delivers, or a wake queued behind the running turn, only governs the
/// requests built after it, never the remaining calls of the response in flight.
#[derive(Default)]
pub(crate) struct ViewBinding {
    state: std::sync::Mutex<BindingState>,
}

#[derive(Default)]
struct BindingState {
    /// Version behind the request whose tool calls are executing.
    request: u64,
    /// `through_seq` of the latest wake the model's context holds.
    wake: u64,
    /// Latest state a tool delivered, as (wake it followed, its version).
    published: Option<(u64, u64)>,
    /// Model requests sent so far; tells apart the responses that reuse a call id.
    requests: u64,
}

impl ViewBinding {
    fn state(&self) -> std::sync::MutexGuard<'_, BindingState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The version every call of the current response is bound to.
    fn request_version(&self) -> u64 {
        self.state().request
    }

    /// Records a state a tool just delivered to the model; it governs the next request.
    fn publish(&self, version: u64) {
        let mut state = self.state();
        state.published = Some((state.wake, version));
    }

    /// Fixes the version for the request about to be sent from the wake its
    /// context holds (`through_seq`, version) and what tools delivered since.
    fn freeze(&self, wake: Option<(u64, u64)>) {
        let mut state = self.state();
        state.requests += 1;
        let published = state.published;
        match wake {
            Some((through_seq, version)) => {
                state.wake = through_seq;
                state.request = match published {
                    Some((after, published)) if after == through_seq => published,
                    _ => version,
                };
            }
            None => state.request = published.map_or(0, |(_, version)| version),
        }
    }
}

#[derive(Clone)]
enum SeatTool {
    Observe,
    Invoke(String),
    Say,
    Message,
    Python,
}

pub(crate) fn participant_tools(
    binding: &SeatBinding,
    port: &Arc<dyn SeatPort>,
    views: &Arc<ViewBinding>,
) -> Vec<Arc<dyn AgentTool>> {
    let mut definitions = Vec::new();
    // A stateless seat is handed its view on every wake and by every action.
    if binding.agent_context == AgentContext::Conversation {
        definitions.push((
            SeatTool::Observe,
            "observe_environment".to_owned(),
            "Read your seat's current observation.".to_owned(),
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ));
    }
    for action in &binding.actions {
        if action.allowed_roles().contains(&binding.role) {
            definitions.push((
                SeatTool::Invoke(action.name.clone()),
                format!("env_{}", action.name),
                action.description.clone(),
                action.input_schema.clone(),
            ));
        }
    }
    if binding.protocol_version == 2 {
        definitions.push((SeatTool::Say, "say".into(), "Send text through the environment's input handler.".into(), json!({"type":"object","properties":{"text":{"type":"string","maxLength":16384}},"required":["text"],"additionalProperties":false})));
    }
    if binding.direct_messages {
        definitions.push((SeatTool::Message, "message_seat".into(), "Send a private message to another seat; this wakes the recipient.".into(), json!({"type":"object","properties":{"to":{"type":"string","enum":binding.peers},"text":{"type":"string","maxLength":16384}},"required":["to","text"],"additionalProperties":false})));
    }
    if binding.python {
        definitions.push((SeatTool::Python, "ipython".into(), "Run persistent Python for your seat. env.observe(), env.invoke(name, arguments), and env.integration(name, arguments) use your bound identity. env.invoke acts on the state your request saw: once any environment change commits, including one from this cell, later env.invoke calls fail and need a new request.".into(), json!({"type":"object","properties":{"code":{"type":"string"}},"required":["code"],"additionalProperties":false})));
    }
    let stateless = binding.agent_context == AgentContext::Stateless;
    definitions.into_iter().map(|(tool, name, description, parameters)| {
        let port = Arc::clone(port);
        let views = Arc::clone(views);
        super::tool_bridge::bridge_tool(ToolDefinition {
            label: name.clone(), name, description, parameters,
            prompt_snippet: String::new(), execution_mode: Some(ExecutionMode::Sequential), prepare_arguments: None,
            execute: Arc::new(move |id, params, signal, _on_update| {
                let port = Arc::clone(&port); let tool = tool.clone(); let views = Arc::clone(&views);
                // Providers number tool calls per response (`name:0`), so the model's id
                // alone repeats across turns and would replay a cached receipt. The request
                // that wrote the call makes it unique; a retry of the same call keeps it.
                let id = {
                    let state = views.state();
                    let scope = format!("{}:{}:{}", state.wake, state.request, state.requests);
                    format!("tool-{:x}", Sha256::digest(format!("{scope}\0{id}").as_bytes()))
                };
                Box::pin(async move {
                    let text = |key: &str| -> Result<String> { Ok(params.get(key).and_then(Value::as_str).with_context(|| format!("{key} is required"))?.to_owned()) };
                    let view_version = Some(views.request_version());
                    let call = match tool {
                        SeatTool::Observe => SeatCall::Observe,
                        SeatTool::Invoke(action) => SeatCall::Invoke { action, arguments: params, operation_id: id, view_version },
                        SeatTool::Say => SeatCall::Say { text: text("text")?, operation_id: id, view_version },
                        SeatTool::Message => SeatCall::Message { to: SeatId::try_from(text("to")?).map_err(anyhow::Error::msg)?, text: text("text")?, message_id: id },
                        SeatTool::Python => SeatCall::Python { code: text("code")?, operation_id: id, view_version },
                    };
                    let signal = signal.unwrap_or_default();
                    let reply = if let SeatCall::Python { operation_id, .. } = &call {
                        let operation_id = operation_id.clone();
                        tokio::select! {
                            result = port.call(call) => result?,
                            () = signal.cancelled() => {
                                port.call(SeatCall::CancelPython { operation_id }).await?;
                                anyhow::bail!("Python cancelled; reconcile the seat namespace before retrying");
                            }
                        }
                    } else if stateless && matches!(call, SeatCall::Invoke { .. } | SeatCall::Say { .. }) {
                        // A stateless seat cannot re-observe, so a rejection carries the
                        // current state it must re-read before trying again.
                        match port.call(call).await {
                            Ok(reply) => reply,
                            Err(error) => {
                                let state = port.call(SeatCall::AgentView).await.ok();
                                if let Some(version) = state.as_ref().and_then(|reply| reply.view_version) {
                                    views.publish(version);
                                }
                                anyhow::bail!("{error:#}{}", state.map_or_else(String::new, |reply| format!("\nCurrent state: {}", reply.result)));
                            }
                        }
                    } else { port.call(call).await? };
                    // The state this reply delivered governs the next request, not the
                    // rest of this response.
                    if let Some(version) = reply.view_version {
                        views.publish(version);
                    }
                    let text = match reply.view {
                        Some(view) => json!({"result":reply.result,"state":view}).to_string(),
                        None => serde_json::to_string(&SeatReply { view_version: None, stale_view: false, ..reply })?,
                    };
                    Ok(ToolExecutionResult::text(text))
                })
            }),
        })
    }).collect()
}

/// Shapes a seat's context and learns whether this turn is a retry. A stateless
/// seat resends only its latest wake and the turn's own messages (the transcript
/// stays complete on disk). Every seat records the latest wake's retry attempt
/// for the request limits to read.
pub(crate) fn seat_context(
    stateless: bool,
    attempt: Arc<AtomicU64>,
    views: Arc<ViewBinding>,
) -> TransformContextFn {
    Arc::new(move |messages, _signal| {
        let attempt = Arc::clone(&attempt);
        let views = Arc::clone(&views);
        Box::pin(async move {
            let latest = messages.iter().rposition(|message| {
                matches!(message, AgentMessage::Custom(custom)
                    if custom.payload["customType"] == ENVIRONMENT_WAKE_CUSTOM_TYPE)
            });
            let wake = latest.and_then(|index| match &messages[index] {
                AgentMessage::Custom(wake) => Some(wake),
                AgentMessage::Standard(_) => None,
            });
            views.freeze(wake.map(|wake| {
                let details = &wake.payload["details"];
                (
                    details["through_seq"].as_u64().unwrap_or_default(),
                    details["view_version"].as_u64().unwrap_or_default(),
                )
            }));
            if let Some(AgentMessage::Custom(wake)) = latest.map(|index| &messages[index]) {
                attempt.store(
                    wake.payload["details"]["attempt"]
                        .as_u64()
                        .unwrap_or_default(),
                    Ordering::Relaxed,
                );
            }
            let start = if stateless {
                latest.unwrap_or_default()
            } else {
                0
            };
            Ok(messages.into_iter().skip(start).collect())
        })
    })
}

/// Holds each seat request to the run's output cap, thinking included. A retry
/// (the seat timed out or hit its cap without acting) runs with thinking off,
/// so it can only act, not deliberate again.
pub(crate) fn limited_stream(
    inner: StreamFn,
    max_output_tokens: Option<u64>,
    attempt: Arc<AtomicU64>,
) -> StreamFn {
    Arc::new(move |model, context, mut options| {
        if attempt.load(Ordering::Relaxed) > 0 {
            options.reasoning = ThinkingLevel::Off;
        }
        // The cap only needs the API, id and maximum output; the loop's model
        // descriptor does not share pa-ai's wire shape, so build one from those.
        let ai_model = serde_json::from_value::<pa_types::ai::Model>(json!({
            "id": model.id, "name": model.name, "api": model.api, "provider": model.provider,
            "baseUrl": model.base_url, "reasoning": model.reasoning, "input": ["text"],
            "cost": {"input":0,"output":0,"cacheRead":0,"cacheWrite":0},
            "contextWindow": model.context_window, "maxTokens": model.max_tokens,
        }))
        .ok();
        if let (Some(ai_model), Some(cap)) = (ai_model, max_output_tokens) {
            let (max_tokens, level) =
                pa_ai::cap_request_output(&ai_model, model_thinking_level(options.reasoning), cap);
            options.max_tokens = Some(max_tokens);
            options.reasoning = map_thinking_level(level);
        }
        inner(model, context, options)
    })
}

/// Rendered setup configuration beyond this size is withheld rather than truncated mid-value.
const MAX_CONFIG_PROMPT_BYTES: usize = 8 * 1024;

pub(crate) fn participant_prompt(binding: &SeatBinding) -> String {
    let configuration = binding.config.as_ref().map_or_else(String::new, |config| {
        let rendered = serde_json::to_string_pretty(config).unwrap_or_default();
        if rendered.len() > MAX_CONFIG_PROMPT_BYTES {
            "Actual configuration for this instance is too large to list here; rely on your observations.\n".to_owned()
        } else {
            format!("Actual configuration for this instance (not definition defaults):\n{rendered}\nUse these values for this run. Environment observations and legality checks remain authoritative; defaults in generic descriptions are not run settings.\n")
        }
    });
    let turns = match binding.agent_context {
        AgentContext::Conversation => "On a wake, inspect its observation and deliveries, act when permitted, and end your turn when waiting.",
        AgentContext::Stateless => "Each wake carries your complete current state and any private messages; you keep no memory of earlier turns, so rely on that state, which includes the history you need. Actions return your updated state, and a rejected action returns the current state: the state is authoritative, so if it shows nothing for you to do, end your turn. An action reported as stale was not executed because the environment changed after the state you acted on; reconsider against the state it returns and act again only if that is still right. Act when permitted and end your turn when waiting.",
    };
    let limits = match (binding.turn_timeout_seconds, binding.max_output_tokens) {
        (0, None) => String::new(),
        (seconds, tokens) => format!(
            " Each turn must reach an action within {seconds} seconds{}; keep any reasoning brief and act. A turn that runs out of time or tokens is retried without thinking.",
            tokens.map_or_else(String::new, |tokens| format!(" and at most {tokens} output tokens, thinking included")),
        ),
    };
    format!("You occupy seat {} (role {}) in {}.\n{configuration}{}\nUse only declared actions and your filtered observations. The environment is authoritative. Replies and thinking are private; publish through say or an environment action. Direct messages are visible only to the recipient. {turns}{limits} Never invent an external wake or spawn another player.\nInstance: {}\nDefinition version: {}\nOther seats: {}\nPython: {}", binding.seat, binding.role, binding.manifest_name, binding.instructions, binding.instance_id, binding.version, binding.peers.iter().map(|seat| binding.peer_roles.get(seat).map_or_else(|| seat.to_string(), |role| format!("{seat} (role {role})"))).collect::<Vec<_>>().join(", "), binding.python)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_agent::abort::AbortSignal;
    use pa_agent::types::{CustomAgentMessage, ToolResultContent};

    fn wake(text: &str) -> AgentMessage {
        AgentMessage::Custom(CustomAgentMessage {
            role: "custom".into(),
            payload: json!({"customType":ENVIRONMENT_WAKE_CUSTOM_TYPE,"content":text}),
        })
    }

    #[tokio::test]
    async fn stateless_context_keeps_the_latest_wake_and_what_follows() {
        let transform = seat_context(true, Arc::new(AtomicU64::new(0)), Arc::default());
        let history = vec![
            wake("first"),
            AgentMessage::user("reply to first"),
            wake("second"),
            AgentMessage::user("reply to second"),
        ];
        assert_eq!(
            transform(history.clone(), AbortSignal::default())
                .await
                .unwrap(),
            history[2..].to_vec()
        );
        let no_wake = vec![AgentMessage::user("before any wake")];
        assert_eq!(
            transform(no_wake.clone(), AbortSignal::default())
                .await
                .unwrap(),
            no_wake
        );
    }

    /// Records every call and replies as a stateless host would.
    struct RecordingPort {
        calls: std::sync::Mutex<Vec<SeatCall>>,
        replies: std::sync::Mutex<std::collections::VecDeque<SeatReply>>,
    }

    impl SeatPort for RecordingPort {
        fn call(
            &self,
            call: SeatCall,
        ) -> Pin<Box<dyn Future<Output = Result<SeatReply>> + Send + '_>> {
            self.calls.lock().unwrap().push(call);
            let reply = self.replies.lock().unwrap().pop_front();
            Box::pin(async move { reply.context("no scripted reply") })
        }
    }

    fn reply(version: u64, stale_view: bool) -> SeatReply {
        SeatReply {
            result: json!({"ok":true}),
            deliveries: Vec::new(),
            view: Some(json!({"version":version})),
            view_version: Some(version),
            stale_view,
        }
    }

    fn versioned_wake(through_seq: u64, version: u64) -> AgentMessage {
        AgentMessage::Custom(CustomAgentMessage {
            role: "custom".into(),
            payload: json!({"customType":ENVIRONMENT_WAKE_CUSTOM_TYPE,"content":"wake","details":{"through_seq":through_seq,"view_version":version}}),
        })
    }

    fn market_binding() -> SeatBinding {
        serde_json::from_value(json!({
            "protocol_version":2,"instance_id":"i","definition_id":"d","version":"v",
            "seat":"player-1","role":"player",
            "actions":[{"name":"buy","description":"","input_schema":{"type":"object"},"output_schema":{"type":"object"},"roles":["player"]}],
            "manifest_name":"m","instructions":"","python":false,"direct_messages":false,"peers":[],
            "agent_context":"stateless"
        }))
        .unwrap()
    }

    async fn call_buy(tool: &Arc<dyn AgentTool>, id: &str) {
        Arc::clone(tool)
            .execute(
                id.to_owned(),
                json!({}),
                AbortSignal::default(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
    }

    /// Every call of one response is bound to the view that response was written from.
    /// What a call delivers, or a wake queued behind it, only governs later requests.
    #[tokio::test]
    async fn tool_calls_are_bound_to_the_request_that_generated_them() {
        let port = Arc::new(RecordingPort {
            calls: std::sync::Mutex::default(),
            replies: std::sync::Mutex::new(
                [reply(7, false), reply(7, true), reply(12, false)].into(),
            ),
        });
        let dyn_port: Arc<dyn SeatPort> = Arc::clone(&port) as Arc<dyn SeatPort>;
        let views = Arc::new(ViewBinding::default());
        let tools = participant_tools(&market_binding(), &dyn_port, &views);
        let buy = tools.iter().find(|tool| tool.name() == "env_buy").unwrap();
        let transform = seat_context(true, Arc::new(AtomicU64::new(0)), Arc::clone(&views));
        let bound = || -> Vec<Option<u64>> {
            port.calls
                .lock()
                .unwrap()
                .iter()
                .map(|call| match call {
                    SeatCall::Invoke { view_version, .. } => *view_version,
                    other => panic!("unexpected {other:?}"),
                })
                .collect()
        };
        // Request 1 sees the wake at version 5. Its response holds two calls.
        transform(vec![versioned_wake(3, 5)], AbortSignal::default())
            .await
            .unwrap();
        call_buy(buy, "first").await;
        call_buy(buy, "second").await;
        assert_eq!(bound(), vec![Some(5), Some(5)]);
        // Request 2 sees what the first call delivered (version 7), not a seat-wide latest.
        transform(vec![versioned_wake(3, 5)], AbortSignal::default())
            .await
            .unwrap();
        call_buy(buy, "third").await;
        assert_eq!(bound(), vec![Some(5), Some(5), Some(7)]);
        // A later wake supersedes what an earlier wake's calls delivered.
        transform(vec![versioned_wake(4, 12)], AbortSignal::default())
            .await
            .unwrap();
        assert_eq!(views.request_version(), 12);
    }

    /// Providers number tool calls per response, so separate turns reuse the same call id.
    /// Each must reach the host as its own operation instead of replaying an earlier receipt.
    #[tokio::test]
    async fn a_call_id_reused_by_a_later_request_is_a_new_operation() {
        let port = Arc::new(RecordingPort {
            calls: std::sync::Mutex::default(),
            replies: std::sync::Mutex::new([reply(5, false), reply(5, false)].into()),
        });
        let dyn_port: Arc<dyn SeatPort> = Arc::clone(&port) as Arc<dyn SeatPort>;
        let views = Arc::new(ViewBinding::default());
        let tools = participant_tools(&market_binding(), &dyn_port, &views);
        let buy = tools.iter().find(|tool| tool.name() == "env_buy").unwrap();
        let transform = seat_context(true, Arc::new(AtomicU64::new(0)), Arc::clone(&views));
        for _ in 0..2 {
            transform(vec![versioned_wake(3, 5)], AbortSignal::default())
                .await
                .unwrap();
            call_buy(buy, "env_buy:0").await;
        }
        let operation_ids: Vec<String> = port
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| match call {
                SeatCall::Invoke { operation_id, .. } => operation_id.clone(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(operation_ids.len(), 2);
        assert_ne!(operation_ids[0], operation_ids[1]);
    }

    /// A refused call hands the model the current state, which governs its next request.
    #[tokio::test]
    async fn a_stale_reply_rebinds_only_the_next_request() {
        let port = Arc::new(RecordingPort {
            calls: std::sync::Mutex::default(),
            replies: std::sync::Mutex::new([reply(9, true)].into()),
        });
        let dyn_port: Arc<dyn SeatPort> = Arc::clone(&port) as Arc<dyn SeatPort>;
        let views = Arc::new(ViewBinding::default());
        let tools = participant_tools(&market_binding(), &dyn_port, &views);
        let transform = seat_context(true, Arc::new(AtomicU64::new(0)), Arc::clone(&views));
        transform(vec![versioned_wake(3, 5)], AbortSignal::default())
            .await
            .unwrap();
        let result = Arc::clone(&tools[0])
            .execute(
                "call".into(),
                json!({}),
                AbortSignal::default(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        let ToolResultContent::Text(text) = &result.content[0] else {
            panic!("text result")
        };
        assert_eq!(
            serde_json::from_str::<Value>(&text.text).unwrap(),
            json!({"result":{"ok":true},"state":{"version":9}})
        );
        assert_eq!(views.request_version(), 5);
        transform(vec![versioned_wake(3, 5)], AbortSignal::default())
            .await
            .unwrap();
        assert_eq!(views.request_version(), 9);
    }

    fn wake_with_attempt(attempt: u64) -> AgentMessage {
        AgentMessage::Custom(CustomAgentMessage {
            role: "custom".into(),
            payload: json!({"customType":ENVIRONMENT_WAKE_CUSTOM_TYPE,"content":"wake","details":{"through_seq":1,"attempt":attempt}}),
        })
    }

    #[tokio::test]
    async fn seat_context_reads_the_retry_attempt_from_the_latest_wake() {
        let attempt = Arc::new(AtomicU64::new(0));
        let transform = seat_context(false, Arc::clone(&attempt), Arc::default());
        let history = vec![
            wake_with_attempt(2),
            AgentMessage::user("reply"),
            wake_with_attempt(1),
        ];
        // A conversation seat keeps everything; only the attempt is learned.
        assert_eq!(
            transform(history.clone(), AbortSignal::default())
                .await
                .unwrap(),
            history
        );
        assert_eq!(attempt.load(Ordering::Relaxed), 1);
        transform(vec![wake_with_attempt(0)], AbortSignal::default())
            .await
            .unwrap();
        assert_eq!(attempt.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn limited_stream_caps_requests_and_retries_without_thinking() {
        use pa_agent::stream::{LlmContext, StreamRequestOptions};
        use pa_agent::types::Model;
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let inner: StreamFn = {
            let sent = Arc::clone(&sent);
            Arc::new(move |_model, _context, options| {
                sent.lock()
                    .unwrap()
                    .push((options.max_tokens, options.reasoning));
                Box::pin(async { Err(anyhow::anyhow!("recorded")) })
            })
        };
        let model: Model = serde_json::from_value(json!({
            "id":"google/gemini-3.8-flash","api":"anthropic-messages","provider":"vercel-ai-gateway",
            "reasoning":true,"contextWindow":1_000_000,"maxTokens":65_536
        }))
        .unwrap();
        let request = |attempt: u64| {
            let stream = limited_stream(
                Arc::clone(&inner),
                Some(8_192),
                Arc::new(AtomicU64::new(attempt)),
            );
            let model = model.clone();
            async move {
                let context = LlmContext {
                    system_prompt: None,
                    messages: Vec::new(),
                    tools: Vec::new(),
                };
                let options = StreamRequestOptions {
                    temperature: None,
                    max_tokens: None,
                    reasoning: ThinkingLevel::Low,
                    session_id: None,
                    service_tier: None,
                    api_key: None,
                    signal: AbortSignal::default(),
                    on_payload: None,
                    on_response: None,
                };
                let _ = stream(model, context, options).await;
            }
        };
        request(0).await;
        request(1).await;
        // Thinking plus answer fit the cap; a retry spends all of it on the answer.
        assert_eq!(
            *sent.lock().unwrap(),
            vec![
                (Some(6_144), ThinkingLevel::Low),
                (Some(8_192), ThinkingLevel::Off)
            ]
        );
    }
}
