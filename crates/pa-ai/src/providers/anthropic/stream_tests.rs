//! Characterization tests for the Anthropic SSE stream, replayed through a
//! local in-process SSE server.

use serde_json::{json, Map};

use crate::event_stream::AssistantMessageEventExt;
use crate::providers::anthropic::{stream_anthropic, AnthropicOptions};
use crate::types::{
    AssistantContent, Context, Model, StopReason, StreamOptions, ThinkingContent, ToolCall,
};

#[tokio::test]
#[allow(clippy::too_many_lines)] // one inline SSE script plus a whole-value snapshot of every event
async fn stream_events_snapshot_current_content() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut sse = [
        r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":1,"output_tokens":2}}}"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"think"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"lookup","input":{}}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"a\":1}"}}"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":1}"#,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
        r#"event: message_stop
data: {"type":"message_stop"}"#,
    ]
    .join("\n\n");
    sse.push_str("\n\n");
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let _ = socket.read(&mut request).await.unwrap();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{sse}",
                    sse.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let model: Model = serde_json::from_value(json!({
        "id": "claude-test", "name": "Claude Test", "api": "anthropic-messages",
        "provider": "anthropic", "baseUrl": format!("http://{addr}"), "reasoning": false,
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 128_000, "maxTokens": 8192
    }))
    .unwrap();
    let options = AnthropicOptions::from_base(StreamOptions {
        api_key: Some("sk-ant-api-test".into()),
        ..Default::default()
    });
    let mut reader = stream_anthropic(
        &model,
        &Context {
            system_prompt: None,
            messages: vec![],
            tools: None,
        },
        Some(&options),
    );
    let mut events = Vec::new();
    while let Some(event) = reader.next_event().await {
        events.push(event);
    }

    let thinking_base = ThinkingContent {
        thinking: String::new(),
        thinking_signature: Some(String::new()),
        redacted: None,
        rest: Map::default(),
    };
    let thinking_delta = AssistantContent::Thinking(ThinkingContent {
        thinking: "think".into(),
        ..thinking_base.clone()
    });
    let thinking_signed = AssistantContent::Thinking(ThinkingContent {
        thinking: "think".into(),
        thinking_signature: Some("sig".into()),
        ..thinking_base.clone()
    });
    let tool_call_base = ToolCall {
        id: "t1".into(),
        name: "lookup".into(),
        arguments: Map::default(),
        thought_signature: None,
        rest: Map::default(),
    };
    let tool_call_args = AssistantContent::ToolCall(ToolCall {
        arguments: json!({"a": 1}).as_object().cloned().unwrap(),
        ..tool_call_base.clone()
    });
    assert_eq!(
        events
            .iter()
            .map(|event| (event.event_type(), event.partial().content.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("start", vec![]),
            (
                "thinking_start",
                vec![AssistantContent::Thinking(thinking_base)]
            ),
            ("thinking_delta", vec![thinking_delta]),
            ("thinking_end", vec![thinking_signed.clone()]),
            (
                "toolcall_start",
                vec![
                    thinking_signed.clone(),
                    AssistantContent::ToolCall(tool_call_base),
                ],
            ),
            (
                "toolcall_delta",
                vec![thinking_signed.clone(), tool_call_args.clone()]
            ),
            (
                "toolcall_end",
                vec![thinking_signed.clone(), tool_call_args.clone()]
            ),
            ("done", vec![thinking_signed, tool_call_args]),
        ]
    );
    assert_eq!(
        events.last().unwrap().partial().stop_reason,
        StopReason::ToolUse
    );
}

/// Sends one request through `stream_anthropic` to a local server and returns
/// the JSON body that went over the wire. `host_path` is appended to the base
/// URL so a test can make it look like a gateway endpoint.
async fn wire_request_body(
    host_path: &str,
    compat: Option<serde_json::Value>,
) -> serde_json::Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        let body = loop {
            let read = socket.read(&mut chunk).await.unwrap();
            request.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&request).into_owned();
            let Some((head, body)) = text.split_once("\r\n\r\n") else {
                continue;
            };
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")?
                        .trim()
                        .parse::<usize>()
                        .ok()
                })
                .unwrap();
            if body.len() >= length {
                break body.to_string();
            }
        };
        let sse = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{sse}",
                    sse.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        body
    });
    let mut model: Model = serde_json::from_value(json!({
        "id": "vendor/model", "name": "Model", "api": "anthropic-messages",
        "provider": "vercel-ai-gateway", "baseUrl": format!("http://{addr}{host_path}"),
        "reasoning": false, "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 128_000, "maxTokens": 8192
    }))
    .unwrap();
    model.compat = compat.map(|raw| serde_json::from_value(raw).unwrap());
    let options = AnthropicOptions::from_base(StreamOptions {
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let mut reader = stream_anthropic(&model, &Context::default(), Some(&options));
    while reader.next_event().await.is_some() {}
    serde_json::from_str(&server.await.unwrap()).unwrap()
}

#[tokio::test]
async fn gateway_messages_request_carries_the_only_restriction() {
    let body = wire_request_body(
        "/ai-gateway.vercel.sh",
        Some(json!({
            "supportsLongCacheRetention": false,
            "vercelGatewayRouting": { "only": ["relace"] }
        })),
    )
    .await;
    assert_eq!(
        body["providerOptions"],
        json!({ "gateway": { "only": ["relace"] } })
    );
}

#[tokio::test]
async fn gateway_messages_request_keeps_only_and_order_separate() {
    let body = wire_request_body(
        "/ai-gateway.vercel.sh",
        Some(json!({ "vercelGatewayRouting": { "only": ["a"], "order": ["b", "a"] } })),
    )
    .await;
    assert_eq!(
        body["providerOptions"],
        json!({ "gateway": { "only": ["a"], "order": ["b", "a"] } })
    );
}

#[tokio::test]
async fn non_gateway_messages_request_has_no_provider_options() {
    let routed = wire_request_body(
        "",
        Some(json!({ "vercelGatewayRouting": { "only": ["relace"] } })),
    )
    .await;
    let plain = wire_request_body("", None).await;
    let gateway_without_routing = wire_request_body("/ai-gateway.vercel.sh", None).await;
    assert_eq!(routed, plain);
    assert_eq!(gateway_without_routing, plain);
    assert!(plain.get("providerOptions").is_none());
}
