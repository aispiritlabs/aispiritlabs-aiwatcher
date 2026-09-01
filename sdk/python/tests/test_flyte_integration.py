"""Reading Flyte's environment, and what it becomes on the wire.

Against a dictionary rather than a cluster, which is the point of
``environ=``: ``FLYTE_INTERNAL_*`` is documented as internal to Flyte, so the
failure worth testing is what happens when one of them is missing or renamed.
"""

from __future__ import annotations

from typing import Any

import pytest

from aiwatcher_sdk import AiwatcherClient
from aiwatcher_sdk.integrations.flyte import (
    AIWATCHER_RUN_ID_ENV,
    flyte_execution,
    workflow_arguments,
)

POD = {
    "FLYTE_INTERNAL_EXECUTION_ID": "a018f3a2b7c417b3e9d5",
    "FLYTE_INTERNAL_EXECUTION_PROJECT": "planner",
    "FLYTE_INTERNAL_EXECUTION_DOMAIN": "production",
    "FLYTE_INTERNAL_EXECUTION_WORKFLOW": "house_import_flow",
    "FLYTE_INTERNAL_TASK_NAME": "acquire",
    "FLYTE_INTERNAL_TASK_VERSION": "v7",
    "FLYTE_ATTEMPT_NUMBER": "1",
}


class RecordingTransport:
    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.events.extend(batch)

    def close(self) -> None:
        return None


def test_outside_flyte_there_is_no_execution() -> None:
    # planner's own `settings.flyte_enabled = False` path. Returning None
    # rather than raising is what lets one call site serve both.
    assert flyte_execution(environ={}) is None


def test_the_pods_environment_becomes_the_correlation_ids() -> None:
    execution = flyte_execution(environ=POD)
    assert execution is not None
    assert execution.workflow_id == "house_import_flow"
    assert execution.workflow_run_id == "a018f3a2b7c417b3e9d5"
    assert execution.node == "acquire"
    assert execution.attempt == 1
    assert not execution.launched_from_aiwatcher


def test_an_id_aiwatcher_minted_wins_over_flytes_own() -> None:
    # It is the id the panel started streaming before the first pod existed.
    # Preferring Flyte's here would leave that view empty for the whole run.
    minted = "018f3a2b7c417b3e9d552f6a1c0b8e77"

    from_input = flyte_execution(workflow_run_id=minted, environ=POD)
    assert from_input is not None
    assert from_input.workflow_run_id == minted
    assert from_input.launched_from_aiwatcher

    from_environment = flyte_execution(environ={**POD, AIWATCHER_RUN_ID_ENV: minted})
    assert from_environment is not None
    assert from_environment.workflow_run_id == minted
    assert from_environment.launched_from_aiwatcher


def test_a_renamed_internal_variable_degrades_rather_than_raising() -> None:
    # `FLYTE_INTERNAL_*` has changed shape before. The cost of it changing
    # again is the workflow name falling back, never a task that cannot start.
    without_workflow = {key: value for key, value in POD.items() if "WORKFLOW" not in key}
    execution = flyte_execution(environ=without_workflow)
    assert execution is not None
    assert execution.workflow_id == "acquire"

    bare = {"FLYTE_INTERNAL_EXECUTION_ID": "a0", "FLYTE_ATTEMPT_NUMBER": "not-a-number"}
    minimal = flyte_execution(environ=bare)
    assert minimal is not None
    assert minimal.workflow_id == "a0"
    assert minimal.attempt == 0


def test_the_arguments_join_every_stage_of_one_execution() -> None:
    transport = RecordingTransport()
    client = AiwatcherClient(service="planner-import", transport=transport)

    for node in ("acquire", "normalize"):
        pod = {**POD, "FLYTE_INTERNAL_TASK_NAME": node}
        with (
            client.workflow(
                **workflow_arguments(
                    "house-import",
                    nodes=["acquire", "normalize"],
                    edges=[("acquire", "normalize")],
                    environ=pod,
                )
            ) as flow,
            flow.node(node),
        ):
            pass

    started = [event for event in transport.events if event["event_type"] == "run.started"]
    assert len(started) == 2
    # Two pods, two runs, one execution — the join a runs list cannot express.
    assert {event["run_id"] for event in started} != {"a018f3a2b7c417b3e9d5"}
    assert {event["workflow_run_id"] for event in started} == {"a018f3a2b7c417b3e9d5"}
    assert {event["workflow_id"] for event in started} == {"house-import"}


def test_off_flyte_the_run_is_the_execution() -> None:
    arguments = workflow_arguments("house-import", environ={})
    assert arguments == {"workflow_id": "house-import"}

    with pytest.raises(ValueError, match="workflow_id is required"):
        workflow_arguments(environ={})
