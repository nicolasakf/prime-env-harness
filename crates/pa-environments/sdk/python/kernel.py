"""Persistent participant Python. Revive pickles only in this same backend."""
import ast
import contextlib
import io
import json
import os
import pickle
import sys
import traceback

out = sys.stdout
sys.stdout = sys.stderr
number = 0


def emit(value):
    out.write(json.dumps(value, allow_nan=False) + "\n")
    out.flush()


class ActionClient:
    def _call(self, method, params):
        global number
        number += 1
        request_id = str(number)
        emit({"kind": "host_call", "id": request_id, "method": method, "params": params})
        response = json.loads(sys.stdin.readline())
        if response.get("id") != request_id or not response.get("success"):
            raise RuntimeError(response.get("error", "host call failed"))
        return response.get("result")

    def observe(self):
        return self._call("environment.observe", {})

    def invoke(self, action, arguments):
        return self._call("environment.invoke", {"action": action, "arguments": arguments})

    def integration(self, name, arguments):
        return self._call("integration.call", {"name": name, "arguments": arguments})


namespace = {"__builtins__": __builtins__, "env": ActionClient()}
snapshot_path = os.path.join(os.getcwd(), ".python-state.pkl")
if os.path.exists(snapshot_path):
    with open(snapshot_path, "rb") as saved:
        namespace.update(pickle.load(saved))
namespace["env"] = ActionClient()
emit({"kind": "ready", "protocol_version": 1})
for line in sys.stdin:
    request = json.loads(line)
    try:
        if request.get("protocol_version") != 1:
            raise ValueError("unsupported kernel protocol")
        method = request["method"]
        if method == "execute":
            output = io.StringIO()
            tree = ast.parse(request["params"]["code"])
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                last = tree.body.pop() if tree.body and isinstance(tree.body[-1], ast.Expr) else None
                exec(compile(tree, "<participant>", "exec"), namespace)
                value = eval(compile(ast.Expression(last.value), "<participant>", "eval"), namespace) if last else None
            result = {"stdout": output.getvalue()[:65536], "result": repr(value)[:65536] if value is not None else None}
        elif method in ("checkpoint", "dispose"):
            variables = {}
            omitted = []
            for name, value in namespace.items():
                if name in ("__builtins__", "env") or name.startswith("__"):
                    continue
                try:
                    payload = pickle.dumps(value)
                    if len(payload) > 16 * 1024 * 1024:
                        omitted.append(name)
                    else:
                        variables[name] = value
                except Exception:
                    omitted.append(name)
            payload = pickle.dumps(variables)
            if len(payload) > 256 * 1024 * 1024:
                raise ValueError("Python snapshot exceeds 256 MiB")
            with open(snapshot_path + ".tmp", "wb") as saved:
                saved.write(payload)
                saved.flush()
                os.fsync(saved.fileno())
            os.replace(snapshot_path + ".tmp", snapshot_path)
            result = {"omitted_variables": omitted}
        else:
            raise ValueError("unknown participant kernel method")
        emit({"kind": "response", "id": request["id"], "success": True, "result": result})
        if method == "dispose":
            break
    except Exception:
        emit({"kind": "response", "id": request["id"], "success": False, "error": traceback.format_exc()[-65536:]})
