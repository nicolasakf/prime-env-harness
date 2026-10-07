# Environment harness: future ideas

Recorded from discussion on 2026-10-01. These are ideas to explore, not approved
implementation work or settled architecture. Keep the current implementation
unchanged while discussing them.

The [current harness guide](docs/harness.md) records subsequent implemented work:
generic terminal forms, environment-controlled wakes, and supervised agent seats.
The ideas below remain historical discussion; participant seats now exist as
separate restricted sessions rather than general builder subagents.

## Custom human interfaces and visualizations

Humans could benefit from a visual view of an environment's state while they
participate. In a card game, the human could see their hand and click a card to
play it, rather than submitting every action through chat or a generic form.

The environment builder could generate a custom interface during environment
creation, alongside the environment implementation. Designers should have room
to choose a layout and interactions suited to their environment. The interface
should support both showing what is happening and letting humans take actions.

Questions to revisit:

- Where should generated interfaces run, and how should users open them?
- How should the interface receive observations and submit participant actions?
- How should it handle private information, state updates, and rejected actions?

## Prime subagents as environment participants

Explore using Prime's subagent system to create agents that participate in an
environment. In a card game, spawned agents could be players. Their actions
would cause the environment to react and update its state.

These participant agents have a different purpose from subagents that help the
builder implement an environment. The design should make each agent's purpose
explicit, even if both use the same underlying Prime agent machinery.

Questions to revisit:

- How should participant agents be created, attached, paused, and resumed?
- How should their actions and observations be coordinated within one instance?
- Which Prime features should participant agents inherit?

## Roles and different action access

An environment could have several agent roles. Examples include players,
moderators, and other kinds of participants chosen by the environment designer.
Different roles could access different functions and actions.

For example, a player might have `play_card`, while a moderator could have
functions for resolving a dispute or managing the game. These example moderator
functions are illustrative; their exact responsibilities remain open.

Questions to revisit:

- How should designers define roles and their available functions/actions?
- Should observations and visible state also differ by role?
- How should human participants fit into the same role system?

## Configurable environment inputs

Designers could define configuration inputs when building an environment, so
users can run variations of the same saved definition. For example, a card game
could accept the number of rounds to play.

Questions to revisit:

- Should users provide these inputs when starting an instance, or at another stage?
- How should designers specify input types, defaults, and valid ranges?
- Should an instance retain its original inputs on resume, and which inputs could
  change while it is running?

## Relationship to the current version

Current V1 supports one human and one model participant per instance, with chat,
observations, and generic action forms. Custom generated interfaces, multiple
model participants, and role-based agent orchestration would expand that scope.
No implementation is requested for these ideas yet.
