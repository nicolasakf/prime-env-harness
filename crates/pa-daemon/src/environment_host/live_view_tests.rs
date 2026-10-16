//! Live views serve the saved viewer and stream only the frames a perspective may see.
use super::*;
use pa_types::environments::{EnvironmentLanguage, ExecutionPolicy};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Roster seats are `human-1` and `agent-1`; each seat's frame adds its own secret.
async fn live_host(root: &std::path::Path) -> Arc<EnvironmentHost> {
    let manager = pa_environments::EnvironmentManager::new(root.into(), None).unwrap();
    let draft = manager
        .store()
        .create_draft(
            "live",
            EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    let environment = r#"class E:
    def initialize(self, params):
        self.count = 0
        self.secrets = {seat["id"]: "secret-" + seat["id"] for seat in params["seats"]}
        return {}
    def describe_actions(self): return [{"name":"tick","description":"tick","input_schema":{"type":"object"},"output_schema":{"type":"object"},"roles":["human"]}]
    def observe(self, participant): return {"count":self.count}
    def frame(self, perspective):
        view = {"count": self.count}
        if perspective["kind"] == "seat":
            view["secret"] = self.secrets[perspective["seat"]]
        return view
    def handle_input(self, participant, text): return {"observation":{}}
    def invoke(self, participant, action, arguments):
        self.count += 1
        return {}
    def snapshot(self): return {"count":self.count,"secrets":self.secrets}
    def restore(self, snapshot):
        self.count = snapshot["count"]
        self.secrets = snapshot["secrets"]
        return {}
    def dispose(self): return {}
environment=E()
"#;
    std::fs::write(draft.join("environment.py"), environment).unwrap();
    std::fs::write(draft.join("checks.py"), "from environment import E\n").unwrap();
    let report = manager
        .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
        .await
        .unwrap();
    assert!(report.passed, "{:?}", report.diagnostics);
    manager.save(draft.to_str().unwrap(), "live").unwrap();
    let definition = manager.store().definition("live", None).unwrap().0;
    let instance = manager
        .enter_with_roster(
            "live",
            None,
            ExecutionPolicy::default(),
            json!({}),
            pa_environments::default_roster(&definition.manifest).unwrap(),
        )
        .await
        .unwrap();
    Arc::new(EnvironmentHost {
        table: TableLog::open(instance.path()).unwrap(),
        state: Mutex::new(serde_json::from_value(json!({"phase":"running","reason":null,"operations":0,"wakes":{},"busy":{},"no_progress_turns":0,"revision":null,"started_at":crate::util::now_ms(),"settled":{}})).unwrap()),
        gate: Mutex::new(()), link: SupervisorLink::new(root.join("unused.sock")),
        builder_token: "b".repeat(64), host_id: "host".into(), telemetry: None,
        defaults: json!({}), seats: Mutex::new(BTreeMap::new()),
        python_cancellations: Mutex::new(BTreeMap::new()), changed: Notify::new(), live: Mutex::new(None),
        announced_phase: std::sync::Mutex::new(None), shutdown: CancellationToken::new(),
        view: Arc::new(|_| Ok(())), instance,
    })
}

fn save_viewer(root: &std::path::Path, version: &str) {
    let viewer = root.join("viewers/live/board");
    std::fs::create_dir_all(&viewer).unwrap();
    std::fs::write(
        viewer.join("viewer.json"),
        json!({"id":"board","title":"Board","compatible_versions":[version],"revision":"v1","shell":true}).to_string(),
    )
    .unwrap();
    std::fs::write(
        viewer.join("index.html"),
        "<!doctype html><html><head><title>t</title></head><body><script src=\"app.js\"></script></body></html>",
    )
    .unwrap();
    std::fs::write(viewer.join("app.js"), "env.onFrame(() => {});").unwrap();
}

async fn get(port: u16, path: &str, host: &str) -> (String, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

/// The next server-sent event's data, read from an open event stream.
async fn next_event(stream: &mut TcpStream, buffer: &mut String) -> serde_json::Value {
    loop {
        if let Some(end) = buffer.find("\n\n") {
            let event: String = buffer.drain(..end + 2).collect();
            if let Some(data) = event.lines().find_map(|line| line.strip_prefix("data: ")) {
                return serde_json::from_str(data).unwrap();
            }
            continue;
        }
        let mut chunk = [0_u8; 4096];
        let read =
            tokio::time::timeout(std::time::Duration::from_secs(20), stream.read(&mut chunk))
                .await
                .expect("live view event")
                .unwrap();
        assert!(read > 0, "event stream closed");
        buffer.push_str(std::str::from_utf8(&chunk[..read]).unwrap());
    }
}

#[tokio::test]
async fn a_live_view_serves_the_viewer_and_streams_its_perspective_only() {
    let root = tempfile::tempdir().unwrap();
    let host = live_host(root.path()).await;
    let missing = host.client_operation("view", &json!({})).await.unwrap_err();
    assert!(
        format!("{missing:#}").contains("no saved viewer"),
        "{missing:#}"
    );
    save_viewer(root.path(), &host.instance.instance.version);
    let agent = json!({"perspective":{"kind":"seat","seat":"agent-1"}});
    assert!(host.client_operation("view", &agent).await.is_err());

    let issued = host.client_operation("view", &json!({})).await.unwrap();
    assert_eq!(
        issued["perspective"],
        json!({"kind":"seat","seat":"human-1"})
    );
    let url = issued["url"].as_str().unwrap();
    let (port, path) = url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.split_once('/'))
        .map(|(port, path)| (port.parse::<u16>().unwrap(), format!("/{path}")))
        .unwrap();
    let local = format!("127.0.0.1:{port}");
    let (status, page) = get(port, &path, &local).await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(page.contains("id=\"env-live-config\"") && page.contains("\"mode\":\"live\""));
    assert_eq!(
        get(port, &format!("{path}app.js"), &local).await,
        ("HTTP/1.1 200 OK".into(), "env.onFrame(() => {});".into())
    );
    for refused in [
        format!("{path}viewer.json"),
        format!("{path}../board/app.js"),
        "/v/unknown/".into(),
    ] {
        assert_eq!(
            get(port, &refused, &local).await.0,
            "HTTP/1.1 404 Not Found"
        );
    }
    assert_eq!(
        get(port, &path, &format!("rebound.example:{port}")).await.0,
        "HTTP/1.1 403 Forbidden"
    );

    let mut events = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    events
        .write_all(format!("GET {path}events HTTP/1.1\r\nHost: {local}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buffer = String::new();
    let first = next_event(&mut events, &mut buffer).await;
    assert_eq!(
        (first["sequence"].clone(), first["state"].clone()),
        (json!(1), json!({"count":0,"secret":"secret-human-1"}))
    );
    let public = host
        .client_operation("view", &json!({"perspective":{"kind":"public"}}))
        .await
        .unwrap();
    assert_eq!(public["perspective"], json!({"kind":"public"}));
    let public_path = public["url"]
        .as_str()
        .unwrap()
        .split_once(&local)
        .unwrap()
        .1
        .to_owned();
    let mut spectator = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    spectator
        .write_all(format!("GET {public_path}events HTTP/1.1\r\nHost: {local}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let spectated = next_event(&mut spectator, &mut String::new()).await;
    assert_eq!(spectated["state"], json!({"count":0}));

    let human = SeatId::try_from("human-1".to_owned()).unwrap();
    host.instance
        .invoke(
            &human,
            "tick",
            json!({}),
            "tick-1",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let second = next_event(&mut events, &mut buffer).await;
    // A frame this small travels whole: a patch would be larger.
    assert_eq!(
        (
            second["sequence"].clone(),
            second["state"].clone(),
            second["op"]["action"].clone()
        ),
        (
            json!(2),
            json!({"count":1,"secret":"secret-human-1"}),
            json!("tick")
        )
    );
    host.shutdown.cancel();
}
