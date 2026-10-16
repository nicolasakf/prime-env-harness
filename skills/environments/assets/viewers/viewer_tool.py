#!/usr/bin/env python3
"""Environment viewers: optional replay pages for ended runs (stdlib only).

A viewer is an environment-specific web page that draws a run's state. The
harness supplies the run data, a playback shell and the `env` bridge; the
viewer supplies every pixel. Two kinds of object live under the environments
root (default ~/.prime-env-harness/environments):

  viewers/<definition-id>/<viewer-id>/          reusable viewer source, no run data
      viewer.json   id, title, definition_id, compatible_versions, revision, content_hash, shell, ...
      index.html    entry page; local scripts, stylesheets and images beside it
  instances/<instance-id>/viewers/<viewer-id>/  one run's export
      index.html    self-contained replay page: viewer, run data, playback shell
      provenance.json

Commands print JSON. Exit status is non-zero when the request is refused.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import html
import json
import mimetypes
import re
import shutil
import sys
from datetime import datetime, timezone
from pathlib import Path

DEFAULT_ROOT = Path.home() / ".prime-env-harness" / "environments"
RUNTIME = Path(__file__).with_name("viewer_runtime.js")
DATA_FORMAT = 1
MAX_SOURCE_BYTES = 5 * 1024 * 1024
UUID = re.compile(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
HASH = re.compile(r"\b[0-9a-f]{64}\b")
REMOTE = re.compile(
    r"""(?:\b(?:src|href)\s*=\s*["']?|url\(\s*["']?|\bimport\s*\(?\s*(?:[^'"();]*\bfrom\s*)?["'])(?:[a-z][a-z0-9+.-]*:)?//""",
    re.IGNORECASE,
)
SCRIPT_SRC = re.compile(r"""<script\b([^>]*?)\bsrc\s*=\s*["']([^"']+)["']([^>]*)>\s*</script\s*>""", re.IGNORECASE)
LINK_TAG = re.compile(r"""<link\b[^>]*>""", re.IGNORECASE)
HREF = re.compile(r"""\bhref\s*=\s*["']([^"']+)["']""", re.IGNORECASE)
IMG_SRC = re.compile(r"""(<img\b[^>]*?\bsrc\s*=\s*)["']([^"':]+)["']""", re.IGNORECASE)
CSS_URL = re.compile(r"""url\(\s*["']?([^"')]+?)["']?\s*\)""", re.IGNORECASE)
HEAD_OPEN = re.compile(r"<head\b[^>]*>", re.IGNORECASE)
# Inherited by the srcdoc frame: inline code and data URLs only, no network.
CSP = ("default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; "
       "img-src data: blob:; font-src data:; media-src data: blob:")


class Refused(Exception):
    """A request the rules do not allow; the message tells the builder what to do instead."""


def read_json(path: Path):
    return json.loads(path.read_text())


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


# --- Snapshot history: the RFC 6902 subset the harness writes ------------------------------------

def unescape(token: str) -> str:
    return token.replace("~1", "/").replace("~0", "~")


def escape(token: str) -> str:
    return token.replace("~", "~0").replace("/", "~1")


def apply(document, op: dict):
    """Apply one patch operation in place and return the (possibly replaced) document."""
    if op["path"] == "":
        if op["op"] != "replace":
            raise Refused("snapshot history can only replace the whole document")
        return op["value"]
    *parents, last = [unescape(token) for token in op["path"][1:].split("/")]
    parent = document
    for token in parents:
        parent = parent[int(token)] if isinstance(parent, list) else parent[token]
    if isinstance(parent, list):
        index = len(parent) if last == "-" else int(last)
        if op["op"] == "add":
            parent.insert(index, op["value"])
        elif op["op"] == "replace":
            parent[index] = op["value"]
        else:
            del parent[index]
    elif op["op"] == "remove":
        del parent[last]
    else:
        parent[last] = op["value"]
    return document


def diff(old, new, path: str = "", ops: list | None = None) -> list:
    """Operations turning `old` into `new`; arrays recurse over their common prefix."""
    ops = [] if ops is None else ops
    if isinstance(old, dict) and isinstance(new, dict):
        for key in old:
            if key not in new:
                ops.append({"op": "remove", "path": f"{path}/{escape(key)}"})
        for key, value in new.items():
            if key in old:
                diff(old[key], value, f"{path}/{escape(key)}", ops)
            else:
                ops.append({"op": "add", "path": f"{path}/{escape(key)}", "value": value})
    elif isinstance(old, list) and isinstance(new, list):
        common = min(len(old), len(new))
        for index in range(common):
            diff(old[index], new[index], f"{path}/{index}", ops)
        for index in range(len(old) - 1, len(new) - 1, -1):
            ops.append({"op": "remove", "path": f"{path}/{index}"})
        for value in new[common:]:
            ops.append({"op": "add", "path": f"{path}/-", "value": value})
    elif old != new or type(old) is not type(new):
        ops.append({"op": "replace", "path": path, "value": new})
    return ops


def snapshot_history(instance: Path, committed: int) -> dict[int, str]:
    """Committed snapshots as JSON text by sequence; later rows for a sequence win."""
    path = instance / "snapshots.jsonl"
    if not path.exists():
        return {}
    raw = path.read_bytes()
    states: dict[int, str] = {}
    for line in raw[: raw.rfind(b"\n") + 1].splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        if "snapshot" in row:
            states[row["sequence"]] = json.dumps(row["snapshot"])
            continue
        if row["base"] not in states:
            raise Refused(f"snapshot history row {row['sequence']} names missing base {row['base']}")
        state = json.loads(states[row["base"]])
        for op in row["patch"]:
            state = apply(state, op)
        states[row["sequence"]] = json.dumps(state)
    return {sequence: state for sequence, state in states.items() if sequence <= committed}


# --- Run data -------------------------------------------------------------------------------------

def instance_paths(root: Path, instance: str) -> tuple[Path, dict]:
    path = root / "instances" / instance
    if not (path / "instance.json").exists():
        raise Refused(f"no instance {instance!r} under {root / 'instances'}")
    return path, read_json(path / "instance.json")


def run_state(path: Path) -> dict:
    checkpoint = read_json(path / "checkpoint.json") if (path / "checkpoint.json").exists() else {}
    run = read_json(path / "run.json") if (path / "run.json").exists() else {}
    committed = max(int(checkpoint.get("next_sequence", 1)) - 1, 0)
    return {"checkpoint": checkpoint, "phase": run.get("phase"), "committed": committed,
            "ended": bool(checkpoint.get("finished")) or run.get("phase") == "stopped"}


def history_summary(path: Path, committed: int) -> dict:
    recorded = set(snapshot_history(path, committed))
    return {"operations": committed, "recorded": len(recorded),
            "missing": [sequence for sequence in range(1, committed + 1) if sequence not in recorded][:20]}


def operation_time(path: Path, operation_id: str) -> int | None:
    intent = path / "operations" / f"{operation_id}.json"
    return int(intent.stat().st_mtime * 1000) if intent.exists() else None


def run_data(root: Path, path: Path, info: dict, viewer: dict) -> dict:
    """Everything the page replays: run facts, and one step per committed operation."""
    state = run_state(path)
    checkpoint, committed = state["checkpoint"], state["committed"]
    if not state["ended"]:
        raise Refused("this run has not ended. Viewers replay finished or stopped runs only, because a live "
                      "run's full state holds private information.")
    history = snapshot_history(path, committed)
    if not history:
        raise Refused("this run has no snapshot history. It was recorded before the harness kept one; replay "
                      "a run started with the current harness.")
    receipts = {receipt["sequence"]: (operation_id, receipt)
                for operation_id, receipt in checkpoint.get("receipts", {}).items() if receipt.get("sequence")}
    deliveries: dict[int, list] = {}
    table = path / "table.jsonl"
    current = 1
    for line in table.read_text().splitlines() if table.exists() else []:
        if not line.strip():
            continue
        row = json.loads(line)
        current = row.get("op_seq") or current
        deliveries.setdefault(current, []).append(
            {key: row.get(key) for key in ("seq", "kind", "from", "to", "message", "timestamp")})
    initialization = checkpoint.get("receipts", {}).get("initialization", {})
    definition = root / "definitions" / info["definition_id"] / info["version"] / "definition.json"
    name = read_json(definition)["manifest"].get("name") if definition.exists() else info["definition_id"]
    steps, previous_text, previous = [], None, None
    for sequence in range(1, committed + 1):
        operation_id, receipt = receipts.get(sequence, (None, {}))
        request = receipt.get("request", {})
        params = request.get("params", {})
        text = history.get(sequence, previous_text)
        if text is None:
            continue  # nothing recorded yet; replay starts at the first recorded state
        step = {
            "sequence": sequence,
            "recorded": sequence in history,
            "time": operation_time(path, operation_id) if operation_id else None,
            "op": {"id": operation_id, "method": request.get("method"), "seat": request.get("participant"),
                   "action": params.get("action"), "arguments": params.get("arguments"),
                   "text": params.get("text"), "success": receipt.get("success"), "result": receipt.get("result"),
                   "finished": (receipt.get("finished") or {}).get("summary")},
            "deliveries": deliveries.get(sequence, []),
        }
        current_state = json.loads(text)
        patch = None if previous is None else diff(previous, current_state) if text != previous_text else []
        if patch is None or len(json.dumps(patch)) >= len(text):
            step["state"] = current_state  # a keyframe when a patch would be larger
        else:
            step["patch"] = patch
        steps.append(step)
        previous_text, previous = text, current_state
    seats = [{key: seat.get(key) for key in ("id", "role", "occupant", "model")}
             for seat in (info.get("roster") or {}).get("seats", [])]
    return {
        "format": DATA_FORMAT,
        "meta": {"definition_id": info["definition_id"], "name": name, "version": info["version"],
                 "instance_id": info["id"], "created_at": info.get("created_at"), "seats": seats,
                 "config": initialization.get("request", {}).get("params", {}).get("config"),
                 "phase": state["phase"], "finished": (checkpoint.get("finished") or {}).get("summary"),
                 "perspective": "omniscient", "mode": "replay",
                 "viewer": {key: viewer.get(key) for key in ("id", "title", "revision", "shell")}},
        "steps": steps,
    }


# --- Viewer source --------------------------------------------------------------------------------

def local_file(source: Path, reference: str) -> Path:
    target = (source / reference.split("?")[0].split("#")[0]).resolve()
    if source.resolve() not in target.parents or not target.is_file():
        raise Refused(f"viewer file {reference!r} must exist inside the viewer folder")
    return target


def data_uri(path: Path) -> str:
    kind = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
    return f"data:{kind};base64,{base64.b64encode(path.read_bytes()).decode()}"


def source_files(source: Path) -> list[Path]:
    return sorted(p for p in source.rglob("*") if p.is_file() and p.name != "viewer.json" and not p.name.startswith("."))


def lint(source: Path) -> list[str]:
    problems = []
    if not (source / "index.html").is_file():
        return [f"{source} has no index.html"]
    total = 0
    for path in source_files(source):
        total += path.stat().st_size
        if path.suffix.lower() not in (".html", ".js", ".mjs", ".css", ".svg", ".json"):
            continue
        text = path.read_text(errors="replace")
        relative = path.relative_to(source)
        if REMOTE.search(text):
            problems.append(f"{relative} loads a remote resource; viewers work offline, so bundle it locally")
        if UUID.search(text) or HASH.search(text):
            problems.append(f"{relative} contains an instance id or version hash; viewers must stay generic")
    if total > MAX_SOURCE_BYTES:
        problems.append(f"viewer source is {total} bytes; keep it under {MAX_SOURCE_BYTES}")
    return problems


def content_hash_of(source: Path) -> str:
    digest = hashlib.sha256()
    for path in source_files(source):
        digest.update(str(path.relative_to(source)).encode() + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()[:16]


def inline_css(source: Path, css: str) -> str:
    def replace(match):
        reference = match.group(1)
        if reference.startswith(("data:", "#")):
            return match.group(0)
        return f'url("{data_uri(local_file(source, reference))}")'
    return CSS_URL.sub(replace, css)


def inline_viewer(source: Path) -> str:
    """The viewer's index.html with its local scripts, stylesheets and images inlined."""
    page = (source / "index.html").read_text()

    def script(match):
        body = local_file(source, match.group(2)).read_text().replace("</script", "<\\/script")
        return f"<script{match.group(1)}{match.group(3)}>{body}</script>"

    def link(match):
        tag = match.group(0)
        href = HREF.search(tag)
        if not href or not re.search(r"""\brel\s*=\s*["']?stylesheet""", tag, re.IGNORECASE):
            return tag
        css = inline_css(source, local_file(source, href.group(1)).read_text()).replace("</style", "<\\/style")
        return f"<style>{css}</style>"

    def image(match):
        return f'{match.group(1)}"{data_uri(local_file(source, match.group(2)))}"'

    return IMG_SRC.sub(image, LINK_TAG.sub(link, SCRIPT_SRC.sub(script, page)))


def build_page(source: Path, data: dict) -> str:
    """A wrapper page holding the viewer in a sandboxed frame with the bridge and data injected first."""
    payload = json.dumps(data, separators=(",", ":"), ensure_ascii=False).replace("</", "<\\/")
    injected = (f'<meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="{CSP}">'
                f'<script type="application/json" id="env-replay-data">{payload}</script>'
                f"<script>{RUNTIME.read_text()}</script>")
    page = inline_viewer(source)
    head = HEAD_OPEN.search(page)
    page = page[: head.end()] + injected + page[head.end():] if head else injected + page
    title = html.escape(f"{data['meta']['name']} replay")
    # The frame loads from a blob URL; `sandbox` without allow-same-origin gives it an opaque origin.
    # The page text is stored with `&` and `<` escaped, so nothing in it can end the holding element.
    stored = page.replace("&", "&amp;").replace("<", "&lt;")
    return ("<!doctype html><html><head><meta charset=\"utf-8\">"
            f'<meta http-equiv="Content-Security-Policy" content="{CSP}; frame-src blob:">'
            '<meta name="viewport" content="width=device-width, initial-scale=1">'
            f"<title>{title}</title><style>html,body{{margin:0;height:100%;background:#111}}"
            "iframe{border:0;width:100%;height:100%;display:block}</style></head><body>"
            f'<iframe sandbox="allow-scripts" title="{title}"></iframe>'
            f'<script type="text/plain" id="env-viewer-page">{stored}</script>'
            "<script>(()=>{const page=document.getElementById('env-viewer-page').textContent"
            ".replace(/&(amp|lt);/g,(_,k)=>k==='amp'?'&':'<');const frame=document.querySelector('iframe');"
            "frame.src=URL.createObjectURL(new Blob([page],{type:'text/html'}));frame.focus();})();</script>"
            "</body></html>\n")


# --- Commands -------------------------------------------------------------------------------------

def viewers_for(root: Path, definition: str) -> list[dict]:
    base = root / "viewers" / definition
    return [read_json(meta) for meta in sorted(base.glob("*/viewer.json"))] if base.exists() else []


def cmd_find(root: Path, args) -> dict:
    path, info = instance_paths(root, args.instance)
    state = run_state(path)
    exports = []
    for provenance in sorted((path / "viewers").glob("*/provenance.json")):
        record = read_json(provenance)
        saved = root / "viewers" / info["definition_id"] / record["viewer_id"] / "viewer.json"
        now = read_json(saved) if saved.exists() else None
        exports.append({"viewer_id": record["viewer_id"], "html": str(provenance.parent / "index.html"),
                        "generated_at": record["generated_at"], "viewer_revision_used": record.get("viewer_revision"),
                        "state": "development build" if record.get("source") else
                                 "viewer missing" if now is None else
                                 "current" if now["content_hash"] == record.get("viewer_content_hash")
                                 else "stale: viewer changed since export"})
    return {"instance": args.instance, "definition_id": info["definition_id"], "version": info["version"],
            "ended": state["ended"], "phase": state["phase"], "history": history_summary(path, state["committed"]),
            "viewers": [{"id": v["id"], "title": v.get("title"), "revision": v["revision"],
                         "compatible": info["version"] in v["compatible_versions"],
                         "compatible_versions": v["compatible_versions"]} for v in viewers_for(root, info["definition_id"])],
            "exports": exports}


def cmd_save_viewer(root: Path, args) -> dict:
    source = Path(args.source).expanduser()
    problems = lint(source)
    if problems:
        raise Refused("viewer rejected: " + "; ".join(problems))
    dest = root / "viewers" / args.definition / args.viewer
    exists = (dest / "viewer.json").exists()
    if exists and not args.approved:
        raise Refused(f"viewer {args.viewer!r} already exists. Changing a saved viewer changes every later export; "
                      "ask the user, then re-run with --approved. Its verified versions reset to the --version values.")
    content_hash = content_hash_of(source)
    previous = read_json(dest / "viewer.json") if exists else None
    if previous is None:
        revision = "v1"
    elif previous["content_hash"] == content_hash:
        revision = previous["revision"]
    else:
        revision = f"v{int(previous['revision'].removeprefix('v')) + 1}"
    staging = dest.with_name(f".{args.viewer}.staging")
    shutil.rmtree(staging, ignore_errors=True)
    shutil.copytree(source, staging, ignore=shutil.ignore_patterns(".*", "viewer.json"))
    write_json(staging / "viewer.json", {
        "id": args.viewer, "title": args.title, "definition_id": args.definition,
        "compatible_versions": sorted(set(args.version)), "revision": revision, "content_hash": content_hash,
        "shell": not args.no_shell, "verified_against": [{"instance": i} for i in args.verified_instance],
        "saved_at": datetime.now(timezone.utc).isoformat(timespec="seconds")})
    shutil.rmtree(dest, ignore_errors=True)
    staging.rename(dest)
    return {"saved": str(dest), "revision": revision, "replaced_existing": exists}


def cmd_add_version(root: Path, args) -> dict:
    meta_path = root / "viewers" / args.definition / args.viewer / "viewer.json"
    if not meta_path.exists():
        raise Refused(f"no viewer {args.viewer!r} for {args.definition!r}")
    meta = read_json(meta_path)
    _, info = instance_paths(root, args.verified_instance)
    if info["definition_id"] != args.definition:
        raise Refused("the verifying instance belongs to a different definition")
    if info["version"] not in meta["compatible_versions"]:
        meta["compatible_versions"] = sorted(meta["compatible_versions"] + [info["version"]])
    meta["verified_against"].append({"instance": args.verified_instance})
    write_json(meta_path, meta)
    return {"viewer": args.viewer, "compatible_versions": meta["compatible_versions"]}


def cmd_export(root: Path, args) -> dict:
    path, info = instance_paths(root, args.instance)
    if args.source:
        source = Path(args.source).expanduser()
        problems = lint(source)
        if problems:
            raise Refused("viewer rejected: " + "; ".join(problems))
        meta = {"id": "_development", "title": "Development build", "revision": None,
                "content_hash": content_hash_of(source), "shell": not args.no_shell}
        dest = Path(args.out).expanduser().parent if args.out else path / "viewers" / "_development"
    else:
        candidates = viewers_for(root, info["definition_id"])
        if args.viewer:
            candidates = [v for v in candidates if v["id"] == args.viewer]
            if not candidates:
                raise Refused(f"no viewer {args.viewer!r} for {info['definition_id']!r}")
        compatible = [v for v in candidates if info["version"] in v["compatible_versions"]]
        trial = not compatible and args.trial and bool(candidates)
        if trial:
            compatible = candidates[:1]
        if not compatible:
            raise Refused(f"no viewer for {info['definition_id']!r} lists version {info['version'][:12]}. Existing: "
                          f"{[(v['id'], [x[:12] for x in v['compatible_versions']]) for v in candidates]}. "
                          "Export with --trial to check one, or build a viewer if the user asked for one.")
        if len(compatible) > 1:
            raise Refused(f"several viewers match ({[v['id'] for v in compatible]}); pass --viewer")
        meta = dict(compatible[0], trial=trial)
        source = root / "viewers" / info["definition_id"] / meta["id"]
        dest = path / "viewers" / meta["id"]
        if (dest / "index.html").exists() and not args.replace:
            raise Refused(f"{dest} already exists; pass --replace to regenerate it")
    page = build_page(source, run_data(root, path, info, meta))
    target = Path(args.out).expanduser() if args.source and args.out else dest / "index.html"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(page)
    write_json(target.parent / "provenance.json", {
        "viewer_id": meta["id"], "viewer_revision": meta.get("revision"),
        "viewer_content_hash": meta["content_hash"], "source": str(source) if args.source else None,
        "trial": meta.get("trial", False), "definition_id": info["definition_id"], "version": info["version"],
        "instance_id": args.instance, "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds")})
    return {"html": str(target), "viewer": meta["id"], "viewer_revision": meta.get("revision"),
            "trial": meta.get("trial", False), "bytes": target.stat().st_size}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT, help="environments root")
    sub = parser.add_subparsers(dest="command", required=True)

    find = sub.add_parser("find", help="viewers for an instance's definition/version, its history, and its exports")
    find.add_argument("instance")

    save = sub.add_parser("save-viewer", help="lint and save a reusable viewer from a source folder")
    save.add_argument("definition")
    save.add_argument("viewer")
    save.add_argument("source", help="folder holding index.html and its local files; it is copied, not moved")
    save.add_argument("--title", required=True)
    save.add_argument("--version", action="append", required=True, help="a version hash it was verified on (repeatable)")
    save.add_argument("--verified-instance", action="append", default=[])
    save.add_argument("--no-shell", action="store_true", help="hide the harness playback bar; the viewer draws its own")
    save.add_argument("--approved", action="store_true", help="the user approved replacing an existing viewer")

    add = sub.add_parser("add-version", help="record that a viewer also works on another version")
    add.add_argument("definition")
    add.add_argument("viewer")
    add.add_argument("--verified-instance", required=True, help="an instance of the new version it replayed correctly")

    export = sub.add_parser("export", help="write a self-contained replay page for an ended run")
    export.add_argument("instance")
    export.add_argument("--viewer", help="saved viewer id when several match")
    export.add_argument("--source", help="development: export from an unsaved viewer folder instead")
    export.add_argument("--out", help="with --source: where to write the page")
    export.add_argument("--no-shell", action="store_true", help="with --source: hide the playback bar")
    export.add_argument("--trial", action="store_true", help="try a saved viewer that does not list this version yet")
    export.add_argument("--replace", action="store_true", help="regenerate an existing export")
    return parser


COMMANDS = {"find": cmd_find, "save-viewer": cmd_save_viewer, "add-version": cmd_add_version, "export": cmd_export}


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
