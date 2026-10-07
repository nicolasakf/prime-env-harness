use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
pub struct Supervisor(pub Child, pub std::path::PathBuf);
impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        for directory in std::fs::read_dir(self.1.join("agent/daemon-workers"))
            .into_iter()
            .flatten()
            .flatten()
        {
            for entry in std::fs::read_dir(directory.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    if let Ok(descriptor) = serde_json::from_slice::<Value>(&bytes) {
                        if let Some(pid) = descriptor["pid"]
                            .as_u64()
                            .and_then(|pid| u32::try_from(pid).ok())
                        {
                            if pa_daemon::protocol::process_start_id(pid).as_deref()
                                == descriptor["processStartId"].as_str()
                                && descriptor["processStartId"].is_string()
                            {
                                let _ = Command::new("kill")
                                    .arg("-KILL")
                                    .arg(pid.to_string())
                                    .stderr(Stdio::null())
                                    .status();
                            }
                        }
                    }
                }
            }
        }
    }
}

pub struct Client {
    root: std::path::PathBuf,
    frames: std::sync::mpsc::Receiver<Value>,
    writer: UnixStream,
    next: usize,
    pub events: Vec<Value>,
}

impl Client {
    pub fn request(&mut self, command: &Value) -> Value {
        self.next += 1;
        let id = self.next.to_string();
        writeln!(self.writer, "{}", json!({"type":"command","id":id,"protocol":{"name":"prime-agent.daemon","version":7},"command":command})).unwrap();
        loop {
            let frame = self.frames.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|error| {
                for instance in std::fs::read_dir(self.root.join("agent/environments/instances")).into_iter().flatten().flatten() {
                    let path = instance.path();
                    let checkpoint: Value = std::fs::read(path.join("checkpoint.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null);
                    let receipts = checkpoint["receipts"].as_object().into_iter().flat_map(|receipts| receipts.keys()).collect::<Vec<_>>();
                    let intents = std::fs::read_dir(path.join("operations")).into_iter().flatten().flatten().map(|entry| entry.file_name()).collect::<Vec<_>>();
                    eprintln!("operation receipts: {receipts:?}; intents: {intents:?}");
                }
                writeln!(self.writer, "{}", json!({"type":"command","id":"diagnostic-state","protocol":{"name":"prime-agent.daemon","version":7},"command":{"type":"get_connection_state","activeSessionId":command["activeSessionId"]}})).unwrap();
                for _ in 0..10 {
                    if let Ok(frame) = self.frames.recv_timeout(Duration::from_secs(1)) {
                        if frame["id"] == "diagnostic-state" { eprintln!("stalled session state: {frame}"); break; }
                    }
                }
                for entry in std::fs::read_dir(self.root.join("agent/logs")).into_iter().flatten().flatten() {
                    if let Ok(log) = std::fs::read_to_string(entry.path()) { eprintln!("worker log: {log}"); }
                }
                panic!("{} request {id} operation {}: {error}", command["type"], command["operation"])
            });
            if frame["id"] == id {
                return frame;
            }
            if frame["type"] == "session_event" {
                self.events.push(frame["event"].clone());
            }
            if frame["type"] == "roster_update" || frame["type"] == "environment_runs_changed" {
                self.events.push(frame.clone());
            }
        }
    }

    pub fn environment(&mut self, session: &str, operation: &str, parameters: &Value) -> Value {
        let response = self.request(&json!({"type":"env","activeSessionId":session,"operation":operation,"parameters":parameters}));
        assert_eq!(response["success"], true, "{response}");
        response["data"].clone()
    }
}

pub fn start(root: &std::path::Path, script: &Value) -> (Supervisor, Client) {
    let socket = root.join("daemon.sock");
    let agent = root.join("agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::write(root.join("faux.json"), script.to_string()).unwrap();
    let mut supervisor = Supervisor(
        Command::new(env!("CARGO_BIN_EXE_pa-daemon"))
            .args(["supervisor", "--socket"])
            .arg(&socket)
            .arg("--agent-dir")
            .arg(&agent)
            .env_remove("PRIME_API_KEY")
            .env(
                "PRIME_AGENT_INTERNAL_DAEMON_WORKER_SUPERVISOR_LOST_EXIT_MS",
                "1000",
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
        root.to_path_buf(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let stream = loop {
        match UnixStream::connect(&socket) {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    supervisor.0.try_wait().unwrap().is_none(),
                    "supervisor exited: {error}"
                );
                assert!(Instant::now() < deadline, "socket readiness: {error}");
                std::thread::yield_now();
            }
        }
    };
    let writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut hello = String::new();
    reader.read_line(&mut hello).unwrap();
    assert!(hello.contains("environments_v1"));
    let (sender, frames) = std::sync::mpsc::channel();
    std::thread::spawn(move || loop {
        let mut line = String::new();
        if !matches!(reader.read_line(&mut line), Ok(count) if count > 0) {
            break;
        }
        if sender.send(serde_json::from_str(&line).unwrap()).is_err() {
            break;
        }
    });
    (
        supervisor,
        Client {
            root: root.to_path_buf(),
            writer,
            frames,
            next: 0,
            events: Vec::new(),
        },
    )
}

impl Client {
    pub fn wait_event(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        if let Some(event) = self.events.iter().find(|event| predicate(event)) {
            return event.clone();
        }
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let frame = self
                .frames
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    for entry in std::fs::read_dir(self.root.join("agent/logs"))
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
                        if let Ok(log) = std::fs::read_to_string(entry.path()) {
                            eprintln!("worker log: {log}");
                        }
                    }
                    for entry in std::fs::read_dir(self.root.join("agent/environments/instances"))
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
                        if let Ok(run) = std::fs::read_to_string(entry.path().join("run.json")) {
                            eprintln!("run state: {run}");
                        }
                    }
                    panic!("environment event readiness: {error}");
                });
            if frame["type"] == "session_event"
                || frame["type"] == "roster_update"
                || frame["type"] == "environment_runs_changed"
            {
                let event = if frame["type"] == "session_event" {
                    frame["event"].clone()
                } else {
                    frame
                };
                self.events.push(event.clone());
                if predicate(&event) {
                    return event;
                }
            }
        }
    }
}
