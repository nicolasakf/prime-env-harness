# Design conversation

Most of an environment's cost is set before any code exists: who acts when,
what each seat sees, how long a run lasts, and what happens when a model is slow
or never acts. Raise those decisions with the user at creation time, so they
are made on purpose instead of discovered in the first stalled run. The user
can make each one now or defer it.

## How to run it

1. Read the description and note what it already answers. Do not ask about
   those points again.
2. Ask the open questions in two or three short rounds, most consequential
   first: rules, participants, and turn flow; then length, configuration, and
   agent context; then pacing, failure handling, and analysis. Do not send all
   of the questions at once.
3. Give each question your recommendation and the default you will use if the
   user defers. "Decide later" is a valid answer. Record it as an open item; do
   not push for an answer.
4. Before writing code, summarize the decisions and open items in a few lines
   and ask the user to confirm. Bring open items back when they become concrete:
   at validation, before the first run, or after the first run shows the need.
5. Keep the execution policy question separate. It always requires an explicit
   answer before validation (see the skill's policy section).

Skip topics that do not apply. A one-seat document workflow has no turn order
or opponent privacy.

## Rules and outcome

- Which rules are ambiguous or interact in ways the description does not
  settle? Ask about edge cases: ties, acting against oneself, simultaneous
  claims, and moves that are legal but pointless.
- How is the outcome or score computed, exactly? Payoffs, scoring, and win
  conditions must be precise enough to implement and test.
- What happens to an illegal move? Usually a rejection with a reason. The seat
  keeps its turn and the state does not change.

Why: the environment code is authoritative. An unsettled rule becomes a guess
in code that models will exploit or misread.

## Participants

- Which roles exist, and how many seats does each allow?
- Which occupant mixes must work: one human alone, a human against agents,
  agents only? Recommend one occupant-flexible role when the same definition
  should serve both human-vs-agent and agent-vs-agent runs.
- Do seats differ only by position, such as who starts? Then use seat order
  from the roster, not separate roles.

Limits: 1–16 seats and at most one human. Models and thinking levels are chosen
per seat when a run starts, not in the definition.

## Turn flow and wakes

- Is play sequential, simultaneous, or free-form? Who acts first, and does the
  starting seat rotate between rounds or episodes?
- After each kind of move, which seat acts next? That seat, and only that seat,
  gets a wake. A human should never have to type "your turn" or "go on".
- Is there always a safe legal action, such as pass, fold, or hold? Without
  one, a seat that cannot decide either makes a bad move or stalls the run.
- What counts as progress? Advance the `progress` revision on real domain
  progress and list waiting seats, including the human, in `waiting_for`. The
  harness uses both for re-prompts and stall detection.

The environment cannot set timers or act on a seat's behalf. Anything that
happens must follow from an action, a text input, or initialization.

## Length, episodes, and configuration

- How does a run end? Finish after a fixed count, at a score threshold, or when
  a terminal state is reached?
- Does a run repeat episodes (rounds, hands, matches)? What resets between them
  and what carries over, such as cumulative score?
- Which parameters should be configurable per run? Put them in `setup_schema`
  with titles, bounds, and defaults. Note derived quantities, such as total
  turns per episode, that players need to plan.

Every agent seat's system prompt automatically includes the instance's actual
setup configuration. So write `instructions` in terms of parameter names and
never repeat numeric defaults there: a seat would see two different numbers.
If derived values matter, compute them in the observation or `agent_view`.

Estimate operations and wakes per run (actions per episode × episodes). A long
run can exceed the default run limits, so recommend limits the user can pass
when starting it.

## Information and communication

- What is private to each seat, what is public, and what does a spectator see?
  Public `notify("*", ...)` notices must stay safe when every seat is an agent.
- When does hidden information become public, for example at the end of an
  episode?
- Should seats talk? `say` goes through `handle_input`, so the environment
  decides visibility. `message_seat` is a private side channel, enabled by
  default. Set `direct_messages: false` when all communication must pass
  through the rules.
- Tell agents in `instructions` what other seats can see of their messages.
  Their replies and thinking are private.

## Agent context

- How many turns will each agent seat take in a long run? Does a model need
  its own earlier reasoning, or can it decide from the current state plus a
  summary?
- In conversation mode (the default), the whole transcript is resent on every
  request. Cost grows with the square of run length. Recommend
  `"agent_context": "stateless"` for long runs or large observations.
- Under stateless context, design `agent_view(seat)`: the current state plus a
  bounded history, at most 32 KiB. Keep full detail for the current episode and
  one line per older episode. Fixed rules go in `instructions`, where they are
  cached. Action results and rejections both return the updated view.

## Pacing, deadlines, and budgets

These are run limits, chosen when a run starts, not definition fields. The
design should still anticipate them, and the builder should propose values
before the first run.

| Question | Run limit (default) |
| --- | --- |
| How long may a seat deliberate before acting? | `seat_turn_timeout_seconds` (120) |
| How many output tokens per model request, thinking included? | `max_output_tokens` (8192; `null` uses the provider default) |
| How many re-prompts for a seat that ends its turn without acting? | `idle_turn_retries` (2; 0 disables) |
| How long may the whole run take, paused time included? | `max_wall_clock_seconds` (7200) |
| How many mutations and wakes per seat? | `max_operations` (500), `max_wakes_per_seat` (100) |

Harness behavior to explain when asking:

- The deadline measures time to action while the environment waits on that
  seat, and restarts after each action. A seat that already acted is not
  retried.
- A turn that hits the deadline or the token cap without acting is retried
  with thinking off. A turn that ends without acting while still awaited is
  re-prompted. Both draw on `idle_turn_retries`. When it runs out, the run
  stalls (idle) or pauses (timeout) until someone resumes it.
- Higher thinking levels slow every move. Ask which levels the user expects,
  and suggest a short probe run before a long one.

Design implications: keep each decision small, so thinking is not spent on
bookkeeping. List the currently legal actions or bounds in the observation to
cut rejected moves. Provide a safe action where the game allows one.

## Analysis

- What will the user want to measure after runs? Examples: score per episode,
  every action with the state it was taken in, and why each episode ended.
- Record that in environment state or public end-of-episode notices, revealing
  hidden information only once it is public, so reports do not depend on
  scraping private transcripts.
- Reports: ask whether the user wants a reusable report template for this environment. If so, say so in
  the design summary; the builder creates it after the first finished run (see `reports.md`).
- The builder can also read `table_log`, `seat_transcript`, and
  `instance_status`. The status includes per-seat `time_to_action`, `deadline_retries`,
  `post_action_aborts`, and `idle_retries`.
