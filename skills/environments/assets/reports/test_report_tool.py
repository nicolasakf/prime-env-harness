"""Tests for report_tool.py. Run: python3 -m unittest skills/environments/assets/reports/test_report_tool.py

They execute a tiny real notebook, so nbconvert, nbclient and ipykernel must be installed.
"""
import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

import report_tool

DEFINITION, VERSION, OTHER_VERSION = "demo-env", "a" * 64, "b" * 64


def notebook(body: str, run_dir_line: str = report_tool.PARAM_LINE) -> dict:
    code = lambda text: {"cell_type": "code", "metadata": {}, "source": text, "outputs": [], "execution_count": None}  # noqa: E731
    return {"nbformat": 4, "nbformat_minor": 5, "metadata": {"kernelspec": {"name": "python3", "display_name": "Python 3"}},
            "cells": [code(run_dir_line), code(body)]}


class ReportToolTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        for name, version in (("run-1", VERSION), ("run-2", OTHER_VERSION)):
            instance = self.root / "instances" / name
            instance.mkdir(parents=True)
            (instance / "instance.json").write_text(json.dumps({"definition_id": DEFINITION, "version": version}))
            (instance / "checkpoint.json").write_text(json.dumps({"finished": True}))

    def tearDown(self):
        self.tmp.cleanup()

    def run_tool(self, *argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = report_tool.main(["--root", str(self.root), *argv])
        return code, json.loads(out.getvalue())

    def save(self, body="print('hello from the report')", *extra, version=VERSION):
        path = self.root / "dev.ipynb"
        path.write_text(json.dumps(notebook(body)))
        return self.run_tool("save-template", DEFINITION, "demo-report", str(path), "--title", "Demo",
                             "--version", version, *extra)

    def test_templates_are_stripped_linted_and_versioned_by_readable_revision(self):
        dirty = notebook("1", f'RUN_DIR = "{"1b4e28ba-2fa1-11d2-883f-0016d3cca427"}"  {report_tool.PARAM_MARKER}')
        dirty["cells"][1]["outputs"] = [{"output_type": "stream", "name": "stdout", "text": "secret"}]
        path = self.root / "dirty.ipynb"
        path.write_text(json.dumps(dirty))
        code, result = self.run_tool("save-template", DEFINITION, "demo-report", str(path), "--title", "Demo", "--version", VERSION)
        self.assertEqual((code, "instance id" in result["refused"]), (1, True))

        code, result = self.save()
        self.assertEqual((code, result["revision"], result["replaced_existing"]), (0, "v1", False))
        saved = json.loads((self.root / "report-templates" / DEFINITION / "demo-report" / "report.ipynb").read_text())
        self.assertEqual(saved["cells"][0]["source"], report_tool.PARAM_LINE)
        self.assertEqual([c.get("outputs", []) for c in saved["cells"]], [[], []])

        code, result = self.save("print('changed')")
        self.assertEqual(code, 1)
        self.assertIn("approval", result["refused"])
        code, result = self.save("print('same content again')", "--approved")
        self.assertEqual((code, result["revision"]), (0, "v2"))
        code, result = self.save("print('same content again')", "--approved")
        self.assertEqual((code, result["revision"]), (0, "v2"))

    def test_instantiate_runs_a_copy_in_the_instance_and_tracks_staleness_and_divergence(self):
        self.save()
        code, result = self.run_tool("instantiate", "run-2")
        self.assertEqual(code, 1)
        self.assertIn("lists version", result["refused"])

        code, result = self.run_tool("instantiate", "run-1")
        self.assertEqual(code, 0, result)
        report = self.root / "instances" / "run-1" / "reports" / "demo-report"
        self.assertEqual(sorted(p.name for p in report.iterdir()),
                         ["provenance.json", "report.html", "report.ipynb", "template.json"])
        executed = json.loads((report / "report.ipynb").read_text())
        self.assertIn(str(self.root / "instances" / "run-1"), report_tool.source(executed["cells"][0]))
        self.assertIn("hello from the report", json.dumps(executed["cells"][1]["outputs"]))
        html = (report / "report.html").read_text()
        self.assertIn("hello from the report", html)
        self.assertNotIn("print(", html)

        _, found = self.run_tool("find", "run-1")
        self.assertEqual([(r["state"], r["diverged"]) for r in found["existing_reports"]], [("current", False)])

        executed["cells"][1]["source"] = "print('edited for this run only')"
        (report / "report.ipynb").write_text(json.dumps(executed))
        self.run_tool("record-divergence", "run-1", "demo-report", "one-off question")
        _, found = self.run_tool("find", "run-1")
        self.assertEqual([(r["diverged"], r["divergence_reason"]) for r in found["existing_reports"]],
                         [(True, "one-off question")])

        self.save("print('template improved')", "--approved")
        _, found = self.run_tool("find", "run-1")
        self.assertEqual(found["existing_reports"][0]["state"], "stale: template changed since this run")

    def test_trial_failure_leaves_no_half_made_report_and_trial_success_can_be_added(self):
        self.save("import pathlib; assert 'bbbbbbbb' not in (pathlib.Path(RUN_DIR) / 'instance.json').read_text()")
        code, result = self.run_tool("instantiate", "run-2", "--trial")
        self.assertEqual(code, 1)
        self.assertIn("notebook run failed", result["refused"])
        self.assertFalse((self.root / "instances" / "run-2" / "reports").exists() and
                         any((self.root / "instances" / "run-2" / "reports").iterdir()))

        self.save("print('works on both')", "--approved")
        code, _ = self.run_tool("instantiate", "run-2", "--trial")
        self.assertEqual(code, 0)
        code, result = self.run_tool("add-version", DEFINITION, "demo-report", "--verified-instance", "run-2")
        self.assertEqual((code, result["compatible_versions"]), (0, [VERSION, OTHER_VERSION]))


if __name__ == "__main__":
    unittest.main()
