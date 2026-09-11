"""Workflow definitions describe a process; execution placement belongs to Runtime."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import asdict, dataclass, field
from graphlib import CycleError, TopologicalSorter
from typing import Any

from aiwatcher_sdk.task import Task


@dataclass(frozen=True)
class RetryPolicy:
    max_attempts: int = 3
    max_unavailable_attempts: int = 10
    delays_seconds: tuple[int, ...] = (1, 5, 30)
    delays_seconds_unavailable: tuple[int, ...] = (5, 15, 30, 60)


@dataclass(frozen=True)
class WorkflowInput:
    step: str
    output: str


@dataclass(frozen=True)
class PodRequest:
    """A pod of the step's own, from a template an operator wrote (ADR_0029).

    ``template`` names it and ``image`` has to be on that template's list,
    matched by repository; the tag or digest is yours. ``cpu`` and ``memory``
    are Kubernetes quantities, each the request and the limit both, and absent
    ones come from the template. The server checks all of it when the
    definition is registered, against the deployment's own templates, so
    nothing is checked here. Everything else a pod holds — volumes, secrets, a
    service account — is the template's and cannot be said in a step.
    """

    template: str
    image: str
    cpu: str | None = None
    memory: str | None = None

    def as_definition(self) -> dict[str, object]:
        pod: dict[str, object] = {"template": self.template, "image": self.image}
        if self.cpu is not None:
            pod["cpu"] = self.cpu
        if self.memory is not None:
            pod["memory"] = self.memory
        return pod


@dataclass(frozen=True)
class WorkflowStep:
    """One invocation in a static workflow, not one worker or deployment."""

    name: str
    task: Task[Any, Any]
    after: tuple[str, ...] = ()
    inputs: tuple[WorkflowInput, ...] = ()
    outputs: tuple[str, ...] = ()
    params: Mapping[str, object] = field(default_factory=dict)
    retry: RetryPolicy = field(default_factory=RetryPolicy)
    timeout_seconds: int = 300
    #: Set to run this step in a pod of its own rather than on whichever
    #: worker holds the queue. Opt-in per step.
    pod: PodRequest | None = None

    @property
    def dependencies(self) -> tuple[str, ...]:
        return tuple(dict.fromkeys((*self.after, *(item.step for item in self.inputs))))


@dataclass(frozen=True)
class OnTimeout:
    """What happens to a gate whose deadline ran out.

    ``fail`` stops the run, ``skip`` passes the step over, and ``answer``
    writes ``response`` and records that a policy did, never a person. Authored
    with a deadline and refused without one.
    """

    on: str = "fail"
    response: object | None = None

    def as_definition(self) -> dict[str, object]:
        if self.on == "answer":
            return {"on": "answer", "response": self.response}
        return {"on": self.on}


@dataclass(frozen=True)
class ApprovalStep:
    """A step that waits for a person instead of running code.

    The graph stops here until somebody answers, and the answer is the whole of
    what this step does: it registers no task, claims no queue, produces no
    output and is taken once. Order it with ``after`` like any other step, and
    give it ``inputs`` when the person deciding should see what they are
    deciding about.

    What a valid question is — the length of the prompt, the answers, which
    roles may be named — is the server's rule and is not repeated here. It refuses a bad one
    with every problem at once, which a check in this file could only turn into
    the first of them.
    """

    name: str
    prompt: str
    choices: tuple[str, ...] = ()
    #: Who may answer. A gate only ever raises the floor — every write already
    #: needs an editor — so this is `editor` or `admin`, and the server refuses
    #: anything weaker.
    role: str = "editor"
    #: How long the graph waits here. `None` waits as long as it takes, which is
    #: the default and the honest thing for a decision somebody thinks about.
    timeout_seconds: int | None = None
    on_timeout: OnTimeout = field(default_factory=OnTimeout)
    after: tuple[str, ...] = ()
    inputs: tuple[WorkflowInput, ...] = ()

    @property
    def dependencies(self) -> tuple[str, ...]:
        return tuple(dict.fromkeys((*self.after, *(item.step for item in self.inputs))))


type Step = WorkflowStep | ApprovalStep


@dataclass(frozen=True)
class Workflow:
    """A versioned static process with explicit dependency edges.

    Building a definition validates its shape without executing application code.
    Runtime registers the definition and starts executions through the API.
    The Rust server compiles the graph and owns all scheduling and retries.
    """

    name: str
    version: str
    steps: tuple[Step, ...]

    def __post_init__(self) -> None:
        if any(
            not value.strip() or value != value.strip() or "@" in value
            for value in (self.name, self.version)
        ):
            raise ValueError("a workflow requires a name and version without @ or outer whitespace")
        if not self.steps:
            raise ValueError("a workflow must contain at least one step")
        names = {step.name for step in self.steps}
        if len(names) != len(self.steps) or any(not name.strip() for name in names):
            raise ValueError("workflow step names must be nonempty and unique")
        for step in self.steps:
            if missing := set(step.dependencies) - names:
                raise ValueError(f"step {step.name!r} depends on unknown steps: {sorted(missing)}")
        try:
            tuple(
                TopologicalSorter(
                    {step.name: step.dependencies for step in self.steps}
                ).static_order()
            )
        except CycleError as error:
            raise ValueError("workflow dependencies contain a cycle") from error

    @property
    def ref(self) -> str:
        return f"{self.name}@{self.version}"

    def get_tasks(self) -> tuple[Task[Any, Any], ...]:
        """Every task a worker of this workflow has to register.

        A gate contributes none: nothing claims it, so a worker that registered
        one would be advertising work it can never be handed.
        """
        tasks: dict[str, Task[Any, Any]] = {}
        for step in self.steps:
            if not isinstance(step, WorkflowStep):
                continue
            if step.task.ref in tasks and tasks[step.task.ref] is not step.task:
                raise ValueError(f"conflicting implementations of task {step.task.ref}")
            tasks[step.task.ref] = step.task
        return tuple(tasks.values())

    def to_definition(self, queue: str) -> dict[str, object]:
        """Bind execution placement at the runtime boundary, outside task code."""
        return {
            "name": self.name,
            "version": self.version,
            "steps": [as_definition_step(step, queue) for step in self.steps],
        }


def as_definition_step(step: Step, queue: str) -> dict[str, object]:
    """One step as the server's definition holds it.

    A gate omits every field a step that runs needs rather than sending them
    empty, because the server refuses a gate that names one: a queue nobody
    claims on and a timeout nothing arms would sit on a stored definition and
    read, to whoever opens it next, as things this system does.
    """
    if isinstance(step, ApprovalStep):
        approval: dict[str, object] = {
            "prompt": step.prompt,
            "role": step.role,
            "choices": list(step.choices),
        }
        # Sent only when there is one: the pair is authored together, and a
        # policy with no deadline is refused rather than stored.
        if step.timeout_seconds is not None:
            approval["timeout_seconds"] = step.timeout_seconds
            approval["on_timeout"] = step.on_timeout.as_definition()
        return {
            "id": step.name,
            "approval": approval,
            "after": list(step.after),
            "inputs": [asdict(item) for item in step.inputs],
        }
    definition: dict[str, object] = {
        "id": step.name,
        "task_ref": step.task.ref,
        "queue": queue,
        "after": list(step.after),
        "inputs": [asdict(item) for item in step.inputs],
        "outputs": list(step.outputs),
        "params": dict(step.params),
        "retry": asdict(step.retry),
        "timeout_seconds": step.timeout_seconds,
    }
    # Sent only when there is one, so a step without a pod registers the same
    # revision it always did.
    if step.pod is not None:
        definition["pod"] = step.pod.as_definition()
    return definition
