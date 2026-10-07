# Environment authoring contract

New drafts target protocol 2. Creation writes the current SDK, runner, counter
example, dependency files, and checks into an absolute draft path. Work in that
path. Replace the counter behavior and checks together; do not assume the user
launched from the harness repository. Do not replace the SDK or runner to bypass
validation or add tools.

## Manifest and roster

`environment.json` has these fields:

| Field | Contract |
| --- | --- |
| `protocol_version` | `2` for a new environment |
| `name`, `purpose` | Nonempty strings |
| `language` | `python` or `typescript` |
| `instructions` | Rules and operating guidance delivered to each agent seat |
| `roles` | 1–16 unique role declarations |
| `setup_schema` | Optional JSON Schema for environment configuration |
| `python_execution` | Whether agents may request a persistent participant Python kernel |
| `recommended_backend` | `local` or `docker`; a recommendation, not consent |
| `direct_messages` | Optional; defaults to enabled. `false` forbids the independent seat-message channel |
| `agent_context` | Optional; `"conversation"` (default) or `"stateless"`. See [Agent context](#agent-context) |

A role has `name`, optional `description`, `min_seats`, `max_seats`, and
`occupants`. Defaults are one required seat, one maximum seat, and both occupant
kinds. Bounds satisfy `1 <= min_seats <= max_seats <= 16`; all role minima must
fit within 16 total seats. Optional roles with a zero minimum are unsupported.

Role names and seat IDs use 1–64 ASCII letters, digits, underscores, or hyphens.
Roles describe permissions; seat IDs identify individual participants; occupants
identify humans or models. Do not use these interchangeably. Multiple seats may
share a role. At most one seat is human. A human seat cannot select a model or
thinking level. Every supplied seat must fit a declared role and occupant rule.

Unless the user supplies a roster, the host creates each role's minimum seats
in manifest order. It assigns the first eligible seat to a human and the others
to agents. An agent-only role therefore produces agents by default. Supporting
both human and agent occupants does not itself choose an agents-only roster.

`initialize` receives only the domain roster and setup configuration:

```json
{"seats":[{"id":"alice","role":"player"},{"id":"bob","role":"player"}],"config":{"rounds":2}}
```

Occupants, models, thinking levels, execution grants, and host credentials are
absent. Write domain behavior against IDs and roles so the same rules work for
human and model occupants. Never take an acting seat from action arguments.

## Required methods

Python exports `environment = YourEnvironment()`. TypeScript exports the
environment as its default export. Use `sdk.py` or `./sdk.ts` from the draft.

| Method | Responsibility |
| --- | --- |
| `initialize(params)` | Create state from `seats` and `config`; publish initial attention and progress |
| `describe_actions()` | Return public action descriptors with schemas and permitted roles |
| `observe(seat)` | Return only the information that seat may see; no mutation or notices |
| `agent_view(seat)` | Optional. What an agent seat's model sees each turn under `agent_context: "stateless"`; defaults to `observe(seat)`. No mutation or notices |
| `handle_input(seat, text)` | Process seat text and return exactly `{"observation": ...}`; select resulting notices explicitly |
| `invoke(seat, action, arguments)` | Check domain legality, mutate, return schema-valid output, publish attention/progress/completion |
| `snapshot()` | Return JSON covering all state needed to continue, including roster, phase, private state, and random state where relevant |
| `restore(snapshot)` | Reconstruct that state in a fresh process without initialization, side effects, or notices |
| `dispose()` | Release resources; no gameplay mutation or notices |

Python methods may be synchronous or awaitable. TypeScript methods may return
values or promises. All results and snapshots must be JSON serializable.
SDK notifications are permitted only in `initialize`, `invoke`, and
`handle_input`; reads and restoration cannot create turns.

An action descriptor contains `name`, `description`, `input_schema`,
`output_schema`, and `roles`. Name it so `env_<name>` is a useful model tool.
Protocol 2 uses `roles`; protocol 1 uses `participants`. The host enforces role
access and validates inputs and outputs. Your code enforces turn order, funds,
ownership, remaining moves, and other domain constraints before mutating state.

Action descriptors are shared metadata. Never put private hands, sealed bids,
credentials, or secret per-seat choices in their descriptions or schemas.
The current terminal UI has no private, dynamic choice-provider protocol.
Use public schemas with private observations and server-side legality checks.

## Agent context

By default each agent seat is a conversation: every wake, with its full
observation, is appended to the seat's transcript and the whole transcript is
resent to the model on every request. Cost and latency grow with the square of the
run length, and old observations bury the current one. Environments with long runs
or large observations should set `"agent_context": "stateless"` (protocol 2).

Under `stateless` each model request contains only the system prompt and your
`instructions`, the latest wake, and the current turn's own tool calls and
results. The transcript is still saved in full; it is just not resent. The
environment, not the transcript, carries history:

- `agent_view(seat)` returns the one JSON value the model sees: the current state
  plus a **bounded** history summary, filtered for that seat exactly like
  `observe`. If you omit it, `observe(seat)` is used. At most 32 KiB; validation
  rejects a larger view, so summarize older history instead of listing it.
- Each wake delivers `{"state": <agent_view>, "messages": [...]}`. `messages` holds
  only notices addressed to the seat and direct messages. Broadcast (`"*"`)
  notices are not shown, so anything the model needs from them belongs in the view.
- The seat has no `observe_environment` tool. Every `env_<action>` result is
  `{"result": ..., "state": <agent_view>}` so the model sees the new state without
  asking. A rejected action returns its error with the current view, so the model
  can correct itself instead of repeating the call. `ipython`, `say`, and
  `message_seat` keep their normal replies.
- A wake that waited in the queue while the seat's previous turn ran gets a
  fresh view when its turn starts. Every model-generated action is also bound to
  the committed state its request was written from: if any engine operation (an
  action or `say`, even a rejected one) committed since, the host refuses the
  call without running your code and returns the current view. Design actions to
  be safe to reconsider, and note that simultaneous seats refuse each other's
  stale calls and retry from the fresh view. `observe`/`agent_view` reads never
  invalidate a view.
- The model remembers nothing between wakes, including its own earlier reasoning.
  Put what it needs to decide (recent moves, opponent tendencies, scores) in the
  view; keep fixed rules in `instructions`, where they are cached, not in the view.
- Keep the view small and stable in shape: key rounds that matter in detail and
  fold older ones into one line or running totals. Never include another seat's
  private information, and check that in `checks.py`.

## Setup and action forms

Use property `title`, `description`, `default`, `required`, bounds, and enums to
make forms understandable. Strings, numbers, booleans, primitive enums, and
unique arrays of enum values have typed controls. Nested objects and complex
arrays fall back to JSON for that field; unsupported root shapes use whole-input
JSON. The host still performs full schema validation.

Declare gameplay configuration in `setup_schema`, separate from policy and roster.
Give required setup properties valid defaults: automatic validation initializes
with defaults and the minimum roster. Supplied instance configuration is
validated before code runs. Unknown configuration should be rejected with
`additionalProperties: false` where appropriate. Without a setup schema, only
empty configuration is supported.

Each agent seat's system prompt lists the instance's resolved configuration
(defaults plus overrides), above your `instructions`, and keeps it across
resume, restart, and compaction. Configurations over 8 KiB are omitted with a
note. Write `instructions` against parameter names and never quote defaults
there, because a seat would see conflicting numbers. Put quantities derived from
the configuration, such as total turns, in the observation or `agent_view`.

## Dependencies, resources, and integrations

Python drafts contain `environment.py`, `sdk.py`, `runner.py`, `checks.py`, and
`requirements.lock`. Pin third-party packages exactly and include pip hashes;
preparation uses `pip install --require-hashes`. TypeScript drafts contain
`environment.ts`, `sdk.ts`, `runner.ts`, `checks.ts`, `package.json`, and
`package-lock.json`; preparation uses `npm ci --ignore-scripts`. Node executes
TypeScript directly, so use syntax compatible with native type stripping.
Local execution requires Node 22.20+ for TypeScript and Python 3.10+ with venv.

Put required resource files in the draft. Keep authoritative state in snapshots;
external effects need inspectable receipts because interrupted mutations are
reconciled explicitly, not retried automatically.

Trusted integration commands belong to the user's host configuration
`<data-dir>/environment-integrations.json`, outside the draft. The environment
uses `host_call("integration.call", {"name": ..., "arguments": ...})`; participant
Python uses `env.integration(name, arguments)`. The host binds the instance and
acting seat and checks named grants. Generated code cannot choose an executable.
Do not embed credentials or request engine-workspace mounts for participants.

## Validation and delivery

The host prepares dependencies, initializes the minimum roster with defaults,
describes actions, observes each seat, and checks snapshot equality after restore
in a fresh worker. If participant Python is requested and granted, it checks its
bridge and namespace restoration. It then runs `checks.py` or `checks.ts`.

Generic validation does not prove game rules, privacy, or wake choreography.
Replace the scaffold checks with assertions for actual behavior: supported
roster sizes, setup values, legal and illegal actions, per-seat observations,
initial and subsequent wakes, terminal completion, and restoration at meaningful
phases. Check each supported human/agent arrangement through host tests when its
delivery behavior matters. The [relay checks](../assets/relay/checks.py) show
state and attention assertions at SDK request boundaries.

Show the requested capability, dependencies, and recommendation; apply the
user's selected policy before validation. Existing authorization remains valid
for unchanged permissions. A manifest change invalidates policy selection.
Run validation automatically once a policy is selected. The host allows the
initial attempt and three repair passes total, retains the reports, and refuses
to save source that changed after the passing report. Saving creates an immutable
version and never enters it. Starting authorized runs uses model tokens; show the
roster and bounds and respect the user's existing authorization.
