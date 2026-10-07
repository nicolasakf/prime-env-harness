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
