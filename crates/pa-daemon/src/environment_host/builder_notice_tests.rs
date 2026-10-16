use super::tests::{waiting_host, MissedTurns};
use super::*;
use crate::protocol::{response_line, response_success};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Serves the supervisor socket the host's link dials and forwards each
/// `send_message` text it receives.
fn builder_inbox(socket: &std::path::Path) -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let listener = tokio::net::UnixListener::bind(socket).unwrap();
    let (messages, inbox) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            writer
                .write_all(b"{\"type\":\"daemon_hello\",\"protocol\":{\"name\":\"prime-agent.daemon\",\"version\":7}}\n")
                .await
                .unwrap();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            messages
                .send(request["command"]["message"].as_str().unwrap().to_owned())
                .unwrap();
            let response = response_line(&response_success(
                request["id"].as_str(),
                "send_message",
                Some(json!({"accepted": true})),
            ));
            writer
                .write_all(serde_json::to_string(&response).unwrap().as_bytes())
                .await
                .unwrap();
            writer.write_all(b"\n").await.unwrap();
        }
    });
    inbox
}

// A pause someone requested is not news to the builder; a pause the run's bounds
// forced is, once per pause.
#[tokio::test]
async fn builder_hears_of_a_bound_pause_but_not_a_requested_one() {
    let root = tempfile::tempdir().unwrap();
    let host = waiting_host(root.path(), RunPhase::Paused, MissedTurns::Host).await;
    let mut inbox = builder_inbox(&root.path().join("unused.sock"));
    std::fs::write(
        host.instance.path().join("builder-session.json"),
        json!("builder-session").to_string(),
    )
    .unwrap();

    host.client_operation("pause", &json!({})).await.unwrap();
    host.drain().await.unwrap();
    assert_eq!(inbox.try_recv().ok(), None);

    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    host.state.lock().await.started_at = 0;
    host.drain().await.unwrap();
    {
        let state = host.state.lock().await;
        assert_eq!(
            (state.phase.clone(), state.reason.as_deref()),
            (RunPhase::Paused, Some("wall-clock limit"))
        );
    }
    assert_eq!(
        inbox.try_recv().ok(),
        Some(format!(
            "Environment {} is Paused: wall-clock limit. Use environments.instance_status and privileged read APIs to inspect it. The run paused itself and stays paused until it is resumed with environments.resume_instance.",
            host.instance.instance.id
        ))
    );
    host.drain().await.unwrap();
    assert_eq!(inbox.try_recv().ok(), None);

    // Resumed and then paused by a requester, the run no longer counts as bound-paused.
    host.client_operation("resume_run", &json!({}))
        .await
        .unwrap();
    host.client_operation("pause", &json!({})).await.unwrap();
    host.drain().await.unwrap();
    assert_eq!(inbox.try_recv().ok(), None);
    host.instance.leave().await.unwrap();
}
