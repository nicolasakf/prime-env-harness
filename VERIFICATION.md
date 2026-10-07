# Verification — 2026-10-01

## Current documentation and portable builder guidance — 2026-10-04

Read both source chats and checked the guide against the current implementation.
The new [harness guide](docs/harness.md) covers app identity/data roots, skill
discovery, definitions/instances/session roles, supported participant mixtures,
authoring/schema contracts, durable attention, forms, privacy, execution grants,
Python/integrations, run limits/controls, persistence/recovery, and builder APIs.
README and fork/form docs link to it; the old seats plan is labeled historical.

Both environment skills now include self-contained packaged references. The
authoring skill includes a complete relay manifest, implementation, and checks,
so it no longer needs checkout fixtures to explain coordination. The guide and
skills use the actual host configuration filename environment-integrations.json.
No Rust runtime, dependencies, public APIs, or upstream builder behavior changed
in this documentation task. All changes remain uncommitted.

Passed:

- `python3 scripts/verify_environment_skills.py`: the actual environment binary
  discovers both skills from an unrelated directory, a temporary home directory,
  and a staged package selected through PI_PACKAGE_DIR. Locations are absolute
  and readable; the environments inventory entry is Python-importable; all skill
  references and required package assets are present. Uses temporary data roots,
  isolated subprocess HOME, and no provider calls.
- Both skills pass the skill-creator quick validator using the existing Prime
  kernel Python, which supplies PyYAML. The system and bundled artifact Python
  lack that validator dependency; no packages were installed.
- The packaged relay checks pass on Python 3.14 for 1, 2, 4, and 16 seats:
  sequential wakes, public narration without wakes, illegal/out-of-turn moves,
  monotonic progress, completion, and fresh-object restoration including terminal
  state. Changed Python sources parse successfully.
- `cargo test -j 2 -p pa-daemon --test environment_host_e2e
  builder_kernel_provisions_and_reads_a_run_without_player_tools`: passes using
  the real builder REPL/skill, temporary working directory, deterministic model,
  instance provisioning, and privileged reads. The initial sandboxed attempt
  could not bind its local socket; the permitted local-IPC rerun passed.
- Local documentation/skill links and whitespace were reviewed; `git diff --check`
  passes. The full check passes formatting.

`CARGO_BUILD_JOBS=2 make check` was run once after the additions were ready.
It stops during all-target clippy at the unchanged macOS compile error
`crates/pa-cli/tests/terminal_state_differential_e2e/harness.rs:337`:
`libc::termios` has no field c_line. Workspace tests and release build are not
reached. The focused integration pass does not replace this gate. No dependency
changes were made in this task, so make deny was not rerun.

Scope: documentation plus private verification/example scripts in existing skill
and scripts directories. Environment surfaces remain exempt from upstream TS
parity. Compatibility identifiers PI_PACKAGE_DIR and integration.call remain
unchanged and appear in the guide only to document their real wire behavior.

## Environment-decided wakes and N-seat runs — 2026-10-04

The approved plan is implemented through P5.3. New drafts use protocol v2;
saved v1 definitions retain legacy identities and request/manifest compatibility.
Optional receipt compaction (P5.4) remains deferred. Changes are uncommitted.
The other chat's schema-driven setup and action forms are preserved.

Instances now run in supervised model-less hosts with private supervised agent
seats, capability-authenticated calls, durable coalesced wake inboxes and table
logs. Admission journals precede wake acknowledgements; host cursors advance
after those acknowledgements. Notices, finish and progress metadata commit with
operation receipts and snapshots. Public host events contain only the allowed
human view, or explicitly public events for agents-only spectators.

Validation covers role/roster bounds, per-seat Python namespaces and recovery,
run limits/stalls, pause/resume/stop, direct messages and read-only builder
provisioning/debug APIs. Stop and leave keep completion reports available while
workers settle; Python bridges obey the same host mutation admission as tools.
Telemetry tests inspect emitted feature outcomes and exclude message content.
Local Python still has host-user filesystem access; Docker supplies isolation.

Passed on macOS with deterministic faux providers and no paid model calls:

- `cargo test -j 2 -p pa-environments --features docker-integration -- --test-threads=1`:
  18 tests, including both languages, all four backend/Python combinations,
  notice/finish persistence, malformed-success recovery, partial table append
  repair, legacy resume, seat isolation/cancellation and recovery validation.
- `cargo test -j 2 -p pa-daemon --test environment_host_e2e --test environment_flow_e2e`:
  11 tests. Includes concurrent legacy instances, human plus two agents, agents-only
  relay, simultaneous sealed bids, private text/thinking, restricted seat clients,
  seat/host/supervisor crashes, builder kernel provisioning/reads, and pause/stop
  during an active Python cell.
- `cargo test -j 2 -p pa-daemon --lib environment_`: seven tests, including actual
  emitted telemetry, direct-message replay/rate limits, operation/stall controls,
  policy narrowing, wake coalescing and failed-journal admission.
- `cargo test -j 2 -p pa-cli --test environment_tui_e2e -- --test-threads=1`:
  four real-terminal tests. Covers tutorial and pickers, create/validate/save,
  numeric setup, concise status, model-less table chrome, input acknowledgements,
  action failure/correction, leave/resume and abrupt disconnect.
- `cargo test -j 2 -p pa-tui --lib environment`: 14 tests covering completion,
  human-role filtering, public table rows and the preserved setup/action forms.
- `cargo test -j 2 -p pa-core --test golden_system_prompt`: the updated environment
  skill inventory/prompt matches its checked-in snapshot without update mode.
- Scoped all-target clippy for pa-types, pa-environments, pa-core, pa-daemon and
  pa-telemetry; Docker-feature all-target clippy for pa-environments; library,
  binary and environment-terminal-target clippy for pa-tui/pa-cli, all with
  `-D warnings`. Existing vendored crossterm warnings remain capped upstream.
- Both environment skills pass the skill-creator validator; Python management
  sources pass syntax parsing. Formatting and whitespace checks pass.

Full gates remain blocked. `CARGO_BUILD_JOBS=2 make check` passes formatting,
then stops at unchanged macOS `clippy::cast_lossless` failures for
`TIOCSCTTY as c_ulong`: the first run reported
`crates/pa-cli/tests/slow_drain_exit_guard_e2e.rs:497`; the final run reported
`crates/pa-cli/tests/kitty_release_handoff_e2e.rs:451`.
Its workspace-test and release-build stages were therefore not reached.
`make deny` stops because cargo-deny is not installed. Scoped passes do not
replace these merge gates; no GitHub CI run or release validation is claimed.

Ownership/classification: new public shared seat/role/roster/call/receipt types
belong to pa-types; execution, table logs, temporary inspection and per-seat
Python persistence belong to pa-environments, whose only workspace dependency
is pa-types. Core exports SeatPort/create_seat_session without depending on
pa-environments. Daemon owns provisioning, supervision, authentication and
scheduling. TUI uses shared wire types only. These public APIs are architectural
changes; crate READMEs document their scopes. Environment surfaces are exempt
from upstream TypeScript parity; upstream builder behavior is preserved.

Branding scan found no prohibited names in the new environment runtime, seat,
host, tools, skills or fork documentation. Repository-wide matches include
unchanged upstream user-visible onboarding/README labels, test fixtures/comments,
and wire identifiers. Preserved compatibility identifiers include PI_PACKAGE_DIR,
PI_REQUEST_TIMING, ai.primeintellect namespaces and prime-inference provider IDs.
No unrelated branding sweep was performed.

Checkout: `nicolasakf/prime-env-harness`, branch
`codex/prime-environment-harness`, upstream baseline
`3358e0016bce7cf34a195af58bbd91a26e17d694`. Changes are uncommitted.

Local platform: macOS, Rust/Cargo 1.98.1, Node 24.8.0, Python 3.14, Docker 29.7.2.
No live providers or paid tokens were used. Model tests use Prime's faux provider.

## Passed

- `cargo test -p pa-environments --features docker-integration`: six acceptance
  tests covering both languages, card rules and filtered observations, document
  processing, all four capability/backend combinations, identity binding, denied
  integrations, immutable versions, independent instances, ownership, receipt
  deduplication, uncertain external operations, and Python restoration.
- `cargo test -p pa-environments --test lifecycle`: all three lifecycle tests pass,
  including explicit reconciliation after initialization crashes before the
  first checkpoint. Resume restores the reconciled snapshot without replaying
  initialization.
- `cargo test -p pa-environments --features docker-integration --test cancellation`:
  cancellation and leave during engine actions, Python execution, and Python
  checkpointing; local worker grandchildren stop, unresolved intents block resume.
- Docker isolation was additionally tested with engine-private files, a host
  credential fixture, and fake `OPENAI_API_KEY`/`ANTHROPIC_API_KEY` host values.
  Participant containers cannot read those files, engine source, or the Docker
  socket, and do not inherit provider variables.
- `cargo test -p pa-cli --test environment_tui_e2e -p pa-daemon --test environment_flow_e2e`:
  create → policy → validate → save → enter → act → leave → resume using real
  daemon workers and the terminal renderer. Tests also cover enum forms, rejected
  shell shortcuts, private/public chat, compaction, saved policy, and abrupt
  disconnect followed by resume.
- Focused unit tests in pa-core, pa-tui, and pa-daemon: participant resources/tools,
  enum input validation, and cancellation-safe supervisor command reads.
  The partial-read regression failed before the production fix and passes after it.
- `cargo fmt --all --check`; `git diff --check`.
- Clippy at `-D warnings` for changed production crates, the entire
  pa-environments crate including all test targets/features, and the new daemon
  and terminal integration targets.
- Python syntax compilation for the SDKs, fixtures, and bundled builder skill.
- Node syntax checks and strict TypeScript checks with `erasableSyntaxOnly` for
  the SDK, runner, counter, and document fixture. The existing sibling TypeScript
  compiler was used read-only; that checkout was not changed.
- `make deny`: dependency advisories and licenses pass.
- `cargo build --workspace --release`: the production workspace builds on macOS.
- Fork help and manual-update guidance identify `prime-env-harness`, its isolated
  data directory, `/env` commands, and independent Python/Docker selections.

Vendored crossterm emits six existing compiler warnings about configuration,
parentheses, and dead code. They are outside the changed production code.

## Full workspace gates on macOS

`cargo test --workspace` and `make check` were attempted. Upstream CLI test
targets do not compile on macOS: `libc::prctl`, `PR_SET_PDEATHSIG`, and
`ptsname_r` are unavailable; `libc::termios` has no `c_line` field. Other upstream
terminal tests fail clippy's `cast_lossless` lint for their ioctl request casts.
These test files are unchanged by this fork.

A temporary portability experiment allowed the upstream terminal restoration
test to start, but it stalled during macOS PTY teardown. The test's owned process
was stopped, and the experiment was reverted. The new environment PTY test closes
its master before reaping and passes on this machine.

Full workspace test/clippy success is therefore **not verified** locally.
`.github/workflows/environments.yml` provisions Linux, Node, Python, and Docker,
runs Docker integration tests unconditionally, and runs workspace formatting,
clippy, tests through `make check`, and dependency checks. That workflow has not
been published or run on GitHub.

Local Python and Node have host-user permissions. Broker allowlists restrict host
integration dispatch, not arbitrary local filesystem or network access. Docker
is optional and has no automatic local fallback.

## Private environment responses

The daemon lifecycle regression now runs an actual `observe_environment` tool
call against the card fixture. It checks that live frames, message reads,
compaction, chunked attach, and resumed history omit the agent's card, while the
durable model transcript retains it. The PTY regression checks that the screen
renders an environment delivery notice without exposing the card. Both passed.
Privacy unit tests cover nested run/turn history, streamed tool responses and
arguments, legacy input rows, compaction summaries, and unchanged human output.

Formatting and production workspace clippy passed; clippy also passed for all
targets in pa-core, pa-daemon, pa-environments, and pa-telemetry. The full environment
suite passed. Full workspace library tests additionally found existing failures
in release path canonicalization on macOS and a bash golden fixture with a
commit hash mismatch. `make deny` could not run because cargo-deny is unavailable;
this privacy change adds no dependencies.

## Environment command discovery

`/env` completion now lists commands, argument hints, and editable examples.
`/env act` uses the pinned environment's human-action descriptors; selecting an
action fills the command and a separate submission opens guided input.
`/env help act` explains fields, required/optional status, choices, defaults,
and schema-derived examples. Participant completion never searches host files.
The implementation stays inside pa-tui and uses existing pa-types descriptors;
no dependency or public API was added. Adoption uses the version-2
`agent feature outcome` schema with `environment_command_help` and
`environment_action_picker`, without command or schema content in telemetry.

The 28 autocomplete tests pass, including hierarchical menu transitions,
example insertion, human/agent filtering, and separate selection/submission.
The real terminal create/validate/save/enter/pick/form/leave/resume test and the
daemon environment-flow test pass. Formatting, diff checks, production workspace
clippy, all-target clippy for pa-tui/pa-telemetry, and clippy for the terminal
integration target pass.

Full gates remain unverified: `make check` stops at the unchanged macOS
`termios.c_line` test compilation error, and workspace tests stop at Linux-only
`prctl`/`PR_SET_PDEATHSIG`. The full TUI library run passes 1,299 tests and fails
10 untouched key-label/clipboard fixtures on this host. The environment runtime
suite also fails its existing uncertain-crash recovery test after worker
cancellation returns `Operation not permitted`; that runtime was not modified
for command discovery. No dependencies changed in this change.

## Saved-environment list

`/env list` uses the existing catalog daemon request and renders saved
environment IDs and names, sorted and deduplicated, with an explicit empty-list message.
It appears in completion, `/env help`, and CLI help. `/env catalog` keeps its
detailed JSON output. Both remain builder commands; participant policy is
unchanged. The version-2 `agent feature outcome` schema includes
`environment_list`, emitted after a successful catalog request.

Five focused environment-completion tests pass. The real terminal workflow
checks an empty list, lists the newly saved card environment, and verifies the
catalog command before entering the environment. Production workspace clippy,
formatting, and diff checks pass. `make check` still fails in untouched upstream
macOS terminal tests (`cast_lossless` for an ioctl request); no dependencies or
daemon protocol changes were introduced.

## Stalled agent retry on explicit resume — 2026-10-05

A real championship match stalled after an awaited GLM seat returned a thinking-only
response with no tool call. Its wake was durably settled, so the old resume path
had no pending inbox payload to replay. The game checkpoint still awaited that seat.

The host now appends a fresh targeted status wake on explicit stalled resume for
agent seats in checkpoint progress.waiting_for, only when no pending or unseen
wake exists. The append precedes saving Running; a restart cannot lose the retry.
The engine snapshot, cards, quotes, receipts and P&L are not rewritten or replayed.
Running/paused resumes do not synthesize attention. Saved completion/recovery also
blocks resume before state reconciliation. Existing pending-inbox replay is retained.
No new dependencies, public API, policy grants, automatic retry loop, or telemetry
schema changes; the existing environment_run_control event covers corrected resume.
Ownership is private daemon host control logic; environment-only parity exemption applies.

Focused validation: cargo test -j 2 -p pa-daemon --lib environment_host::tests passed
both tests, including the new consumed-wake regression, no-op repeated resume,
unseen wake, human wait and completion guard. Initial test attempts failed on
fixture API/scaffold checks and were corrected; no live model actions were replayed.
CARGO_BUILD_JOBS=2 make check passed formatting, then stopped at the untouched
macOS cast_lossless lint in crates/pa-cli/tests/kitty_verdict_time_e2e.rs:739.
Workspace tests and release-build stages were not reached. Scoped daemon lint
(cargo clippy -j 2 -p pa-daemon --lib --tests -- -D warnings) passed. The debug
binary rebuild passed (cargo build -j 2 -p pa-cli --bin prime-env-harness).
Live instances have not been resumed or restarted by the builder; deployment
requires user controls. Existing vendored crossterm warnings remain unchanged.
