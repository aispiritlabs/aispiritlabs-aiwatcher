"""DeepEval's ``PromptOptimizer``, into the registry.

DeepEval will happily search for a better prompt and hand back an
``OptimizationReport``. What it does not do is keep the result anywhere, decide
whether the result is real, or connect it to the traces the optimised prompt
then produces. This module is the three lines that do.

    from deepeval.optimizer import PromptOptimizer
    from aiwatcher_sdk.integrations.deepeval import record_optimization

    report = PromptOptimizer(...).optimize(...)
    record = record_optimization(
        registry,
        "planner.floor-plan",
        report=report,
        baseline=baseline_version_id,
        dev=scores(dev_before, dev_after),
        test=scores(test_before, test_after),   # the held-out split
        dataset="house-catalog@3",
        promote=True,
    )
    if not record.admitted:
        raise SystemExit(f"not promoted: {record.reason}")

**Nothing here imports deepeval.** The report is read structurally — the
attributes DeepEval 4.1 and 4.2 both expose — so this SDK stays dependency-free
and a version bump in DeepEval does not become a version bump here. A service
that has deepeval installed passes its report; one that does not can pass any
object with the same shape, which is also what makes this testable.

**The held-out split is the caller's job.** DeepEval's optimiser reports the
scores it searched against, and those are exactly the scores that must not
decide anything: it selected the candidate by maximising them. So ``test=``
takes numbers the caller measured on cases the optimiser never saw, and an
optimisation recorded without them is refused a promotion by the server. See
``docs/ADR/ADR_0011_PROMPT_REGISTRY.md``.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any, Protocol, runtime_checkable

from aiwatcher_sdk.prompts import OptimizationRecord, Score

__all__ = [
    "ALGORITHM",
    "Recorder",
    "best_prompt_text",
    "record_optimization",
    "serialize_report",
]

#: What lands in the record's ``algorithm`` field when the caller names none.
ALGORITHM = "deepeval/PromptOptimizer"


class Recorder(Protocol):
    """The one method this bridge needs from a registry.

    A structural type rather than :class:`~aiwatcher_sdk.prompts.PromptRegistry`
    itself, and written out in full rather than as ``**kwargs``: it says
    exactly what the bridge sends, so a change to the registry's signature is a
    type error here rather than a runtime one. It is also what lets a test
    stand in for the registry without an HTTP server.
    """

    def record_optimization(
        self,
        name: str,
        *,
        algorithm: str,
        baseline: str,
        candidate_text: str,
        primary_metric: str,
        dev: Sequence[Score] = ...,
        test: Sequence[Score] = ...,
        dataset: str | None = ...,
        evaluation_id: str | None = ...,
        optimization_id: str | None = ...,
        started_at: str | None = ...,
        duration_ms: float | None = ...,
        iterations: int | None = ...,
        report: Mapping[str, Any] | None = ...,
        promote: bool = ...,
    ) -> OptimizationRecord: ...


@runtime_checkable
class OptimizationReport(Protocol):
    """The part of DeepEval's report this reads.

    A structural type rather than an import: it documents the coupling exactly,
    it holds across the DeepEval versions that share these names, and it lets a
    test pass a plain object instead of standing up an optimiser.
    """

    optimization_id: str
    best_id: str
    prompt_configurations: Mapping[str, Any]


def best_prompt_text(report: object) -> str:
    """The winning prompt's text, out of the report's configuration tree.

    DeepEval nests it: ``prompt_configurations[best_id].prompts[module].text_template``.
    A configuration can hold several modules; where it does, the first with
    text wins, because a single-prompt optimisation — which is what this is for
    — has exactly one.

    Falls back to the chat template's messages joined by blank lines when the
    optimiser was working on messages rather than on one string. That is a
    lossy rendering of a structured prompt, and it is the honest one: what the
    registry versions is the text a model was given.
    """
    configurations = getattr(report, "prompt_configurations", None) or {}
    best_id = getattr(report, "best_id", None)
    configuration = configurations.get(best_id) if isinstance(configurations, Mapping) else None
    if configuration is None:
        raise ValueError(
            f"the report has no configuration for its best id {best_id!r}; "
            "pass candidate_text= explicitly"
        )

    prompts = getattr(configuration, "prompts", None)
    if isinstance(configuration, Mapping):
        prompts = configuration.get("prompts", prompts)
    for prompt in (prompts or {}).values():
        text = _text_of(prompt)
        if text:
            return text
    raise ValueError(
        "the winning configuration carries no prompt text; pass candidate_text= explicitly"
    )


def _text_of(prompt: object) -> str | None:
    template = _attribute(prompt, "text_template")
    if isinstance(template, str) and template.strip():
        return template
    messages = _attribute(prompt, "messages_template")
    if not messages:
        return None
    rendered = "\n\n".join(
        str(_attribute(message, "content") or "") for message in messages
    ).strip()
    return rendered or None


def _attribute(source: object, name: str) -> Any:
    if isinstance(source, Mapping):
        return source.get(name)
    return getattr(source, name, None)


def serialize_report(report: object) -> dict[str, Any]:
    """The report as JSON, for the record's ``report`` field.

    Only the parts that answer "what did the search do": which configurations
    it kept, what they scored, which iterations it accepted, and the text of
    each. Enough to reconstruct the run, and no live objects.
    """
    configurations: dict[str, Any] = {}
    source = getattr(report, "prompt_configurations", None) or {}
    if isinstance(source, Mapping):
        for configuration_id, snapshot in source.items():
            prompts = _attribute(snapshot, "prompts") or {}
            configurations[str(configuration_id)] = {
                "parent": _attribute(snapshot, "parent"),
                "prompts": {
                    str(module_id): {
                        "alias": _attribute(prompt, "alias"),
                        "text": _text_of(prompt),
                    }
                    for module_id, prompt in prompts.items()
                },
            }

    return {
        "optimization_id": getattr(report, "optimization_id", None),
        "best_id": getattr(report, "best_id", None),
        "accepted_iterations": [
            _iteration(iteration)
            for iteration in getattr(report, "accepted_iterations", None) or ()
        ],
        "pareto_scores": _plain(getattr(report, "pareto_scores", None)),
        "parents": _plain(getattr(report, "parents", None)),
        "prompt_configurations": configurations,
    }


def _iteration(iteration: object) -> dict[str, Any]:
    dump = getattr(iteration, "model_dump", None)
    if callable(dump):
        result = dump(mode="json")
        if isinstance(result, dict):
            return result
    return {
        "before": _attribute(iteration, "before"),
        "after": _attribute(iteration, "after"),
    }


def _plain(value: object) -> Any:
    """Mappings and sequences, as plain JSON-able values."""
    if isinstance(value, Mapping):
        return {str(key): _plain(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(item) for item in value]
    return value


def record_optimization(
    registry: Recorder,
    prompt: str,
    *,
    report: object,
    baseline: str,
    dev: Sequence[Score] = (),
    test: Sequence[Score] = (),
    primary_metric: str = "mean_score",
    candidate_text: str | None = None,
    algorithm: str | None = None,
    dataset: str | None = None,
    evaluation_id: str | None = None,
    started_at: str | None = None,
    duration_ms: float | None = None,
    iterations: int | None = None,
    attach_report: bool = True,
    promote: bool = False,
) -> OptimizationRecord:
    """Store a DeepEval optimisation and its candidate, and get the verdict.

    ``dev`` should be what the optimiser searched against and ``test`` what it
    never saw. Both are the caller's measurements: DeepEval does not know which
    of its cases were held out, and a client that guessed would be guessing
    about the only number that decides anything.

    Returns the record the server wrote, whose ``outcome`` and ``reason`` are
    the server's decision — not the optimiser's opinion of its own output.
    """
    text = candidate_text if candidate_text is not None else best_prompt_text(report)
    optimization_id = getattr(report, "optimization_id", None)
    return registry.record_optimization(
        prompt,
        algorithm=algorithm or _algorithm_of(report),
        baseline=baseline,
        candidate_text=text,
        primary_metric=primary_metric,
        dev=dev,
        test=test,
        dataset=dataset,
        evaluation_id=evaluation_id,
        # Passed through so a retried CI step writes one record rather than
        # two rows claiming the same experiment.
        optimization_id=_identifier(optimization_id),
        started_at=started_at,
        duration_ms=duration_ms,
        iterations=(
            iterations
            if iterations is not None
            else len(getattr(report, "accepted_iterations", None) or ()) or None
        ),
        report=serialize_report(report) if attach_report else None,
        promote=promote,
    )


def _algorithm_of(report: object) -> str:
    """``deepeval/SIMBA`` where the report says which search it ran."""
    for attribute in ("algorithm", "algorithm_name", "optimizer"):
        value = getattr(report, attribute, None)
        name = getattr(value, "__name__", None) or (value if isinstance(value, str) else None)
        if name:
            return f"deepeval/{name}"
    return ALGORITHM


def _identifier(value: object) -> str | None:
    """DeepEval's id, if it is one the registry will accept as an object key.

    DeepEval generates UUID-shaped ids, which are fine. Anything else is
    dropped rather than sent: the server would refuse it, and the derived id it
    falls back to is just as stable.
    """
    if not isinstance(value, str) or not value:
        return None
    if len(value) > 128:
        return None
    allowed = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")
    if set(value) - allowed or value[0] in ".-":
        return None
    return value
