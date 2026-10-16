#!/usr/bin/env python3
"""Environment changelogs and harness recommendations (stdlib only).

Two kinds of note live under the environments root (default
~/.prime-env-harness/environments), outside the definitions they describe:

  changelogs/<definition-id>/CHANGELOG.md        what each saved version changed, newest first
  harness-recommendations/<date>-<slug>.md       a harness-level change the builder could not make

A changelog sits beside `definitions/`, never inside a definition or its draft: a
version is a content hash, so an entry cannot be part of the source it describes,
and `definitions/<id>/` may hold only version directories.

Commands print JSON. Exit status is non-zero when the request is refused.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

DEFAULT_ROOT = Path.home() / ".prime-env-harness" / "environments"
HASH = re.compile(r"^[0-9a-f]{64}$")
STATUSES = ("open", "accepted", "done", "rejected")


class Refused(Exception):
    """A request the rules do not allow; the message tells the builder what to do instead."""


def today() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%d")


def saved_versions(root: Path, definition: str) -> list[dict]:
    """Saved versions of a definition, oldest first (the order the host uses to pick the latest)."""
    base = root / "definitions" / definition
    if not base.is_dir():
        raise Refused(f"no saved definition {definition!r} under {root / 'definitions'}")
    versions = []
    for entry in base.iterdir():
        if entry.is_dir() and not entry.name.startswith(".") and (entry / "definition.json").is_file():
            saved = datetime.fromtimestamp(entry.stat().st_mtime, timezone.utc)
            versions.append({"version": entry.name, "saved": saved.strftime("%Y-%m-%d"), "mtime": entry.stat().st_mtime})
    versions.sort(key=lambda item: (item["mtime"], item["version"]))
    return [{"version": v["version"], "saved": v["saved"]} for v in versions]


def changelog_path(root: Path, definition: str) -> Path:
    return root / "changelogs" / definition / "CHANGELOG.md"


def logged_versions(path: Path) -> list[str]:
    if not path.is_file():
        return []
    return re.findall(r"^Version: ([0-9a-f]{64})$", path.read_text(), re.MULTILINE)


def cmd_versions(root: Path, args) -> dict:
    versions = saved_versions(root, args.definition)
    logged = set(logged_versions(changelog_path(root, args.definition)))
    return {"definition": args.definition,
            "versions": [{**v, "logged": v["version"] in logged} for v in versions]}


def cmd_changelog_show(root: Path, args) -> dict:
    path = changelog_path(root, args.definition)
    versions = saved_versions(root, args.definition)
    logged = set(logged_versions(path))
    return {"definition": args.definition, "path": str(path),
            "missing": [v["version"] for v in versions if v["version"] not in logged],
            "text": path.read_text() if path.is_file() else None}


def cmd_changelog_add(root: Path, args) -> dict:
    if not HASH.match(args.version):
        raise Refused("--version must be the full 64-character version hash that save_environment returned")
    versions = saved_versions(root, args.definition)
    order = [v["version"] for v in versions]
    if args.version not in order:
        raise Refused(f"{args.version} is not a saved version of {args.definition!r}; save the draft first")
    path = changelog_path(root, args.definition)
    if args.version in logged_versions(path):
        raise Refused(f"{args.version} already has an entry in {path}; edit that file to correct it")
    previous = args.previous
    if previous is None:
        index = order.index(args.version)
        previous = order[index - 1] if index > 0 else None
    elif previous not in order:
        raise Refused(f"--previous {previous} is not a saved version of {args.definition!r}")
    if previous is None and not args.change:
        args.change = ["First saved version."]
    if not args.change:
        raise Refused("give at least one --change describing what differs from the previous version")
    date = args.date or next(v["saved"] for v in versions if v["version"] == args.version)
    lines = [f"## {args.version[:12]} ({date})", "", f"Version: {args.version}",
             f"Previous: {previous or 'none (first saved version)'}", "", "Changes:"]
    lines += [f"- {change}" for change in args.change]
    if args.why:
        lines += ["", f"Why: {args.why}"]
    if args.compatibility:
        lines += ["", f"Compatibility: {args.compatibility}"]
    entry = "\n".join(lines) + "\n"
    header = f"# Changelog: {args.definition}\n\nNewest version first. One entry per saved version.\n\n"
    existing = path.read_text() if path.is_file() else header
    head, _, rest = existing.partition("\n## ")
    body = f"## {rest}" if rest else ""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(head.rstrip("\n") + "\n\n" + entry + ("\n" + body if body else ""))
    return {"definition": args.definition, "version": args.version, "previous": previous, "path": str(path)}


def slug(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")[:60] or "recommendation"


def cmd_recommend(root: Path, args) -> dict:
    if args.version and not args.definition:
        raise Refused("--version needs --definition")
    base = root / "harness-recommendations"
    base.mkdir(parents=True, exist_ok=True)
    stem = f"{today()}-{slug(args.title)}"
    path = base / f"{stem}.md"
    counter = 2
    while path.exists():
        path = base / f"{stem}-{counter}.md"
        counter += 1
    origin = args.definition or "none"
    if args.definition and args.version:
        origin = f"{args.definition} {args.version}"
    lines = [f"# {args.title}", "", "Status: open", f"Date: {today()}", f"Found while working on: {origin}", "",
             "## Problem", "", args.problem, "", "## Evidence", "", args.evidence, "",
             "## Proposed harness change", "", args.proposal]
    if args.workaround:
        lines += ["", "## Workaround used in the environment", "", args.workaround]
    path.write_text("\n".join(lines) + "\n")
    return {"path": str(path), "status": "open"}


def cmd_recommendations(root: Path, args) -> dict:
    items = []
    for path in sorted((root / "harness-recommendations").glob("*.md")):
        text = path.read_text()
        status = re.search(r"^Status: (\w+)$", text, re.MULTILINE)
        title = re.match(r"# (.*)", text)
        items.append({"path": str(path), "title": title.group(1) if title else path.stem,
                      "status": status.group(1) if status else "open"})
    if args.status:
        items = [item for item in items if item["status"] == args.status]
    return {"recommendations": items}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT, help="environments root")
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("versions", help="saved versions of a definition, oldest first, and whether each has a changelog entry")
    p.add_argument("definition")

    p = sub.add_parser("changelog-show", help="print a definition's changelog and list saved versions without an entry")
    p.add_argument("definition")

    p = sub.add_parser("changelog-add", help="add the entry for a saved version")
    p.add_argument("definition")
    p.add_argument("--version", required=True, help="full version hash from save_environment")
    p.add_argument("--previous", help="version it was derived from; defaults to the preceding saved version")
    p.add_argument("--change", action="append", default=[], help="one change from the previous version; repeat")
    p.add_argument("--why", help="the reason for the change")
    p.add_argument("--compatibility", help="effect on report templates, viewers or runs pinned to earlier versions")
    p.add_argument("--date", help="YYYY-MM-DD; defaults to the version's save date")

    p = sub.add_parser("recommend", help="record a harness-level change for the maintainers")
    p.add_argument("--title", required=True)
    p.add_argument("--problem", required=True, help="the defect or missing capability, stated without naming the environment")
    p.add_argument("--evidence", required=True, help="what you observed: instance ids, errors, the failing check")
    p.add_argument("--proposal", required=True, help="the generic harness change you suggest")
    p.add_argument("--workaround", help="what you did in the environment meanwhile, if anything")
    p.add_argument("--definition", help="definition the problem surfaced in")
    p.add_argument("--version", help="its version hash")

    p = sub.add_parser("recommendations", help="list recorded harness recommendations")
    p.add_argument("--status", choices=STATUSES)
    return parser


COMMANDS = {"versions": cmd_versions, "changelog-show": cmd_changelog_show, "changelog-add": cmd_changelog_add,
            "recommend": cmd_recommend, "recommendations": cmd_recommendations}


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
