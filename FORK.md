# Prime Environment Harness

Fork: https://github.com/nicolasakf/prime-env-harness
Upstream: https://github.com/PrimeIntellect-ai/prime-agent
Starting revision: `3358e0016bce7cf34a195af58bbd91a26e17d694`

The [current harness guide](docs/harness.md) documents the implemented workflow,
session roles, participant combinations, attention, forms, policy, persistence,
and diagnostics. Authoring references and a complete relay example ship inside
the environment skills, independently of the launch directory.

Run `cargo run -p pa-cli --bin prime-env-harness -- --data-dir <directory>`.
The default directory is `~/.prime-env-harness`. Its daemon, worker descriptors,
locks, credentials, settings, sessions, definitions, and instances are independent
of ordinary Prime Agent. Existing checkouts remain unchanged.
The retained `prime-agent` binary is the upstream reference surface.

## Local development

From this checkout, launch with:

```sh
cargo run -p pa-cli --bin prime-env-harness
```

Edit the code, exit the app, and rerun the command. Cargo rebuilds automatically;
no separate build step is needed. Run focused tests while iterating. Use `-j 2`
on Cargo commands if you want less compilation parallelism.

Run heavy Cargo commands sequentially. Once a code change is ready, run
`make check` for the full workspace gates, including the optimized release build.
Avoid repeating its individual gates separately. Use
`CARGO_BUILD_JOBS=2 make check` to limit build parallelism. Dependency changes also
require `make deny`; environment and parity verification requirements remain in
[AGENTS.md](AGENTS.md). Documentation-only edits need diff and whitespace review.

Fork update configuration: source repository `nicolasakf/prime-env-harness`,
manual Cargo builds. CLI and TUI upstream installers are disabled in the fork
application until a fork release channel is provisioned.

Local workers require Node 22.20 or newer for TypeScript and Python 3.10 or newer
with venv support. Docker requires a running daemon and permission to pull/build
the versioned runner images. Dependency preparation defaults to 10 minutes;
runtime calls default to 120 seconds.

Start the application, use `/login` to configure a provider, then:

```text
/env create <description>
/env policy <draft-path> {"backend":"local"}
/env validate <draft-path>
/env save <draft-path> <definition-id>
/env enter <definition-id>
/env enter <definition-id> local
/env act <action-name>
/env status
/env leave
/env resume <instance-id>
```

The agent learns these commands from the built-in `env-harness` skill
([skills/env-harness/SKILL.md](skills/env-harness/SKILL.md)), which ships with the
other built-in skills and loads in any working directory, including sessions started
outside this checkout. Source builds fall back to the compiled checkout's skills;
packaged builds use the package's skills tree. For a moved build, keep complete
skill directories, including references, example assets, and the environments
Python package, next to the executable or under `<data-dir>/skills/`.
`python3 scripts/verify_environment_skills.py` checks discovery from unrelated
and home directories and a staged package without provider calls.

Creation supplies the builder a contract, SDK, and behavioral checks. Before
writing code, the builder holds a short design conversation covering rules,
participants, turn flow and wakes, run length and configuration, privacy, agent
context, deadlines and token budgets, failure handling, and analysis. It gives
a recommendation for each, and you can decide or defer. Review its
purpose, actions, dependencies, requested Python capability, recommendation,
grants, and validation report. The builder asks which policy you want, waits for
your answer, and applies it with `environments.select_policy`. You can also use
`/env policy` manually. Once selected, the builder validates and saves automatically
without asking for separate validation approval. Policy selection is required before validation;
changing the manifest requires selecting again. Validation allows the initial
attempt and three repair passes, retains failures, and saving rejects edited or
unsuccessfully validated content. Saving does not enter the environment.

Enter without a mode to choose local/Docker and fill declared setup parameters
in a terminal form. New instances also accept a policy JSON object; existing
instances retain their saved policy. `/env` lists definitions. Inside an instance,
click `[ Actions ]` or use `/env act` to choose an action. `/env act <action>`
opens its form with typed controls and a JSON editor for complex fields. The
same panel handles setup through the optional manifest `setup_schema`; see
[environment forms](docs/environment-forms.md). Human chat passes through the
environment's input handler. Its explicit notices determine which seats wake.
`/env leave` pauses human participation and returns to the builder; spectators
can leave an agents-only run without stopping it.

Humans attach to a model-less table host. Agent observations, actions, Python
responses, tool arguments, assistant text and thinking stay in private seat
sessions; they never enter host frames or history. Human observations and human
action responses remain visible. Legacy participant projection also redacts
private responses, thinking, streamed deltas and compaction summaries. The
builder can inspect private seat data through authenticated read-only APIs.
This is display privacy; a human with local file access can inspect stored state.

Execution capability and isolation are independent: all environments expose
actions; the definition may additionally request persistent Python. Both can
run locally or in Docker. Local generated code has host-user permissions.
Tool allowlists only govern brokered integrations and do not prevent local
Python or Node from accessing host files or networking. Docker is recommended
for Python, never mandatory, and unavailability never causes a local fallback.

These policy objects cover the four combinations, for both `/env policy` and
`/env enter <definition-id> <policy-json>`. Python requires both a definition
request and an explicit grant:

```json
{"backend":"local","python_execution":false}
{"backend":"docker","python_execution":false}
{"backend":"local","python_execution":true}
{"backend":"docker","python_execution":true}
```

Docker engines and participant kernels use separate containers and scratch
mounts. Participant containers receive the action client and kernel runtime,
without engine source or private workspace. Runtime networking is off by default.
Dependency preparation is a separate network-enabled step. Images use non-root
users, no capabilities, a read-only filesystem, two CPUs, and 1 GiB by default.
Policy JSON accepts networking, explicit mounts, named secrets, integrations,
call/preparation timeouts, CPUs, and memory. Secrets resolve from
`PRIME_ENV_SECRET_<name>` on the host; provider credentials are never inherited.

Trusted integrations are defined in `<data-dir>/environment-integrations.json`, outside
generated definitions. Each entry specifies an absolute executable, arguments,
input/output JSON schemas, and optional named-secret mappings. The executable
receives one JSON request on stdin and returns one JSON value on stdout. Grant
its name with `"integrations":["lookup"]` in the instance policy. The host binds
the participant and instance; environment-supplied identity cannot override them.
Adapter secrets resolve from `PRIME_ENV_INTEGRATION_SECRET_<name>` and remain on
the host. Direct actions and participant Python use the same dispatcher.

Definitions are immutable content-addressed versions. Instances pin a version
and policy, with separate engine checkpoints, participant scratch/Python state,
and conversation history. Intents are fsynced before actions. Completed receipts
and resulting snapshots are committed together. Interrupted operations block
resume until a user explicitly reconciles the outcome; they are never replayed.

Use `/env recover <instance-id> <recovery-json>` only after inspecting the saved
state and any external receipts. The recovery object contains `snapshot`,
`receipts`, `python_receipts`, `reset_python`, and a nonempty `evidence` string.
Every receipt must match its unresolved intent. V2 recovery additionally accepts
`seat_python_receipts` and `reset_python_seats`; an uncertain Python execution
requires an explicit reset for that seat. Recovery records the supplied results
without running initialization or previous actions.

Python checkpoints preserve pickle-compatible variables in the chosen backend.
Modules, open files, and other unpickleable objects are omitted and reported;
individual values above 16 MiB are omitted and total snapshots are limited to
256 MiB. Scratch files and conversation history remain separate from engine
state. The environment's snapshot is authoritative.

Verification uses deterministic providers without live provider calls. The
environment suite exercises both languages, all four execution combinations,
private card observations, document processing, identity/grant enforcement,
Python restoration, ownership, cancellation, crash recovery, and Docker
isolation. Daemon and real PTY tests cover the full workflow, private inputs,
compaction, and abrupt disconnect/resume. `.github/workflows/environments.yml`
provisions Docker and runs those tests unconditionally on Linux, followed by the
Rust gates. Local Docker failures remain errors until the user explicitly changes
mode. See the delivery report for gate results; a configured CI job is not a
completed CI run. Recorded results and platform blockers are in
[VERIFICATION.md](VERIFICATION.md).

Hosted execution, generated custom TUIs, branching/rewind and automatic state
migration remain outside this fork's scope. Saved v1 definitions retain their
one-human/one-agent contract; v2 enables multiple model seats and agents-only runs.

## Environment seats and attention

New drafts use contract v2: role declarations, a validated roster (at most
16 seats and one human), seat-scoped observations/actions, notify, progress and
finish. Saved v1 definitions keep their legacy request bytes and transcript
paths. SDKs support both versions; saved source remains immutable.

Each instance runs in a supervised model-less host holding owner.lock. Agent
seats are separate supervised sessions, with durable capability-authenticated
wake inboxes. Queue journaling precedes the ACK; host delivery cursors advance
only after it. Failed turns retain pending wakes; completed sequence reports
and operation IDs make replay idempotent. The public host transcript contains
only the human's allowed view, or explicitly public notices for agents-only
spectators. Agent text/thinking and raw seat calls never broadcast to that table.

`/env seats`, `/env msg`, `/env pause|resume|stop`, `/env runs`, and `/env watch`
provide oversight. Explicit rosters are accepted after the selected backend.
For setup plus roster use `/env enter <id> {"policy":{"backend":"local"},
"roster":{"seats":[...]},"setup_arguments":{...}}`. Spectator detach keeps
agents-only runs alive; human leave pauses and releases its instance.
Defaults bound operations, wakes, elapsed time, time to action and output
tokens per request; monotonic reported environment progress and quiescence
determine stalls. A seat that misses its deadline or token cap without acting is
retried with thinking off, and an awaited seat that ends a turn without acting
is re-prompted, within `idle_turn_retries`. Agent seats see the instance's actual
setup configuration in their prompt. A definition may set
`agent_context: "stateless"` so each request carries only its `agent_view`
instead of the whole transcript. Timers/ticks,
multiple humans and spectators alongside a human remain deferred.

The builder provisions authorized runs through environments.start_instance and
has privileged read-only status/observation/transcript/table APIs, and can pause,
resume, or stop running instances. It does not act. Host
finish/stall/recovery transitions notify the builder. Per-seat Python state and uncertain receipts are independent;
local Python still shares host-user filesystem access. Use Docker for adversarial
play. No automatic Docker-to-local fallback exists.

Ownership: pa-environments owns execution/persistence and depends only on
pa-types among workspace crates. pa-core's exported SeatPort uses shared wire
types and no pa-environments dependency; pa-daemon owns supervision, wake
admission and provisioning. pa-tui renders public wire events only. The new
public SeatPort, roster and table APIs are architectural surfaces. Environment
surfaces are exempt from upstream TypeScript parity; upstream builder tools and
protocol identifiers remain unchanged.
