//! Regression: an event arriving between command fragments must not lose input.
use super::*;
use crate::supervisor::SupervisorOptions;
use pa_types::platform::transport::{AsyncReadHalf, AsyncWriteHalf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::net::unix::OwnedReadHalf;
use tokio::sync::watch;

struct ObservedTransport {
    stream: tokio::net::UnixStream,
    consumed: watch::Sender<usize>,
}

struct ObservedRead {
    inner: OwnedReadHalf,
    consumed: watch::Sender<usize>,
}

impl AsyncRead for ObservedRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buffer);
        let added = buffer.filled().len() - before;
        if added > 0 {
            self.consumed.send_modify(|count| *count += added);
        }
        result
    }
}

impl TransportStream for ObservedTransport {
    fn split(self: Box<Self>) -> (Box<dyn AsyncReadHalf>, Box<dyn AsyncWriteHalf>) {
        let (inner, writer) = self.stream.into_split();
        (
            Box::new(ObservedRead {
                inner,
                consumed: self.consumed,
            }),
            Box::new(writer),
        )
    }
}

#[tokio::test]
async fn event_between_command_fragments_preserves_the_entire_command() {
    let root = tempfile::tempdir().unwrap();
    let supervisor = Arc::new(
        Supervisor::new(SupervisorOptions {
            socket_path: root.path().join("daemon.sock"),
            agent_dir: root.path().join("agent"),
        })
        .unwrap(),
    );
    let (server, client) = tokio::net::UnixStream::pair().unwrap();
    let (consumed, mut read_position) = watch::channel(0);
    let connection = {
        let supervisor = Arc::clone(&supervisor);
        tokio::spawn(async move {
            supervisor
                .handle_client(Box::new(ObservedTransport {
                    stream: server,
                    consumed,
                }))
                .await
        })
    };
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    lines.next_line().await.unwrap().unwrap();
    let envelope = serde_json::to_vec(&json!({"type":"command","id":"fragmented","protocol":{"name":"prime-agent.daemon","version":7},"command":{"type":"list"}})).unwrap();
    let split = envelope.len() / 2;
    writer.write_all(&envelope[..split]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while *read_position.borrow_and_update() < split {
            read_position.changed().await.unwrap();
        }
    })
    .await
    .expect("the supervisor must consume the first fragment");
    supervisor
        .events
        .send((
            ClientRouting::Broadcast,
            Arc::new(json!({"type":"fragment_read_marker"})),
        ))
        .unwrap();
    let marker = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(marker.contains("fragment_read_marker"));
    writer.write_all(&envelope[split..]).await.unwrap();
    writer.write_all(b"\n").await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], "fragmented", "{response}");
    assert_eq!(response["success"], true, "{response}");
    writer.shutdown().await.unwrap();
    connection.await.unwrap().unwrap();
}
