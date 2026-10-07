#!/usr/bin/env python3
"""Verify builder skill discovery without a project, login, or provider call."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET


def verify(binary: Path, launch: Path, data: Path, package: Path | None) -> None:
    environment = dict(os.environ)
    for name in (
        "PI_PACKAGE_DIR", "PI_CODING_AGENT_DIR", "PRIME_ENV_HARNESS_DATA_DIR",
        "PRIME_ENV_HARNESS_APP", "PRIME_AGENT_INTERNAL_DAEMON_WORKER",
    ):
        environment.pop(name, None)
    environment["HOME"] = str(launch.parent / "home")
    if package is not None:
        environment["PI_PACKAGE_DIR"] = str(package)
    result = subprocess.run(
        [str(binary), "--data-dir", str(data), "prompt", "--json"],
        cwd=launch, env=environment, capture_output=True, text=True, timeout=60,
    )
    if result.returncode:
        raise RuntimeError(f"prompt failed: {result.stderr}")
    breakdown = json.loads(result.stdout)
    if "harness-purpose" not in {segment["name"] for segment in breakdown["segments"]}:
        raise AssertionError("environment builder purpose is absent")
    match = re.search(r"<available_skills>.*?</available_skills>", breakdown["prompt"], re.S)
    if match is None:
        raise AssertionError("skill inventory is absent")
    inventory = {skill.findtext("name"): skill for skill in ET.fromstring(match.group())}
    for name in ("env-harness", "environments"):
        skill = inventory[name]
        location = Path(skill.findtext("location", ""))
        if not location.is_absolute() or not location.is_file():
            raise AssertionError(f"{name} lacks a readable absolute location: {location}")
        if package is not None and location != package / "skills" / name / "SKILL.md":
            raise AssertionError(f"{name} did not load from the staged package: {location}")
        skill_root = location.parent.resolve()
        for markdown in skill_root.rglob("*.md"):
            for target in re.findall(r"\[[^\]]+\]\(([^)]+)\)", markdown.read_text()):
                if "://" in target or target.startswith("#"):
                    continue
                reference = (markdown.parent / target.split("#", 1)[0]).resolve()
                if not reference.is_relative_to(skill_root) or not reference.is_file():
                    raise AssertionError(f"unpackaged skill reference: {markdown}: {target}")
        if name == "environments":
            if skill.findtext("type") != "python" or skill.findtext("python_import") != name:
                raise AssertionError("builder management skill is not importable Python")
            for relative in ("pyproject.toml", "src/environments/__init__.py"):
                if not (skill_root / relative).is_file():
                    raise AssertionError(f"Python skill asset missing: {relative}")
        print(f"PASS {launch.name}: {name} -> {location}")


def main() -> None:
    checkout = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=checkout / "target/debug/prime-env-harness")
    arguments = parser.parse_args()
    binary = arguments.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="environment-skills-") as scratch:
        root = Path(scratch)
        home = root / "home"
        unrelated = root / "unrelated-project"
        home.mkdir()
        unrelated.mkdir()
        verify(binary, unrelated, root / "data-unrelated", None)
        verify(binary, home, root / "data-home", None)
        package = root / "package"
        for name in ("env-harness", "environments"):
            shutil.copytree(checkout / "skills" / name, package / "skills" / name,
                            ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        verify(binary, home, root / "data-packaged", package)
    print("Builder skill discovery passed for unrelated cwd, home cwd, and staged package.")


if __name__ == "__main__":
    main()
