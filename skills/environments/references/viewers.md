# Viewers: replay and live pages for runs

A viewer is an optional web page, written for one environment, that draws a run's
state. The same page works two ways:

- **Replay**: a self-contained HTML export of an ended run, stepped through with
  the full state.
- **Live**: served by the run's host while it runs, following each committed
  operation from one perspective, filtered by the environment.

The harness supplies the data, a playback bar, and a small `env` bridge. The viewer
supplies every pixel: layout, pieces, animation, and charts are whatever suits that
environment. There is no shared game UI and no widget library.

## Optional, and off by default

**Do not build a viewer unless the user explicitly asks for one.** By default,
building an environment means the engine, its checks, and nothing else. Do not
offer a viewer during the design conversation, add one or `frame(perspective)` to
the design summary, or create one after a run unless the user asked for a visual
replay, a live view, a viewer, or a way to watch runs. When they do ask, read this
whole file first.

## Launching an existing viewer is the default

Building a viewer needs a request; using one does not. When the user asks for a
run (start one, rerun, "try it with three agents") and the definition already has
a saved viewer listing the run's version, assume they want to watch it. Launch the
viewer as part of the run without asking:

- **Run in progress**: right after `start_instance`, call
  `view_instance(instance=...)` and give the user the URL. It works only when the
  environment defines `frame(perspective)` and a saved viewer lists the version.
  If the call is refused, carry on without a viewer and say nothing about it.
- **Run already ended**: when the user asks to see or review a run that has ended,
  run `find <instance>`; if a viewer is `compatible`, run `export <instance>` (reuse
  a `current` export, re-export a `stale` one) and give the HTML path.

This changes nothing else. A run request is not a reason to build, edit, or
`add-version` a viewer, and it is not a reason to suggest one when none exists.
Registering a viewer for a version you just saved is part of that save; see the
next section. A user who asks for no viewer, or to run headless, gets none.
Starting a run still follows the usual token-use authorization.

## A new version of an environment that already has a viewer

Saving a new version does not update the viewer. Live view and replay open a
saved viewer only for versions listed in its `compatible_versions`. When the
definition you are changing already has one or more saved viewers, the new
version is not finished until every one of them is **compatible** with that
version and **registered** for it.

Do this after `save_environment` returns the hash, and before the changelog
entry:

1. **See which viewers exist.** They live under
   `<root>/viewers/<definition-id>/`, one folder per viewer. If that directory
   has no viewer, skip this section. Do not create one because you saved a
   version.
2. **Make each viewer compatible.** Compatible means the page draws this version
   correctly: the fields it reads are still there, masked live frames still
   render, and chapter boundaries still make sense. Check it on an instance of
   the new version — `export <instance> --trial` once the run has ended, or
   `view_instance` while it is running. If the snapshot or `frame` shape changed
   and the page is wrong, update the page, check it again, and `save-viewer`
   with `--approved`, passing `--version` for the new hash and for every older
   hash the updated page still supports. Ask the user before replacing a saved
   viewer, because the replacement changes every later export and live view.
3. **Register the version.** When the existing page already draws the new
   version, run `add-version <definition> <id> --verified-instance <instance>`
   for each viewer. The instance must be one of this version that you just
   checked. Registering is part of shipping the version: do it once the check
   passes, and leave no saved viewer omitting the hash.

`changelog-add` refuses the newest saved version while a viewer omits it. In the
changelog's Compatibility line, name each viewer and record that it was
re-checked and now lists this version. If you changed the page, say what changed.

## What exists

| | Viewer | Export |
| --- | --- | --- |
| What it is | Reusable page source for one environment definition | One run's self-contained replay page |
| Where | `<root>/viewers/<definition-id>/<viewer-id>/` | `<root>/instances/<instance-id>/viewers/<viewer-id>/` |
| Contents | `viewer.json`, `index.html`, local `.js`, `.css`, images | `index.html` (viewer, run data, playback bar), `provenance.json` |
| Names a run? | Never | Always |
| Pinned to | Definition versions it was checked on (`compatible_versions`) | The viewer revision and version it was built from |
| Edit it | Only with the user's approval | Never by hand; export again |

`<root>` is `~/.prime-env-harness/environments`. The viewer id is a short name
such as `replay`. A live view serves the saved viewer directly; nothing is copied.

## Where the data comes from

**Replay.** The harness appends every committed snapshot to the instance's
`snapshots.jsonl`. An export replays one step per committed operation, in order:
initialization, each action and text input, and rejected operations, whose state
is unchanged. The step's `state` is the environment's own snapshot. Exports cover
ended runs only (finished, or permanently stopped), because they show every
seat's private state. Runs recorded before snapshot history existed cannot be
replayed.

**Live.** The environment implements `frame(perspective)`, and the run's host
streams it after each commit. `perspective` is `{"kind": "public"}` or
`{"kind": "seat", "seat": "<id>"}`. A public frame holds only what every
spectator may see; a seat frame adds only that seat's private view. Filter it
exactly like `observe`, and check it in `checks.py`. It must return a JSON object
under 1 MiB, and validation calls it for every perspective. Without `frame`, the
run has no live view.

Give `frame` the snapshot's shape with private parts removed or masked, so one
viewer renders both replay and live. For example, replace hidden values with a
placeholder and drop hidden pools entirely.

A live page receives every step since its perspective was first opened on the
run's host, then follows new ones. Commits in quick succession can arrive as one
step.

Who sees what live:

| Requester | Perspectives |
| --- | --- |
| The human at the run's own table (`/env view`) | That human's seat (default), or `public` |
| The builder (`/env view <instance>`, `view_instance`) | `public` only |

No live perspective shows another seat's private state. The live server listens on
127.0.0.1 only, each URL carries a random token bound to one perspective, and
pages may load only their own viewer files.

## The bridge

The harness injects the bridge before the viewer's own scripts, so `env` is
available when they run.

```js
env.meta      // definition_id, name, version, instance_id, seats [{id, role, occupant, model}],
              // config, perspective ({kind: "omniscient"|"public"|"seat", seat?}), mode ("replay"|"live")
env.length    // number of steps so far; grows on a live page
env.live      // whether new steps can still arrive
env.frame(i)  // one step, computed on demand and safe to call for every i
env.onFrame((frame) => render(frame));  // called on the first step and on every position change
env.setMarks([{ index, label }]);       // chapter labels for the playback bar
env.seek(i); env.step(1); env.play(); env.pause(); env.toggle(); env.setSpeed(4);
env.nextMark(); env.previousMark(); env.index; env.playing;
```

A frame is:

| Field | Meaning |
| --- | --- |
| `index`, `sequence` | Position in the steps and the operation sequence it records |
| `state` | The snapshot (replay) or `frame(perspective)` (live) after this operation, deeply frozen |
| `previous` | The state before it (`null` on the first step), for transitions and "what changed" |
| `op` | `method`, `seat`, `action`, `arguments`, `text`, `success`, `result` (the error when rejected). Live, only the viewing seat's own operations; otherwise `null` |
| `deliveries` | Table events routed by this operation that the perspective may see |
| `time` | When the operation was dispatched or the step was streamed, in Unix milliseconds |
| `finished` | The run's public finish summary once it has finished, else `null` |
| `first`, `last` | Whether this is the first or last step so far |

States share unchanged parts with their neighbors, so never mutate them; copy what
you need to change. On a live page, a viewer that wants public moves described
must get them from the frame or from public notices, since `op` is withheld.

## Writing a viewer

Put `index.html` and its local files in a folder outside the draft, so the
environment's version does not change. Keep every resource local. Remote scripts,
fonts, and images are rejected and blocked. Reference local files with
`<script src>`, `<link rel="stylesheet" href>`, `<img src>`, and CSS `url(...)`.

- Render from `frame.state` only. Use `frame.previous` and `frame.op` to animate
  or describe the change.
- Handle every step: initialization, rejected operations, steps where nothing
  visible changes, and masked values in live frames.
- An operation can cross a boundary, for example when one move ends a round and
  starts the next. Use `previous` to show what the last move completed before
  the new state replaces it.
- Mark meaningful chapters with `env.setMarks`. On a live page `env.length` grows,
  so extend marks from `onFrame` rather than computing them once at load.
- Derive every name, count, and limit from the state, `env.meta`, or the config.
  Hard-code none, and never put an instance id or a version hash in the source.
- Never show model reasoning or transcripts. `op` holds the actions themselves.
- Leave room for the playback bar with `padding-bottom: var(--env-shell-height)`.
  Pass `--no-shell` when saving if the viewer draws its own controls.
- Work at phone width as well as on a desktop, and in light and dark color schemes.

Errors thrown by the viewer show in the playback bar and the browser console.

## Tool

`assets/viewers/viewer_tool.py` (resolve the path relative to this skill) does the
file work, uses only the standard library, prints JSON, and exits non-zero with a
`refused` message when a rule blocks the request.

| Command | Use |
| --- | --- |
| `find <instance>` | Whether the run ended, its recorded history, matching viewers, and existing exports (`current` or `stale`) |
| `export <instance> --source <folder> [--out <file>]` | Development: build a page from an unsaved viewer folder |
| `save-viewer <definition> <id> <folder> --title ... --version <hash>... [--verified-instance id]... [--no-shell] [--approved]` | Lint and save the viewer. Replacing one needs `--approved` |
| `add-version <definition> <id> --verified-instance <instance>` | Record that the viewer also works on another version |
| `export <instance> [--viewer id] [--trial] [--replace]` | Build the replay page for an ended run from a saved viewer |

Live views need the viewer saved and listing the run's version. Open one with
`/env view` (the user) or `await environments.view_instance(instance=...)`, which
returns a public URL to give the user.

## Workflow when the user asks for a viewer

1. **Look.** Run `find <instance>` on an ended run of the definition. If none
   exists, say a viewer needs one and offer to wait for the next run.
2. **Ask briefly what they want to see**, and whether they want to watch runs
   live. Recommend a view of the environment's main objects, what changed at
   each step, and chapter marks. Keep it to one short round.
3. **For live viewing, add `frame(perspective)`** to the environment with checks
   that each perspective hides what it must, then validate and save it. This is
   a new version. If a viewer already exists, that version still has to be
   registered on it, as in the section above.
4. **Build in a folder**, export with `--source`, open the page in a browser, and
   step through the whole run: the start, rejected moves, chapter boundaries, and
   the final step. Check a phone-width window too. For live viewing, also open a
   running instance with `view_instance` and check masked values.
5. **Save** with `save-viewer`, listing every version and instance you checked,
   then run `export <instance>` and give the user the HTML path.

For a version you did not just save, if the viewer does not list it, try
`export --trial`. If it replays correctly, ask the user whether to record the
version with `add-version`. A version you just saved is registered once that
check passes; see "A new version of an environment that already has a viewer".
Changing a saved viewer changes every later export and live view, so ask the
user before replacing one.
