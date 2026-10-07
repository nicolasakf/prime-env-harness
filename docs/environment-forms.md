# Human environment forms

The harness uses one terminal panel for human actions and environment setup.
Click `[ Actions ]` above the participant prompt, or submit `/env act`, to open
available human actions. Choosing an action opens its form. `/env act <name>`
opens that form directly. Existing commands with explicit JSON still work.

The panel replaces the prompt while open and owns a separate input buffer.
Opening it through the Actions button preserves a draft chat message. Selecting
an action or editing a field does not execute anything: activate the action's
submit button to send it. Escape closes the panel. Success closes it; a rejection
keeps the entered values and shows the error. A pending request blocks duplicate
submission. A transport failure blocks further submits in that panel because its
outcome may be uncertain; close it to inspect `/env status` or use `/env leave`.

| Input schema | Terminal control |
| --- | --- |
| `string` | Text editor; Shift+Enter adds a newline |
| `integer` / `number` | Number editor; Up/Down increase/decrease |
| `boolean` | Clickable checkbox; Space toggles |
| Primitive `enum` | Clickable single-choice list; arrows move, Space chooses |
| `array` with `items.enum` and `uniqueItems: true` | Multiple-choice checkboxes |
| Other shapes | JSON editor for that field |

Object properties become fields. Nested objects and complex arrays use a JSON
editor without forcing their simple sibling fields into JSON. Unsupported root
schemas use a whole-input JSON editor. The host still performs complete JSON
Schema validation; the panel catches invalid numbers, bounds, choices and lengths
before submitting. It uses `title`, `description`, `default`, and `required` from
the existing schema. Required fields appear first in a stable order. Choices
retain their typed values, even when their displayed labels are humanized.

Tab/Shift+Tab move between fields and the submit/cancel buttons. Enter advances
from a field and activates a focused button. Number/text fields retain the normal
editor's editing shortcuts. Optional fields can be included or omitted; the form
preserves false, zero, an empty string, and absence as different values. Defaults
are visible. A required enum without a default starts unselected. Large forms and
choice lists show a bounded window around the focused field or option.

## Setup

Submitting `/env enter <id>` opens the same panel, with an explicit local/Docker
choice and any declared setup fields. No backend is silently selected. With
`/env enter <id> local` or an explicit policy JSON, the backend is already selected;
the form still opens when the definition declares setup parameters. Execution
policy is kept separate from environment parameters, and gameplay fields cannot
change grants. Docker remains optional and never falls back to local automatically.
Resume restores an instance without showing setup or initializing again.

An environment declares its parameters in `environment.json`:

```json
{
  "setup_schema": {
    "type": "object",
    "properties": {
      "cards": {"type": "integer", "title": "Cards per player", "minimum": 1, "maximum": 20, "default": 5},
      "trades": {"type": "integer", "title": "Maximum trades", "minimum": 0, "default": 2},
      "allow_passing": {"type": "boolean", "title": "Allow passing", "default": true}
    },
    "required": ["cards", "trades"],
    "additionalProperties": false
  }
}
```

The host applies property defaults, validates the parameters before creating an
instance, passes them to `initialize(params)` (protocol 2 uses `params.config`
alongside `params.seats`; protocol 1 receives parameters directly), and records them in the durable
initialization receipt. Automatic environment validation initializes with schema
defaults, so give required setup parameters defaults. Definitions without a setup
schema use empty configuration (protocol 2 still receives the seats/config
wrapper). The daemon accepts `setup_arguments` in
its existing `env enter` request alongside the separate `policy`.

The form does not edit rosters or per-seat models. Explicit roster commands start
directly with setup defaults. For parameters and roster together, use
`/env enter <id> {"policy":{"backend":"local"},"roster":{"seats":[...]},"setup_arguments":{...}}`.
See the [harness guide](harness.md#participants-rosters-and-models) for complete
examples and supported human/agent arrangements.

## Ownership

The private `pa-tui::environment_panel` component owns controls, focus, rendering,
and values. Session UI sends the existing daemon commands. `pa-types` owns the
additive optional manifest field. `pa-environments::EnvironmentManager` adds
`enter_with_arguments` and owns default application, validation and persistence;
`enter` retains its existing API. `pa-daemon` passes the parameters through.
The dependency direction and builder behavior remain unchanged. There are no new
dependencies or public TUI APIs.

Adoption uses the existing `agent feature outcome` event with the catalogued
`environment_action_form` and `environment_setup_form` feature values. Open,
success, rejection and dismissal emit initiated/completed/failed/canceled.
Telemetry includes no action names or entered values.

Choice lists use the action schemas already published by the environment. This
change does not add a live, participant-specific choice protocol; environments
must keep public action schemas free of secret state. The environment remains
responsible for legality, and a stale choice can be rejected without losing the
form draft. Automatic field grouping, custom widgets, file uploads and dynamic
participant-specific pickers are outside this first version.
