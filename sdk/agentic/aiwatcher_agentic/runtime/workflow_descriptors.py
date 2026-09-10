from __future__ import annotations

import inspect
import textwrap
from collections.abc import Callable
from dataclasses import dataclass

from aiwatcher_agentic.workflow._workflow import AgenticWorkflow


@dataclass(frozen=True, slots=True)
class WorkflowDescriptor:
    agent_name: str
    workflow_name: str
    description: str
    capabilities: tuple[str, ...]
    handler: Callable[..., object] | None


def build_workflow_descriptor(workflow: AgenticWorkflow) -> WorkflowDescriptor:
    description = workflow.description
    return WorkflowDescriptor(
        agent_name=description.agent_name,
        workflow_name=workflow.__class__.__name__,
        description=description.description,
        capabilities=tuple(description.capabilities),
        handler=getattr(workflow, "handle", None),
    )


def render_workflow_descriptor(descriptor: WorkflowDescriptor) -> str:
    capabilities = ", ".join(descriptor.capabilities)
    try:
        if descriptor.handler is None:
            raise TypeError("workflow has no handle")
        handler_source = inspect.getsource(descriptor.handler)
        rendered_source = textwrap.dedent(handler_source).strip()
    except (OSError, TypeError):
        rendered_source = (
            f"def handle(message):\n"
            f"    # source unavailable\n"
            f"    # workflow={descriptor.workflow_name}\n"
            f"    # capabilities={capabilities}"
        )
    return (
        f"- {descriptor.agent_name} (workflow: {descriptor.workflow_name}): "
        f"{descriptor.description} (capabilities: {capabilities})\n"
        f"{rendered_source}"
    )


def render_workflow_descriptors(workflows: list[AgenticWorkflow]) -> str:
    return "\n\n".join(
        render_workflow_descriptor(build_workflow_descriptor(workflow)) for workflow in workflows
    )
