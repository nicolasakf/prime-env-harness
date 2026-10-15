"""Run an environment in a separate process; stdout belongs to the protocol."""
import asyncio
import inspect
import json
import sys
from pathlib import Path

from sdk import _protocol_out, _begin_request, _response_metadata

sys.stdout = sys.stderr
from environment import environment


def emit(value):
    _protocol_out.write(json.dumps(value, allow_nan=False) + "\n")
    _protocol_out.flush()


protocol_version = json.loads(Path(__file__).with_name("environment.json").read_text())["protocol_version"]
if protocol_version not in (1, 2):
    raise ValueError("unsupported environment protocol")
emit({"kind": "ready", "protocol_version": protocol_version})
for line in sys.stdin:
    request = json.loads(line)
    request_id = request.get("id")
    try:
        if request.get("protocol_version") != protocol_version:
            raise ValueError("unsupported environment protocol")
        method = request["method"]
        _begin_request(method)
        params = request.get("params") or {}
        participant = request.get("participant")
        if method == "initialize":
            result = environment.initialize(params)
        elif method == "describe_actions":
            result = environment.describe_actions()
        elif method == "observe":
            result = environment.observe(participant)
        elif method == "agent_view":
            result = (getattr(environment, "agent_view", None) or environment.observe)(participant)
        elif method == "handle_input":
            result = environment.handle_input(participant, params["text"])
        elif method == "invoke":
            result = environment.invoke(participant, params["action"], params["arguments"])
        elif method == "snapshot":
            result = environment.snapshot()
        elif method == "restore":
            result = environment.restore(request["params"])
        elif method == "dispose":
            result = environment.dispose()
        else:
            raise ValueError("unknown environment method")
        if inspect.isawaitable(result):
            result = asyncio.run(result)
        emit({"kind": "response", "id": request_id, "success": True, "result": result, **_response_metadata()})
        if method == "dispose":
            break
    except Exception as error:
        emit({"kind": "response", "id": request_id, "success": False, "error": str(error)})
