---
name: daemon-restart-notice
description: After changing code in this harness, tell the user whether the running daemon must be restarted before the change takes effect. Use at the end of any task that edited Rust sources, Cargo files, or the prompt layers under crates/.
---

# Daemon restart notice

The user runs the harness as a long-lived daemon: a supervisor process plus one worker
process per session, under `~/.prime-env-harness`. A rebuild does not replace a daemon
that is already running, so code you edited may not be live until the user restarts it.
Do not restart or kill the daemon yourself; the user may have active sessions. Tell them.

## When to apply

At the end of any task that changed files under `crates/`, `Cargo.toml`, or `Cargo.lock`.
Skip it for documentation-only changes, `.agents/`, and test-only changes.

## How the daemon decides to reuse itself

On launch the client probes the running daemon and replaces it only when the protocol
version or schema ID differs (`DAEMON_PROTOCOL_VERSION` / `DAEMON_SCHEMA_ID` in
`crates/pa-types/src/daemon/mod.rs`). The binary's contents are never compared, so a
rebuilt binary with the same protocol keeps talking to the old daemon.
A stale idle daemon is replaced automatically; a busy one makes the client refuse and
ask for `shutdown --force`.

## Classify the change

| Changed code | Restart needed? |
| --- | --- |
| `pa-tui`, `pa-cli` (flags, commands, print mode) | No. The client process is relaunched by `cargo run`. |
| `pa-daemon` supervisor, routing, `protocol.rs`, the `/env` daemon extension | **Yes.** The supervisor already has the old code loaded. |
| `pa-core` engine/tools/prompts, `pa-environments`, `pa-daemon` worker code | **Yes for existing sessions.** Their workers keep the old code; a new session after the rebuild should pick up the new binary. Recommend a restart to be certain. |
| `pa-types` wire types, or a bumped protocol/schema constant | Handled by the client when the daemon is idle; if refused as busy, restart. |
| `Cargo.toml` / `Cargo.lock` | Treat as the crates that depend on the change. |

When a change spans several rows, report the strictest one.

## What to tell the user

End your final message with one line stating the outcome and the reason, for example:

- `Daemon restart needed: yes. I changed the worker turn loop in pa-daemon, which existing sessions have already loaded.`
- `Daemon restart needed: no. The change is TUI-only.`

When a restart is needed, include the command in its own code block:

```bash
cargo run -p pa-cli --bin prime-env-harness -- shutdown --force
```

Then relaunch with `cargo run -p pa-cli --bin prime-env-harness`. Mention that
`shutdown --force` stops every session's worker, so unsaved in-flight work is lost; plain
`shutdown` asks for confirmation first.
