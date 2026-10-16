"""Tests for viewer_tool.py. Run: python3 -m unittest skills/environments/assets/viewers/test_viewer_tool.py"""
import contextlib
import io
import json
import re
import tempfile
import unittest
from pathlib import Path

import viewer_tool

DEFINITION, VERSION, OTHER_VERSION = "demo-env", "a" * 64, "b" * 64
STATES = [{"round": 1, "log": []}, {"round": 1, "log": ["x"]}, {"round": 1, "log": ["x"]}, {"round": 2, "log": ["x", "y"]}]


def history_rows():
    """Rows as the harness writes them: a keyframe, deltas, and an uncommitted row past the checkpoint."""
    rows = [{"sequence": 1, "snapshot": STATES[0]}]
    for sequence in (2, 3, 4):
        rows.append({"sequence": sequence, "base": sequence - 1,
                     "patch": viewer_tool.diff(STATES[sequence - 2], STATES[sequence - 1])})
    rows.append({"sequence": 5, "base": 4, "patch": [{"op": "replace", "path": "/round", "value": 99}]})
    return rows


def decode_page(page: str) -> dict:
    """The run data embedded in an exported page, decoded the way the page decodes it."""
    stored = re.search(r'<script type="text/plain" id="env-viewer-page">(.*?)</script>', page, re.S).group(1)
    inner = re.sub(r"&(amp|lt);", lambda m: "&" if m.group(1) == "amp" else "<", stored)
    payload = re.search(r'<script type="application/json" id="env-replay-data">(.*?)</script>', inner, re.S).group(1)
    return json.loads(payload.replace("<\\/", "</"))


class ViewerToolTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        for name, version in (("run-1", VERSION), ("run-2", OTHER_VERSION), ("live", VERSION)):
            instance = self.root / "instances" / name
            instance.mkdir(parents=True)
            (instance / "instance.json").write_text(json.dumps({
                "id": name, "definition_id": DEFINITION, "version": version,
                "roster": {"seats": [{"id": "ana", "role": "player", "occupant": "agent", "model": "m/one"}]}}))
            receipts = {"initialization": {"request": {"method": "initialize", "params": {"config": {"rounds": 2}}},
                                           "success": True, "result": {}, "sequence": 1}}
            for sequence in (2, 3, 4):
                receipts[f"op-{sequence}"] = {
                    "request": {"method": "invoke", "participant": "ana", "params": {"action": "move", "arguments": {}}},
                    "success": sequence != 3, "result": "illegal" if sequence == 3 else {}, "sequence": sequence}
            checkpoint = {"protocol_version": 2, "snapshot": STATES[-1], "receipts": receipts, "next_sequence": 5}
            if name != "live":
                checkpoint["finished"] = {"summary": "done"}
            (instance / "checkpoint.json").write_text(json.dumps(checkpoint))
            (instance / "snapshots.jsonl").write_text("".join(json.dumps(row) + "\n" for row in history_rows()))
            (instance / "table.jsonl").write_text(json.dumps(
                {"seq": 1, "event_id": "e", "op_seq": 4, "kind": "notice", "to": "*", "message": "round 2"}) + "\n")
        self.source = self.root / "dev-viewer"
        self.source.mkdir()
        (self.source / "index.html").write_text(
            '<!doctype html><html><head><link rel="stylesheet" href="style.css"></head>'
            '<body><img src="mark.svg"><script src="app.js"></script></body></html>')
        (self.source / "style.css").write_text("body { color: red; }")
        (self.source / "app.js").write_text("env.onFrame((frame) => { document.title = '</script>' + frame.index; });")
        (self.source / "mark.svg").write_text('<svg xmlns="http://www.w3.org/2000/svg"></svg>')

    def tearDown(self):
        self.tmp.cleanup()

    def run_tool(self, *argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = viewer_tool.main(["--root", str(self.root), *argv])
        return code, json.loads(out.getvalue())

    def save(self, *extra, version=VERSION):
        return self.run_tool("save-viewer", DEFINITION, "board", str(self.source), "--title", "Board",
                             "--version", version, *extra)

    def test_patches_rebuild_states_and_uncommitted_rows_are_ignored(self):
        old = {"a/b": 1, "~": [1, 2, 3], "gone": True, "n": 1}
        new = {"a/b": 2, "~": [1, 4], "n": 1.0, "added": None}
        state = json.loads(json.dumps(old))
        for op in viewer_tool.diff(old, new):
            state = viewer_tool.apply(state, op)
        self.assertEqual((state, type(state["n"])), (new, float))
        history = viewer_tool.snapshot_history(self.root / "instances" / "run-1", committed=4)
        self.assertEqual({sequence: json.loads(text) for sequence, text in history.items()},
                         dict(enumerate(STATES, start=1)))

    def test_export_embeds_one_step_per_operation_and_inlines_the_viewer(self):
        code, result = self.run_tool("export", "run-1", "--source", str(self.source))
        self.assertEqual(code, 0, result)
        page = Path(result["html"]).read_text()
        data = decode_page(page)
        states, state = [], None
        for step in data["steps"]:
            state = step["state"] if "state" in step else state
            for op in step.get("patch", []):
                state = viewer_tool.apply(json.loads(json.dumps(state)), op)
            states.append(state)
        self.assertEqual(states, STATES)
        self.assertEqual([(s["op"]["method"], s["op"]["success"]) for s in data["steps"]],
                         [("initialize", True), ("invoke", True), ("invoke", False), ("invoke", True)])
        self.assertEqual([len(s["deliveries"]) for s in data["steps"]], [0, 0, 0, 1])
        self.assertEqual((data["meta"]["config"], data["meta"]["seats"][0]["model"], data["meta"]["finished"]),
                         ({"rounds": 2}, "m/one", "done"))
        self.assertNotIn('src="app.js"', page)
        self.assertIn("body { color: red; }", page)
        self.assertIn("data:image/svg+xml;base64,", page)
        self.assertEqual(page.count("</script>"), 2)  # only the outer page's own script elements

    def test_live_runs_and_runs_without_history_are_refused(self):
        code, result = self.run_tool("export", "live", "--source", str(self.source))
        self.assertEqual((code, "not ended" in result["refused"]), (1, True))
        (self.root / "instances" / "run-1" / "snapshots.jsonl").unlink()
        code, result = self.run_tool("export", "run-1", "--source", str(self.source))
        self.assertEqual((code, "no snapshot history" in result["refused"]), (1, True))

    def test_saved_viewers_are_linted_versioned_and_matched_to_versions(self):
        (self.source / "app.js").write_text("import x from 'https://cdn.example/lib.js';")
        code, result = self.save()
        self.assertEqual((code, "remote resource" in result["refused"]), (1, True))
        (self.source / "app.js").write_text("const run = '1b4e28ba-2fa1-11d2-883f-0016d3cca427';")
        code, result = self.save()
        self.assertEqual((code, "instance id" in result["refused"]), (1, True))
        (self.source / "app.js").write_text("env.onFrame(() => {});")
        self.assertEqual(self.save()[1]["revision"], "v1")
        code, result = self.save()
        self.assertEqual((code, "--approved" in result["refused"]), (1, True))
        (self.source / "app.js").write_text("env.onFrame(() => { document.title = 'v2'; });")
        self.assertEqual(self.save("--approved")[1]["revision"], "v2")

        code, result = self.run_tool("export", "run-2")
        self.assertEqual((code, "--trial" in result["refused"]), (1, True))
        code, result = self.run_tool("export", "run-2", "--trial")
        self.assertEqual((code, result["trial"]), (0, True))
        code, result = self.run_tool("export", "run-1")
        self.assertEqual((code, result["viewer_revision"], result["trial"]), (0, "v2", False))
        code, result = self.run_tool("export", "run-1")
        self.assertEqual((code, "--replace" in result["refused"]), (1, True))
        self.assertEqual(self.run_tool("find", "run-1")[1]["exports"][0]["state"], "current")
        (self.source / "app.js").write_text("env.onFrame(() => { document.title = 'v3'; });")
        self.save("--approved")
        self.assertEqual(self.run_tool("find", "run-1")[1]["exports"][0]["state"], "stale: viewer changed since export")


if __name__ == "__main__":
    unittest.main()
