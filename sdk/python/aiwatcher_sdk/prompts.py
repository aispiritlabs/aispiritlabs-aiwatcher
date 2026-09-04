"""The prompt registry, from Python.

Everything else this SDK does is fire-and-forget: telemetry must never take an
agent down, so the transport swallows its failures and says so on stderr.

**This client is the opposite, and deliberately so.** Reading the prompt a
service is about to run on is not telemetry — it is the work. A registry that
quietly returned ``None`` when the store was unreachable would let a service
start with an empty system prompt, which is worse than not starting. So every
method here raises.

    from aiwatcher_sdk.prompts import PromptRegistry

    registry = PromptRegistry("http://aiwatcher:8080")
    prompt = registry.resolve("planner.floor-plan")
    system = prompt.render(page=page_json, language="pl")

Two things the registry does that are easy to get wrong by hand:

* **A version is its text.** ``version_id`` is ``sha256(text)``, computed the
  same way on both sides, so publishing the same prompt twice lands on one
  version. :func:`version_id_of` is the local computation, for a caller that
  wants to know the id before it talks to anything.
* **The verdict is the server's.** :meth:`PromptRegistry.record_optimization`
  reports what was measured and the server decides whether it counts, from the
  held-out split and from what the candidate did to the baseline's variables.
  An optimiser picked its candidate by maximising the number it is reporting.
"""

from __future__ import annotations

import hashlib
import re
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from types import TracebackType
from typing import Any, Literal, Self
from urllib.parse import quote

import httpx

from aiwatcher_sdk.api import ApiError, Transport

__all__ = [
    "OptimizationRecord",
    "PromptRegistry",
    "PromptVersion",
    "RegistryError",
    "Score",
    "scores",
    "variables_of",
    "version_id_of",
]

#: The label a deployment reads to answer "which version is live".
PRODUCTION = "production"

_DISABLED = "this aiwatcher instance was started without a prompt store; set AIWATCHER_PROMPT_STORE"

# The lookbehind is load-bearing and matches the server's scanner: without it
# `{{{ raw }}}` — a literal brace in a Jinja-style template — reads as a
# variable called `raw`, and the two sides would then disagree about what a
# prompt needs. `variables_of` deciding differently here than in Rust is how a
# candidate gets refused for losing a variable nobody thought it had.
_PLACEHOLDER = re.compile(r"(?<!\{)\{\{\s*([a-zA-Z][a-zA-Z0-9_]*)\s*\}\}")


def version_id_of(text: str) -> str:
    """``sha256(text)``, which is the version id the registry will use.

    Worth having locally: a caller can tell whether the prompt it is holding is
    the one that is live without publishing anything, and a producer that
    already hashes its prompts — as ``planner`` does — arrives at the same id.
    """
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def variables_of(text: str) -> list[str]:
    """The ``{{ placeholders }}`` a prompt interpolates, sorted.

    The same syntax the server reads, so the two agree about what a prompt
    needs. A candidate that no longer interpolates one of these cannot be
    promoted, whatever it scored.
    """
    return sorted(set(_PLACEHOLDER.findall(text)))


class RegistryError(ApiError):
    """The registry refused, or could not be reached.

    ``code`` is the machine-readable discriminator the API returns; switch on
    it rather than on the message. ``registry_disabled`` means the instance was
    started without a prompt store, which is a deployment problem rather than a
    missing prompt — and :attr:`~aiwatcher_sdk.api.ApiError.is_retryable` is
    ``False`` for it, because retrying a deployment decision forever is what a
    pipeline does instead of telling somebody to set a variable.
    """


@dataclass(frozen=True, slots=True)
class PromptVersion:
    """One immutable version, with its text."""

    name: str
    version_id: str
    text: str
    created_at: str
    variables: tuple[str, ...] = ()
    author: str | None = None
    notes: str | None = None
    model: str | None = None
    parent: str | None = None
    #: ``authored`` or ``optimized``.
    origin: str = "authored"
    #: What produced it, when an optimiser did.
    algorithm: str | None = None
    metadata: Mapping[str, str] = field(default_factory=dict)

    @classmethod
    def from_json(cls, body: Mapping[str, Any]) -> PromptVersion:
        return cls(
            name=str(body["name"]),
            version_id=str(body["version_id"]),
            text=str(body["text"]),
            created_at=str(body["created_at"]),
            variables=tuple(body.get("variables") or ()),
            author=body.get("author"),
            notes=body.get("notes"),
            model=body.get("model"),
            parent=body.get("parent"),
            origin=str(body.get("origin", "authored")),
            algorithm=body.get("algorithm"),
            metadata=dict(body.get("metadata") or {}),
        )

    def render(self, **values: object) -> str:
        """Substitute every declared variable, refusing a partial render.

        Strict on both sides: a missing value would ship a prompt with a raw
        ``{{ page }}`` in it, and an extra one is almost always a rename that
        did not reach the caller. Both are silent at runtime and obvious here.
        """
        supplied = set(values)
        declared = set(self.variables)
        missing = declared - supplied
        unexpected = supplied - declared
        if missing:
            raise KeyError(f"{self.name}: no value for {', '.join(sorted(missing))}")
        if unexpected:
            raise KeyError(f"{self.name}: does not use {', '.join(sorted(unexpected))}")
        return _PLACEHOLDER.sub(lambda match: str(values[match.group(1)]), self.text)


@dataclass(frozen=True, slots=True)
class Score:
    """One metric, on the baseline and on the candidate."""

    metric: str
    baseline: float | None = None
    candidate: float | None = None

    @property
    def delta(self) -> float | None:
        """``candidate - baseline``, and ``None`` when either side is missing.

        A metric only one side reported is not a delta of zero.
        """
        if self.baseline is None or self.candidate is None:
            return None
        return self.candidate - self.baseline

    def as_json(self) -> dict[str, Any]:
        body: dict[str, Any] = {"metric": self.metric}
        if self.baseline is not None:
            body["baseline"] = float(self.baseline)
        if self.candidate is not None:
            body["candidate"] = float(self.candidate)
        return body

    @classmethod
    def from_json(cls, body: Mapping[str, Any]) -> Score:
        return cls(
            metric=str(body["metric"]),
            baseline=body.get("baseline"),
            candidate=body.get("candidate"),
        )


def scores(baseline: Mapping[str, float], candidate: Mapping[str, float]) -> list[Score]:
    """Pair two metric dictionaries into :class:`Score` rows.

    Every metric either side reported gets a row, so one that appeared or
    disappeared between the two is visible rather than silently absent.
    """
    return [
        Score(metric=metric, baseline=baseline.get(metric), candidate=candidate.get(metric))
        for metric in sorted({*baseline, *candidate})
    ]


@dataclass(frozen=True, slots=True)
class OptimizationRecord:
    """What the registry decided about one optimisation."""

    optimization_id: str
    prompt: str
    algorithm: str
    baseline: str
    candidate: str
    primary_metric: str
    #: ``admitted`` or ``rejected``.
    outcome: Literal["admitted", "rejected"]
    #: Why it was rejected. ``None`` on an admission.
    reason: str | None = None
    dev: tuple[Score, ...] = ()
    test: tuple[Score, ...] = ()
    variables_lost: tuple[str, ...] = ()
    dataset: str | None = None
    evaluation_id: str | None = None

    @property
    def admitted(self) -> bool:
        return self.outcome == "admitted"

    @property
    def overfit_gap(self) -> float | None:
        """How far the dev gain outran the held-out gain on the deciding metric.

        The number worth watching across a series: a run that gains 0.2 on dev
        and 0.0 on the held-out split found something about the dev cases, not
        about the task.
        """
        dev = _find(self.dev, self.primary_metric)
        test = _find(self.test, self.primary_metric)
        if dev is None or test is None or dev.delta is None or test.delta is None:
            return None
        return dev.delta - test.delta

    @classmethod
    def from_json(cls, body: Mapping[str, Any]) -> OptimizationRecord:
        raw = str(body["outcome"])
        # Narrowed rather than cast: an outcome this client does not know about
        # is a server it does not understand, and guessing "rejected" would
        # silently fail a build that should have passed.
        if raw == "admitted":
            outcome: Literal["admitted", "rejected"] = "admitted"
        elif raw == "rejected":
            outcome = "rejected"
        else:  # pragma: no cover - server contract
            raise RegistryError(f"unknown optimisation outcome {raw!r}")
        return cls(
            optimization_id=str(body["optimization_id"]),
            prompt=str(body["prompt"]),
            algorithm=str(body["algorithm"]),
            baseline=str(body["baseline"]),
            candidate=str(body["candidate"]),
            primary_metric=str(body["primary_metric"]),
            outcome=outcome,
            reason=body.get("reason"),
            dev=tuple(Score.from_json(score) for score in body.get("dev") or ()),
            test=tuple(Score.from_json(score) for score in body.get("test") or ()),
            variables_lost=tuple(body.get("variables_lost") or ()),
            dataset=body.get("dataset"),
            evaluation_id=body.get("evaluation_id"),
        )


def _find(source: Iterable[Score], metric: str) -> Score | None:
    return next((score for score in source if score.metric == metric), None)


class PromptRegistry:
    """A client for ``/api/v1/prompts``.

    Synchronous and blocking. Reading a prompt happens once at start-up or once
    per optimisation run, not per request, so the complexity of an async client
    would buy nothing — and a service that wants one can call this from a
    thread.
    """

    def __init__(
        self,
        base_url: str,
        *,
        token: str | None = None,
        timeout: float = 10.0,
        attempts: int = 3,
        client: httpx.Client | None = None,
    ) -> None:
        self._http = Transport(
            base_url,
            token=token,
            timeout=timeout,
            attempts=attempts,
            error=RegistryError,
            subject="the prompt registry",
            hints={"registry_disabled": _DISABLED},
            client=client,
        )

    @property
    def base_url(self) -> str:
        return self._http.base_url

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    # ── Reads ────────────────────────────────────────────────────────────

    def resolve(self, name: str, *, label: str | None = None) -> PromptVersion:
        """The version a label points at, with its text.

        With no label this is what ``production`` resolves to, falling back to
        the newest version when nothing has been promoted — so a registry is
        readable from the first publish rather than after a ceremony.
        """
        detail = self.get_prompt(name)
        if label is None:
            current = detail.get("current")
            if not current:
                raise RegistryError(f"prompt {name} has no versions", status=404, code="not_found")
            return PromptVersion.from_json(current)
        version_id = (detail.get("head", {}).get("labels") or {}).get(label)
        if not version_id:
            raise RegistryError(
                f"prompt {name} has no {label!r} label", status=404, code="not_found"
            )
        return self.get_version(name, version_id)

    def get_prompt(self, name: str) -> dict[str, Any]:
        """One prompt's head, its version index and its recent optimisations."""
        return self._request("GET", f"/api/v1/prompts/{_segment(name)}")

    def get_version(self, name: str, version_id: str) -> PromptVersion:
        """One version by its content address, with its text."""
        return PromptVersion.from_json(
            self._request(
                "GET", f"/api/v1/prompts/{_segment(name)}/versions/{_segment(version_id)}"
            )
        )

    def get_prompts(
        self,
        *,
        search: str | None = None,
        tag: str | None = None,
        limit: int | None = None,
    ) -> dict[str, Any]:
        """The prompt index, filtered. A page of heads, not their text."""
        return self._request(
            "GET", "/api/v1/prompts", params={"search": search, "tag": tag, "limit": limit}
        )

    def get_optimization(self, name: str, optimization_id: str) -> OptimizationRecord:
        """One optimisation record, with the verdict the server reached."""
        return OptimizationRecord.from_json(
            self._request(
                "GET",
                f"/api/v1/prompts/{_segment(name)}/optimizations/{_segment(optimization_id)}",
            )
        )

    # ── Writes ───────────────────────────────────────────────────────────

    def publish(
        self,
        name: str,
        text: str,
        *,
        author: str | None = None,
        notes: str | None = None,
        model: str | None = None,
        parent: str | None = None,
        description: str | None = None,
        tags: Sequence[str] | None = None,
        label: str | None = None,
        metadata: Mapping[str, Any] | None = None,
    ) -> PromptVersion:
        """Store a version. Idempotent on the text.

        Republishing the same prompt returns the version already stored, with
        its original author and notes — so a deploy job that publishes on every
        start does not rewrite who wrote it.

        ``label`` is the shorthand for publishing and deploying in one request.
        Leave it out to store a draft: everything can read it and nothing is
        using it.
        """
        body: dict[str, Any] = {"name": name, "text": text}
        for key, value in (
            ("author", author),
            ("notes", notes),
            ("model", model),
            ("parent", parent),
            ("description", description),
            ("label", label),
        ):
            if value is not None:
                body[key] = value
        if tags is not None:
            body["tags"] = list(tags)
        if metadata:
            body["metadata"] = {key: str(value) for key, value in metadata.items()}
        return PromptVersion.from_json(self._request("POST", "/api/v1/prompts", body)["version"])

    def set_label(self, name: str, label: str, version_id: str) -> dict[str, Any]:
        """Point a label at a version. The deploy step."""
        return self._request(
            "PUT",
            f"/api/v1/prompts/{_segment(name)}/labels/{_segment(label)}",
            {"version_id": version_id},
        )

    def record_optimization(
        self,
        name: str,
        *,
        algorithm: str,
        baseline: str,
        candidate_text: str,
        primary_metric: str,
        dev: Sequence[Score] = (),
        test: Sequence[Score] = (),
        dataset: str | None = None,
        evaluation_id: str | None = None,
        optimization_id: str | None = None,
        started_at: str | None = None,
        duration_ms: float | None = None,
        iterations: int | None = None,
        report: Mapping[str, Any] | None = None,
        promote: bool = False,
    ) -> OptimizationRecord:
        """Record what an optimiser did, and store its candidate as a version.

        ``dev`` guides a search and proves nothing; ``test`` is the held-out
        split and is the only evidence that admits a candidate. Sending only
        ``dev`` is not an error — it is recorded, and it is refused a
        promotion, which is the outcome the split exists to produce.

        ``promote`` moves ``production`` **if** the server admits the
        candidate. It never overrides the verdict.
        """
        body: dict[str, Any] = {
            "algorithm": algorithm,
            "baseline": baseline,
            "candidate_text": candidate_text,
            "primary_metric": primary_metric,
            "dev": [score.as_json() for score in dev],
            "test": [score.as_json() for score in test],
            "promote": promote,
        }
        for key, value in (
            ("dataset", dataset),
            ("evaluation_id", evaluation_id),
            ("optimization_id", optimization_id),
            ("started_at", started_at),
            ("iterations", iterations),
        ):
            if value is not None:
                body[key] = value
        if duration_ms is not None:
            body["duration_ms"] = int(duration_ms)
        if report is not None:
            body["report"] = dict(report)
        return OptimizationRecord.from_json(
            self._request("POST", f"/api/v1/prompts/{_segment(name)}/optimizations", body)
        )

    def rebuild(self, name: str) -> dict[str, Any]:
        """Re-derive the prompt's index from the objects that are stored."""
        return self._request("POST", f"/api/v1/prompts/{_segment(name)}/rebuild", {})

    # ── Transport ────────────────────────────────────────────────────────

    def _request(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None = None,
        *,
        params: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        # Every write in this registry is idempotent by construction — a
        # version id is `sha256(text)`, a label is set to a value, and an
        # optimisation carries its own id — so a repeat after a lost answer
        # lands on what is already stored rather than beside it.
        return self._http.json(method, path, body, params=params, idempotent=True)


def _segment(value: str) -> str:
    """A path segment, encoded.

    The server validates these too, and would refuse anything with a separator
    in it. Encoding here means a caller that passes a bad name gets a 400 that
    names the field rather than a 404 on a URL it did not intend to build.

    ``httpx`` encodes a *query* — that is what ``params=`` is for — and leaves
    a path alone, correctly, because a client is the only side that knows
    whether a slash in a name is a separator or part of it. Here it is part of
    it: ``planner.floor-plan`` and every namespaced name like it.
    """
    return quote(value, safe="")
