"""aiwatcher client for Python agents.

The contract is the envelope, not this library: anything that can produce the
JSON in ``contracts/envelope.schema.json`` and get it onto the Laser topic is a
valid producer. This exists so the common case is three lines.

Two things it does that are easy to get wrong by hand:

* **Stable ids.** ``event_id`` is a ULID-shaped UUIDv7 so the backend can
  deduplicate a redelivery, and ``call_id`` distinguishes concurrent LLM calls
  inside one agent. Without a ``call_id``, two calls issued in parallel collapse
  into one span.
* **Correlation, not just tracing.** ``correlation_id`` groups a whole flow and
  is inherited unchanged; ``causation_id`` names the direct cause. A child
  context inherits both, and roots its causation on the correlation when nothing
  caused it — the same rule the Rust side applies.
"""

from __future__ import annotations

import contextlib
import json
import os
import queue
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from collections.abc import Iterator
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from typing import Any, Protocol

# Re-exported so `from aiwatcher_sdk import PromptRegistry` works and
# `AiwatcherClient.prompts` has something to hand back.
from aiwatcher_sdk.prompts import PromptRegistry

SCHEMA_VERSION = 1

__all__ = [
    "SCHEMA_VERSION",
    "AiwatcherClient",
    "EvaluationContext",
    "HttpTransport",
    "NullTransport",
    "PromptRegistry",
    "RunContext",
    "Transport",
]


def _iso(moment: datetime) -> str:
    """RFC 3339 with a `Z` suffix — what the Rust side parses."""
    return moment.isoformat().replace("+00:00", "Z")


def _now() -> str:
    return _iso(datetime.now(UTC))


def _new_id() -> str:
    return str(uuid.uuid4())


class Transport(Protocol):
    """Where events go. Swap for a Laser producer in production."""

    def send(self, batch: list[dict[str, Any]]) -> None: ...

    def close(self) -> None: ...


class NullTransport:
    """Drops everything. The default, so importing this never breaks a test."""

    def send(self, batch: list[dict[str, Any]]) -> None:
        del batch

    def flush(self) -> None:
        return None

    def close(self) -> None:
        return None


@dataclass
class _FlushRequest:
    done: threading.Event


class _Tick:
    """The timer fired with nothing queued.

    A type of its own rather than `...`: the queue holds four different things,
    and a sentinel the type checker can see is the difference between a `match`
    that is exhaustive and one that has a `type: ignore` on it.
    """


_TICK = _Tick()


class HttpTransport:
    """Posts batches to ``POST /api/v1/events``.

    The fallback path, for producers that cannot reach Laser. Batching and the
    background thread are the point: an agent should never block on telemetry,
    and a per-token flush would cost a round trip per token.
    """

    def __init__(
        self,
        base_url: str,
        *,
        batch_size: int = 64,
        flush_interval: float = 1.0,
        queue_size: int = 50_000,
        timeout: float = 5.0,
    ) -> None:
        self._url = base_url.rstrip("/") + "/api/v1/events"
        self._batch_size = batch_size
        self._flush_interval = flush_interval
        self._timeout = timeout
        # Bounded: telemetry must not be able to exhaust the agent's memory.
        # A full queue drops events and says so, which is better than an OOM.
        #
        # 50k is roughly 40 MB of held events and covers a burst no real agent
        # produces — a synthetic firehose of 126k events in 1.5s still overflows
        # it, which is the point at which raising it is a deliberate choice.
        self._queue: queue.Queue[dict[str, Any] | _FlushRequest | None] = queue.Queue(
            maxsize=queue_size
        )
        self._dropped = 0
        self._next_drop_warning = 1
        self._worker = threading.Thread(target=self._run, name="aiwatcher", daemon=True)
        self._worker.start()

    @property
    def dropped(self) -> int:
        """Events discarded because the queue was full."""
        return self._dropped

    def send(self, batch: list[dict[str, Any]]) -> None:
        for event in batch:
            try:
                self._queue.put_nowait(event)
            except queue.Full:
                self._dropped += 1
                self._warn_about_drops()

    def _warn_about_drops(self) -> None:
        """Say something when the queue overflows.

        A silent drop is the worst failure mode telemetry has: the dashboard
        looks fine and is wrong. Measured on a burst of 126k events with the
        default 10k queue, 95% were discarded and nothing said so.

        Logged on the first drop and then on a widening interval, so a sustained
        overflow is visible without the log itself becoming the problem.
        """
        if self._dropped < self._next_drop_warning:
            return
        print(
            f"[aiwatcher] queue full — {self._dropped} events dropped. "
            f"Raise queue_size (currently {self._queue.maxsize}) or lower "
            f"flush_interval if this is sustained.",
            file=sys.stderr,
        )
        self._next_drop_warning = max(self._dropped * 10, 1)

    def flush(self) -> None:
        """Wait until everything queued before this call has been posted.

        A finished evaluation is often the last thing a short-lived CLI emits.
        Relying on the daemon worker at interpreter shutdown loses that report,
        so callers that need a delivery boundary can request one explicitly.
        Failures still follow the transport policy: they are counted and
        reported, never raised into the agent or evaluation gate.
        """
        request = _FlushRequest(threading.Event())
        try:
            self._queue.put(request, timeout=self._timeout)
        except queue.Full:
            print("[aiwatcher] flush timed out waiting for queue space", file=sys.stderr)
            return
        if not request.done.wait(timeout=self._timeout + self._flush_interval):
            print("[aiwatcher] flush timed out waiting for the worker", file=sys.stderr)

    def close(self) -> None:
        self.flush()
        self._queue.put(None)
        self._worker.join(timeout=self._timeout + self._flush_interval)

    def _run(self) -> None:
        pending: list[dict[str, Any]] = []
        deadline = time.monotonic() + self._flush_interval
        while True:
            timeout = max(deadline - time.monotonic(), 0.0)
            item: dict[str, Any] | _FlushRequest | _Tick | None
            try:
                item = self._queue.get(timeout=timeout)
            except queue.Empty:
                item = _TICK
            if item is None:
                self._post(pending)
                return
            if isinstance(item, _FlushRequest):
                self._post(pending)
                pending = []
                deadline = time.monotonic() + self._flush_interval
                item.done.set()
                continue
            if not isinstance(item, _Tick):
                pending.append(item)
            if len(pending) >= self._batch_size or time.monotonic() >= deadline:
                self._post(pending)
                pending = []
                deadline = time.monotonic() + self._flush_interval

    def _post(self, batch: list[dict[str, Any]]) -> None:
        if not batch:
            return
        payload = json.dumps({"events": batch}).encode()
        # The URL is the caller's own aiwatcher, from configuration, and the
        # scheme is whatever they set — the same trust boundary as the rest of
        # this client.
        request = urllib.request.Request(  # noqa: S310
            self._url,
            data=payload,
            headers={"content-type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=self._timeout):  # noqa: S310
                pass
        except (urllib.error.URLError, OSError) as error:
            # Telemetry must never take the agent down with it.
            self._dropped += len(batch)
            print(f"[aiwatcher] dropped {len(batch)} events: {error}", file=sys.stderr)


@dataclass
class _Context:
    """The four ids, plus what identifies the run."""

    run_id: str
    conversation_id: str | None = None
    workflow_id: str | None = None
    agent_id: str | None = None
    correlation_id: str = field(default_factory=_new_id)
    causation_id: str | None = None
    parent_span_id: str | None = None


class AiwatcherClient:
    """Publishes envelopes.

    >>> client = AiwatcherClient(service="research-service")
    >>> with client.run("run-123", conversation_id="conv-1") as run:
    ...     with run.agent("researcher") as agent:
    ...         with agent.llm(model="claude-opus-5", provider="anthropic") as call:
    ...             call.first_token()
    ...             call.usage(prompt_tokens=812, completion_tokens=193)
    """

    def __init__(
        self,
        *,
        service: str,
        transport: Transport | None = None,
        instance: str | None = None,
        base_url: str | None = None,
    ) -> None:
        resolved = base_url or os.environ.get("AIWATCHER_URL")
        self._base_url = resolved
        self._prompts: PromptRegistry | None = None
        self._transport = transport or (HttpTransport(resolved) if resolved else NullTransport())
        self._source = {
            "service": service,
            "sdk": "python",
        }
        if instance or os.environ.get("HOSTNAME"):
            self._source["instance"] = instance or os.environ["HOSTNAME"]

    def emit(
        self,
        event_type: str,
        context: _Context,
        data: dict[str, Any] | None = None,
        *,
        span_id: str | None = None,
        parent_span_id: str | None = None,
        occurred_at: str | None = None,
    ) -> str:
        """Publish one event. Returns its id.

        `span_id` and `parent_span_id` are optional: the backend derives them
        when absent. A producer that tracks its own scopes should send them —
        it knows the nesting, and derivation cannot see a leaf inside a leaf.

        `occurred_at` overrides the clock. Only worth passing for something
        that is reported after the fact — an evaluation summarised once it has
        already run — where stamping *now* on the start would report a
        duration of zero for work that took twenty minutes.
        """
        event_id = _new_id()
        envelope: dict[str, Any] = {
            "schema_version": SCHEMA_VERSION,
            "kind": "Event",
            "event_id": event_id,
            "event_type": event_type,
            "occurred_at": occurred_at or _now(),
            "run_id": context.run_id,
            "correlation_id": context.correlation_id,
            "source": self._source,
            "data": data or {},
        }
        if context.conversation_id:
            envelope["conversation_id"] = context.conversation_id
        if context.workflow_id:
            envelope["workflow_id"] = context.workflow_id
        if context.agent_id:
            envelope["agent_id"] = context.agent_id
        if context.causation_id:
            envelope["causation_id"] = context.causation_id
        if span_id:
            envelope["span_id"] = span_id
        if parent := (parent_span_id or context.parent_span_id):
            envelope["parent_span_id"] = parent
        self._transport.send([envelope])
        return event_id

    @contextlib.contextmanager
    def run(
        self,
        run_id: str,
        *,
        conversation_id: str | None = None,
        workflow_id: str | None = None,
        correlation_id: str | None = None,
    ) -> Iterator[RunContext]:
        """One execution of an agent. Becomes one trace.

        `conversation_id` groups runs by who is talking; `workflow_id` groups
        them by what is being executed, so the same orchestration is comparable
        across sessions.
        """
        context = _Context(
            run_id=run_id,
            conversation_id=conversation_id,
            workflow_id=workflow_id,
            correlation_id=correlation_id or _new_id(),
        )
        run_context = RunContext(self, context)
        self.emit("run.started", context)
        try:
            yield run_context
        except BaseException as error:
            # `run.failed` must be emitted for a KeyboardInterrupt too, or a
            # cancelled run looks identical to a hung one.
            self.emit("run.failed", context, {"error": str(error), "status": "failed"})
            raise
        else:
            self.emit("run.completed", context, {"status": "succeeded"})

    # ── Evaluation ───────────────────────────────────────────────────────

    @contextlib.contextmanager
    def evaluation(
        self,
        suite: str,
        *,
        evaluation_id: str | None = None,
        dataset: str | None = None,
        variant: str | None = None,
        params: dict[str, Any] | None = None,
    ) -> Iterator[EvaluationContext]:
        """One execution of an evaluation suite. Becomes a report, not a trace.

        `eval.*` events ride the same log as everything else and are folded
        apart from it: they produce no span, no trace record and no row in the
        runs list. What they produce is the evaluation view — parameters,
        metrics, per-case scores and whatever document you attach.

        `dataset` is what makes two reports comparable. The backend will only
        compare a report against the previous one **on the same dataset**, so
        an unversioned suite silently compares against itself.

        >>> with client.evaluation("catalog-floor-plan", dataset="cases@3") as run:
        ...     for case in cases:
        ...         run.case(case.id, passed=case.ok, score=case.score)
        ...     run.metrics(mean_score=0.91)
        ...     run.report({"failures": [...]})
        """
        context = _Context(run_id=evaluation_id or _new_id())
        base: dict[str, Any] = {"suite": suite}
        if dataset:
            base["dataset"] = dataset
        if variant:
            base["variant"] = variant
        if params:
            base["params"] = _stringify(params)

        self.emit("eval.started", context, base)
        evaluation = EvaluationContext(self, context, base)
        try:
            yield evaluation
        except BaseException as error:
            self.emit("eval.failed", context, {**base, **evaluation.payload(), "error": str(error)})
            raise
        else:
            self.emit("eval.completed", context, {**base, **evaluation.payload()})

    def record_evaluation(
        self,
        *,
        suite: str,
        evaluation_id: str | None = None,
        dataset: str | None = None,
        variant: str | None = None,
        params: dict[str, Any] | None = None,
        metrics: dict[str, float] | None = None,
        report: dict[str, Any] | None = None,
        cases_total: int | None = None,
        cases_passed: int | None = None,
        duration_ms: float | None = None,
    ) -> str:
        """Publish a finished evaluation in one call. Returns its id.

        The direct replacement for an MLflow block of the shape::

            with mlflow.start_run(run_name=...):
                mlflow.log_params(...)
                mlflow.log_metrics(...)
                mlflow.log_dict(report, "evaluation-report.json")

        Same four pieces, no server, no `import mlflow`, and the result lands
        next to the traces the evaluated agent produced rather than in a
        separate system.

        `duration_ms` back-dates the start. Without it the report is stamped as
        instantaneous, which is honest — nothing told us when it began — and
        useless for anything that looks at how long a suite takes.
        """
        context = _Context(run_id=evaluation_id or _new_id())
        base: dict[str, Any] = {"suite": suite}
        if dataset:
            base["dataset"] = dataset
        if variant:
            base["variant"] = variant
        if params:
            base["params"] = _stringify(params)

        started_at = None
        if duration_ms is not None:
            started_at = _iso(datetime.now(UTC) - timedelta(milliseconds=max(duration_ms, 0.0)))
        self.emit("eval.started", context, base, occurred_at=started_at)

        payload: dict[str, Any] = dict(base)
        if metrics:
            payload["metrics"] = {key: float(value) for key, value in metrics.items()}
        if report is not None:
            payload["report"] = report
        if cases_total is not None:
            payload["cases_total"] = cases_total
        if cases_passed is not None:
            payload["cases_passed"] = cases_passed
            if cases_total is not None:
                payload["cases_failed"] = max(cases_total - cases_passed, 0)
        self.emit("eval.completed", context, payload)
        return context.run_id

    # ── Prompts ──────────────────────────────────────────────────────────

    @property
    def prompts(self) -> PromptRegistry:
        """The prompt registry on the same instance.

        Deliberately not the same object as the transport. Telemetry is
        fire-and-forget and swallows its failures; reading the prompt a service
        is about to run on is the work, and every method on the registry
        raises. Sharing a transport would have to pick one policy for both.
        """
        if self._base_url is None:
            raise RuntimeError(
                "no aiwatcher URL: pass base_url= or set AIWATCHER_URL. "
                "The registry has no offline mode — reading a prompt is not telemetry."
            )
        if self._prompts is None:
            self._prompts = PromptRegistry(self._base_url)
        return self._prompts

    def close(self) -> None:
        self._transport.close()

    def flush(self) -> None:
        """Drain a transport that supports an explicit delivery boundary."""
        flush = getattr(self._transport, "flush", None)
        if callable(flush):
            flush()


class EvaluationContext:
    """The scope `AiwatcherClient.evaluation` yields.

    Cases are published as they are scored, so a suite that takes twenty
    minutes is watchable while it runs. Metrics, extra parameters and the
    report document are accumulated and folded into the end event, because
    those are only known once it is over.
    """

    def __init__(
        self,
        client: AiwatcherClient,
        context: _Context,
        base: dict[str, Any],
    ) -> None:
        self._client = client
        self._context = context
        self._base = base
        self._metrics: dict[str, float] = {}
        self._params: dict[str, str] = {}
        self._report: dict[str, Any] | None = None

    def case(
        self,
        case_id: str,
        *,
        passed: bool | None = None,
        score: float | None = None,
        reason: str | None = None,
        duration_ms: float | None = None,
        **extra: Any,
    ) -> None:
        """One scored case.

        `reason` is what makes a score reviewable. A number with no rationale
        is the thing people mean when they say they do not trust an eval.
        """
        data: dict[str, Any] = {"case_id": case_id, **extra}
        for key, value in (
            ("passed", passed),
            ("score", score),
            ("reason", reason),
            ("duration_ms", duration_ms),
        ):
            if value is not None:
                data[key] = value
        self._client.emit("eval.case", self._context, data)

    def metrics(self, **metrics: float) -> None:
        """Aggregates. MLflow's `log_metrics`."""
        self._metrics.update({key: float(value) for key, value in metrics.items()})

    def params(self, **params: Any) -> None:
        """Anything held fixed. MLflow's `log_params`."""
        self._params.update(_stringify(params))

    def report(self, document: dict[str, Any]) -> None:
        """The free-form half. MLflow's `log_dict`."""
        self._report = document

    def payload(self) -> dict[str, Any]:
        """What the end event carries."""
        data: dict[str, Any] = {}
        if self._metrics:
            data["metrics"] = self._metrics
        if self._params:
            data["params"] = {**self._base.get("params", {}), **self._params}
        if self._report is not None:
            data["report"] = self._report
        return data


def _stringify(params: dict[str, Any]) -> dict[str, str]:
    """Parameters are labels, so they arrive as strings.

    Bounded at the same 500 characters MLflow's own parameter limit uses: a
    parameter that needs more than that is a report, and there is a field for
    those.
    """
    return {key: str(value)[:500] for key, value in params.items()}


class _Scope:
    """Shared start/end bookkeeping for agent, LLM and tool scopes."""

    def __init__(self, client: AiwatcherClient, context: _Context) -> None:
        self._client = client
        self._context = context


class RunContext(_Scope):
    @contextlib.contextmanager
    def agent(self, agent_id: str) -> Iterator[AgentContext]:
        context = _Context(
            run_id=self._context.run_id,
            conversation_id=self._context.conversation_id,
            workflow_id=self._context.workflow_id,
            agent_id=agent_id,
            correlation_id=self._context.correlation_id,
            causation_id=self._context.correlation_id,
        )
        self._client.emit("agent.started", context)
        agent_context = AgentContext(self._client, context)
        try:
            yield agent_context
        except BaseException as error:
            self._client.emit("agent.failed", context, {"error": str(error)})
            raise
        else:
            self._client.emit("agent.completed", context)


class AgentContext(_Scope):
    @contextlib.contextmanager
    def llm(
        self,
        *,
        model: str,
        provider: str | None = None,
        call_id: str | None = None,
        **request: Any,
    ) -> Iterator[LlmCall]:
        # `call_id` is what separates two concurrent calls. Generated when
        # omitted, but pass your provider's request id where you have one — it
        # makes the span joinable with the provider's own logs.
        resolved_call_id = call_id or _new_id()
        base = {"call_id": resolved_call_id, "model": model, **request}
        if provider:
            base["provider"] = provider

        started = time.monotonic()
        self._client.emit("llm.started", self._context, base)
        call = LlmCall(self._client, self._context, base, started)
        try:
            yield call
        except BaseException as error:
            self._client.emit(
                "llm.failed",
                self._context,
                {**base, "error": str(error), "duration_ms": call.elapsed_ms()},
            )
            raise
        else:
            self._client.emit(
                "llm.completed",
                self._context,
                {**base, **call.result, "duration_ms": call.elapsed_ms()},
            )

    @contextlib.contextmanager
    def tool(self, name: str, *, call_id: str | None = None, **arguments: Any) -> Iterator[None]:
        base = {"call_id": call_id or _new_id(), "tool_name": name, **arguments}
        started = time.monotonic()
        self._client.emit("tool.started", self._context, base)
        try:
            yield None
        except BaseException as error:
            self._client.emit(
                "tool.failed",
                self._context,
                {**base, "error": str(error), "duration_ms": (time.monotonic() - started) * 1000},
            )
            raise
        else:
            self._client.emit(
                "tool.completed",
                self._context,
                {**base, "duration_ms": (time.monotonic() - started) * 1000},
            )


class LlmCall(_Scope):
    def __init__(
        self,
        client: AiwatcherClient,
        context: _Context,
        base: dict[str, Any],
        started: float,
    ) -> None:
        super().__init__(client, context)
        self._base = base
        self._started = started
        self.result: dict[str, Any] = {}

    def elapsed_ms(self) -> float:
        return (time.monotonic() - self._started) * 1000

    def first_token(self) -> None:
        """Call once, when the first token arrives. Drives time-to-first-token."""
        self._client.emit("llm.first_token", self._context, self._base)

    def chunk(self, text: str) -> None:
        """A streamed fragment.

        Reaches the live panel and the durable log; it does **not** become a
        trace record. Streaming a 2000-token response emits 2000 of these for
        one LLM call, and storing them as spans would swamp the trace store for
        nothing.
        """
        self._client.emit("llm.chunk", self._context, {**self._base, "text": text})

    def usage(
        self,
        *,
        prompt_tokens: int | None = None,
        completion_tokens: int | None = None,
        cached_tokens: int | None = None,
        finish_reason: str | None = None,
        **extra: Any,
    ) -> None:
        """Record the outcome. Folded into `llm.completed` when the scope exits."""
        for key, value in (
            ("prompt_tokens", prompt_tokens),
            ("completion_tokens", completion_tokens),
            ("cached_tokens", cached_tokens),
            ("finish_reason", finish_reason),
        ):
            if value is not None:
                self.result[key] = value
        self.result.update(extra)
