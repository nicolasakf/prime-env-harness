#!/usr/bin/env python3
"""Install the bundled example environments into a Prime Env data directory.

Each directory under examples/ holds one environment's source, its suggested
policy.json, and optionally a report template under game-report/ and a replay
viewer under viewer/. This script copies the source into a fresh draft (drafts
must live in the data directory), the report template into report-templates/,
and the viewer into viewers/. It never validates, saves, or runs anything: the
builder does that after you pick an execution policy.
"""

import argparse
import json
import shutil
import sys
import uuid
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
EXAMPLES = REPO / "examples"
SKIP = shutil.ignore_patterns("__pycache__", ".DS_Store", "game-report", "viewer")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--data-dir",
        type=Path,
        default=Path.home() / ".prime-env-harness",
        help="Prime Env data directory (default: ~/.prime-env-harness)",
    )
    parser.add_argument("names", nargs="*", help="examples to install (default: all)")
    args = parser.parse_args()

    available = sorted(p.name for p in EXAMPLES.iterdir() if (p / "environment.json").is_file())
    names = args.names or available
    unknown = [name for name in names if name not in available]
    if unknown:
        print(f"unknown example(s): {', '.join(unknown)}; available: {', '.join(available)}", file=sys.stderr)
        return 2

    root = args.data_dir / "environments"
    for name in names:
        source = EXAMPLES / name
        draft = root / "drafts" / str(uuid.uuid4())
        shutil.copytree(source, draft, ignore=SKIP)
        template = source / "game-report"
        if template.is_dir():
            definition_id = json.loads((template / "template.json").read_text())["definition_id"]
            target = root / "report-templates" / definition_id / "game-report"
            if target.exists():
                print(f"{name}: report template already installed, left unchanged")
            else:
                shutil.copytree(template, target)
        viewer = source / "viewer"
        if viewer.is_dir():
            meta = json.loads((viewer / "viewer.json").read_text())
            target = root / "viewers" / meta["definition_id"] / meta["id"]
            if target.exists():
                print(f"{name}: viewer already installed, left unchanged")
            else:
                shutil.copytree(viewer, target, ignore=shutil.ignore_patterns(".DS_Store"))
        print(f"{name}: draft created at {draft}")
        print(f"  /env policy {draft} {json.dumps(json.loads((source / 'policy.json').read_text()), separators=(',', ':'))}")
        print(f"  /env validate {draft}")
        print(f"  /env save {draft} {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
