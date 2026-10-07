# Seats, visibility, and automatic attention

Design the domain turn state and attention together. The host starts a model
turn only after it commits a delivery whose `wake` is true. Returning an action
result, changing an observation, mentioning another agent in text, or an agent
writing a final reply does not automatically wake a peer.

## Supported participant arrangements

| Arrangement | Manifest and roster | Attention |
| --- | --- | --- |
| One human alone | One role permitting `human`, one human seat | Targeted human notices, observations, and action forms; no model turns |
| One agent alone | Role permitting `agent`, one agent seat | Initial wake, then another self-wake only if a further turn is needed; finish when done |
| One human with one or more agents | Shared or different roles permitting the requested occupants; one human maximum | Wake eligible agent seats; target the human and include it in `waiting_for` during human turns |
| Several agents with a human watching | Explicit all-agent roster or agent-only roles | Wake eligible agents; publish spectator-safe notices to `"*"` |
| Several agents without a human attached | Same all-agent roster | Identical autonomous delivery; no human relay is required |

Each required role must be occupied. A role's minimum cannot be zero, so a
required human-only role prevents using that definition for agents-only runs.
Use an occupant-flexible player role when the same definition should support
both arrangements. Use an explicit roster to avoid the default human assignment.
Multiple human participants and spectator access alongside an occupied human
seat are outside the current contract. A spectator can pause/resume/stop an
agents-only table, but cannot act or send participant messages.

## SDK attention calls

```python
from sdk import notify, progress, finish

notify([next_seat], "Your turn")                     # wake defaults to True
notify([human_seat], "Choose your next move", wake=False)
notify([next_seat], "Your turn", actionable=True)  # the task; dropped once the seat is no longer awaited
notify("*", "Round 2 has begun", wake=False)        # public, no model turn
progress(revision, waiting_for=[next_seat])          # keyword argument in Python
finish("The run is complete")                      # public summary
```

```typescript
import { notify, progress, finish } from "./sdk.ts";

notify([nextSeat], "Your turn", { actionable: true });
notify([humanSeat], "Choose your next move", { wake: false });
notify("*", "Round 2 has begun", { wake: false });
progress(revision, { waiting_for: [nextSeat] });
finish("The run is complete");
```

`actionable` marks the wake as the task for the named seats to act on now. The
host drops it from a wake that starts after `progress(waiting_for=...)` stopped
naming the seat (or a newer actionable task for the seat superseded it), so a
queued "your turn" never reaches a model that no longer has one; messages and
ordinary notices are never dropped, and a seat the environment awaits always
keeps its current task. Use it for turn prompts in sequential play, together
with an accurate `waiting_for`. It needs `wake=True` and named seats.

Targets are existing seat IDs, not role names. Target lists must be unique and
nonempty, with at most 16 IDs. `"*"` is public to all seats and spectators;
`[all_seat_ids]` is still targeted and does not make a spectator event public.
Notices allow an optional message of at most 16 KiB and at most 64 notices per
operation. A human-targeted wake displays a notice; it cannot operate the human's
controls. There are no OS push notifications or arbitrary environment timers.

`progress` records a nondecreasing safe-integer revision and existing waiting
seats. Increase the revision for actual domain progress, rather than each model
reply or no-op. `waiting_for` explains legitimate waits and must include the
human when the environment awaits human input. It never schedules a turn by
itself. `finish` commits terminal state with the operation and a public summary
of at most 16 KiB; the host stops seats and forbids further mutations.

Notices, progress, finish, action receipts, and the resulting snapshot commit
together. If code mutates and then returns malformed metadata, failed output,
or loses its response, the runtime may require explicit recovery. Validate
arguments and legality before changing state.

## Sequential turns

Store roster, current turn, revision, and completion condition. Initialization
notifies the first eligible seat and reports it as waiting. A legal move advances
state, publishes a safe table update, then either wakes the next seat and reports
the wait or calls finish. Do not wake every player after every move.

The [relay manifest](../assets/relay/environment.json),
[environment](../assets/relay/environment.py), and
[checks](../assets/relay/checks.py) are a complete example. Copy them into a
Python draft while retaining that draft's SDK, runner, and dependency lockfile.
The same player role supports a single human, a human with agents, or all agents.
Its text handler publishes public chat without waking players; domain moves
control the next turn. Adapt that choice if text itself requires a response.

## Simultaneous rounds and private submissions

Initialize the round, wake all eligible submitters, and report them in
`waiting_for`. Keep submissions in seat-keyed private state. `observe(seat)`
returns only that seat's private submission plus permitted public round facts.
Validate one submission per eligible seat before mutating. After each accepted
submission, increase revision and remove that seat from the waiting set. Do not
re-wake everyone still waiting merely because one submission arrived.

Once all submissions are present, publish only the allowed aggregate or result.
Then finish or create the next round and wake its eligible submitters. Put secret
reminders in targeted notices. A public `notify("*", ...)` must stay safe even
when every occupant is an agent and a spectator connects later.

## Conversations, negotiation, and asymmetric roles

`say` goes through `handle_input(seat, text)`. That handler decides whether text
is public, private, a proposal, or a domain move, and which peers need attention.
For meaningful negotiation, advance the domain revision when a proposal or
agreement changes the environment, and report the responding seats as waiting.
Avoid reciprocal wakes that continue without progress.

`message_seat` is a separate private channel that wakes its recipient and bypasses
the environment's input handler. It is enabled by default, limited to 16 KiB and
32 new messages per sending agent wake turn, and disallows messaging oneself.
The human counter has no model-turn reset and currently permits 32 new direct
messages over the run.
Disable `direct_messages` in the manifest when rules require all communication
to pass through the environment. Final assistant replies and thinking remain
private transcripts; agents must use say, messages, or actions to communicate.

For different responsibilities, declare roles such as proposer/reviewer or
buyer/seller and restrict each action's `roles`. Derive IDs from the initialization
roster; do not assume IDs equal those role names. Target the seat that can perform
the next domain action, even if several seats share a role.

## Wake lifecycle and failure limits

The model-less host owns the environment. Each model seat has a private supervised
session, a capability, a durable inbox, and a delivery cursor. The host appends
stable table events, coalesces unseen seat deliveries, supplies a fresh filtered
observation, and advances its cursor only after durable wake acceptance. Failed
turns retain pending wakes and pause the run; resume can retry admission.

Replaying a wake and retrying a recorded operation ID are deduplicated. A model
that makes the same logical move with a new tool-call ID is a new operation;
domain rules must reject it when inappropriate. Restore must preserve phase and
submission/turn state. Never claim unconditional exactly-once external effects.

Default run bounds are 500 environment operations, 100 wakes per agent, two hours
elapsed (pauses included), 120 seconds to act, 8192 output tokens per request,
two re-prompts per awaited seat, and `2 * agent_count + 2` settled turns without
revision progress. The 120-second deadline applies only while `waiting_for`
lists the seat, and restarts after each action. A seat that already acted is not
retried; its leftover turn is cut and counted as a post-action abort.

Accurate `progress` metadata drives automatic recovery. When an agent's turn
ends while `waiting_for` still lists it, the revision has not moved since the
turn started, and no newer wake is pending, the host re-prompts that seat. A
turn that hits the deadline or token cap without acting is retried with thinking
off. Both use one per-seat budget (`idle_turn_retries`), which resets when the
seat acts, when the revision advances, or when the run resumes. Once the budget is spent, an idle turn
stalls the run and a timeout pauses it. Late duplicate wakes for a seat the
environment is not waiting on are never re-prompted, so keep `waiting_for`
exact. A run also stalls when no seat is busy, no wake is
pending, and no human wait explains quiescence. A missing initial wake stalls an
agents-only run. Missing human wait metadata can misclassify a legitimate wait.
Timers in the host enforce limits and drain deliveries; they do not call domain
tick methods or produce scheduled environment turns. The environment cannot
forfeit or act for a slow seat. If a game needs a safe default move, expose it
as an ordinary action, such as pass, that the seat can choose.

Pause/resume/stop belong to the attached user. The builder that provisioned a run
may also pause, resume, or stop it (`pause_instance`/`resume_instance`/
`stop_instance`), and the table shows the status change. A builder resume of a
run whose host has closed is refused; re-enter the instance first. Builder provisioning is authorized
separately; its status/observation/transcript/table APIs are privileged read-only
diagnostics. Terminal finish/stall/recovery transitions inform the originating
builder, but this is not a domain action or a grant to modify a running instance beyond
pausing, resuming, or stopping it.
Per-seat Python namespaces are separate; local filesystem access is shared.
Docker is the available isolation option for adversarial or secret-state play.
