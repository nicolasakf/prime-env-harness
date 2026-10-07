//! Regression for animation wakeups on a quiet daemon link, using the real terminal renderer.
#![cfg(unix)]

use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::pty::{openpty, Winsize};
use pa_tui::interactive::{
    run_interactive, InteractiveOptions, ModelSelection, SessionSelection, UiMode,
};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct TerminalChild {
    child: Child,
    master: Option<std::fs::File>,
}

impl Drop for TerminalChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        self.master.take();
        let _ = self.child.wait();
    }
}

/// This entry runs only in the isolated PTY subprocess; normal test runs skip its body.
#[test]
fn terminal_child() {
    let Some(socket) = std::env::var_os("PA_SPINNER_TEST_SOCKET") else {
        return;
    };
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_interactive(
            InteractiveOptions {
                models: None,
                socket_path: PathBuf::from(socket),
                cwd: PathBuf::from("/tmp"),
                session_dir: None,
                script_path: None,
                model_selection: ModelSelection::default(),
                model_catalog: Vec::new(),
                model_configured_providers: std::collections::HashSet::default(),
                model_recent_models: Vec::new(),
                default_thinking_level: None,
                no_session: false,
                session: SessionSelection::New,
                initial_message: None,
                show_images: true,
                fullscreen_mouse: false,
                theme: "prime".to_string(),
                code_block_indent: "  ".to_string(),
                tree_filter_mode: String::new(),
                branch_summary_skip_prompt: false,
                version: "0.0.0".to_string(),
                onboarding: None,
                telemetry_disabled: None,
                client_auth: None,
                traces: None,
                provider_auth: None,
                update_commands: None,
                telemetry: None,
                keybindings: pa_tui::keybindings::KeybindingsManager::new(),
                session_rlm_depth: None,
                prompt_stash: std::sync::Arc::default(),
                session_has_children: false,
                restore_dock_focus: false,
                client_settings: None,
            },
            UiMode::Terminal,
        ))
        .unwrap();
}

// Keep the mock daemon and VT decoding together with their single PTY verifier.
#[allow(clippy::too_many_lines)]
#[test]
fn quiet_turn_keeps_spinner_and_seconds_advancing() {
    const FRAME_END: &[u8] = b"\x1b[?2026l";
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("tui.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        writeln!(
            writer,
            "{}",
            json!({
                "type": "daemon_hello", "protocol": {"name": "prime-agent.daemon", "version": 7},
                "serverCapabilities": [], "clientId": "mock"
            })
        )
        .unwrap();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                return;
            };
            let request: Value = serde_json::from_str(&line).unwrap();
            let command = request["command"]["type"].as_str().unwrap();
            let data = match command {
                "create" => json!({"activeSessionId": "s1", "id": "s1", "sessionId": "sess-1"}),
                "attach" => json!({
                    "activeSessionId": "s1",
                    "snapshot": {
                        "activeSessionId": "s1", "summary": {"id": "s1", "cwd": "/tmp"},
                        "state": {"activeSessionId": "s1", "cwd": "/tmp", "sessionId": "sess-1",
                                  "isStreaming": true, "isCompacting": false},
                        "messages": [], "lastEventSequence": 0
                    }
                }),
                _ => json!({}),
            };
            if writeln!(
                writer,
                "{}",
                json!({
                    "type": "response", "id": request["id"], "command": command,
                    "success": true, "data": data
                })
            )
            .is_err()
            {
                return;
            }
        }
    });
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
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "terminal_child", "--nocapture"])
        .env("PA_SPINNER_TEST_SOCKET", socket)
        .env("TERM", "xterm-256color")
        .env_remove("TMUX")
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()));
    #[cfg(target_os = "linux")]
    let controlling_terminal = libc::TIOCSCTTY;
    #[cfg(not(target_os = "linux"))]
    let controlling_terminal: libc::c_ulong = libc::TIOCSCTTY.into();
    // SAFETY: only async-signal-safe terminal setup runs in the forked child.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 || libc::ioctl(0, controlling_terminal, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    drop(pty.slave);
    fcntl(pty.master.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).unwrap();
    let mut terminal = TerminalChild {
        child,
        master: Some(pty.master.into()),
    };
    let mut screen = vec![vec![' '; 160]; 40];
    let mut screen_row = 0;
    let mut screen_col = 0;
    let mut pending = Vec::new();
    let mut previous_spinner = None;
    let mut changes = 0;
    let mut changes_at_input = None;
    let mut ready_at = None;
    let boot_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut buffer = [0; 16_384];
        match terminal.master.as_mut().unwrap().read(&mut buffer) {
            Ok(count) => pending.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("PTY read: {error}"),
        }
        while let Some(end) = pending
            .windows(FRAME_END.len())
            .position(|bytes| bytes == FRAME_END)
        {
            let frame: Vec<_> = pending.drain(..end + FRAME_END.len()).collect();
            // Decode the renderer's CSI/OSC subset at each synchronized-update release.
            let bytes = std::str::from_utf8(&frame).unwrap();
            let mut chars = bytes.chars();
            while let Some(ch) = chars.next() {
                match ch {
                    '\x1b' => match chars.next() {
                        Some('[') => {
                            let mut params = String::new();
                            for byte in chars.by_ref() {
                                if ('@'..='~').contains(&byte) {
                                    match byte {
                                        'H' => {
                                            let mut coords = params.split(';');
                                            screen_row = coords
                                                .next()
                                                .and_then(|s| s.parse::<usize>().ok())
                                                .unwrap_or(1)
                                                .saturating_sub(1);
                                            screen_col = coords
                                                .next()
                                                .and_then(|s| s.parse::<usize>().ok())
                                                .unwrap_or(1)
                                                .saturating_sub(1);
                                        }
                                        'J' if params == "2" => {
                                            for row in &mut screen {
                                                row.fill(' ');
                                            }
                                        }
                                        _ => {}
                                    }
                                    break;
                                }
                                params.push(byte);
                            }
                        }
                        Some(']') => {
                            let mut escaped = false;
                            for byte in chars.by_ref() {
                                if byte == '\x07' || (escaped && byte == '\\') {
                                    break;
                                }
                                escaped = byte == '\x1b';
                            }
                        }
                        _ => {}
                    },
                    '\r' => screen_col = 0,
                    '\n' => screen_row += 1,
                    ch if !ch.is_control() => {
                        if let Some(cell) = screen
                            .get_mut(screen_row)
                            .and_then(|row| row.get_mut(screen_col))
                        {
                            *cell = ch;
                        }
                        screen_col += 1;
                    }
                    _ => {}
                }
            }
            for row in &screen {
                let line: String = row.iter().collect();
                if let Some(label) = line.find("Waiting") {
                    ready_at.get_or_insert_with(Instant::now);
                    let spinner = line[..label].trim().chars().next().unwrap();
                    if previous_spinner != Some(spinner) {
                        previous_spinner = Some(spinner);
                        changes += 1;
                    }
                    if line.contains("1s") && changes_at_input.is_none() {
                        assert!(changes >= 8, "only {changes} spinner frames before 1s");
                        // A keystroke takes the inline paint path, which also
                        // consumes the frame deadline. Animation must survive it.
                        terminal.master.as_mut().unwrap().write_all(b"x").unwrap();
                        changes_at_input = Some(changes);
                    }
                    if line.contains("2s") {
                        let before_input = changes_at_input.expect("counter passed 1s");
                        assert!(changes - before_input >= 8, "spinner stalled after typing");
                        assert!(
                            screen.iter().any(|row| row
                                .iter()
                                .collect::<String>()
                                .split_whitespace()
                                .eq([">", "x"])),
                            "typed draft missing: {:?}",
                            screen
                                .iter()
                                .map(|row| row.iter().collect::<String>().trim().to_owned())
                                .filter(|line| !line.is_empty())
                                .collect::<Vec<_>>()
                        );
                        drop(terminal);
                        server.join().unwrap();
                        return;
                    }
                }
            }
        }
        assert!(
            terminal.child.try_wait().unwrap().is_none(),
            "terminal child exited"
        );
        if let Some(ready) = ready_at {
            assert!(
                ready.elapsed()
                    < Duration::from_millis(if changes_at_input.is_some() {
                        2600
                    } else {
                        1600
                    }),
                "quiet turn stopped repainting: {changes} spinner frames"
            );
        } else {
            assert!(Instant::now() < boot_deadline, "loader never appeared");
        }
        std::thread::yield_now();
    }
}
