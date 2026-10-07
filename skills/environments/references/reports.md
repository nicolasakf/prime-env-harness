# Reports: templates and instance reports

When the user asks for a report on an environment or on a run, you work with two
different kinds of object. Keep them apart.

| | Report template | Instance report |
|---|---|---|
| What it is | The reusable, generic report for one environment definition | One run's executed copy of a template |
| Where | `<root>/report-templates/<definition-id>/<template-id>/` | `<root>/instances/<instance-id>/reports/<template-id>/` |
| Contents | `template.json`, `report.ipynb` (outputs stripped, one parameters cell) | `report.ipynb` (executed), `report.html`, `template.json`, `provenance.json` |
| Names a run? | Never. No instance ids, seat names, totals or counts | Always: `RUN_DIR` is set to its instance |
| Pinned to | Exact definition versions it was verified on (`compatible_versions`) | The template revision and version it was generated from |
| Edit it | Only with the user's approval | Freely; the edit is recorded as a divergence |

`<root>` is `~/.prime-env-harness/environments`. The template id is a short readable
name such as `game-report`. A template's `revision` is `v1`, `v2`, and so on.

A **definition** is what `save_environment` stores; a **version** is its content
hash; an **instance** is one run of a version. A template is about a definition and
lists the versions it works on. It is never about one instance.

## The rule for any change

Ask: *would this edit be correct for every finished run of this version?*

- **Yes** (a new chart, a bug fix, a clearer table): edit the template. Ask the user
  first, because every future report changes with it.
- **No** (a one-off question, an annotation, a workaround for this run's data):
  edit only the instance copy, then record why with `record-divergence`.

Never edit a template from inside an instance folder, and never put an instance id,
seat name, total or output in a template.

## Tool

`assets/reports/report_tool.py` (resolve the path relative to this skill, not to the
working directory) does the file work and enforces the rules above. It uses only the
standard library, prints JSON, and exits non-zero with a `refused` message when a
rule blocks the request. Run it from the REPL with
`subprocess.run([sys.executable, path, ...])`. Executing a notebook needs `nbconvert`,
`nbclient`, `ipykernel`, and whatever the template lists under `requires`; the tool
names any missing package.

| Command | Use |
|---|---|
| `find <instance>` | Templates for the instance's definition, which list its version, and its existing reports (current, stale, diverged) |
| `instantiate <instance> [--template id] [--trial] [--replace]` | Copy the template in, set `RUN_DIR`, run it, export HTML without code cells, write provenance |
| `save-template <definition> <id> <notebook> --title ... --version <hash>... [--verified-instance id]... [--approved]` | Lint, strip outputs, save as a template. Replacing an existing template needs `--approved` and resets the verified versions to the `--version` values |
| `add-version <definition> <id> --verified-instance <instance>` | After a passing trial, record that the template also works on another version |
| `record-divergence <instance> <id> "<reason>"` | Say why an instance copy differs from its template |

## Workflow when the user asks for a report

1. **Identify the target.** An instance id means one run. An environment name
   means a template (and, if they also want results, a report on its runs). If it
   is unclear, ask.
2. **Look.** Run `find <instance>`. Do not write anything before this.
   - `finished` is false: stop. Unfinished runs hold live private information;
     tell the user the run must finish first.
   - An existing report that is `current` and not `diverged`: offer the existing
     `report.html` instead of regenerating it.
3. **A template lists this version:** run `instantiate <instance>`. If more than
   one matches, ask which one. Report the HTML path. Mention any `stale` older
   reports so the user can regenerate them.
4. **A template exists, but not for this version:** run
   `instantiate <instance> --trial`.
   - It passes: tell the user, and ask whether to add the version. If they agree, run
     `add-version`, then `instantiate --replace` (without `--trial`).
   - It fails: the version differs in a way the template does not handle. Do not
     patch the instance copy to force it through. Create a new template (step 5).
5. **No template:** hold a short design conversation first. Ask what the user wants
   measured. Build the report as a notebook against this run's saved data
   (`checkpoint`, `table_log`, `seat_transcript`, `instance_status`), and make it
   generic as you go:
   - one cell marked `# report-parameters` containing `RUN_DIR = ""`;
   - derive names, counts and totals from the run; hard-code none;
   - reject unfinished runs and unsupported schema versions clearly;
   - reuse the environment's own accounting or replay checks; do not reinvent them;
   - read private state (hidden information, transcripts) only after the run has finished, and
     never show raw model reasoning.
   Then verify it on at least one finished instance, save it with `save-template`
   (listing the versions and instances you verified), and finish with `instantiate`.
6. **Deliver.** Give the user the `report.html` path. Say which template and
   revision produced it.

An instance report is disposable: it can be regenerated from its template at any
time. A template is the asset to protect.

## What the guard cell does

Templates should include a guard cell that reads the `template.json` beside the
notebook and refuses to run on another definition, or on a version the template does
not list unless the copy is marked `"trial": true`. When the notebook is run
standalone (no `template.json`), the guard skips the version check.
