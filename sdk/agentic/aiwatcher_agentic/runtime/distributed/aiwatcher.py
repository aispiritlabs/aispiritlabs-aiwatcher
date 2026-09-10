"""Distributed agents on aiwatcher: a hop is a run, and a reply is a row.

Apache Iggy did four things for a message here: carried it to a named agent, let
a group of that agent's workers take each one once, took back what a dead worker
never acknowledged, and parked what could not be handled. aiwatcher already does
all four for a managed step — a claim on a queue, a lease, a retry budget and a
failed step — so this module implements none of them. It maps.

| Iggy                              | aiwatcher                                           |
|-----------------------------------|-----------------------------------------------------|
| topic `messages.<agent>`          | the agent's one-step workflow, `<prefix>.<agent>`   |
| a producer send                   | `POST /executions`, `Idempotency-Key` = message id  |
| a consumer group poll             | a worker claim on the queue `<prefix>.<agent>`      |
| an ack                            | the claim's result                                  |
| `XAUTOCLAIM` of an idle entry     | lease expiry                                        |
| the dead-letter topic             | a failed step                                       |
| the `control` and `health` topics | the definitions aiwatcher holds                     |
| `messages.chat`, tailed           | a mailbox: one hosted execution per reply address   |

## Why a hop is a run, and not a row in one shared stream

A hosted execution's history is shared, and nothing in it can be claimed: the
engine never schedules an attempt for a hosted run, because its worker is the
one deciding what runs next. Only a compiled step reaches the claim table. An
agent is already a one-step workflow (AW-2, Phase E), so handing it a message is
starting that workflow with the message as its parameter — and because the
message id is the `Idempotency-Key`, a message sent twice is one run.

## The words

The message goes to a payload store; the run is handed a reference, a digest and
a size — §40.4's `external` policy, the join's rule. The store is a directory
under `AIWATCHER_PAYLOAD_ROOT` unless one is given, and every process that reads
a hop has to be able to open it: workers on one machine share it as it is,
workers on several need a shared volume.

## A reply

A client is not an agent. Nothing claims what is addressed to it, and every
client tails the same address and keeps what belongs to its own turn — the shape
the Iggy topic had. So an address that starts with ``mailbox:`` is not a workflow
but a hosted execution, one per address, started under a fixed key so that
every process finds the same one, and a reply is appended to it.

## At least once

A retried attempt runs its handler again. What stops that handing off twice is
the id each answer gets: derived from the message it answers and its position,
so the retry starts the same runs under the same keys, and aiwatcher answers
with the ones that already exist.
"""

from __future__ import annotations

import json
import os
import re
import signal
import socket
import threading
import time
import uuid
from collections.abc import Callable, Mapping, Sequence
from datetime import datetime
from typing import TYPE_CHECKING, Any, Final

from structlog import get_logger

from aiwatcher_agentic.runtime.distributed.registry import AgentSnapshot
from aiwatcher_agentic.runtime.distributed.serialization import deserialize_record, serialize_record
from aiwatcher_agentic.runtime.distributed.service import (
    DeliveryMetrics,
    PermanentMessageError,
    failure_replies,
    normalize_outputs,
)
from aiwatcher_agentic.runtime.distributed.transport import (
    BEGINNING,
    ConsumedRecord,
    MalformedRecord,
    normalize_distributed_message,
)
from aiwatcher_agentic.workflow.messages import Event, Message, RecordedMessageMetadata

if TYPE_CHECKING:
    from types import FrameType

    import httpx
    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.integrations.agentic import PayloadStore
    from aiwatcher_sdk.task_errors import TaskError
    from aiwatcher_sdk.workflow import Workflow

    from aiwatcher_agentic.runtime.distributed.discovery import AgenticServiceDiscovery

logger = get_logger(__name__)

__all__ = [
    "HOP",
    "MAILBOX",
    "AiwatcherService",
    "AiwatcherServiceRegistry",
    "AiwatcherTransport",
    "UnknownAgentError",
]

#: What an address starts with when it is a client's mailbox rather than an agent.
MAILBOX: Final = "mailbox:"

#: The one step of every agent's workflow — one name for all of them, so every
#: hop reads the same in the Workflows view.
HOP: Final = "hop"

#: The release label on every definition this module registers. The revision
#: aiwatcher pins is the content, so this changes only with the shape.
VERSION: Final = "1"

#: The definition every mailbox is a run of. No agent may take the name.
MAILBOX_DEFINITION: Final = "mailbox"

#: How many times an append re-reads the version when another writer got in
#: first. Replies to one mailbox arrive from several workers at once.
CONFLICT_RETRIES: Final = 16

#: How many history rows one read asks for. The route caps at 500.
PAGE: Final = 200

#: What an agent or a mailbox may be called: what a workflow name and a queue
#: both accept, with nothing to escape in a URL.
_NAME: Final = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")

#: Fixed, so a derived message id is the same in every process and release.
_NAMESPACE: Final = uuid.UUID("5d3c8e0a-3f1e-4c55-9d0e-6a3b2f1c7e44")


class UnknownAgentError(LookupError):
    """A hop to an agent aiwatcher has no workflow for.

    A refusal rather than a wait. A registered agent with no worker is a queue
    the hop waits in; an unregistered one has no queue at all, and a message
    nobody can ever claim is the silent loss a typo in a topic name used to be.
    """

    def __init__(self, agent: str, workflow: str) -> None:
        super().__init__(
            f"no agent `{agent}` is registered on aiwatcher (there is no workflow "
            f"`{workflow}`): start its service, which registers it"
        )
        self.agent = agent
        self.workflow = workflow


class AiwatcherTransport:
    """Hops as runs, replies as mailbox rows. Synchronous, one connection pool."""

    def __init__(
        self,
        url: str,
        *,
        token: str | None = None,
        prefix: str = "agentic",
        payloads: PayloadStore | None = None,
        client: httpx.Client | None = None,
        poll_seconds: float = 0.2,
    ) -> None:
        try:
            from aiwatcher_sdk.api import Transport
            from aiwatcher_sdk.integrations.agentic import FilePayloadStore
        except ImportError as error:  # pragma: no cover - the extra is what installs it
            raise RuntimeError(
                "the aiwatcher transport needs the aiwatcher SDK: install "
                "`agentic-runtime[aiwatcher]`"
            ) from error
        from aiwatcher_agentic.runtime.hosted import payload_root

        address = url.strip().rstrip("/")
        if not address:
            raise ValueError("the aiwatcher transport needs aiwatcher's URL")
        self.url = address
        self.token = token
        self._prefix = _name(prefix, "the prefix")
        self._api = Transport(address, token=token, subject="aiwatcher", client=client)
        self._payloads = (
            payloads if payloads is not None else FilePayloadStore(payload_root() / "hops")
        )
        self._poll_seconds = poll_seconds
        #: Who appends to a mailbox. The append route checks it against the
        #: decider lease, which nothing takes for a mailbox.
        self._holder = f"{socket.gethostname()}-{os.getpid()}"
        self._mailboxes: dict[str, str] = {}
        self._lock = threading.Lock()

    @classmethod
    def from_env(cls, *, prefix: str = "agentic") -> AiwatcherTransport:
        """The aiwatcher at `AIWATCHER_URL`, as `AIWATCHER_TOKEN`."""
        url = os.environ.get("AIWATCHER_URL", "").strip()
        if not url:
            raise RuntimeError(
                "distributed agents run on aiwatcher: set AIWATCHER_URL to its address"
            )
        return cls(url, token=os.environ.get("AIWATCHER_TOKEN") or None, prefix=prefix)

    # ── Names ────────────────────────────────────────────────────────────

    @property
    def prefix(self) -> str:
        return self._prefix

    def workflow_name(self, agent: str) -> str:
        """The workflow a hop to *agent* is a run of. Also its queue."""
        resolved = _name(agent, "an agent's name")
        if resolved == MAILBOX_DEFINITION:
            raise ValueError(f"`{MAILBOX_DEFINITION}` is reserved for replies to clients")
        return f"{self._prefix}.{resolved}"

    def queue_for(self, agent: str) -> str:
        """The queue *agent*'s workers claim — §40.6: the target agent."""
        return self.workflow_name(agent)

    def reply_address(self, name: str) -> str:
        """A client called *name*'s mailbox. Idempotent, so it may be applied twice."""
        if name.startswith(MAILBOX):
            return MAILBOX + _name(name.removeprefix(MAILBOX), "a mailbox's name")
        return MAILBOX + _name(name, "a mailbox's name")

    def message_stream(self, target: str) -> str:
        return target

    # ── Sending ──────────────────────────────────────────────────────────

    def publish_message(self, message: Message) -> str:
        """Start *message*'s hop, or append it to a mailbox. Returns where it went.

        For a hop that is the run's execution id, and it is the same id every
        time the same message is sent. For a mailbox it is the stream version
        the reply landed at.
        """
        normalized = normalize_distributed_message(message)
        target = normalized.metadata.target
        if not target:
            raise ValueError("Distributed messages must have a target")
        if target.startswith(MAILBOX):
            return self._append(self.reply_address(target), normalized)
        return self._start(target, normalized)

    def _start(self, target: str, message: Message) -> str:
        from aiwatcher_sdk.api import ApiError

        workflow = self.workflow_name(target)
        message_id = _message_id(message)
        hop = {
            "message_id": message_id,
            "type": message.type,
            "from": message.metadata.source or "",
            "to": target,
            **self._stored(message),
        }
        try:
            body = self._api.json(
                "POST",
                "/api/v1/executions",
                {"target": {"kind": "workflow", "name": workflow}, "parameters": {"hop": hop}},
                idempotent=True,
                idempotency_key=message_id,
            )
        except ApiError as error:
            if error.status == 404:
                raise UnknownAgentError(target, workflow) from error
            raise
        return str(body["execution"]["execution_id"])

    def _append(self, address: str, message: Message) -> str:
        from aiwatcher_sdk.api import ApiError

        execution = self.mailbox(address)
        stored = self._stored(message)
        wire = {
            "message_type": message.type,
            "metadata": _plain(message),
            "payload": {**stored, "policy": "external"},
        }
        for _ in range(CONFLICT_RETRIES):
            try:
                answer = self._api.json(
                    "POST",
                    f"/api/v1/executions/{execution}/stream",
                    {
                        "holder": self._holder,
                        "messages": [wire],
                        "expected_version": self._version(execution),
                    },
                    idempotent=True,
                    idempotency_key=_message_id(message),
                )
            except ApiError as error:
                if error.status == 409 and error.code == "version_conflict":
                    continue
                raise
            return str(int(answer["version"]))
        raise RuntimeError(
            f"{address} kept moving: {CONFLICT_RETRIES} appends in a row lost to another writer"
        )

    def _stored(self, message: Message) -> dict[str, Any]:
        """The message in the payload store, and what a run or a row says about it."""
        from aiwatcher_sdk.integrations.agentic import digest_of, encode_payload

        data = json.loads(serialize_record(message))
        digest = digest_of(data)
        return {
            "reference": self._payloads.store_payload(digest, data),
            "digest": digest,
            "size": len(encode_payload(data)),
        }

    # ── Reading a mailbox ────────────────────────────────────────────────

    def last_message_id(self, target: str) -> str:
        return str(self._version(self.mailbox(self.reply_address(target))))

    def read_messages(
        self,
        target: str,
        *,
        after_id: str = BEGINNING,
        block_ms: int = 1_000,
        count: int = 10,
    ) -> list[ConsumedRecord]:
        address = self.reply_address(target)
        execution = self.mailbox(address)
        after = _position(after_id)
        deadline = time.monotonic() + block_ms / 1000.0
        while True:
            records = self._read(execution, address, after, count)
            remaining = deadline - time.monotonic()
            if records or remaining <= 0:
                return records
            time.sleep(min(self._poll_seconds, remaining))

    def _read(self, execution: str, address: str, after: int, count: int) -> list[ConsumedRecord]:
        records: list[ConsumedRecord] = []
        while True:
            page = self._page(execution, after, PAGE)
            for row in page.get("messages", ()):
                wire = row.get("message", {})
                # The engine's own rows — the run's start and the marker each
                # append leaves — are not anybody's reply.
                if wire.get("kind") != "hosted" or wire.get("message_type") == "HostedAppend":
                    continue
                records.append(
                    ConsumedRecord(
                        stream=address,
                        entry_id=str(int(row.get("stream_version", 0))),
                        record=self.record_from(wire.get("payload")),
                    )
                )
                if len(records) >= count:
                    return records
            following = page.get("next_after")
            if records or following is None:
                return records
            after = int(following)

    def record_from(self, payload: object) -> object:
        """The message a reference names, checked against its digest.

        Anything that cannot be read back is a :class:`MalformedRecord` naming
        why — never the words it could not parse, which would put them in
        whatever reports the failure.
        """
        from aiwatcher_sdk.integrations.agentic import digest_of

        if not isinstance(payload, Mapping) or not isinstance(payload.get("reference"), str):
            return MalformedRecord("", "MissingReference", "the hop names no payload")
        reference = str(payload["reference"])
        try:
            data = self._payloads.get_payload(reference)
        except KeyError:
            return MalformedRecord(
                reference,
                "PayloadMissing",
                f"`{reference}` is not in this process's payload store: every process "
                "that reads a hop needs the same AIWATCHER_PAYLOAD_ROOT",
            )
        found = digest_of(data)
        if found != payload.get("digest"):
            return MalformedRecord(
                reference,
                "DigestMismatch",
                f"`{reference}` holds {found} where the hop recorded "
                f"{payload.get('digest')}: this is not the message that was sent",
            )
        try:
            return deserialize_record(json.dumps(data))
        except Exception as error:  # noqa: BLE001 - what will not decode is malformed, whatever broke
            return MalformedRecord(reference, type(error).__name__, str(error))

    # ── The registry's half ──────────────────────────────────────────────

    def registered_agents(self) -> list[AgentSnapshot]:
        """Every agent with a hop workflow under this prefix, by name."""
        saved = self._api.send("GET", "/api/v1/workflow-definitions").json()
        agents: dict[str, AgentSnapshot] = {}
        for entry in saved if isinstance(saved, list) else ():
            snapshot = self._agent_of(entry)
            if snapshot is None:
                continue
            known = agents.get(snapshot.agent_name)
            if known is None or snapshot.last_seen_ns >= known.last_seen_ns:
                agents[snapshot.agent_name] = snapshot
        return sorted(agents.values(), key=lambda agent: agent.agent_name)

    def _agent_of(self, entry: Any) -> AgentSnapshot | None:
        definition = entry.get("definition", {}) if isinstance(entry, dict) else {}
        name = str(definition.get("name", ""))
        agent = name.removeprefix(f"{self._prefix}.")
        steps = definition.get("steps") or []
        if agent in (name, MAILBOX_DEFINITION) or len(steps) != 1:
            return None
        step = steps[0]
        params = step.get("params") or {}
        # Only this module's workflows carry capabilities, so they are what
        # tells a hop workflow from anything else that shares the prefix.
        if step.get("id") != HOP or "capabilities" not in params:
            return None
        return AgentSnapshot(
            agent_name=agent,
            capabilities=tuple(str(capability) for capability in params["capabilities"]),
            role=str(params.get("role", "")),
            consumer_group=str(step.get("queue", "")),
            status="registered",
            last_seen_ns=_nanoseconds(entry.get("registered_at")),
        )

    # ── Mailboxes ────────────────────────────────────────────────────────

    def mailbox(self, address: str) -> str:
        """The hosted execution *address* is, started the first time anybody asks.

        Public because it is the id an operator reads a client's replies under.
        """
        with self._lock:
            known = self._mailboxes.get(address)
        if known is not None:
            return known
        name = f"{self._prefix}.{MAILBOX_DEFINITION}"
        # A hosted run's plan is its shape and is never scheduled, so this step
        # is claimed by nobody. It is here because a definition has to have one.
        self._api.json(
            "POST",
            "/api/v1/workflow-definitions",
            {
                "name": name,
                "version": VERSION,
                "steps": [
                    {
                        "id": MAILBOX_DEFINITION,
                        "task_ref": f"{name}@{VERSION}",
                        "queue": name,
                        "timeout_seconds": 60,
                    }
                ],
            },
            idempotent=True,
        )
        # A fixed key over a fixed plan is a fixed execution id: every process
        # that asks for this address reaches the same run, and none of them had
        # to be told its id.
        body = self._api.json(
            "POST",
            "/api/v1/executions",
            {"target": {"kind": "workflow", "name": name}, "decided_by": "worker"},
            idempotent=True,
            idempotency_key=f"{self._prefix}/{address}",
        )
        execution = str(body["execution"]["execution_id"])
        with self._lock:
            self._mailboxes[address] = execution
        return execution

    def _page(self, execution: str, after: int, limit: int) -> dict[str, Any]:
        return self._api.json(
            "GET",
            f"/api/v1/executions/{execution}/history",
            params={"after": after, "limit": limit},
        )

    def _version(self, execution: str) -> int:
        return int(self._page(execution, 0, 1).get("version", 0))

    def close(self) -> None:
        self._api.close()


class AiwatcherServiceRegistry:
    """Who may be handed a hop: the agents aiwatcher holds a workflow for.

    Liveness is gone, and on purpose. The broker registry answered "who said
    they were alive in the last twenty seconds" because a message sent to a
    group nobody read sat in a topic with nothing to say so. A hop to a
    registered agent with no worker waits in that agent's queue and is taken
    when one starts, so the question a caller needs answered is whether the
    agent exists — which is what a definition says. ``max_age_seconds`` is
    kept so the protocol is unchanged, and nothing reads it.
    """

    def __init__(self, transport: AiwatcherTransport) -> None:
        self._transport = transport

    def live_agents(self, *, max_age_seconds: float) -> list[AgentSnapshot]:
        del max_age_seconds
        return self._transport.registered_agents()

    def find_by_capability(
        self,
        capability: str,
        *,
        max_age_seconds: float,
    ) -> AgentSnapshot | None:
        for agent in self.live_agents(max_age_seconds=max_age_seconds):
            if capability in agent.capabilities:
                return agent
        return None


type MessageHandler = Callable[[Message, "AgenticServiceDiscovery"], Sequence[Message]]
type RetryClassifier = Callable[[Exception], bool]


class AiwatcherService:
    """One agent, as a worker claiming its own queue on aiwatcher.

    `DistributedService`'s job, with its loop, its ledger of attempts and its
    dead-letter publisher gone to the server: this registers the agent's
    workflow, claims its hops, runs the handler, hands each answer on, and
    reports. A handler that fails is retried by aiwatcher's budget when the
    retry classifier says it may be, and fails the step — the sink — when it may
    not, or on the last attempt; either way the client is told rather than left
    to time out.
    """

    def __init__(
        self,
        *,
        agent_name: str,
        capabilities: tuple[str, ...],
        discovery: AgenticServiceDiscovery,
        handler: MessageHandler,
        transport: AiwatcherTransport,
        role: str = "worker",
        close_hook: Callable[[], None] | None = None,
        max_delivery_attempts: int = 3,
        retry_classifier: RetryClassifier | None = None,
        concurrency: int = 1,
        timeout_seconds: int = 600,
        poll_interval: float = 1.0,
        client: httpx.Client | None = None,
        telemetry: AiwatcherClient | None = None,
    ) -> None:
        from aiwatcher_sdk.runtime import ExecutionPool, Runtime
        from aiwatcher_sdk.task import Task
        from aiwatcher_sdk.workflow import RetryPolicy, Workflow, WorkflowStep

        if max_delivery_attempts < 1:
            raise ValueError("max_delivery_attempts must be positive")
        self._agent = _name(agent_name, "an agent's name")
        self._discovery = discovery
        self._handler = handler
        self._transport = transport
        self._retry_classifier = retry_classifier or (
            lambda error: not isinstance(error, PermanentMessageError)
        )
        self._max_attempts = max_delivery_attempts
        self._metrics = DeliveryMetrics()
        self._metrics_lock = threading.Lock()
        name = transport.workflow_name(self._agent)
        step = WorkflowStep(
            HOP,
            Task(self._hop, name, VERSION),
            # Read by the registry, off the definition. The hop is handed them
            # too, as every step parameter is, and ignores them.
            params={"capabilities": list(capabilities), "role": role},
            retry=RetryPolicy(max_attempts=max_delivery_attempts),
            timeout_seconds=timeout_seconds,
        )
        self._workflow = Workflow(name, VERSION, (step,))
        self._runtime = Runtime(
            name=name,
            url=transport.url,
            token=transport.token,
            workflows=[self._workflow],
            pools=[ExecutionPool(self._agent, transport.queue_for(self._agent), concurrency)],
            placement={self._workflow.ref: self._agent},
            poll_interval=poll_interval,
            client=client,
            telemetry=telemetry,
            on_close=close_hook,
        )

    @property
    def workflow(self) -> Workflow:
        return self._workflow

    @property
    def metrics(self) -> DeliveryMetrics:
        with self._metrics_lock:
            return self._metrics

    def register(self) -> None:
        """Make the agent something a hop can be sent to. Idempotent by content."""
        self._runtime.register()

    def run_forever(self) -> None:
        """Register, then claim until stopped — by `close`, Ctrl-C, or SIGTERM."""
        self.register()
        previous = None
        if threading.current_thread() is threading.main_thread():
            previous = signal.signal(signal.SIGTERM, self._stop)
        try:
            self._runtime.serve()
        finally:
            if previous is not None:
                signal.signal(signal.SIGTERM, previous)

    def close(self) -> None:
        """Stop after the hop in hand, then release the handler's resources."""
        self._runtime.close()

    def _stop(self, signum: int, frame: FrameType | None) -> None:
        del signum, frame
        self._runtime.stop()

    # ── One hop ──────────────────────────────────────────────────────────

    def _hop(
        self,
        hop: Mapping[str, Any] | None = None,
        capabilities: object = None,
        role: object = None,
    ) -> dict[str, Any]:
        from aiwatcher_sdk.task_errors import TaskError

        del capabilities, role
        record = self._transport.record_from(hop)
        if not isinstance(record, Message):
            self._count(dead_lettered=1)
            problem = (
                f"{record.error_type}: {record.error_message}"
                if isinstance(record, MalformedRecord)
                else f"not a message: {type(record).__name__}"
            )
            raise TaskError(
                f"`{self._agent}` cannot read its hop — {problem}", classification="validation"
            )
        message = normalize_distributed_message(record)
        try:
            responses = self._handler(message, self._discovery)
        except Exception as error:
            raise self._failed(message, error) from error
        sent = [
            self._send(output)
            for output in self._answers(message, responses)
            # A fact about the turn with nobody to hand it to, as before.
            if not (isinstance(output, Event) and not output.metadata.target)
        ]
        self._count(handled=1)
        return {
            "agent": self._agent,
            "handled": message.type,
            "message_id": _message_id(message),
            "sent": sent,
        }

    def _answers(self, message: Message, responses: Sequence[Message]) -> tuple[Message, ...]:
        """The handler's answers, each under an id every attempt derives alike."""
        answered = _message_id(message)
        return normalize_outputs(
            message=message,
            responses=[
                _with_id(response, _derived(self._agent, answered, str(index)))
                for index, response in enumerate(responses)
            ],
        )

    def _send(self, output: Message) -> dict[str, str | None]:
        from aiwatcher_sdk.api import ApiError
        from aiwatcher_sdk.task_errors import TaskError

        target = output.metadata.target
        try:
            landed = self._transport.publish_message(output)
        except UnknownAgentError as error:
            raise TaskError(str(error), classification="validation") from error
        except ApiError as error:
            self._count(publish_failures=1)
            raise TaskError(
                f"`{self._agent}` answered and could not hand off to `{target}`: {error}",
                classification="infrastructure" if error.is_retryable else "validation",
            ) from error
        return {
            "to": target,
            "type": output.type,
            "message_id": _message_id(output),
            "landed": landed,
        }

    def _failed(self, message: Message, error: Exception) -> TaskError:
        """What a failed handler becomes, and whether the client hears about it now."""
        from aiwatcher_sdk.task_errors import TaskError

        retryable = self._retry_classifier(error)
        if retryable and _attempt() < self._max_attempts:
            self._count(retried=1)
        else:
            # The last attempt, or one the handler said not to repeat. The step
            # is about to fail, and a client waiting on this turn should hear it
            # now rather than when its timeout runs out.
            self._count(dead_lettered=1)
            self._tell(message, error)
        return TaskError(
            f"{type(error).__name__}: {error}",
            classification="infrastructure" if retryable else "validation",
        )

    def _tell(self, message: Message, error: Exception) -> None:
        answered = _message_id(message)
        for index, reply in enumerate(failure_replies(self._agent, message, error)):
            try:
                self._transport.publish_message(
                    _with_id(reply, _derived(self._agent, answered, f"failed/{index}"))
                )
            except Exception as failure:  # noqa: BLE001 - the failure being told is the step's; a lost reply is logged
                logger.warning(
                    "distributed_failure_reply_lost",
                    agent_name=self._agent,
                    target=reply.metadata.target,
                    error_type=type(failure).__name__,
                    error_message=str(failure),
                )

    def _count(self, **deltas: int) -> None:
        with self._metrics_lock:
            current = self._metrics
            self._metrics = DeliveryMetrics(
                handled=current.handled + deltas.get("handled", 0),
                replayed=current.replayed + deltas.get("replayed", 0),
                retried=current.retried + deltas.get("retried", 0),
                dead_lettered=current.dead_lettered + deltas.get("dead_lettered", 0),
                publish_failures=current.publish_failures + deltas.get("publish_failures", 0),
            )


def _name(value: str, label: str) -> str:
    resolved = value.strip()
    if not _NAME.fullmatch(resolved):
        raise ValueError(f"{label} must be letters, digits, `.`, `_` or `-`, got {value!r}")
    return resolved


def _message_id(message: Message) -> str:
    return getattr(message.metadata, "message_id", "") or message.metadata.idempotency_key


def _derived(agent: str, answered: str, position: str) -> str:
    return str(uuid.uuid5(_NAMESPACE, f"{agent}/{answered}/{position}"))


def _with_id(message: Message, message_id: str) -> Message:
    """*message* under *message_id*, unless it already chose one."""
    if _message_id(message):
        return message
    if isinstance(message.metadata, RecordedMessageMetadata):
        return message.with_metadata(message_id=message_id, idempotency_key=message_id)
    return message.with_metadata(idempotency_key=message_id)


def _attempt() -> int:
    """Which attempt of its step this is — one outside a worker."""
    from aiwatcher_sdk.worker import get_task_context

    try:
        return get_task_context().assignment.attempt
    except RuntimeError:
        return 1


def _position(entry_id: str) -> int:
    head, _, _ = entry_id.partition("-")
    try:
        return max(0, int(head))
    except ValueError:
        return 0


def _plain(message: Message) -> dict[str, str]:
    """What a mailbox row says about a reply in the clear: who, to whom, which turn."""
    metadata = message.metadata
    fields = {
        "agentic_kind": message.kind,
        "message_id": _message_id(message),
        "turn_id": metadata.turn_id,
        "correlation_id": metadata.correlation_id,
        "source": metadata.source,
        "target": metadata.target,
        "domain": metadata.domain,
        "status": metadata.status,
        "scope": metadata.scope,
    }
    return {key: str(value) for key, value in fields.items() if value}


def _nanoseconds(stamp: object) -> int:
    if not isinstance(stamp, str):
        return 0
    try:
        return int(datetime.fromisoformat(stamp).timestamp() * 1_000_000_000)
    except ValueError:
        return 0
