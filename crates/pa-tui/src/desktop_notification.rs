//! Desktop notifications for the attached chat (the daemon's
//! `idle_notification` session event). The notice goes out through the
//! terminal's own notification escape where the terminal has one, so the
//! terminal app posts it under its own name and icon; elsewhere on macOS it
//! falls back to `osascript`, which macOS attributes to Script Editor.

/// How the notice reaches the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// `OSC 777 ; notify ; title ; body` (Ghostty, WezTerm): title and body.
    Osc777,
    /// `OSC 9 ; text` (iTerm2): one text field, so the title leads the body.
    Osc9,
    /// No terminal channel: `osascript` on macOS, nothing elsewhere.
    Osascript,
}

/// The terminal facts the route depends on.
#[derive(Default)]
struct TerminalEnvironment<'a> {
    term_program: Option<&'a str>,
    ghostty_resources_dir: Option<&'a str>,
    wezterm_pane: Option<&'a str>,
}

fn route(env: &TerminalEnvironment<'_>) -> Route {
    // Multiplexers set their own TERM_PROGRAM and swallow unknown OSC
    // sequences without passthrough, so they take the fallback.
    if env.ghostty_resources_dir.is_some()
        || env.wezterm_pane.is_some()
        || matches!(env.term_program, Some("ghostty" | "WezTerm"))
    {
        return Route::Osc777;
    }
    if env.term_program == Some("iTerm.app") {
        return Route::Osc9;
    }
    Route::Osascript
}

/// Control characters would end or corrupt the escape sequence.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// The escape sequence for `route`, or `None` when it has no terminal
/// channel.
fn sequence(route: Route, title: &str, body: &str) -> Option<String> {
    match route {
        Route::Osc777 => Some(format!(
            "\x1b]777;notify;{};{}\x07",
            // A `;` in the title would shift the body field.
            sanitize(title).replace(';', ","),
            sanitize(body)
        )),
        Route::Osc9 => Some(format!(
            "\x1b]9;{}: {}\x07",
            sanitize(title),
            sanitize(body)
        )),
        Route::Osascript => None,
    }
}

/// Posts the notice. Does nothing when stdout is not a terminal (headless
/// runs have no desktop user to reach). Errors are a failed terminal write
/// or a failed `osascript` spawn.
pub(crate) fn post(title: &str, body: &str) -> std::io::Result<()> {
    use std::io::Write;
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        return Ok(());
    }
    let term_program = std::env::var("TERM_PROGRAM").ok();
    let ghostty_resources_dir = std::env::var("GHOSTTY_RESOURCES_DIR").ok();
    let wezterm_pane = std::env::var("WEZTERM_PANE").ok();
    let route = route(&TerminalEnvironment {
        term_program: term_program.as_deref(),
        ghostty_resources_dir: ghostty_resources_dir.as_deref(),
        wezterm_pane: wezterm_pane.as_deref(),
    });
    if let Some(sequence) = sequence(route, title, body) {
        let mut out = std::io::stdout();
        out.write_all(sequence.as_bytes())?;
        return out.flush();
    }
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    // The text travels as argv items, so it needs no AppleScript escaping.
    let mut child = tokio::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title (item 2 of argv)",
            "-e",
            "end run",
            "--",
        ])
        .arg(body)
        .arg(title)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false)
        .spawn()?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_terminal_gets_its_own_channel() {
        let routes = [
            route(&TerminalEnvironment {
                term_program: Some("ghostty"),
                ..TerminalEnvironment::default()
            }),
            route(&TerminalEnvironment {
                ghostty_resources_dir: Some("/Applications/Ghostty.app/Contents/Resources"),
                ..TerminalEnvironment::default()
            }),
            route(&TerminalEnvironment {
                wezterm_pane: Some("0"),
                ..TerminalEnvironment::default()
            }),
            route(&TerminalEnvironment {
                term_program: Some("iTerm.app"),
                ..TerminalEnvironment::default()
            }),
            route(&TerminalEnvironment {
                term_program: Some("Apple_Terminal"),
                ..TerminalEnvironment::default()
            }),
            route(&TerminalEnvironment {
                term_program: Some("tmux"),
                ..TerminalEnvironment::default()
            }),
        ];
        assert_eq!(
            routes,
            [
                Route::Osc777,
                Route::Osc777,
                Route::Osc777,
                Route::Osc9,
                Route::Osascript,
                Route::Osascript,
            ]
        );
    }

    #[test]
    fn sequences_keep_untrusted_text_inside_their_fields() {
        let title = "Poker; \x1b]0;pwned\x07";
        let body = "Waiting for your review or reply";
        assert_eq!(
            [
                sequence(Route::Osc777, title, body),
                sequence(Route::Osc9, title, body),
                sequence(Route::Osascript, title, body),
            ],
            [
                Some(
                    "\x1b]777;notify;Poker,  ]0,pwned ;Waiting for your review or reply\x07"
                        .to_string()
                ),
                Some("\x1b]9;Poker;  ]0;pwned : Waiting for your review or reply\x07".to_string()),
                None,
            ]
        );
    }
}
