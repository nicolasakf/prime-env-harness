# pa-environments

Owns immutable environment definitions, validation, instance state, operation
receipts, and local/Docker language workers. Depends on pa-types only among
workspace crates. Model execution belongs to pa-core; supervision to pa-daemon;
terminal rendering to pa-tui.

Public API: EnvironmentStore (including `snapshot_history`), EnvironmentManager, ActiveInstance, Worker, TableLog,
DeliveryCursor, default_roster, validate_roster, and
the host-call callback used by trusted integration adapters.

`EnvironmentManager::enter_with_arguments` accepts human setup parameters;
`enter` retains its existing API with empty input. Optional manifest `setup_schema`
declares their JSON Schema. The manager applies property defaults and validates
before creating an instance, then records parameters in the initialization receipt.
Validation initializes with declared defaults; resume restores the snapshot.
`EnvironmentStore::instance_config` / `ActiveInstance::setup_config` read that persisted
protocol-2 `config` back (never engine state) so the daemon can show every seat the actual
run settings. Configurable environments should declare usable defaults in `setup_schema`
and/or echo their effective interpretation in observations; the host cannot infer
environment-internal defaults.

Worker protocols 1 and 2 use JSON lines. Version 2 receives seat IDs/roles on
initialize, config setup parameters, role-scoped actions and seat observations.
Version 1 preserves the legacy human/agent request identities and old receipts. Definitions implement initialize,
describe_actions, observe, handle_input, invoke, snapshot, restore, and dispose.
`handle_input.agent_message` is private to the agent on the human screen;
`observation` is the human-visible response. The daemon also hides participant
tool responses and arguments. Public action schemas must not list private state.

`missed_turns` (manifest, protocol 2) declares an optional `missed_turn(seat, report)`
method; `ActiveInstance::missed_turn` runs it as a committed operation with a
`MissedTurn` report, and validation checks that the runner defines it. The daemon
decides when a turn was missed, including the `suspended` cause for a seat the
builder took out of play.

`agent_context` (manifest, protocol 2) chooses how an agent seat's model is fed:
`conversation` (default) resends the seat transcript each request; `stateless` sends
only the optional `agent_view(seat)` (default `observe`, at most 32 KiB, checked by
validation and at wake time), the seat's private messages and the turn's own tool
calls. `ActiveInstance::agent_view` reads it; the daemon owns the request shaping.

Committed-state versions: `ActiveInstance::version` is the checkpoint operation
sequence, which advances with every engine operation receipt and nothing else.
`observe_versioned` / `agent_view_versioned` read a view and its version atomically,
and `invoke_guarded` / `input_seat_guarded` take a `ViewGuard`: with
`ViewGuard::Version(v)` the check runs under the mutation lock before any intent is
written or environment code runs. The call is refused with the `StaleView` error, with
nothing executed, only when an operation committed after version `v` changed the engine
snapshot; operations that left it unchanged, such as rejected actions, keep older views
valid. A committed operation ID returns its receipt before the check.
`ViewGuard::Unbound` (the default of `invoke` / `input_seat`) is for builders and tests;
model-driven seats always supply a version. The checkpoint records `progress_sequence`,
the receipt that published the current `progress`, which the host uses to revalidate
actionable notices (`EnvironmentNotice::actionable`).

An environment may request Python execution independently of its recommended
execution backend. Local code has host-user permissions. Docker isolates the
engine and participant kernel in separate containers.

The Python and TypeScript SDKs live under `sdk/`; `fixtures/` contains legacy
counter, private card hands and document examples, plus v2 relay and simultaneous
sealed bids. Stdout carries protocol frames only; stderr carries diagnostics.
Each seat call carries a host-bound identity. JSON Schema checks action structure;
definitions enforce domain rules.

EnvironmentStore stores immutable source/dependency versions and mutable instance
directories separately. Its `list_instances` API reads saved instance metadata
without starting workers, newest first for the resume selector. New instances
persist their creation time in Unix milliseconds. Older metadata uses the file's
creation time, falling back to modification time when creation time is unavailable.
ActiveInstance serializes mutations and owns an exclusive
lock. Intent records precede dispatch, and snapshots contain completion receipts.
Each commit first appends its snapshot to `snapshots.jsonl` (a keyframe, then JSON
Patch deltas against the previous committed sequence); rows past the committed
sequence are ignored. `EnvironmentStore::snapshot_history` reconstructs every
recorded state for replay. This is a new public API and adds no dependency.
`ActiveInstance::frame_versioned` reads the environment's optional
`frame(perspective)` (`ViewPerspective` in pa-types) with its committed-state version,
or `None` when the environment defines none; validation calls it for every perspective
when defined. `json_patch` exposes the history's delta encoding so live viewers can
stream frames the same way. Both are new public API with no dependency.
Repeated operation IDs return recorded results. RecoveryRequest records an
explicitly reconciled snapshot and receipts; it never triggers redispatch.

See [../../docs/reference.md](../../docs/reference.md) for execution grants, local isolation limits,
the participant Python checkpoint limits, and user-facing commands.

Attention metadata is committed with the snapshot and receipt: notify targets,
optional finish summary, and monotonic progress/waiting seats. Failed or malformed
successful responses that may have changed state require explicit reconciliation.
TableLog materializes notices by stable operation-sequence/notice-index IDs,
fsyncs its append order, and repairs an incomplete final line after a crash.
Only explicitly public events appear in spectator reads. The daemon owns model
wake admission, run controls, capabilities, and supervised host/seat processes.

`enter_with_roster` validates roles, min/max, uniqueness, occupant constraints and
limits before creating an instance. Each agent has a separate persisted Python
namespace and receipt directory (v1 retains its original paths). An uncertain
cell blocks that seat until RecoveryRequest supplies inspected receipts and an
explicit namespace reset; other kernels remain usable. Local Python is separate
state with shared host-user filesystem access; Docker provides isolation.

`execute_python_seat_with_host` lets the daemon supply authenticated observe/invoke
admission for Python bridges, so run controls and limits apply inside cells. The
runtime retains ownership of kernel execution, namespace checkpoints, cancellation
evidence and integration policy. This is a new public API and adds no dependency.

`inspect` takes exclusive ownership temporarily, restores a saved snapshot for a
privileged seat observation, and disposes without commits. It permits inspection
during recovery without replaying uncertain operations or starting seat Python.
