"""Builder management through validated, host-owned environment handlers."""
from __future__ import annotations

from typing import Literal
from rlm import host_request

Json = None | bool | int | float | str | list["Json"] | dict[str, "Json"]


async def create_environment(description: str, language: Literal["python", "typescript"] = "python", python_execution: bool = False) -> Json:
    return await host_request("environments.create_environment", {"description": description, "language": language, "python_execution": python_execution})


async def select_policy(draft: str, policy: dict[str, Json]) -> Json:
    """Apply the policy the user requested; ask before choosing backend or grants."""
    return await host_request("environments.select_policy", {"draft": draft, "policy": policy})


async def validate_environment(draft: str) -> Json:
    return await host_request("environments.validate_environment", {"draft": draft})


async def save_environment(draft: str, id: str) -> Json:
    return await host_request("environments.save_environment", {"draft": draft, "id": id})


async def list_environments() -> Json:
    return await host_request("environments.list_environments")


async def start_instance(definition: str, seats: list[dict[str, Json]], limits: dict[str, Json] | None = None, policy: dict[str, Json] | None = None, config: dict[str, Json] | None = None) -> Json:
    """Provision a run after the user agrees to its token use and execution policy."""
    arguments: dict[str, Json] = {"definition": definition, "seats": seats}
    if limits is not None:
        arguments["limits"] = limits
    if policy is not None:
        arguments["policy"] = policy
    if config is not None:
        arguments["config"] = config
    return await host_request("environments.start_instance", arguments)


async def list_instances() -> Json:
    return await host_request("environments.list_instances")


async def instance_status(instance: str) -> Json:
    return await host_request("environments.instance_status", {"instance": instance})


async def pause_instance(instance: str) -> Json:
    """Pause a run: agent turns and environment mutations stop, state is kept. The user resumes it with /env resume_run."""
    return await host_request("environments.pause_instance", {"instance": instance})


async def resume_instance(instance: str) -> Json:
    """Resume a paused or stalled run, replaying pending seat inputs. Cumulative limits still apply; a stopped or finished run cannot resume."""
    return await host_request("environments.resume_instance", {"instance": instance})


async def stop_instance(instance: str) -> Json:
    """Stop a run permanently and end its seat sessions. Ask the user first unless they told you to stop it."""
    return await host_request("environments.stop_instance", {"instance": instance})


async def observe_seat(instance: str, seat: str) -> Json:
    return await host_request("environments.observe_seat", {"instance": instance, "seat": seat})


async def seat_transcript(instance: str, seat: str, since: int = 0) -> Json:
    return await host_request("environments.seat_transcript", {"instance": instance, "seat": seat, "since": since})


async def table_log(instance: str, since_seq: int = 0) -> Json:
    return await host_request("environments.table_log", {"instance": instance, "since_seq": since_seq})


async def view_instance(instance: str) -> Json:
    """Public live-view URL for a running instance whose saved viewer lists its version."""
    return await host_request("environments.view_instance", {"instance": instance})


async def act_for_seat(instance: str, seat: str, action: str, reason: str, arguments: dict[str, Json] | None = None) -> Json:
    """Take one action for a stuck agent seat on a turn the environment awaits from it.

    The environment validates it like the seat's own action; only that seat is told the
    builder acted and why. Works on a running, paused, or stalled run; a paused or
    stalled run stays so until resume_instance. Use it only when the seat cannot act
    itself, and choose the least consequential legal action unless the user said otherwise.
    """
    return await host_request("environments.act_for_seat", {"instance": instance, "seat": seat, "action": action, "arguments": arguments or {}, "reason": reason})


async def suspend_seat(instance: str, seat: str, reason: str) -> Json:
    """Take an agent seat out of play: its turn is cut and it gets no further turns.

    An environment with missed_turn hears of each turn it awaits from the seat and may
    play for it; otherwise the run stalls on that turn until you act_for_seat (then
    resume_instance) or resume_seat. Tell the user which seat you suspended and why.
    """
    return await host_request("environments.suspend_seat", {"instance": instance, "seat": seat, "reason": reason})


async def resume_seat(instance: str, seat: str) -> Json:
    """Return a suspended seat to play; it receives the wakes held back while suspended."""
    return await host_request("environments.resume_seat", {"instance": instance, "seat": seat})
