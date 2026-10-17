# Staying in your lane: environment changes, harness recommendations, changelogs

Every problem you meet while building or running an environment belongs to one of
two layers. Decide which before you change anything.

| Layer | What it is | You may change it? |
|---|---|---|
| Environment | One definition's rules, actions, observations, `agent_view`, `instructions`, setup schema, checks, dependencies, and its own helper modules | Yes. This is your job. |
| Harness | The Prime Env program and everything that serves every environment: the host, broker and runner, the SDK and runner copies inside a draft (`sdk.py`, `sdk.ts`, `runner.py`), prompt layers, skills, docs, the `environments` Python module, the TUI | **Never.** |

You build, validate, run, supervise, and edit environments. You do not edit the
harness, whether it is an installed binary, a source checkout you happen to be
standing in, or the copies of the SDK and runner that ship in each draft. A
change to harness code can break every environment and every running table, and
you cannot test it against them. The harness has its own maintainers and its own
review. Do not look for a way around this, such as patching a draft's `sdk.py`,
editing files under the skills directory, or monkey-patching the host from the
REPL.

## Telling the layers apart

Ask: *can I state the problem without naming this environment?*

- **No** (a rule is ambiguous, a move is mis-scored, a seat is told the wrong
  thing, a check is weak, an observation is too large): it is an environment
  problem. Fix it in the draft.
- **Yes** (the host drops a notice, an SDK call has no way to express something
  any environment would need, a run limit cannot be set, validation rejects valid
  JSON, a skill or prompt tells the builder something false): it is a harness
  problem.
- Unsure: treat it as an environment problem and look for an environment-level
  fix first. Most "harness bugs" turn out to be a contract the environment broke;
  re-read [the authoring contract](contract.md) before concluding otherwise.

## When it is a harness problem

1. Keep going if you can do so honestly. Use an environment-level workaround only
   if it is a sound design on its own merits, and say so in the changelog. Do not
   ship a workaround that hides a defect from the user or weakens a check.
2. Record a recommendation. Do not only mention it in chat; chat is lost.
3. Tell the user in one or two sentences: what you found, what you did meanwhile,
   and where the recommendation is saved. If the problem blocks the work and no
   sound workaround exists, stop and say so.

Recommendations are stored as one file each under
`<root>/harness-recommendations/` (`<root>` is `~/.prime-env-harness/environments`).
The user reviews that folder and turns accepted items into harness changes.

Write each recommendation so a maintainer who has never seen the environment can
act on it:

- **Problem**: the defect or missing capability, in generic terms. Do not use the
  environment's name or vocabulary here.
- **Evidence**: what you saw. Instance ids, the failing check or error text, the
  operation sequence. The environment and version it surfaced in are recorded
  separately.
- **Proposed harness change**: the generic behavior you suggest, and why it helps
  every environment. Say if you are unsure it is the right design.
- **Workaround** (optional): what you did in the environment meanwhile.

## The tool

`assets/maintenance/maintenance_tool.py` (resolve the path relative to this skill,
not to the working directory) does the file work. It uses only the standard
library, prints JSON, and exits non-zero with a `refused` message when a request
is not allowed. Run it from the REPL with `subprocess.run([sys.executable, path, ...])`.

| Command | Use |
|---|---|
| `recommend --title ... --problem ... --evidence ... --proposal ... [--workaround ...] [--definition id --version hash]` | Save a harness recommendation (`Status: open`) |
| `recommendations [--status open]` | List saved recommendations; check before writing a duplicate |
| `versions <definition>` | Saved versions, oldest first, whether each has a changelog entry, and which saved viewers do not list it |
| `changelog-add <definition> --version <hash> --change "..." [--change ...] [--why ...] [--compatibility ...]` | Add the entry for a version you just saved. Refuses the newest version while a saved viewer omits it |
| `changelog-show <definition>` | Print the changelog, list saved versions that have no entry, and list versions a saved viewer does not list |

## Changelog between versions

Saving a changed environment creates a new immutable version (a content hash).
Hashes say nothing about what differs. Every time you save a new version of an
existing definition, finish it in this order before you present it:

1. **Viewer.** If the definition has a saved viewer, that viewer must be
   compatible with the new version and registered for it. Live view and replay
   ignore a version the viewer does not list. The check, the page update, and
   `add-version` are in [viewers](viewers.md). Skip this only when
   `<root>/viewers/<definition-id>/` has no viewer. Do not create a viewer
   because you saved a version.
2. **Changelog.** Write a short entry for the hash `save_environment` returned.
   `changelog-add` refuses the newest saved version while a viewer omits it, so
   the entry comes after registration.

```python
import subprocess, sys
tool = "<this skill>/assets/maintenance/maintenance_tool.py"
subprocess.run([sys.executable, tool, "changelog-add", "my-environment",
                "--version", saved["version"],
                "--change", "Illegal bids now keep the turn instead of passing it.",
                "--change", "Added `max_rounds` to the setup schema (default 10).",
                "--why", "Two runs stalled when a seat's rejected bid ended its turn.",
                "--compatibility", "Report template needs re-verification: the round log gained a `rejected` record."])
```

The first version of a new definition gets an entry too (no `--change` needed;
it records "First saved version.").

The changelog lives at `<root>/changelogs/<definition-id>/CHANGELOG.md`, newest
version first. It is not inside the draft or the definition: a version is the
hash of its source, so a file inside the source cannot name its own version, and
editing one would create a new version.

Write the entry for a reader who knows the previous version and wants to know
what is different now:

- **Changes**: one bullet per difference a user, a seat, or a report author would
  notice: rules and scoring, actions and their arguments, what each role sees,
  setup parameters and defaults, the event log schema, dependencies. Not a diff
  of the code; do not list refactors that change nothing observable (say
  "internal cleanup" if you must).
- **Why**: the run, failure, or user request that caused it. One sentence.
- **Compatibility**: when a saved viewer exists, name it and record that it was
  re-checked on this version and now lists the hash. Also say whether report
  templates pinned to earlier versions still apply, and whether in-flight
  instances (which stay on their version) behave differently from new ones.
  Leave the line out only when there is no viewer and nothing else is affected.

Do not edit past entries except to correct a mistake. If a change belongs to the
same save, it is one entry. Run `changelog-show` when you are unsure whether the
log is up to date; it lists saved versions that have no entry, and versions a
saved viewer does not list. `changelog-add` enforces viewer registration on the
newest saved version only, so an entry for an older version can be written when
the current viewer never listed it. `versions` still reports that gap. To write
entries for versions saved before this practice, compare each version's source
with the one before it (`versions` gives the order) and add the entries in
order, oldest first, passing `--date` only if the save date is wrong. Register
the viewer before backfilling the newest version.
