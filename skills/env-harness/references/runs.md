# Configure and inspect runs

Choose the domain roles when building the definition. Choose human/agent occupants,
agent models, and run limits when entering an instance. The environment receives
seat IDs and roles; occupant and model choices stay on the host.

Protocol 2 supports 1–16 total seats, with zero or one human. The default roster
fills each role's minimum and assigns its first eligible seat to a human. For
agents-only runs of an occupant-flexible definition, supply an explicit roster.
One human alone and one agent alone are supported if all role minima permit it.

## Human plus models

For a definition with a `player` role allowing 1–16 human or agent occupants:

```text
/env enter relay local {"seats":[{"id":"you","role":"player","occupant":"human"},{"id":"alice","role":"player","occupant":"agent"},{"id":"bob","role":"player","occupant":"agent"}],"limits":{"max_operations":100}}
```

This explicit-roster command starts directly with setup defaults. It bypasses
the setup form. For configuration and roster together, use the full request:

```text
/env enter relay {"policy":{"backend":"local"},"roster":{"seats":[{"id":"you","role":"player","occupant":"human"},{"id":"alice","role":"player","occupant":"agent"},{"id":"bob","role":"player","occupant":"agent"}],"limits":{"max_operations":100}},"setup_arguments":{"rounds":2}}
```

Use the manifest's actual roles. Omitted seat IDs become `<role>-<index>`.
Optional agent `model` uses a provider/model identifier accepted by the builder's
catalog; optional `thinking` uses a supported thinking-level value. Omitted
choices inherit the builder's selection. Human seats cannot specify either.
The current form chooses execution policy and domain configuration; it does not
edit a roster or per-seat model selections.

At a human table, chat calls handle_input, action forms invoke domain actions,
and `/env msg` uses the separate private seat-message channel. `/env observe`
and `/env status` show the human's allowed observation. Notifications targeting
the human display in the table. The human submits their own action; a wake flag
does not press UI buttons or supply human decisions.

## Agents only and spectators

```text
/env enter relay local {"seats":[{"id":"alice","role":"player","occupant":"agent"},{"id":"bob","role":"player","occupant":"agent"}],"limits":{"max_operations":100}}
/env leave
/env runs
/env watch <instance-id>
```

The attached user watches explicitly public deliveries. Private targeted notices,
observations, action results, and assistant text stay off that table. Spectators
can inspect status/seats and pause, resume, or stop; they cannot act or message
as participants. Leaving a spectator table keeps the run alive. Leaving a human
table pauses the run, stops seat sessions, releases the instance, and returns to
the builder; `/env resume <instance-id>` restores it later. Resuming a paused run
may still require `/env resume_run` to restart dispatch; inspect status.

`/env watch` is restricted to agents-only instances. Multiple human seats and a
separate spectator view for tables with a human seat are unsupported.
Environment initialization must emit the first agent wake. Missing wakes or
unexplained quiescence produce a stalled run rather than inventing turns.

## Builder provisioning and reads

Run only after the user authorized the roster's token use and policy. Existing
authorization suffices; there is no additional approval for unchanged choices.

```python
run = await environments.start_instance(
    definition="relay",
    seats=[{"id": "alice", "role": "player", "occupant": "agent"},
           {"id": "bob", "role": "player", "occupant": "agent"}],
    config={"rounds": 2},
    limits={"max_operations": 100, "max_wakes_per_seat": 20},
)
instance = run["instance_id"]
await environments.list_instances()
await environments.instance_status(instance=instance)
await environments.pause_instance(instance=instance)
await environments.resume_instance(instance=instance)
await environments.observe_seat(instance=instance, seat="alice")
await environments.seat_transcript(instance=instance, seat="alice", since=0)
await environments.table_log(instance=instance, since_seq=0)
```

Omitted policy uses the user-selected validation policy. A supplied policy can
narrow it; it cannot change backend, add grants, or increase resource/time bounds.
This restriction applies to builder provisioning. A human can explicitly select
a new policy for a new instance with `/env enter`.

The creating builder's identity and capability authenticate privileged reads.
`observe_seat` returns that seat's filtered observation; `seat_transcript` returns
raw private transcript lines after a line offset; `table_log` returns the full
delivery log after a sequence, including targeted events. These are debugging
privileges, not spectator visibility. Do not disclose inspected private content
to a player when the environment's rules require secrecy. Offline reads can
inspect saved state after ownership is released; they never replay actions.
The Python builder API has no act or participant tools. The builder that
provisioned an instance can `pause_instance` it, `resume_instance` a paused or
stalled one (the same retained-wake replay as `/env resume_run`; cumulative limits
still apply), or `stop_instance` it (permanent). All reach a live host with the
builder capability. Once the host has closed, pause and stop mark the persisted
run, but resume is refused until the user re-enters the instance.
The user can do the same from the builder with `/env pause` and `/env stop`.
Finish, stall, and recovery transitions send an informational message to the
originating builder. That message does not authorize mutation.

## Limits and restart semantics

| Run limit | Default |
| --- | --- |
| `max_operations` | 500 mutations after initialization |
| `max_wakes_per_seat` | 100 for each agent |
| `max_wall_clock_seconds` | 7200 elapsed seconds from start, pauses included |
| `seat_turn_timeout_seconds` | 120 seconds to act while `waiting_for` lists the seat, restarting after each action; a turn past it without acting is aborted and retried with thinking off. Must be positive; a large value effectively disables it |
| `max_output_tokens` | 8192 per model request, thinking included; `null` leaves the provider default. A turn that hits it without acting is retried with thinking off |
| `idle_turn_retries` | 2 re-prompts for an awaited seat whose turn ended without progress; shared with deadline retries; 0 disables |
| `stall_turns` | 0 selects `2 * agent_count + 2` |

Limits go in the roster's `limits` object at `/env enter` or `start_instance`
and are saved with the instance. Omitted fields take the defaults above. They are
run settings, not definition or policy fields, so the same definition can run
with short limits for a probe and long ones for a full match. A long run can
exceed the defaults, so estimate its operations, wakes, and duration and
propose limits before starting it. Each seat's prompt states its deadline and
token cap.

A seat that already acted is not retried when the deadline passes; its leftover
turn is cut and counted as a post-action abort. The retry budget resets when the
seat acts, when the revision advances, or on resume. Once spent, an idle turn
stalls the run and a timeout pauses it. `instance_status` reports per-seat
`time_to_action` (count, total, and maximum milliseconds), `deadline_retries`,
`post_action_aborts`, and `idle_retries`.

Run limits are separate from execution policy call/preparation/resource bounds.
Operation, wake, wall-clock, and turn limits pause dispatch. No revision progress
or unexplained quiescence stalls it. Resume clears the no-progress counter and
busy bookkeeping, and admits retained wakes. When resuming a stalled run whose
awaited agents consumed their wakes without advancing the environment, it sends
those agents a fresh retry notice using the saved `waiting_for` state. It does
not retry game actions or reset elapsed time, operation totals, wake totals, or
environment state. A cumulative limit may pause
again immediately. Stopped and finished runs cannot resume. Recovery-required
runs need inspected reconciliation before normal resume.

Durable inbox acceptance precedes host cursor acknowledgment. Wakes can coalesce;
the model receives unseen deliveries and its current observation. Host or seat
restart can replay an admitted wake, while operation IDs protect recorded actions
from duplicate dispatch. New model tool-call IDs still create new operations;
domain rules must prevent duplicate logical submissions. An uncertain mutation
is never automatically replayed. Do not offer reset/retry as a substitute for
recovering evidence about external effects.
