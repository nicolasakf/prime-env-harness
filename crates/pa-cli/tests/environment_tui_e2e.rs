//! The fork binary, real terminal renderer, supervisor, and workers together.
#![cfg(unix)]

use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::pty::{openpty, Winsize};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Terminal {
    child: Child,
    supervisor: Child,
    master: Option<std::fs::File>,
    output: Vec<u8>,
    root: tempfile::TempDir,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // Close the terminal before reaping: macOS may wait for the PTY
        // master to close while destroying a killed controlling process.
        self.master.take();
        let _ = self.child.wait();
        let _ = self.supervisor.kill();
        let _ = self.supervisor.wait();
    }
}

impl Terminal {
    // Fixture seeds, daemon readiness, and controlling-terminal setup stay together.
    #[allow(clippy::too_many_lines)]
    fn start(
        instances: &[pa_types::environments::Instance],
        definitions: &[pa_types::environments::Definition],
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let instances_path = root.path().join("environments/instances");
        std::fs::create_dir_all(instances_path.join("session-artifacts")).unwrap();
        for instance in instances {
            let path = instances_path.join(&instance.id);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(
                path.join("instance.json"),
                serde_json::to_vec(instance).unwrap(),
            )
            .unwrap();
        }
        for definition in definitions {
            let path = root
                .path()
                .join("environments/definitions")
                .join(&definition.id)
                .join(&definition.version);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("definition.json"),
                serde_json::to_vec(definition).unwrap(),
            )
            .unwrap();
        }
        let script = root.path().join("faux.json");
        std::fs::write(
            &script,
            json!({"engine":"faux","responses":["BUILDER-READY","DRAFT-READY"]}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.path().join("settings.json"),
            json!({"onboardingShown":true,"terminal":{"fullscreenMouse":true},"telemetry":{"enabled":false}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.path().join("models.json"),
            json!({"providers":{"environment-test":{"api":"openai-completions","baseUrl":"http://127.0.0.1:9/v1","apiKey":"test-only","models":[{"id":"env-test","name":"Env Test Model","contextWindow":128_000,"maxTokens":4096}]}}}).to_string(),
        )
        .unwrap();
        let mut supervisor = Command::new(env!("CARGO_BIN_EXE_prime-env-harness"))
            .arg("--data-dir")
            .arg(root.path())
            .args(["--mode", "daemon"])
            .env("PRIME_AGENT_TELEMETRY", "0")
            .env(
                "PRIME_AGENT_INTERNAL_DAEMON_WORKER_SUPERVISOR_LOST_EXIT_MS",
                "1000",
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while std::os::unix::net::UnixStream::connect(root.path().join("daemon.sock")).is_err() {
            assert!(supervisor.try_wait().unwrap().is_none());
            assert!(Instant::now() < deadline, "supervisor did not become ready");
            std::thread::yield_now();
        }
        let pty = openpty(
            Some(&Winsize {
                ws_row: 40,
                ws_col: 160,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_prime-env-harness"));
        command
            .arg("--data-dir")
            .arg(root.path())
            .args(["--no-tools", "builder readiness probe"])
            .current_dir(root.path())
            .env("PRIME_AGENT_FAUX_SCRIPT", script)
            .env("PRIME_AGENT_TELEMETRY", "0")
            .env("TERM", "xterm-256color")
            .env_remove("TMUX")
            .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
            .stdout(Stdio::from(pty.slave.try_clone().unwrap()))
            .stderr(Stdio::from(pty.slave.try_clone().unwrap()));
        #[cfg(target_os = "linux")]
        let controlling_terminal = libc::TIOCSCTTY;
        #[cfg(not(target_os = "linux"))]
        let controlling_terminal: libc::c_ulong = libc::TIOCSCTTY.into();
        // SAFETY: only async-signal-safe terminal setup runs after fork.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 || libc::ioctl(0, controlling_terminal, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        fcntl(pty.master.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).unwrap();
        Self {
            child,
            supervisor,
            master: Some(pty.master.into()),
            output: Vec::new(),
            root,
        }
    }

    fn wait(&mut self, mark: usize, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let mut bytes = [0; 16_384];
            match self.master.as_mut().unwrap().read(&mut bytes) {
                Ok(count) => self.output.extend_from_slice(&bytes[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("terminal read: {error}"),
            }
            if self.screen(mark).1.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "terminal never rendered {needle:?}: {}",
                self.screen(0)
                    .0
                    .iter()
                    .map(|row| row.iter().collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "UI exited: {}",
                String::from_utf8_lossy(&self.output)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn submit(&mut self, message: &str, ready: &str) {
        let mark = self.output.len();
        self.input(message);
        self.input("\r");
        self.wait(mark, ready);
    }

    fn input(&mut self, text: &str) {
        self.master
            .as_mut()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    fn screen(&self, since: usize) -> (Vec<Vec<char>>, String) {
        // Decode the renderer's CSI/OSC subset to click the actual painted button.
        let mut screen = vec![vec![' '; 160]; 40];
        let (mut row, mut col) = (0usize, 0usize);
        let text = String::from_utf8_lossy(&self.output);
        let mut chars = text.char_indices();
        let mut painted = String::new();
        while let Some((offset, ch)) = chars.next() {
            match ch {
                '\x1b' => match chars.next() {
                    Some((_, '[')) => {
                        let mut params = String::new();
                        for (_, byte) in chars.by_ref() {
                            if ('@'..='~').contains(&byte) {
                                match byte {
                                    'H' | 'f' => {
                                        let mut coords = params.split(';');
                                        row = coords
                                            .next()
                                            .and_then(|s| s.parse::<usize>().ok())
                                            .unwrap_or(1)
                                            .saturating_sub(1);
                                        col = coords
                                            .next()
                                            .and_then(|s| s.parse::<usize>().ok())
                                            .unwrap_or(1)
                                            .saturating_sub(1);
                                    }
                                    'l' if params == "?2026" && offset >= since => {
                                        for line in &screen {
                                            painted.extend(line.iter());
                                            painted.push('\n');
                                        }
                                    }
                                    'J' if params == "2" => {
                                        for line in &mut screen {
                                            line.fill(' ');
                                        }
                                    }
                                    'K' => {
                                        if let Some(line) = screen.get_mut(row) {
                                            line.iter_mut().skip(col).for_each(|cell| *cell = ' ');
                                        }
                                    }
                                    _ => {}
                                }
                                break;
                            }
                            params.push(byte);
                        }
                    }
                    Some((_, ']')) => {
                        while let Some((_, byte)) = chars.next() {
                            if byte == '\x07'
                                || (byte == '\x1b' && chars.next().map(|(_, ch)| ch) == Some('\\'))
                            {
                                break;
                            }
                        }
                    }
                    Some(_) | None => {}
                },
                '\r' => col = 0,
                '\n' => row += 1,
                _ => {
                    if let Some(cell) = screen.get_mut(row).and_then(|line| line.get_mut(col)) {
                        *cell = ch;
                    }
                    col += 1;
                }
            }
        }
        (screen, painted)
    }

    fn click(&mut self, label: &str, ready: &str) {
        let screen = self.screen(0).0;
        let (row, col) = screen
            .iter()
            .enumerate()
            .find_map(|(row, line)| {
                let text: String = line.iter().collect();
                text.find(label).map(|col| (row + 1, col + 2))
            })
            .unwrap_or_else(|| panic!("button {label:?} missing from screen: {screen:?}"));
        let mark = self.output.len();
        self.input(&format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m"));
        self.wait(mark, ready);
    }
}

fn only_directory(path: &Path) -> PathBuf {
    let dirs: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|name| name != "session-artifacts")
        })
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.into_iter().next().unwrap()
}

#[test]
fn tutorial_hint_and_command_render_in_the_harness() {
    let mut terminal = Terminal::start(&[], &[]);
    terminal.wait(0, "Type /tutorial");
    terminal.wait(0, "BUILDER-READY");
    terminal.submit("/tutorial", "Show this tutorial again");
}

#[test]
fn typing_resume_space_shows_timestamped_instances_newest_first() {
    let instances: Vec<pa_types::environments::Instance> = serde_json::from_value(json!([
        {"id":"picker-older","definition_id":"cards","version":"v1","policy":{"backend":"local"},"created_at":1_759_305_600_000u64},
        {"id":"picker-newest","definition_id":"counter","version":"v2","policy":{"backend":"local"},"created_at":1_759_392_000_000u64}
    ])).unwrap();
    let mut terminal = Terminal::start(&instances, &[]);
    terminal.wait(0, "BUILDER-READY");
    let mark = terminal.output.len();
    terminal.input("/env resume ");
    terminal.wait(mark, "picker-newest");
    terminal.wait(mark, "picker-older");
    terminal.wait(mark, "Created 25/10/2 08:00 UTC");
    let rendered = String::from_utf8_lossy(&terminal.output[mark..]);
    assert!(rendered.find("picker-newest").unwrap() < rendered.find("picker-older").unwrap());
}

#[test]
fn pause_and_stop_pickers_list_only_the_runs_each_can_affect() {
    let instances: Vec<pa_types::environments::Instance> = ["live", "idle", "done"]
        .iter()
        .map(|id| {
            serde_json::from_value(json!({"id":id,"definition_id":format!("{id}-cards"),"version":"v1","policy":{"backend":"local"},"created_at":1_759_305_600_000u64})).unwrap()
        })
        .collect();
    let mut terminal = Terminal::start(&instances, &[]);
    terminal.wait(0, "BUILDER-READY");
    for (id, phase) in [
        ("live", "running"),
        ("idle", "paused"),
        ("done", "finished"),
    ] {
        std::fs::write(
            terminal.root.path().join("environments/instances").join(id).join("run.json"),
            json!({"phase":phase,"reason":null,"operations":0,"wakes":{},"busy":{},"no_progress_turns":0,"revision":null,"started_at":0,"settled":{}}).to_string(),
        )
        .unwrap();
    }
    let mark = terminal.output.len();
    terminal.input("/env pause ");
    terminal.wait(mark, "live-cards");
    let rendered = String::from_utf8_lossy(&terminal.output[mark..]).into_owned();
    assert!(
        !rendered.contains("idle-cards") && !rendered.contains("done-cards"),
        "{rendered}"
    );
    terminal.input("\u{15}");
    let mark = terminal.output.len();
    terminal.input("/env stop ");
    terminal.wait(mark, "idle-cards");
    let rendered = String::from_utf8_lossy(&terminal.output[mark..]).into_owned();
    assert!(
        rendered.contains("live-cards") && !rendered.contains("done-cards"),
        "{rendered}"
    );
    // Run phases move on their own; reopening the list reads the current ones.
    terminal.input("\u{15}");
    std::fs::write(
        terminal.root.path().join("environments/instances/live/run.json"),
        json!({"phase":"finished","reason":null,"operations":0,"wakes":{},"busy":{},"no_progress_turns":0,"revision":null,"started_at":0,"settled":{}}).to_string(),
    )
    .unwrap();
    let mark = terminal.output.len();
    terminal.input("/env stop ");
    terminal.wait(mark, "idle-cards");
    let rendered = String::from_utf8_lossy(&terminal.output[mark..]).into_owned();
    assert!(!rendered.contains("live-cards"), "{rendered}");
}

#[test]
fn the_builder_dock_mounts_the_environments_group() {
    let mut terminal = Terminal::start(&[], &[]);
    terminal.wait(0, "BUILDER-READY");
    terminal.wait(0, "◈ 0 environments");
}

#[test]
fn typing_enter_space_shows_saved_environments_without_submitting() {
    let cards: pa_types::environments::Definition = serde_json::from_value(json!({
        "id":"picker-cards", "version":"v1",
        "manifest":{"protocol_version":1,"name":"Card game","purpose":"test","language":"python","instructions":"test"},
        "validation":{"content_hash":"v1","policy":{"backend":"local"},"passed":true,"diagnostics":[],"actions":[]}
    })).unwrap();
    let mut counter = cards.clone();
    counter.id = "picker-counter".into();
    counter.manifest.name = "Counter".into();
    let mut terminal = Terminal::start(&[], &[counter, cards]);
    terminal.wait(0, "BUILDER-READY");
    let mark = terminal.output.len();
    terminal.input("/env enter ");
    terminal.wait(mark, "picker-cards");
    terminal.wait(mark, "picker-counter");
    terminal.wait(mark, "Card game");
    let rendered = String::from_utf8_lossy(&terminal.output[mark..]);
    assert!(rendered.find("picker-cards").unwrap() < rendered.find("picker-counter").unwrap());
}

#[test]
// Keep the rendered workflow and abrupt disconnect against the same pinned instance.
#[allow(clippy::too_many_lines)]
fn terminal_create_validate_save_enter_form_leave_resume() {
    let mut terminal = Terminal::start(&[], &[]);
    terminal.wait(0, "BUILDER-READY");
    terminal.submit("/env enter", "No saved environments.");
    terminal.submit("/env resume", "No environment sessions to resume.");
    terminal.submit("/env list", "No saved environments.");
    let mark = terminal.output.len();
    terminal.input("/env cr");
    terminal.wait(mark, "Build a new environment draft");
    terminal.input("\u{15}");
    terminal.submit("/env create private card hands", "DRAFT-READY");
    let draft = only_directory(&terminal.root.path().join("environments/drafts"));
    // Deterministic builder output, with no provider calls or paid tokens.
    let manifest_path = std::path::Path::new(&draft).join("environment.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["protocol_version"] = json!(1);
    manifest.as_object_mut().unwrap().remove("roles");
    std::fs::write(manifest_path, manifest.to_string()).unwrap();
    std::fs::write(
        draft.join("environment.py"),
        include_str!("../../pa-environments/fixtures/cards/environment.py").replace(
            "        return {}\n\n    def describe_actions",
            "        self.state[\"setup\"] = params\n        return {}\n\n    def describe_actions",
        ),
    )
    .unwrap();
    std::fs::write(
        draft.join("checks.py"),
        include_str!("../../pa-environments/fixtures/cards/checks.py"),
    )
    .unwrap();
    let manifest_path = draft.join("environment.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["setup_schema"] = json!({"type":"object","properties":{"cards":{"type":"integer","title":"Cards per player","default":5,"minimum":1},"trades":{"type":"integer","title":"Maximum trades","default":2,"minimum":0}},"required":["cards","trades"],"additionalProperties":false});
    std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    terminal.submit(
        &format!("/env policy {} {{\"backend\":\"local\"}}", draft.display()),
        "selected",
    );
    terminal.submit(
        &format!("/env validate {}", draft.display()),
        "Validation passed",
    );
    terminal.submit(
        &format!("/env save {} cards", draft.display()),
        "Saved cards version",
    );
    // Incremental redraws can reuse name characters already on screen.
    terminal.submit("/env list", "cards —");
    terminal.submit("/env catalog", "Environment catalog: ready");
    terminal.submit("/env enter cards ", "Run environment");
    // Select local, then edit both numeric setup parameters without JSON.
    terminal.input(" ");
    terminal.input("\t\u{1b}[A\t\u{1b}[A");
    terminal.submit("", "Cards per player: 6");
    terminal.submit("", "off");
    let instance = only_directory(&terminal.root.path().join("environments/instances"));
    // Left edits a nonempty prompt, including at position zero.
    let mark = terminal.output.len();
    terminal.input("/env sttus");
    terminal.wait(mark, "/env sttus");
    terminal.input("\u{1}\u{1b}[D\u{5}\u{1b}[D\u{1b}[D\u{1b}[Da");
    terminal.submit("", "opponent_count");
    assert!(!String::from_utf8_lossy(&terminal.output[mark..])
        .contains("Environment session policy blocks this shortcut"));
    let mark = terminal.output.len();
    terminal.input("\u{1b}[D");
    terminal.wait(mark, "Use /env leave to return to the builder.");
    // The public host runs no model; model controls belong to private seats.
    terminal.submit("/model", "Environment tables use /env");
    terminal.submit("/env status", "opponent_count");
    assert!(
        !terminal
            .screen(0)
            .0
            .iter()
            .any(|row| row.iter().collect::<String>().contains("faux-1")),
        "a table must not retain the builder's model label"
    );
    terminal.submit(
        "!echo HOST-SHELL-MUST-NOT-RUN",
        "shell shortcuts are unavailable",
    );
    // An accepted input must visibly acknowledge even an unchanged observation.
    terminal.submit("private: PRIVATE-NOTE", "Environment input accepted");
    terminal.submit("hello participant", "Environment input accepted");
    assert!(!String::from_utf8_lossy(&terminal.output).contains("ace_of_spades"));
    terminal.submit("/env help act", "card_id: string (required)");
    terminal.click("[ Actions ]", "Environment actions");
    terminal.click("[ Play card ]", "Card id:");
    terminal.submit("not_owned", "not_owned");
    terminal.click("[ Play card ]", "card is not owned");
    terminal.click("Card id:", "Card id: not_owned");
    terminal.input("\u{15}");
    terminal.submit("seven_of_hearts", "seven_of_hearts");
    terminal.click("[ Play card ]", "Environment act: ready");
    terminal.submit("/env leave", "DRAFT-READY");
    terminal.submit(
        &format!(
            "/env resume {} ",
            instance.file_name().unwrap().to_str().unwrap()
        ),
        "off",
    );
    terminal.submit("/env status", "opponent_count");
    let state: Value =
        serde_json::from_slice(&std::fs::read(instance.join("checkpoint.json")).unwrap()).unwrap();
    assert_eq!(state["snapshot"]["hands"]["human"], json!([]));
    assert_eq!(state["snapshot"]["setup"], json!({"cards":6,"trades":3}));
    assert_eq!(
        state["receipts"]["initialization"]["request"]["params"],
        json!({"cards":6,"trades":3})
    );
    terminal.submit("/env leave", "DRAFT-READY");
    terminal.submit(
        &format!(
            "/env resume {}",
            instance.file_name().unwrap().to_str().unwrap()
        ),
        "off",
    );
    terminal.child.kill().unwrap();
    terminal.master.take();
    terminal.child.wait().unwrap();
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(instance.join("owner.lock"))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while owner.try_lock().is_err() {
        assert!(
            Instant::now() < deadline,
            "disconnect did not shut down instance workers"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(owner);
    let builder: String =
        serde_json::from_slice(&std::fs::read(instance.join("builder-session.json")).unwrap())
            .unwrap();
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
        let (client, _events) = pa_tui::daemon_client::DaemonClient::connect(&terminal.root.path().join("daemon.sock")).await.unwrap();
        let command = serde_json::from_value(json!({"type":"env","activeSessionId":builder,"operation":"resume","parameters":{"id":instance.file_name().unwrap().to_str().unwrap()}})).unwrap();
        let resumed = client.request_ok(command).await.unwrap();
        let id = &resumed["session"]["activeSessionId"];
        let state = client.request_ok(serde_json::from_value(json!({"type":"env","activeSessionId":id,"operation":"status","parameters":{}})).unwrap()).await.unwrap();
        assert_eq!(state["observation"]["hand"], json!([]));
        assert_eq!(state["instance"]["policy"]["backend"], "local");
        client.request_ok(serde_json::from_value(json!({"type":"env","activeSessionId":id,"operation":"leave","parameters":{}})).unwrap()).await.unwrap();
        client.close();
    });
}
