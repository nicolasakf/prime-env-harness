#!/usr/bin/env python3
"""Report templates and instance reports for the environment harness (stdlib only).

Two kinds of object live under the environments root (default
~/.prime-env-harness/environments):

  report-templates/<definition-id>/<template-id>/   reusable, generic, no outputs
      template.json   id, definition_id, compatible_versions, revision (v1, v2, ...), content_hash, ...
      report.ipynb    one parameters cell (marked `# report-parameters`)
  instances/<instance-id>/reports/<template-id>/    one run's executed copy
      report.ipynb    executed copy with outputs
      report.html     exported without code cells
      template.json   the template metadata the copy was made from
      provenance.json which template revision produced it, and when

Commands print JSON. Exit status is non-zero when the request is refused.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

PARAM_MARKER = "# report-parameters"
PARAM_LINE = f'RUN_DIR = ""  {PARAM_MARKER}'
UUID = re.compile(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
HASH = re.compile(r"\b[0-9a-f]{64}\b")
ANSI = re.compile(r"\x1b\[[0-9;]*m")
DEFAULT_ROOT = Path.home() / ".prime-env-harness" / "environments"
DEFAULT_REQUIRES = ["nbconvert", "nbclient", "ipykernel"]


class Refused(Exception):
    """A request the rules do not allow; the message tells the builder what to do instead."""


def read_json(path: Path):
    return json.loads(path.read_text())


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def source(cell) -> str:
    text = cell["source"]
    return "".join(text) if isinstance(text, list) else text


def is_parameters_cell(cell) -> bool:
    return cell["cell_type"] == "code" and PARAM_MARKER in source(cell)


def strip_notebook(nb: dict) -> dict:
    """Drop outputs, execution counts and run-specific metadata so a template carries no run data."""
    out = json.loads(json.dumps(nb))
    for cell in out["cells"]:
        cell.pop("id", None)
        if cell["cell_type"] == "code":
            cell["outputs"] = []
            cell["execution_count"] = None
        cell["metadata"] = {}
    out["metadata"] = {k: v for k, v in nb.get("metadata", {}).items() if k in ("kernelspec", "language_info")}
    return out


def content_hash_of(nb: dict) -> str:
    """Hash of the notebook's sources with the parameters cell normalised; detects real edits."""
    cells = [(c["cell_type"], PARAM_LINE if is_parameters_cell(c) else source(c)) for c in nb["cells"]]
    return hashlib.sha256(json.dumps(cells).encode()).hexdigest()[:16]


def lint(nb: dict) -> list[str]:
    problems = []
    marked = [c for c in nb["cells"] if is_parameters_cell(c)]
    if len(marked) != 1:
        problems.append(f"need exactly one code cell marked `{PARAM_MARKER}` (found {len(marked)})")
    for i, cell in enumerate(nb["cells"]):
        text = source(cell)
        if UUID.search(text) or HASH.search(text):
            problems.append(f"cell {i} contains an instance id or version hash; templates must stay generic")
    return problems


def environments_paths(root: Path, instance: str) -> tuple[Path, dict]:
    path = root / "instances" / instance
    if not (path / "instance.json").exists():
        raise Refused(f"no instance {instance!r} under {root / 'instances'}")
    return path, read_json(path / "instance.json")


def templates_for(root: Path, definition: str) -> list[dict]:
    base = root / "report-templates" / definition
    found = []
    for meta in sorted(base.glob("*/template.json")) if base.exists() else []:
        found.append(read_json(meta))
    return found


def current_template(root: Path, definition: str, template: str) -> dict | None:
    """The saved template's readable revision and the hash of its notebook as it is on disk now."""
    base = root / "report-templates" / definition / template
    if not (base / "template.json").exists():
        return None
    return {"revision": read_json(base / "template.json")["revision"],
            "content_hash": content_hash_of(read_json(base / "report.ipynb"))}


def instance_reports(root: Path, instance: str, definition: str) -> list[dict]:
    rows = []
    for prov_path in sorted((root / "instances" / instance / "reports").glob("*/provenance.json")):
        prov = read_json(prov_path)
        notebook = prov_path.parent / "report.ipynb"
        copy_hash = content_hash_of(read_json(notebook)) if notebook.exists() else None
        latest = current_template(root, definition, prov["template_id"])
        rows.append({
            "template_id": prov["template_id"], "path": str(prov_path.parent),
            "generated_at": prov["generated_at"], "trial": prov.get("trial", False),
            "template_revision_used": prov["template_revision"],
            "template_revision_now": latest and latest["revision"],
            "state": "template missing" if latest is None else
                     "current" if prov["template_content_hash"] == latest["content_hash"]
                     else "stale: template changed since this run",
            "diverged": copy_hash != prov["template_content_hash"],
            "divergence_reason": prov.get("divergence_reason"),
        })
    return rows


def cmd_find(root: Path, args) -> dict:
    path, info = environments_paths(root, args.instance)
    definition, version = info["definition_id"], info["version"]
    finished = False
    if (path / "checkpoint.json").exists():
        finished = bool(read_json(path / "checkpoint.json").get("finished"))
    templates = [{"id": t["id"], "title": t.get("title"), "revision": t["revision"],
                  "compatible": version in t["compatible_versions"],
                  "compatible_versions": t["compatible_versions"]}
                 for t in templates_for(root, definition)]
    return {"instance": args.instance, "definition_id": definition, "version": version, "finished": finished,
            "templates": templates, "existing_reports": instance_reports(root, args.instance, definition)}


def cmd_save_template(root: Path, args) -> dict:
    nb = strip_notebook(read_json(Path(args.notebook)))
    problems = lint(nb)
    if problems:
        raise Refused("template rejected: " + "; ".join(problems))
    for i, cell in enumerate(nb["cells"]):
        if is_parameters_cell(cell):
            nb["cells"][i]["source"] = PARAM_LINE
    dest = root / "report-templates" / args.definition / args.template
    exists = (dest / "template.json").exists()
    if exists and not args.approved:
        raise Refused(f"template {args.template!r} already exists. Editing a reusable template needs the user's "
                      "approval; ask them, then re-run with --approved. Versions it was verified against are reset to "
                      "the --version values you pass.")
    content_hash = content_hash_of(nb)
    previous = read_json(dest / "template.json") if exists else None
    if previous is None:
        revision = "v1"
    elif previous["content_hash"] == content_hash:
        revision = previous["revision"]
    else:
        revision = f"v{int(previous['revision'].removeprefix('v')) + 1}"
    meta = {"id": args.template, "title": args.title, "definition_id": args.definition,
            "compatible_versions": sorted(set(args.version)), "revision": revision, "content_hash": content_hash,
            "requires": sorted(set(DEFAULT_REQUIRES + args.requires)),
            "verified_against": [{"instance": i} for i in args.verified_instance],
            "saved_at": datetime.now(timezone.utc).isoformat(timespec="seconds")}
    write_json(dest / "report.ipynb", nb)
    write_json(dest / "template.json", meta)
    return {"saved": str(dest), "revision": meta["revision"], "replaced_existing": exists}


def cmd_add_version(root: Path, args) -> dict:
    meta_path = root / "report-templates" / args.definition / args.template / "template.json"
    if not meta_path.exists():
        raise Refused(f"no template {args.template!r} for {args.definition!r}")
    meta = read_json(meta_path)
    _, info = environments_paths(root, args.verified_instance)
    if info["definition_id"] != args.definition:
        raise Refused("the verifying instance belongs to a different definition")
    if info["version"] not in meta["compatible_versions"]:
        meta["compatible_versions"] = sorted(meta["compatible_versions"] + [info["version"]])
    meta["verified_against"].append({"instance": args.verified_instance})
    write_json(meta_path, meta)
    return {"template": args.template, "compatible_versions": meta["compatible_versions"]}


def run_nbconvert(directory: Path, *arguments: str) -> None:
    result = subprocess.run([sys.executable, "-m", "jupyter", "nbconvert", *arguments], cwd=directory,
                            capture_output=True, text=True)
    if result.returncode != 0:
        lines = ANSI.sub("", result.stderr).strip().splitlines()
        raise Refused("notebook run failed: " + " | ".join(line.strip() for line in lines[-2:] if line.strip()))


def cmd_instantiate(root: Path, args) -> dict:
    path, info = environments_paths(root, args.instance)
    definition, version = info["definition_id"], info["version"]
    candidates = templates_for(root, definition)
    if args.template:
        candidates = [t for t in candidates if t["id"] == args.template]
        if not candidates:
            raise Refused(f"no template {args.template!r} for {definition!r}")
    compatible = [t for t in candidates if version in t["compatible_versions"]]
    trial = False
    if not compatible and args.trial and candidates:
        compatible, trial = candidates[:1], True
    if not compatible:
        raise Refused(f"no report template for {definition!r} lists version {version[:12]}. Existing: "
                      f"{[(t['id'], [v[:12] for v in t['compatible_versions']]) for t in candidates]}. "
                      "Do a --trial run, or create a template.")
    if len(compatible) > 1:
        raise Refused(f"several templates match ({[t['id'] for t in compatible]}); pass --template")
    meta = compatible[0]
    missing = [m for m in meta["requires"] if importlib.util.find_spec(m) is None]
    if missing:
        raise Refused(f"missing Python packages for this template: {missing}. Install them in this Python "
                      f"({sys.executable}) and retry.")

    dest = path / "reports" / meta["id"]
    if dest.exists() and not args.replace:
        raise Refused(f"{dest} already exists; pass --replace to regenerate it")
    shutil.rmtree(dest, ignore_errors=True)
    dest.mkdir(parents=True)
    nb = read_json(root / "report-templates" / definition / meta["id"] / "report.ipynb")
    for cell in nb["cells"]:
        if is_parameters_cell(cell):
            cell["source"] = f"RUN_DIR = {json.dumps(str(path))}  {PARAM_MARKER}"
    write_json(dest / "report.ipynb", nb)
    write_json(dest / "template.json", dict(meta, trial=trial))

    try:
        run_nbconvert(dest, "--to", "notebook", "--execute", "--inplace", "report.ipynb",
                      f"--ExecutePreprocessor.timeout={args.timeout}")
        run_nbconvert(dest, "--to", "html", "--no-input", "report.ipynb")
    except Refused:
        shutil.rmtree(dest, ignore_errors=True)  # a failed run leaves no half-made report behind
        raise
    write_json(dest / "provenance.json", {
        "template_id": meta["id"], "template_revision": meta["revision"], "template_content_hash": meta["content_hash"],
        "definition_id": definition,
        "version": version, "instance_id": args.instance, "trial": trial,
        "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds")})
    return {"report_dir": str(dest), "html": str(dest / "report.html"), "notebook": str(dest / "report.ipynb"),
            "template_revision": meta["revision"], "trial": trial}


def cmd_record_divergence(root: Path, args) -> dict:
    prov_path = root / "instances" / args.instance / "reports" / args.template / "provenance.json"
    if not prov_path.exists():
        raise Refused(f"no instance report {args.template!r} for {args.instance!r}")
    prov = read_json(prov_path)
    prov["divergence_reason"] = args.reason
    write_json(prov_path, prov)
    return {"recorded": args.reason}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT, help="environments root")
    sub = parser.add_subparsers(dest="command", required=True)

    find = sub.add_parser("find", help="templates for an instance's definition/version, plus its existing reports")
    find.add_argument("instance")

    save = sub.add_parser("save-template", help="lint, strip outputs and save a reusable template")
    save.add_argument("definition")
    save.add_argument("template")
    save.add_argument("notebook", help="a finished development notebook; it is copied, not moved")
    save.add_argument("--title", required=True)
    save.add_argument("--version", action="append", required=True, help="a version hash it was verified on (repeatable)")
    save.add_argument("--verified-instance", action="append", default=[])
    save.add_argument("--requires", action="append", default=[], help="extra importable Python packages")
    save.add_argument("--approved", action="store_true", help="the user approved replacing an existing template")

    add = sub.add_parser("add-version", help="record that a template also works on another version")
    add.add_argument("definition")
    add.add_argument("template")
    add.add_argument("--verified-instance", required=True, help="a passing instance of the new version")

    inst = sub.add_parser("instantiate", help="copy a template into the instance, set RUN_DIR, run, export HTML")
    inst.add_argument("instance")
    inst.add_argument("--template")
    inst.add_argument("--trial", action="store_true", help="try a template that does not list this version yet")
    inst.add_argument("--replace", action="store_true", help="regenerate an existing instance report")
    inst.add_argument("--timeout", type=int, default=600, help="seconds allowed per cell")

    div = sub.add_parser("record-divergence", help="say why an instance copy was edited away from its template")
    div.add_argument("instance")
    div.add_argument("template")
    div.add_argument("reason")
    return parser


COMMANDS = {"find": cmd_find, "save-template": cmd_save_template, "add-version": cmd_add_version,
            "instantiate": cmd_instantiate, "record-divergence": cmd_record_divergence}


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        print(json.dumps(COMMANDS[args.command](args.root.expanduser(), args), indent=2))
    except Refused as refusal:
        print(json.dumps({"refused": str(refusal)}, indent=2))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
