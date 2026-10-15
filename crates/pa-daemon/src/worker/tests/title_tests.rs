//! Conversation titles at the worker surface: `generate_session_title`
//! (the command behind `/name` with no argument) and the one-shot
//! automatic title after the first settled turn. The title model is a
//! models.json provider on a private faux API, so no request leaves the
//! process.
use super::*;
use crate::worker::WorkerConfig;

/// The agent dir the worker reads settings and models from: the title
/// model configured (or not) over a faux-backed provider.
fn agent_dir_with_title_model(dir: &std::path::Path, api: &str, configured: bool) {
    let agent = dir.join("agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::write(
        agent.join("models.json"),
        json!({ "providers": { "titles": {
            "baseUrl": "http://127.0.0.1:9",
            "apiKey": "title-key",
            "api": api,
            "models": [{ "id": "title-1", "name": "Title 1", "contextWindow": 8000 }]
        } } })
        .to_string(),
    )
    .unwrap();
    if configured {
        std::fs::write(
            agent.join("settings.json"),
            json!({ "titleModel": { "provider": "titles", "id": "title-1" } }).to_string(),
        )
        .unwrap();
    }
}

fn register_title_faux(api: &str, reply: &str) -> pa_ai::faux::FauxProviderRegistration {
    let faux = pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
        api: Some(api.to_string()),
        provider: Some("titles".to_string()),
        models: Some(vec![pa_ai::faux::FauxModelDefinition {
            id: "title-1".to_string(),
            ..Default::default()
        }]),
        ..Default::default()
    });
    faux.set_responses(vec![pa_ai::faux::FauxResponseStep::Message(
        pa_ai::faux::faux_assistant_text_message(
            reply,
            pa_ai::faux::FauxAssistantMessageOptions::default(),
        ),
    )]);
    faux
}

fn titled_worker(dir: &std::path::Path, session_id: &str) -> Worker {
    Worker::new(
        WorkerConfig {
            socket_path: dir.join("worker.sock"),
            supervisor_socket_path: PathBuf::new(),
            token: "title-test".into(),
            worker_instance_id: String::new(),
            active_session_id: session_id.into(),
            agent_dir: dir.join("agent"),
            recovery_journal_path: dir.join("recovery.jsonl"),
            telemetry_disabled: Some(true),
            script: Some(json!({ "responses": ["acknowledged"] })),
        },
        None,
    )
}

async fn create_with_session_dir(worker: &Worker, dir: &std::path::Path) {
    let session_dir = dir.join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();
    let created = worker
        .dispatch(
            "create",
            &json!({ "cwd": "/tmp", "sessionDir": session_dir }),
        )
        .await;
    assert!(created.success, "create failed: {created:?}");
}

fn session_name(worker: &Worker) -> Option<String> {
    worker
        .core
        .lock()
        .unwrap()
        .store
        .as_ref()
        .and_then(|store| store.session_name().map(str::to_string))
}

/// Wait for the next `session_info_changed` frame on a subscription.
async fn next_name_event(
    subscription: &mut tokio::sync::broadcast::Receiver<Arc<crate::worker::OutboundFrame>>,
) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let frame = subscription
                .recv()
                .await
                .expect("the event pump stays open");
            if frame.outbound_type != "session_event" {
                continue;
            }
            let outbound: Value = serde_json::from_slice(&frame.payload).unwrap();
            if outbound["event"]["type"] == "session_info_changed" {
                return outbound["event"].clone();
            }
        }
    })
    .await
    .expect("a session_info_changed event")
}

#[tokio::test]
async fn generate_session_title_needs_a_configured_title_model() {
    let dir = tempfile::tempdir().unwrap();
    agent_dir_with_title_model(dir.path(), "faux-title-unconfigured", false);
    let worker = titled_worker(dir.path(), "title-unconfigured");
    create_with_session_dir(&worker, dir.path()).await;

    let response = worker.dispatch("generate_session_title", &json!({})).await;
    assert!(!response.success);
    assert_eq!(
        response.error.as_deref(),
        Some("No conversation title model is configured. Set one in /settings.")
    );
    assert_eq!(session_name(&worker), None);
    worker.dispatch("kill", &json!({})).await;
}

#[tokio::test]
async fn generate_session_title_names_the_session_and_announces_it() {
    let api = "faux-title-command";
    let dir = tempfile::tempdir().unwrap();
    agent_dir_with_title_model(dir.path(), api, true);
    // No turn settles in this test, so the command is the only generation.
    let faux = register_title_faux(api, "Title: \"Debug the flaky build.\"");
    let worker = titled_worker(dir.path(), "title-command");
    create_with_session_dir(&worker, dir.path()).await;
    let mut subscription = worker.events.subscribe();
    {
        let mut core = worker.core.lock().unwrap();
        let store = core.store.as_mut().expect("created store");
        store
            .persist_entry(
                "message",
                json!({ "message": { "role": "user", "content": "the build is flaky", "timestamp": 1 } }),
            )
            .unwrap();
    }

    let response = worker.dispatch("generate_session_title", &json!({})).await;
    assert!(response.success, "{response:?}");
    assert_eq!(
        response.data,
        Some(json!({ "name": "Debug the flaky build" }))
    );
    assert_eq!(
        next_name_event(&mut subscription).await,
        json!({ "type": "session_info_changed", "name": "Debug the flaky build" })
    );
    assert_eq!(
        session_name(&worker).as_deref(),
        Some("Debug the flaky build")
    );
    assert_eq!(faux.call_count(), 1);
    faux.unregister();
    worker.dispatch("kill", &json!({})).await;
}

#[tokio::test]
async fn the_first_settled_turn_titles_the_session_once() {
    let api = "faux-title-auto";
    let dir = tempfile::tempdir().unwrap();
    agent_dir_with_title_model(dir.path(), api, true);
    let faux = register_title_faux(api, "Greet the assistant");
    let worker = titled_worker(dir.path(), "title-auto");
    create_with_session_dir(&worker, dir.path()).await;
    let mut subscription = worker.events.subscribe();

    let prompted = worker
        .dispatch("prompt_and_wait", &json!({ "message": "hello there" }))
        .await;
    assert!(prompted.success, "{prompted:?}");
    assert_eq!(
        next_name_event(&mut subscription).await,
        json!({ "type": "session_info_changed", "name": "Greet the assistant" })
    );
    assert_eq!(
        session_name(&worker).as_deref(),
        Some("Greet the assistant")
    );

    // A second turn never generates again (the named session is settled).
    let again = worker
        .dispatch("prompt_and_wait", &json!({ "message": "and again" }))
        .await;
    assert!(again.success, "{again:?}");
    assert_eq!(faux.call_count(), 1);
    faux.unregister();
    worker.dispatch("kill", &json!({})).await;
}

#[tokio::test]
async fn a_named_session_is_never_retitled() {
    let api = "faux-title-named";
    let dir = tempfile::tempdir().unwrap();
    agent_dir_with_title_model(dir.path(), api, true);
    let faux = register_title_faux(api, "Should not appear");
    let worker = titled_worker(dir.path(), "title-named");
    create_with_session_dir(&worker, dir.path()).await;

    let renamed = worker
        .dispatch("set_session_name", &json!({ "name": "My own name" }))
        .await;
    assert!(renamed.success, "{renamed:?}");
    let prompted = worker
        .dispatch("prompt_and_wait", &json!({ "message": "hello" }))
        .await;
    assert!(prompted.success, "{prompted:?}");
    assert_eq!(session_name(&worker).as_deref(), Some("My own name"));
    assert_eq!(faux.call_count(), 0, "no title call for a named session");
    faux.unregister();
    worker.dispatch("kill", &json!({})).await;
}
