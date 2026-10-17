"""Tests for maintenance_tool.py. Run: python3 -m unittest skills/environments/assets/maintenance/test_maintenance_tool.py"""
import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path

import maintenance_tool

DEFINITION = "demo-env"
FIRST, SECOND = "a" * 64, "b" * 64


class MaintenanceToolTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        for index, version in enumerate((FIRST, SECOND)):
            directory = self.root / "definitions" / DEFINITION / version
            directory.mkdir(parents=True)
            (directory / "definition.json").write_text("{}")
            os.utime(directory, (1_700_000_000 + index * 86_400, 1_700_000_000 + index * 86_400))

    def tearDown(self):
        self.tmp.cleanup()

    def run_tool(self, *argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = maintenance_tool.main(["--root", str(self.root), *argv])
        return code, json.loads(out.getvalue())

    def test_changelog_entries_are_newest_first_and_link_previous_version(self):
        self.assertEqual(self.run_tool("changelog-add", DEFINITION, "--version", FIRST)[0], 0)
        code, result = self.run_tool("changelog-add", DEFINITION, "--version", SECOND,
                                     "--change", "Rounds end on a tie.", "--why", "Runs stalled on ties.")
        self.assertEqual((code, result["previous"]), (0, FIRST))
        text = (self.root / "changelogs" / DEFINITION / "CHANGELOG.md").read_text()
        self.assertEqual(text, (
            "# Changelog: demo-env\n\nNewest version first. One entry per saved version.\n\n"
            f"## {SECOND[:12]} (2023-11-15)\n\nVersion: {SECOND}\nPrevious: {FIRST}\n\n"
            "Changes:\n- Rounds end on a tie.\n\nWhy: Runs stalled on ties.\n\n"
            f"## {FIRST[:12]} (2023-11-14)\n\nVersion: {FIRST}\nPrevious: none (first saved version)\n\n"
            "Changes:\n- First saved version.\n"))

    def test_unsaved_or_duplicate_versions_are_refused(self):
        self.assertEqual(self.run_tool("changelog-add", DEFINITION, "--version", "c" * 64, "--change", "x")[0], 1)
        self.run_tool("changelog-add", DEFINITION, "--version", FIRST)
        self.assertEqual(self.run_tool("changelog-add", DEFINITION, "--version", FIRST)[0], 1)

    def test_show_lists_versions_without_an_entry(self):
        self.run_tool("changelog-add", DEFINITION, "--version", FIRST)
        _, result = self.run_tool("changelog-show", DEFINITION)
        self.assertEqual(result["missing"], [SECOND])

    def write_viewer(self, *versions, viewer="replay"):
        dest = self.root / "viewers" / DEFINITION / viewer
        dest.mkdir(parents=True)
        (dest / "viewer.json").write_text(json.dumps({"id": viewer, "compatible_versions": list(versions)}))

    def test_newest_changelog_waits_until_every_viewer_lists_the_version(self):
        self.write_viewer(FIRST)
        self.write_viewer(FIRST, SECOND, viewer="scoreboard")
        code, refused = self.run_tool("changelog-add", DEFINITION, "--version", SECOND, "--change", "Rounds end on a tie.")
        self.assertEqual(code, 1)
        self.assertIn("replay", refused["refused"])
        self.assertNotIn("scoreboard", refused["refused"])
        self.assertFalse((self.root / "changelogs" / DEFINITION / "CHANGELOG.md").exists())

        code, older = self.run_tool("changelog-add", DEFINITION, "--version", FIRST)
        self.assertEqual((code, older["unregistered_viewers"]), (0, []))
        _, shown = self.run_tool("changelog-show", DEFINITION)
        self.assertEqual(shown["viewer_gaps"], [{"version": SECOND, "viewers": ["replay"]}])
        _, listed = self.run_tool("versions", DEFINITION)
        by_version = {item["version"]: item["unregistered_viewers"] for item in listed["versions"]}
        self.assertEqual(by_version, {FIRST: [], SECOND: ["replay"]})

        meta_path = self.root / "viewers" / DEFINITION / "replay" / "viewer.json"
        meta = json.loads(meta_path.read_text())
        meta["compatible_versions"].append(SECOND)
        meta_path.write_text(json.dumps(meta))
        code, added = self.run_tool("changelog-add", DEFINITION, "--version", SECOND, "--change", "Rounds end on a tie.")
        self.assertEqual((code, added["unregistered_viewers"]), (0, []))

    def test_unreadable_viewer_metadata_is_refused(self):
        dest = self.root / "viewers" / DEFINITION / "replay"
        dest.mkdir(parents=True)
        (dest / "viewer.json").write_text("{")
        code, result = self.run_tool("changelog-add", DEFINITION, "--version", SECOND, "--change", "x")
        self.assertEqual(code, 1)
        self.assertIn("viewer.json", result["refused"])

    def test_recommendations_are_stored_and_listed_by_status(self):
        _, first = self.run_tool("recommend", "--title", "Cap wake fan-out", "--problem", "p", "--evidence", "e",
                                 "--proposal", "q", "--definition", DEFINITION, "--version", FIRST)
        _, second = self.run_tool("recommend", "--title", "Cap wake fan-out", "--problem", "p", "--evidence", "e",
                                  "--proposal", "q")
        self.assertNotEqual(first["path"], second["path"])
        text = Path(first["path"]).read_text()
        self.assertIn(f"Found while working on: {DEFINITION} {FIRST}", text)
        Path(second["path"]).write_text(Path(second["path"]).read_text().replace("Status: open", "Status: done"))
        _, listing = self.run_tool("recommendations", "--status", "open")
        self.assertEqual([item["path"] for item in listing["recommendations"]], [first["path"]])


if __name__ == "__main__":
    unittest.main()
