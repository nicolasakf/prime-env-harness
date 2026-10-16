---
name: environments
description: Build, validate, and save reusable TypeScript or Python environments with roles, seats, private observations, explicit wakes, and schema-defined actions. Also covers report templates and per-run reports for saved environments, and optional replay viewers when the user asks for one.
---

Build the user's environment in a draft. Use the Python `environments` module
for management; functions call the Rust host and return JSON values.

Start with a design conversation before writing code. Follow
[the design conversation](references/design.md): ask about the open decisions
on rules, participants, turn flow and wakes, run length and configuration,
privacy, agent context, deadlines and token budgets, failure handling, and
analysis. Ask in a few short rounds, give a recommendation and a default for
each question, and accept "decide later". Then confirm a short design summary
before implementing.

Stay on the environment side of the line: you create and edit environments, and you
never edit the harness (its code, the SDK and runner copies in a draft, skills, prompts).
When a problem is a harness problem, or when you save a new version of an existing
environment, read [maintenance](references/maintenance.md): it covers harness
recommendations and the changelog you write for every new version.

Read [the authoring contract](references/contract.md) when implementing the
manifest, SDK methods, schemas, dependencies, and checks. Read
[coordination patterns](references/coordination.md) when deciding who should act,
who should see each update, and which seats should wake. These references and the
[relay example](assets/relay/environment.py) ship inside this skill; resolve their
paths relative to this SKILL.md, never relative to the user's working directory.
Use the SDK and runner supplied in the draft. The source checkout is unnecessary.

```python
draft = await environments.create_environment(description="...", language="python", python_execution=False)
# After asking the user which policy they want and receiving their answer:
await environments.select_policy(draft=draft["draft"], policy={"backend": "local"})
report = await environments.validate_environment(draft=draft["draft"])
saved = await environments.save_environment(draft=draft["draft"], id="my-environment")
await environments.list_environments()
```

New drafts use protocol 2. Implement initialize, describe_actions, observe,
handle_input, invoke, snapshot, restore, and dispose using the draft's SDK.
Declare `roles` in environment.json: unique names, min_seats/max_seats (1–16),
and allowed `occupants` (`human`, `agent`). Instances have at most 16 seats and
one human. Action descriptors use `roles`, never protocol 1 `participants`.
The host binds the acting seat; the environment enforces legality and filters
observations. `initialize(params)` receives `params["seats"]` (id and role only)
and `params["config"]` (validated setup arguments). Occupants and model settings
are harness concerns and are never sent to environment code.

Agent seats are conversations by default: each wake is appended and the whole
transcript is resent on every model request, which gets expensive over a long run.
For long runs or large observations set `"agent_context": "stateless"` and
implement `agent_view(seat)`: each turn the model then sees only that view (current
state plus a bounded history summary), the seat's private messages, and its own
tool calls. Action results and rejected actions both return the current view,
and a wake that waited in the queue gets a fresh view when its turn starts. See
"Agent context" in the authoring contract; put fixed rules in `instructions`,
not in the view.

For configurable environments add `setup_schema` to environment.json with
readable property titles, descriptions, bounds, and defaults. The terminal
supports typed fields and JSON fallback. Keep setup separate from policy and
grants. Validation uses defaults and a synthetic minimum roster, so behavioral
checks must also cover the supported roster sizes. Each agent seat's system
prompt lists the instance's actual setup configuration, so refer to parameters
by name in `instructions` and never restate their defaults there.

The environment decides attention after committed mutations. Import `notify`,
`progress`, and `finish` from `sdk` in Python or `./sdk.ts` in TypeScript.
`notify([seat_id], message, wake=True)` targets a seat; `notify("*", public_text,
wake=False)` publishes a public update without starting model turns. Never put
secrets in notices to `*`. Private observations, replies, and thinking remain
in agent transcripts. Notices may contain at most 16 KiB, with at most 64 notices
per operation. Notify only seats that have something to do; avoid wake storms.

For a turn-based game, initialize by notifying the first player, notify the
next legal player after each move, and call `progress(revision, waiting_for)`
with a monotonically increasing revision when the environment advances. Include
the human seat in waiting_for while waiting for human input. Call
`finish(public_summary)` when the run ends. For simultaneous rounds, notify all
eligible players initially, store each private submission, and finish or notify
the next round only when all submissions are in. A wake starts a private model
turn; no builder relay or human "your turn" prompt is needed.

`handle_input(seat, text)` in protocol 2 returns `{"observation": ...}` and may
notify other seats. Protocol 1 definitions still use `participants` and the
legacy human/agent identities; `agent_message` is adapted to a private wake.
Use the packaged relay example and coordination reference as contract examples. Add exact
dependency versions with lockfiles (Python hashes), resources, and behavioral
checks. Changed source invalidates validation.

Show purpose, actions, dependencies, requested Python capability, and the
execution recommendation before validation. Ask the user which execution policy
(backend, Python capability, and grants) they want to apply, then wait for their
answer. Do not choose or expand permissions on their behalf. Apply their answer
with `await environments.select_policy(draft=..., policy=...)`; the user can also
select it manually with `/env policy <draft> {"backend":"local"}` or Docker
and explicit grants. Validation refuses drafts without a selected policy.
If the manifest changes, explain the changes and ask for a new policy selection.
Docker is recommended for Python but never required. Local generated code
runs with host-user permissions; tool allowlists cannot confine local Python.

After applying the requested policy, validate automatically in the selected
backend without asking for separate validation approval. Read failures and repair
at most three times. Changes invalidate validation. Save only passed, unchanged content.
Present the saved definition and wait for `/env enter`; saving never enters.
When the save created a new version of a definition that already existed, add its
changelog entry first (see [maintenance](references/maintenance.md)).
For programmatic provisioning, ask before starting runs because they consume
model tokens. Existing user authorization suffices; do not ask repeatedly.

```python
run = await environments.start_instance(
    definition="my-environment",
    seats=[{"role": "player", "occupant": "agent", "id": "alice"},
           {"role": "player", "occupant": "agent", "id": "bob"}],
    limits={"max_operations": 100},
)
await environments.list_instances()
await environments.instance_status(instance=run["instance_id"])
await environments.pause_instance(instance=run["instance_id"])  # reversible
await environments.resume_instance(instance=run["instance_id"])  # paused or stalled runs; limits still apply
await environments.stop_instance(instance=run["instance_id"])  # permanent
await environments.observe_seat(instance=run["instance_id"], seat="alice")
await environments.seat_transcript(instance=run["instance_id"], seat="alice", since=0)
await environments.table_log(instance=run["instance_id"], since_seq=0)
# Only for a seat that is stuck (see "Stuck seats" below):
await environments.act_for_seat(instance=run["instance_id"], seat="bob", action="pass", reason="bob kept calling unavailable actions")
await environments.suspend_seat(instance=run["instance_id"], seat="bob", reason="bob cannot follow the turn order")
await environments.resume_seat(instance=run["instance_id"], seat="bob")
```

Provisioning may narrow the user-selected validation policy; it cannot widen
permissions or change backend. The builder never plays in a running instance:
it can pause, resume, or stop it, and act for or suspend a stuck agent seat (see
"Stuck seats" below). Pause freely when a run
misbehaves or burns tokens, but ask before resuming a run the user paused or
before stopping, which is permanent
and ends the seat sessions, unless they told you to. Its other debugging APIs are
privileged and read-only. Finish,
stall, and recovery-required transitions, and a pause forced by a run limit, send
the builder an informational message; a pause or stop someone requested does not. Seats cannot spawn, use builder APIs, or access builder tools.
Default run limits are 500 operations, 100 wakes per seat, two hours of wall
clock including pauses, 120 seconds to act while the environment waits on a seat
(restarting after each action), 8192 output tokens per model request (thinking
included), two re-prompts per awaited seat (`idle_turn_retries`), and 2·agents+2
settled turns without environment progress. A turn that exceeds the time or
token limit without acting is retried with thinking off. A turn that ends without
acting while the seat is still awaited is re-prompted. Both use the same retry
budget; once it is spent, the run stalls or pauses. An environment that declares
`missed_turns` decides what each miss means through `missed_turn` (see
[coordination](references/coordination.md#missed-turns)). These are run settings, not
definition fields. Estimate a long run's operations and wakes, and propose limits
before starting it. `instance_status` reports per-seat `time_to_action`,
`deadline_retries`, `post_action_aborts`, and `idle_retries`.

Stuck seats: when a stall message names a seat, or a seat keeps failing its turn,
read its transcript and observation first. If the environment awaits that seat
and the user wants the run to go on, `act_for_seat` takes that one turn with the
least consequential legal action (a pass or abstention where one exists). The
environment validates it, only that seat is told, and a stalled or paused run
then needs `resume_instance`. If the seat will keep failing, `suspend_seat` takes
it out of play: an environment with `missed_turns` hears of each turn it awaits
from the seat (cause `suspended`) and may play it; otherwise the run stalls on
that turn for you to act for the seat or `resume_seat` it. Interventions never
change environment state directly. Ask before intervening unless the user asked
you to keep the run going, and always report which seat you acted for or
suspended and why. Fix a recurring cause in the environment (clearer legal
actions, a `missed_turn` policy) rather than intervening on every run.

When the user asks for a report on an environment or a run, read
[reports](references/reports.md) first. It separates reusable report templates
(one per environment definition, pinned to the versions they were verified on) from
the executed report of a single instance, and gives the workflow: look for a
template, copy it into the instance folder, run it there. Do not write a new report
before checking for a template.

A viewer is an optional, environment-specific web page that replays an ended run
step by step, or follows a running one live through the environment's optional
`frame(perspective)`. Build one only when the user explicitly asks for a viewer, a
visual replay, a live view, or a way to watch a run; never offer one by default.
When they ask, read [viewers](references/viewers.md) first.
`await environments.view_instance(instance=...)` returns a public live-view URL
for a running instance whose saved viewer lists its version.

Using an existing viewer needs no request. When the user asks for a run and the
definition has a compatible saved viewer, launch it by default: call `view_instance`
right after `start_instance` and give the user the URL; if it is refused, continue
without saying so. For an ended run the user wants to review, export the replay and
give the path. See "Launching an existing viewer is the default" in the viewers
reference.

Participant Python uses env.observe(), env.invoke(action, arguments), and
env.integration(name, arguments). Provider credentials stay on the host.
Never include credentials in generated files or request engine workspace mounts.

Each agent has a separate persistent Python namespace and receipts. Local Python
still runs with host-user permissions and can read other seats’ files; use Docker
for adversarial play. An uncertain Python cell requires reconciliation and an
explicit reset of that seat’s namespace; it does not invalidate other seats.
