"""Environment contracts v1 and v2. No participant identity is accepted from action args."""
from __future__ import annotations

import json
import re
import sys
from typing import Awaitable, Literal, Protocol, TypedDict

SeatId = str
Participant = SeatId
RoleName = str
Json = None | bool | int | float | str | list["Json"] | dict[str, "Json"]


class Action(TypedDict, total=False):
    name: str
    description: str
    input_schema: Json
    output_schema: Json
    participants: list[RoleName]
    roles: list[RoleName]


class InputResult(TypedDict):
    """Human observation plus an optional private message delivered to the agent.

    Agent observations and tool responses are private on the human screen too.
    Put public information in the human observation, never in action schemas.
    """
    observation: Json
    agent_message: str | None


class SeatInputResult(TypedDict):
    observation: Json


class Environment(Protocol):
    def initialize(self, params: Json) -> Json | Awaitable[Json]: ...
    def describe_actions(self) -> list[Action] | Awaitable[list[Action]]: ...
    def observe(self, participant: Participant) -> Json | Awaitable[Json]: ...
    # Optional. With manifest "agent_context": "stateless" this is ALL an agent
    # sees each turn: current state plus a bounded history summary, filtered for
    # the seat. Defaults to observe(). Keep it under 32 KiB.
    # def agent_view(self, participant: Participant) -> Json | Awaitable[Json]: ...
    # Optional. What a live viewer draws: {"kind": "public"} or
    # {"kind": "seat", "seat": id}. Filter it like observe(); without it the
    # run has no live view. Return a JSON object under 1 MiB.
    # def frame(self, perspective: Json) -> Json | Awaitable[Json]: ...
    # Optional, with manifest "missed_turns": true. Called when an awaited agent
    # seat's turn ends without an action. report is {"cause": "idle" | "deadline"
    # | "suspended", "misses": consecutive misses including this one, "limit": host
    # re-prompts}. Warn the seat with notify(..., wake=True), act for it, or do
    # nothing; without a wake from here the host re-prompts generically, and once
    # misses exceed limit it stops the run unless this call resolved the seat's turn.
    # "suspended" means the builder took the seat out of play: it is reported once
    # per progress revision and never re-prompted, so act for the seat to keep the
    # run moving; otherwise the run stalls until the builder acts or resumes it.
    # def missed_turn(self, participant: Participant, report: Json) -> Json | Awaitable[Json]: ...
    def handle_input(self, participant: Participant, text: str) -> InputResult | SeatInputResult | Awaitable[InputResult | SeatInputResult]: ...
    def invoke(self, participant: Participant, action: str, arguments: Json) -> Json | Awaitable[Json]: ...
    def snapshot(self) -> Json | Awaitable[Json]: ...
    def restore(self, snapshot: Json) -> Json | Awaitable[Json]: ...
    def dispose(self) -> Json | Awaitable[Json]: ...


_protocol_out = sys.stdout
_protocol_in = sys.stdin
_call_number = 0


def host_call(method: str, params: Json) -> Json:
    """Call a selected host integration; identity and grants are host-bound."""
    global _call_number
    _call_number += 1
    request_id = str(_call_number)
    _protocol_out.write(json.dumps({"kind": "host_call", "id": request_id, "method": method, "params": params}) + "\n")
    _protocol_out.flush()
    response = json.loads(_protocol_in.readline())
    if response.get("id") != request_id:
        raise RuntimeError("host response id mismatch")
    if not response.get("success"):
        raise RuntimeError(response.get("error", "host call failed"))
    return response.get("result")


_notice_buffer = []
_finish_buffer = None
_progress_buffer = None
_request_method = None


def _begin_request(method):
    global _notice_buffer, _finish_buffer, _progress_buffer, _request_method
    _notice_buffer, _finish_buffer, _progress_buffer = [], None, None
    _request_method = method


def _mutation_metadata():
    if _request_method not in (None, "initialize", "invoke", "handle_input", "missed_turn"):
        raise ValueError("notifications and completion require a committed operation")


def notify(to, message=None, *, wake=True, actionable=False):
    """Commit a notice with the operation; only wake=True requests an agent turn.

    actionable=True marks a wake as the task for the named seats to act on now. The
    host drops it from a wake that starts after the environment stopped awaiting the
    seat (see progress(waiting_for=...)), so a queued "your turn" never reaches a
    model that no longer has one. Messages and ordinary notices are never dropped.
    It needs wake=True and named seats, not "*".
    """
    _mutation_metadata()
    if to != "*" and (not isinstance(to, list) or not to or len(to) > 16 or any(not isinstance(seat, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", seat) for seat in to)):
        raise ValueError("notify target must be '*' or 1–16 seat IDs")
    if message is not None and (not isinstance(message, str) or len(message.encode("utf-8")) > 16384):
        raise ValueError("notice message must be at most 16 KiB")
    if not isinstance(wake, bool) or not isinstance(actionable, bool):
        raise ValueError("wake and actionable must be booleans")
    if actionable and (not wake or to == "*"):
        raise ValueError("an actionable notice must wake named seats")
    if len(_notice_buffer) >= 64:
        raise ValueError("at most 64 notices per operation")
    notice = {"to": to if to == "*" else list(to), "wake": wake}
    if actionable:
        notice["actionable"] = True
    if message is not None:
        notice["message"] = message
    _notice_buffer.append(notice)


def finish(summary):
    """Finish the instance durably with this operation."""
    global _finish_buffer
    _mutation_metadata()
    if not isinstance(summary, str) or len(summary.encode("utf-8")) > 16384:
        raise ValueError("finish summary must be at most 16 KiB")
    _finish_buffer = {"summary": summary}


def progress(revision, *, waiting_for=None):
    """Publish monotonic domain progress and the seats awaited by the environment."""
    global _progress_buffer
    _mutation_metadata()
    seats = [] if waiting_for is None else waiting_for
    if isinstance(revision, bool) or not isinstance(revision, int) or not 0 <= revision <= 9007199254740991:
        raise ValueError("revision must be a nonnegative safe integer")
    if not isinstance(seats, list) or any(not isinstance(seat, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", seat) for seat in seats):
        raise ValueError("waiting_for must be a list of seat IDs")
    _progress_buffer = {"revision": revision, "waiting_for": list(seats)}


def _response_metadata():
    result = {}
    if _notice_buffer:
        result["notices"] = _notice_buffer
    if _finish_buffer is not None:
        result["finished"] = _finish_buffer
    if _progress_buffer is not None:
        result["progress"] = _progress_buffer
    return result
