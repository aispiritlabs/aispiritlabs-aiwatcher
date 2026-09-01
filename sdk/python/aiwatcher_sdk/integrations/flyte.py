"""Flyte's own execution metadata, as aiwatcher's correlation ids.

A task running under Flyte already knows which execution it belongs to — the
scheduler put it in the pod's environment. What it does not know is that
aiwatcher exists, so without these four lines every stage of one pipeline
publishes as an unrelated run and the workflow view has nothing to join on::

    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.integrations.flyte import flyte_execution, workflow_arguments

    client = AiwatcherClient(service="planner-import")
    with client.workflow(**workflow_arguments("house-import", nodes=NODES, edges=EDGES)) as flow:
        with flow.node("acquire") as stage:
            stage.artifact("acquisition.json", uri=uri)

Outside Flyte — which is planner's own `settings.flyte_enabled = False` path —
:func:`flyte_execution` returns ``None`` and
:func:`workflow_arguments` simply omits the execution id, so the run *is* the
execution. That is the correct answer there and it needs no branch at the call
site.

## The two ids, and which one to prefer

``workflow_run_id`` is what joins the stages a per-pod orchestrator scatters
across several runs (ADR_0012). Two values can play that part, and they are not
interchangeable:

* **The id aiwatcher minted**, when the execution was launched from aiwatcher's
  own engine routes (ADR_0016). It arrives as the ``aiwatcher_workflow_run_id``
  input, if the entity declares one — pass it to ``workflow_run_id=`` — or as
  ``AIWATCHER_WORKFLOW_RUN_ID`` if the deployment puts it in the environment.
  This is the one to prefer: it is the id the panel was already streaming
  before the first pod started.
* **Flyte's own execution id**, otherwise. It groups the stages correctly and
  is the right answer for an execution nobody started from aiwatcher — a
  schedule, a `flyte run`, another service.

## These variables are internal to Flyte

``FLYTE_INTERNAL_*`` is documented as internal and has changed shape before.
Everything here degrades to ``None`` rather than raising, and the only cost of
Flyte renaming one is the join going quiet — which is why
:func:`flyte_execution` takes an ``environ`` argument and is tested against a
dictionary rather than against a cluster.
"""

from __future__ import annotations

import os
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

__all__ = [
    "AIWATCHER_RUN_ID_ENV",
    "FlyteExecution",
    "flyte_execution",
    "workflow_arguments",
]

#: Where a deployment may put the id aiwatcher minted, when passing it as a
#: declared input is not convenient. Read before Flyte's own execution id.
AIWATCHER_RUN_ID_ENV = "AIWATCHER_WORKFLOW_RUN_ID"


@dataclass(frozen=True, slots=True)
class FlyteExecution:
    """What the pod's environment says about the work it is doing."""

    #: The registered workflow, which is what to group executions by.
    workflow_id: str
    #: The traversal every stage of this execution shares.
    workflow_run_id: str
    #: The task this pod is, which is the node of the graph it executes.
    node: str | None
    project: str
    domain: str
    version: str
    #: Flyte retries a failed node; the second attempt is a different pod
    #: publishing about the same node.
    attempt: int
    #: Whether the id above came from aiwatcher rather than from Flyte. False
    #: means the execution was started somewhere else, which is not a problem
    #: — only a different answer to "who asked for this".
    launched_from_aiwatcher: bool


def flyte_execution(
    *,
    workflow_run_id: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> FlyteExecution | None:
    """This pod's execution, or ``None`` when it is not running under Flyte.

    ``workflow_run_id`` is the ``aiwatcher_workflow_run_id`` input, when the
    entity declares one and aiwatcher filled it in. It wins over everything
    else, because it is the id somebody in the panel is already watching.
    """
    source = os.environ if environ is None else environ
    execution = _text(source, "FLYTE_INTERNAL_EXECUTION_ID")
    if execution is None:
        # Not under Flyte. planner's in-process path lands here, and so does
        # every test — which is the point of returning rather than raising.
        return None

    supplied = workflow_run_id or _text(source, AIWATCHER_RUN_ID_ENV)
    return FlyteExecution(
        # `EXECUTION_WORKFLOW` is the registered workflow; the task name is the
        # fallback for a single-task execution, where they are the same thing
        # said two ways.
        workflow_id=_text(source, "FLYTE_INTERNAL_EXECUTION_WORKFLOW")
        or _text(source, "FLYTE_INTERNAL_TASK_NAME")
        or execution,
        workflow_run_id=supplied or execution,
        node=_text(source, "FLYTE_INTERNAL_TASK_NAME"),
        project=_text(source, "FLYTE_INTERNAL_EXECUTION_PROJECT") or "",
        domain=_text(source, "FLYTE_INTERNAL_EXECUTION_DOMAIN") or "",
        version=_text(source, "FLYTE_INTERNAL_TASK_VERSION") or "",
        attempt=_number(source, "FLYTE_ATTEMPT_NUMBER"),
        launched_from_aiwatcher=supplied is not None,
    )


def workflow_arguments(
    workflow_id: str | None = None,
    *,
    nodes: list[str] | list[dict[str, Any]] | None = None,
    edges: list[tuple[str, str]] | list[dict[str, Any]] | None = None,
    workflow_run_id: str | None = None,
    environ: Mapping[str, str] | None = None,
) -> dict[str, Any]:
    """Keyword arguments for :meth:`AiwatcherClient.workflow`.

    Off Flyte this is ``{"workflow_id": ...}`` and nothing else, so the same
    call site works in both paths — which is the whole reason it returns a
    mapping rather than opening the context manager itself.

    ``workflow_id`` overrides what the environment reports. Pass it: a name a
    person chose is what the catalog is browsed by, and Flyte's registered name
    is usually a module path.
    """
    execution = flyte_execution(workflow_run_id=workflow_run_id, environ=environ)
    arguments: dict[str, Any] = {
        "workflow_id": workflow_id or (execution.workflow_id if execution else None)
    }
    if arguments["workflow_id"] is None:
        raise ValueError(
            "workflow_id is required when not running under Flyte: "
            "nothing in the environment names the orchestration"
        )
    if nodes is not None:
        arguments["nodes"] = nodes
    if edges is not None:
        arguments["edges"] = edges
    if execution is not None:
        arguments["execution_id"] = execution.workflow_run_id
    return arguments


def _text(source: Mapping[str, str], name: str) -> str | None:
    value = source.get(name, "").strip()
    return value or None


def _number(source: Mapping[str, str], name: str) -> int:
    raw = _text(source, name)
    if raw is None:
        return 0
    try:
        return int(raw)
    except ValueError:
        # An attempt number that will not parse is not worth failing a run
        # over; it is a label on a retry.
        return 0
