---
name: env-harness
description: Operate the Prime Environment Harness and its /env commands - create, policy, validate, save, enter, act, observe, status, leave, resume, recover, list, catalog, inspect. Use when the user asks how the harness works, which /env command to run, how to build, run, resume or recover an environment, what execution policies (local, docker, python, networking, mounts, secrets, integrations) mean, or when you must guide them through the build-then-enter workflow.
---

# Environment Harness

The harness lets up to 16 human or agent seats share one executable environment (a game,
a document workflow, a simulation). The environment's code is authoritative: it
exposes actions with JSON Schema inputs, filters what each participant observes,
and enforces the rules. This skill is the operating guide. To *write* an
environment (SDK contract, behavioral checks, dependencies) read the
`environments` skill instead.

For explicit rosters, setup-plus-roster commands, agents-only runs, and privileged
builder APIs, read [run configuration](references/runs.md). Supporting files ship
with this skill; resolve paths against the location of SKILL.md in the inventory,
even when the user's working directory is their home directory.

## Two modes

| Mode | You are | Slash commands available |
|---|---|---|
| Builder | The agent that creates drafts through the Python `environments` module | `/env list/catalog/create/inspect/policy/validate/save/enter/resume/recover/runs/watch`, plus `help` |
| Table | The human or spectator attached to a model-less host; agent seats have their own private sessions | `/env act/observe/status/seats/msg/pause/resume_run/stop/leave/cancel/help` |

In participant mode builder commands are refused ("Leave the environment before
using builder commands"). There is no builder kernel or builder tool in a
participant session.

Slash commands are typed by the **human**. You cannot run them. Tell the user the
exact command to type; do the work you *can* do through the `environments` Python
module (`create_environment`, `select_policy`, `validate_environment`,
`save_environment`, `list_environments`).

## Command reference

| Command | Mode | What it does |
|---|---|---|
| `/env` or `/env help [command]` | both | Usage and examples; bare `/env` opens autocomplete |
| `/env list` | builder | Saved environment `id — name` rows |
| `/env catalog` | builder | Full JSON catalog of saved definitions |
| `/env create <description>` | builder | Allocates a draft and sends you the build task |
| `/env inspect <id>` | builder | Prints a saved definition (manifest, actions, version) |
| `/env policy <draft> <json>` | builder | Selects the draft's execution policy (required before validation) |
| `/env validate <draft>` | builder | Runs the draft's behavioral checks in the selected backend |
| `/env save <draft> <id>` | builder | Saves a validated, unchanged draft as an immutable version; does not enter |
| `/env enter` | builder | Opens a picker of saved environments |
| `/env enter <id>` | builder | Opens the terminal setup form with an explicit execution choice; starts only on submit |
| `/env enter <id> local\|docker\|<json>` | builder | Selects that policy and opens declared setup fields, or starts immediately when no setup schema exists |
| `/env resume [instance-id]` | builder | Picker of saved instances (newest first), or resumes the given one pinned to its saved version and policy |
| `/env recover <instance-id> <json>` | builder | Reconciles an interrupted operation using evidence (see Recovery) |
| `/env runs` | builder | Lists persisted instances |
| `/env pause [instance-id]` | builder | Picker of running instances, or pauses the given one |
| `/env stop [instance-id]` | builder | Picker of running, paused or stalled instances, or permanently stops the given one |
| `/env watch <instance-id>` | builder | Attaches to the public table of an agents-only instance |
| `/env observe` | participant | Shows the human's current observation |
| `/env status` | participant | Instance state and the human's available actions |
| `/env act` | participant | Picker of the human's actions |
| `/env act <action>` | participant | Terminal form with typed controls and field-level JSON fallback |
| `/env act <action> <json>` | participant | Submits the action directly |
| `/env seats` | table | Lists seat identities, roles, occupants, and models |
| `/env msg <seat> <text>` | human table | Sends a private message; wakes an agent recipient |
| `/env pause` | table | Pauses mutations and aborts agent turns |
| `/env resume` or `/env resume_run` | table | Resumes a paused or stalled run; retains pending wakes |
| `/env stop` | table | Stops the run permanently |
| `/env cancel` | participant | Clears the open action form |
| `/env leave` | participant | Cancels participant work and returns to the builder |
| `/tutorial` | both | Shows the onboarding walkthrough |

In a builder session, the activity dock has an Environments group (`◈ N
environments`, plus paused and stalled counts, live runs only). It shows one
row per run that the session or its subagents provisioned, not one per seat.
Runs stay with the session that started them across daemon restarts; older
runs that can't be matched to a session are listed under "Other runs".
Opening it lists live runs first, then ended ones: Enter watches an agents-only
run or reopens a human table, and `x` stops a run. Enter on an ended run shows
its transcript read-only; nothing can be sent to it, and Esc goes back. Host
and seat sessions are hidden from the agents view.

Plain chat at a human table goes through `handle_input` from the bound human
seat. Explicit environment notices decide which agents wake after a commit.
The host transcript shows only that human's observations, public/targeted notices,
actions, direct messages to that human, and run status. Replies and thinking stay
in private seat transcripts; the builder can inspect them through privileged APIs.

`/env seats` lists roles, occupants, and models. `/env msg <seat> <text>` sends a
direct message and wakes an agent recipient. Messages are bounded to 16 KiB and
32 new messages per agent wake turn. The human counter has no model-turn reset,
so its current bound is 32 new direct messages over the run. A definition may
disable direct messages.
`/env pause`, `/env resume` (at the table), `/env resume_run`, and `/env stop`
control dispatch. Pause aborts model turns and retains pending wakes for resume;
stop is permanent. Resuming a stalled run also gives an awaited agent a fresh
retry notice when its previous wake was consumed without a move. The saved
`waiting_for` state selects the recipient; game actions are never replayed.
Finish and recovery-required states cannot resume.
`/env runs` lists persisted instances in builder mode; `/env pause` and `/env stop`
there list controllable instances like `/env resume` and accept an instance ID. `/env watch <instance>`
attaches to an agents-only table whose rows contain only explicitly public
notices. Detaching a spectator leaves the run active; leaving a human table
pauses the run and releases it.

An explicit roster uses `/env enter <id> local {"seats":[{"id":"human-1",
"role":"human","occupant":"human"},{"id":"agent-1","role":"agent",
"occupant":"agent"}],"limits":{"max_operations":100}}`. Roles must fit the
manifest's min/max and occupant restrictions. Agent model/thinking settings are
optional and use the builder model by default. Local Python namespaces are
separate, but filesystem permissions are shared: choose Docker for adversarial
play. Saved protocol 1 environments retain their human/agent IDs and transcript.

## Standard workflow

1. `/env create <description>`: a draft path is returned and you get the build task.
2. Hold the design conversation from the `environments` skill
   (`references/design.md`) before writing code. Cover rules, participants, turn
   flow and wakes, length and configuration, privacy, agent context, deadlines
   and token budgets, failure handling, and analysis. Give a recommendation for
   each, accept "decide later", and confirm a short design summary.
3. Build the draft following the `environments` skill. Present purpose, actions,
   dependencies, whether Python is requested, and your execution recommendation.
4. **Ask the user which policy they want and wait.** Never pick or widen
   permissions yourself. Apply their answer with
   `await environments.select_policy(draft=..., policy=...)`, or they can type
   `/env policy <draft> <json>`. Changing the manifest later invalidates the
   selection; ask again.
5. Validate in the selected backend without asking for separate approval
   (`environments.validate_environment`). On failure repair and revalidate, at most
   three repair passes after the initial attempt. Any edit invalidates a prior pass.
6. Save only passed, unchanged content (`environments.save_environment(draft=..., id=...)`).
   When this is a new version of an existing definition that already has a viewer,
   make that viewer compatible and register the version before the changelog, as
   the `environments` skill describes. Show the saved definition, then tell the
   user to run `/env enter <id>`. Saving never enters.
7. The user enters, then uses `/env observe`, `/env act`, `/env status`. Agent
   seats are separate private sessions that the host wakes; you do not play.
   For agents-only runs you may provision with `environments.start_instance`
   once the user authorized the token use. Before starting a long run, propose
   run limits (deadline, token cap, retries, wall clock, operations, wakes) from
   your estimate of its length.
8. `/env leave` ends participation; `/env resume` returns later.

Draft paths are the strings the builder returns; use them verbatim in
`/env policy|validate|save`.

## Execution policy

A policy is a JSON object for `/env policy`, `/env enter <id> <json>`, and
`environments.select_policy`. Omitted fields take defaults, so a replacement policy
must restate every grant that should be kept.

| Field | Default | Meaning |
|---|---|---|
| `backend` | `"local"` | `"local"` or `"docker"` |
| `python_execution` | `false` | Persistent participant Python; needs both a definition request and this grant |
| `networking` | `false` | Runtime network (dependency preparation is a separate networked step) |
| `mounts` | `[]` | `[{"source":"/abs/host/path","target":"/path","writable":false}]` |
| `secrets` | `[]` | Names only; values come from host env `PRIME_ENV_SECRET_<name>` |
| `integrations` | `[]` | Names from `<data-dir>/environment-integrations.json` |
| `call_timeout_seconds` | `120` | Per runtime call |
| `preparation_timeout_seconds` | `600` | Dependency preparation |
| `cpus` / `memory_mib` | `2` / `1024` | Docker resource limits |

The four common combinations:

```json
{"backend":"local","python_execution":false}
{"backend":"docker","python_execution":false}
{"backend":"local","python_execution":true}
{"backend":"docker","python_execution":true}
```

Rules to state accurately to the user:

- Local generated code (Python or Node) runs with host-user permissions. Tool
  allowlists only govern brokered integrations and do not confine local code.
- Docker is recommended for Python but optional. If Docker is unavailable it is an
  error; the harness never falls back to local. The user must explicitly choose local.
- Docker engine and participant kernel use separate containers; the participant
  container has no engine source or private workspace. Containers are non-root,
  read-only, no capabilities.
- Never grant networking, mounts, secrets, or integrations the user did not ask for,
  and never put credentials in generated files. Provider credentials stay on the host.
- Existing instances keep their saved policy; `/env resume` takes no new one.

## Privacy on the human screen

Environment-to-agent content is hidden from the human: `handle_input`'s
`agent_message`, agent observations, action and Python responses, and tool
arguments stay in the seat transcript. Human observations and
human action responses are visible. Agent replies and thinking are private,
including streamed deltas and reconnect history. This is display privacy only; the local
instance files still contain the originals.

## Participant tools (agent inside an instance)

- System prompt: seat, role, the instance's actual setup configuration, the
  environment's `instructions`, peers, and the run's deadline and token cap.
- `observe_environment`: your filtered observation. Absent when the definition sets
  `agent_context: "stateless"`; there each wake carries your current state, and
  every action result, including a rejection, returns the updated state.
- `env_<action>`: one tool per action that permits the seat's role; arguments
  must satisfy the action's schema. The environment enforces legality; a rejected
  action is information, not a crash.
- `ipython`: only when the definition requests Python *and* the policy grants it.
  Inside it: `env.observe()`, `env.invoke(action, arguments)`,
  `env.integration(name, arguments)`.
- `say` (protocol 2): sends text through the environment's input handler; that
  handler chooses visibility and resulting wakes.
- `message_seat` (when enabled): sends a private message directly to another seat
  and wakes it. Assistant replies alone do not communicate with peers.
- Follow the environment's `instructions` and its secrecy rules.

## Persistence and recovery

- Definitions are immutable, content-addressed versions. An instance pins one
  version plus its policy, and keeps separate engine checkpoints, participant
  scratch/Python state, and conversation history.
- Intents are fsynced before actions run; receipts and the resulting snapshot are
  committed together. Repeating an operation ID returns the recorded result.
- Every committed snapshot is also appended to the instance's `snapshots.jsonl`
  (a keyframe, then deltas), so an ended run can be replayed step by step.
- An interrupted operation blocks `/env resume`. It is never replayed. Inspect the
  saved state and any external effects first, then:

```text
/env recover <instance-id> {"snapshot":{...},"receipts":{...},"python_receipts":{...},"reset_python":false,"evidence":"what you verified"}
```

  `evidence` must be nonempty and every receipt must match its unresolved intent.
  Legacy uncertain Python state requires `"reset_python":true`. Protocol 2 uses
  `seat_python_receipts` and `reset_python_seats` for the affected seat. Recovery records what you
  supply; it runs neither initialization nor earlier actions. Do not invent
  receipts; ask the user to confirm the outcome.
- Python checkpoints keep pickle-compatible variables only. Modules and open files
  are dropped and reported; values over 16 MiB are omitted, total cap 256 MiB.

## Trusted integrations

Defined by the user in `<data-dir>/environment-integrations.json`, outside generated
definitions: absolute executable, arguments, input/output JSON schemas, optional
named-secret mappings (`PRIME_ENV_INTEGRATION_SECRET_<name>` on the host). The
executable gets one JSON request on stdin and returns one JSON value on stdout.
Granted per instance with `"integrations":["name"]`. Never create or edit this
file on your own initiative.

## Running the harness

```sh
cargo run -p pa-cli --bin prime-env-harness -- --data-dir <directory>
```

- Default data dir `~/.prime-env-harness`; its daemon, sessions, settings,
  credentials, definitions and instances are independent of ordinary Prime.
- First run: `/login` to configure a provider, then `/env create ...`.
- Local workers need Node 22.20+ (TypeScript) and Python 3.10+ with venv. Docker
  needs a running daemon and permission to pull/build the runner images.
- Builder commands include `/model` and `/compact`. Environment tables use `/env` controls; `/tutorial` and `/exit` work in either view.
- A complete relay example ships inside the `environments` skill; its asset paths
  are relative to that skill's inventory location. Source checkouts also have
  fixtures in `crates/pa-environments/fixtures`.

## Common mistakes

- Running or validating generated code before the user selected a policy.
- Typing `/env ...` as if you could execute it. Tell the user instead.
- Saving a draft that changed after its last passing validation.
- Using `/env enter <id>` expecting it to start automatically; select an execution
  policy and submit the setup form first.
- Using builder commands while inside an instance; `/env leave` first.
- Reading private observations back in visible replies.

Built-in discovery checks `skills/` at the package location, then the compiled
source checkout. `PI_PACKAGE_DIR` selects a package location. A relocated binary
must keep its skills tree, including references, assets, pyproject.toml, and Python
sources. User skill copies in `<data-dir>/skills/` are another supported location.
User/project overrides and disabled-skill settings can suppress bundled skills;
do not claim discovery bypasses those choices. Use `prime-env-harness prompt
--json` to inspect the effective inventory without running a model.
