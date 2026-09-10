from __future__ import annotations

import os
import socket
import threading
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, Protocol, cast

from structlog import get_logger

from aiwatcher_agentic.runtime.distributed.contracts import AgentHeartbeat, AgentRegistration
from aiwatcher_agentic.runtime.distributed.registry import ServiceRegistry
from aiwatcher_agentic.runtime.distributed.transport import (
    ConsumedRecord,
    MalformedRecord,
    MessageTransport,
    normalize_distributed_message,
)
from aiwatcher_agentic.workflow import (
    ConcurrencyConflictError,
    DurableWorkflowExecutor,
    Event,
    EventStore,
    ProcessorLock,
)
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Message,
    RecordedMessageMetadata,
    TurnCompleted,
)

if TYPE_CHECKING:
    from aiwatcher_agentic.runtime.distributed.discovery import AgenticServiceDiscovery

logger = get_logger(__name__)

type MessageHandler = Callable[[Message, "AgenticServiceDiscovery"], Sequence[Message]]
type CloseHook = Callable[[], None]
type RetryClassifier = Callable[[Exception], bool]


class PermanentMessageError(RuntimeError):
    """Marks a handler failure as non-retryable."""


@dataclass(frozen=True, slots=True)
class DeliveryMetrics:
    handled: int = 0
    replayed: int = 0
    retried: int = 0
    dead_lettered: int = 0
    publish_failures: int = 0


class ConsumerGroupTransport(MessageTransport, Protocol):
    """What `DistributedService` asks of a transport beyond a client's half.

    A consumer group: one stream an agent's workers share, what one of them has
    not acknowledged, and taking that back. `InMemoryTransport` has one.
    `AiwatcherTransport` has none, and discovery gives it `AiwatcherService`.
    """

    def ensure_consumer_group(self, target: str, group: str) -> None: ...

    def consume_target(
        self,
        target: str,
        *,
        group: str,
        consumer: str,
        block_ms: int = 1_000,
        count: int = 10,
    ) -> list[ConsumedRecord]: ...

    def autoclaim_pending(
        self,
        target: str,
        *,
        group: str,
        consumer: str,
        min_idle_ms: int = 5_000,
        count: int = 10,
    ) -> list[ConsumedRecord]: ...

    def ack(self, stream: str, group: str, entry_id: str) -> int: ...


class RegisteringRegistry(ServiceRegistry, Protocol):
    """A registry an agent announces itself to, which is the in-memory one."""

    def register(self, registration: AgentRegistration) -> None: ...

    def heartbeat(self, heartbeat: AgentHeartbeat) -> None: ...


class AgentService(Protocol):
    """One agent's service, whichever transport feeds it."""

    @property
    def metrics(self) -> DeliveryMetrics: ...

    def run_forever(self) -> None: ...

    def close(self) -> None: ...


def normalize_outputs(*, message: Message, responses: Sequence[Message]) -> tuple[Message, ...]:
    """A handler's answers, completed from the message they answer.

    Shared by every transport's service, so a reply carries the turn, the
    correlation and the causation of what it answered whichever one carried it.
    """
    normalized: list[Message] = []
    input_id = getattr(message.metadata, "message_id", "")
    for response in responses:
        prepared = normalize_distributed_message(response).with_metadata(
            runtime_id=response.metadata.runtime_id or message.metadata.runtime_id,
            session_id=response.metadata.session_id or message.metadata.session_id,
            turn_id=response.metadata.turn_id or message.metadata.turn_id,
            correlation_id=(
                response.metadata.correlation_id
                or message.metadata.correlation_id
                or message.metadata.turn_id
                or input_id
            ),
            causation_id=response.metadata.causation_id or input_id or None,
            idempotency_key=response.metadata.idempotency_key
            or getattr(response.metadata, "message_id", ""),
            domain=response.metadata.domain or message.metadata.domain,
            tenant_id=response.metadata.tenant_id or message.metadata.tenant_id,
            workspace_id=response.metadata.workspace_id or message.metadata.workspace_id,
            reply_to_message_id=response.metadata.reply_to_message_id or input_id or None,
            trace=response.metadata.trace or message.metadata.trace,
        )
        normalized.append(prepared)
    return tuple(normalized)


def reply_target_of(message: Message) -> str:
    """Who is waiting on *message*'s turn: its `reply_target`, else its sender."""
    payload = message.data if isinstance(message.data, dict) else {}
    reply_target = payload.get("reply_target")
    if isinstance(reply_target, str) and reply_target:
        return reply_target
    if message.metadata.source:
        return message.metadata.source
    return "chat"


def failure_replies(agent_name: str, message: Message, error: Exception) -> tuple[Message, ...]:
    """What a client is told when *agent_name* gave up on *message*."""
    reply_target = reply_target_of(message)
    metadata = RecordedMessageMetadata(
        runtime_id=message.metadata.runtime_id,
        session_id=message.metadata.session_id,
        turn_id=message.metadata.turn_id,
        correlation_id=message.metadata.correlation_id,
        domain=message.metadata.domain or "distributed",
        source=agent_name,
        target=reply_target,
        status="error",
        trace=message.metadata.trace,
    )
    return (
        AssistantMessage(
            data=ConversationData(
                role="assistant",
                text=f"{agent_name} failed: {error}",
            ),
            metadata=metadata,
        ),
        TurnCompleted(
            data={
                "workflow": agent_name,
                "error_type": type(error).__name__,
                "error_message": str(error),
            },
            metadata=metadata,
        ),
    )


class DistributedService:
    def __init__(
        self,
        *,
        agent_name: str,
        capabilities: tuple[str, ...],
        discovery: AgenticServiceDiscovery,
        handler: MessageHandler,
        role: str = "worker",
        heartbeat_seconds: float = 5.0,
        close_hook: CloseHook | None = None,
        min_idle_ms: int = 5_000,
        event_store: EventStore | None = None,
        max_delivery_attempts: int = 3,
        retry_classifier: RetryClassifier | None = None,
        workflow_lock: ProcessorLock | None = None,
    ) -> None:
        if max_delivery_attempts < 1:
            raise ValueError("max_delivery_attempts must be positive")
        self._agent_name = agent_name
        self._capabilities = capabilities
        self._discovery = discovery
        # Only a transport with consumer groups is handed to this service:
        # discovery gives `AiwatcherTransport` to `AiwatcherService` instead.
        self._transport = cast(ConsumerGroupTransport, discovery.transport)
        self._registry = cast(RegisteringRegistry, discovery.registry)
        self._handler = handler
        self._role = role
        self._heartbeat_seconds = heartbeat_seconds
        self._close_hook = close_hook
        self._min_idle_ms = min_idle_ms
        self._event_store = event_store
        self._workflow = (
            DurableWorkflowExecutor(event_store, lock=workflow_lock)
            if event_store is not None
            else None
        )
        self._max_delivery_attempts = max_delivery_attempts
        self._retry_classifier = retry_classifier or (
            lambda error: not isinstance(error, PermanentMessageError)
        )
        self._volatile_attempts: dict[str, int] = {}
        self._metrics = DeliveryMetrics()
        self._group = agent_name
        self._consumer_name = f"{socket.gethostname()}-{os.getpid()}"
        self._stop_event = threading.Event()
        self._heartbeat_thread: threading.Thread | None = None

    @property
    def metrics(self) -> DeliveryMetrics:
        return self._metrics

    def run_forever(self) -> None:
        self._registry.register(
            AgentRegistration(
                agent_name=self._agent_name,
                capabilities=self._capabilities,
                role=self._role,
                consumer_group=self._group,
            )
        )
        self._registry.heartbeat(AgentHeartbeat(agent_name=self._agent_name, status="ready"))
        self._transport.ensure_consumer_group(self._agent_name, self._group)

        self._heartbeat_thread = threading.Thread(
            target=self._heartbeat_loop,
            name=f"{self._agent_name}-heartbeat",
            daemon=True,
        )
        self._heartbeat_thread.start()
        self._drain_pending()

        while not self._stop_event.is_set():
            records = self._transport.consume_target(
                self._agent_name,
                group=self._group,
                consumer=self._consumer_name,
                block_ms=1_000,
                count=10,
            )
            if not records:
                self._drain_pending_once()
                continue
            for record in records:
                self._handle_record(record.stream, record.entry_id, record.record)

    def close(self) -> None:
        self._stop_event.set()
        if self._heartbeat_thread is not None:
            self._heartbeat_thread.join(timeout=1.0)
        if self._close_hook is not None:
            self._close_hook()

    def _drain_pending(self) -> None:
        while not self._stop_event.is_set() and self._drain_pending_once():
            continue

    def _drain_pending_once(self) -> bool:
        records = self._transport.autoclaim_pending(
            self._agent_name,
            group=self._group,
            consumer=self._consumer_name,
            min_idle_ms=self._min_idle_ms,
            count=10,
        )
        if not records:
            return False
        logger.info(
            "drain_pending_messages",
            agent_name=self._agent_name,
            count=len(records),
        )
        for record in records:
            self._handle_record(record.stream, record.entry_id, record.record)
        return True

    def _heartbeat_loop(self) -> None:
        while not self._stop_event.wait(self._heartbeat_seconds):
            self._registry.heartbeat(AgentHeartbeat(agent_name=self._agent_name, status="alive"))

    def _handle_record(self, stream: str, entry_id: str, record: object) -> None:
        if not isinstance(record, Message):
            error = (
                f"{record.error_type}: {record.error_message}"
                if isinstance(record, MalformedRecord)
                else f"Unsupported distributed record: {type(record).__name__}"
            )
            if self._publish_dead_letter(stream, entry_id, record, error, attempts=1):
                self._ack(stream, entry_id)
                self._increment_metrics(dead_lettered=1)
            return

        message = normalize_distributed_message(record)
        stream_name = self._workflow_stream_name(message)
        try:
            if self._workflow is None:
                outputs = tuple(
                    self._normalize_outputs(
                        message=message,
                        responses=self._handler(message, self._discovery),
                    )
                )
                duplicate = False
            else:
                result = self._workflow.execute(
                    stream_name,
                    message,
                    lambda current: self._normalize_outputs(
                        message=current,
                        responses=self._handler(current, self._discovery),
                    ),
                )
                outputs = result.outputs
                duplicate = result.duplicate
        except Exception as error:  # noqa: BLE001 - a handler's failure is retried or dead-lettered
            self._handle_failure(stream, entry_id, stream_name, message, error)
            return

        terminal_failure = self._terminal_failure(message)
        if terminal_failure is not None:
            attempts, failure_text = terminal_failure
            if not self._publish_dead_letter(
                stream,
                entry_id,
                message,
                failure_text,
                attempts=attempts,
            ):
                return
        if not self._publish_outputs(outputs):
            return
        self._ack(stream, entry_id)
        self._volatile_attempts.pop(self._message_id(message), None)
        self._increment_metrics(
            handled=0 if duplicate else 1,
            replayed=1 if duplicate else 0,
        )

    def _handle_failure(
        self,
        stream: str,
        entry_id: str,
        workflow_stream: str,
        message: Message,
        error: Exception,
    ) -> None:
        attempt = self._record_failure(message, error)
        retryable = self._retry_classifier(error)
        logger.warning(
            "distributed_service_handler_failed",
            agent_name=self._agent_name,
            attempt=attempt,
            retryable=retryable,
            error_type=type(error).__name__,
            error_message=str(error),
        )
        if retryable and attempt < self._max_delivery_attempts:
            self._increment_metrics(retried=1)
            return

        outputs = tuple(
            self._normalize_outputs(
                message=message,
                responses=self._error_messages(message, error),
            )
        )
        if self._workflow is not None:
            result = self._workflow.persist_outputs(workflow_stream, message, outputs)
            outputs = result.outputs
        if not self._publish_dead_letter(
            stream,
            entry_id,
            message,
            f"{type(error).__name__}: {error}",
            attempts=attempt,
        ):
            return
        if not self._publish_outputs(outputs):
            return
        self._ack(stream, entry_id)
        self._increment_metrics(dead_lettered=1)

    def _record_failure(self, message: Message, error: Exception) -> int:
        message_id = self._message_id(message)
        if self._event_store is None:
            attempt = self._volatile_attempts.get(message_id, 0) + 1
            self._volatile_attempts[message_id] = attempt
            return attempt

        delivery_stream = f"delivery:{self._agent_name}:{message_id}"
        for _ in range(16):
            history = self._event_store.read_stream(delivery_stream)
            attempt = sum(event.type == "delivery.failed" for event in history.events) + 1
            failure = Event(
                type="delivery.failed",
                data={
                    "message_id": message_id,
                    "agent_name": self._agent_name,
                    "attempt": attempt,
                    "error_type": type(error).__name__,
                    "error_message": str(error),
                    "retryable": self._retry_classifier(error),
                },
                metadata=RecordedMessageMetadata(
                    runtime_id=message.metadata.runtime_id,
                    session_id=message.metadata.session_id,
                    turn_id=message.metadata.turn_id,
                    correlation_id=message.metadata.correlation_id,
                    causation_id=message_id,
                    domain=message.metadata.domain or "distributed",
                    source=self._agent_name,
                    contract_name="delivery.failed",
                ),
            )
            try:
                self._event_store.append_to_stream(
                    delivery_stream,
                    (failure,),
                    expected_version=history.current_version,
                )
                return attempt
            except ConcurrencyConflictError:
                continue
        raise ConcurrencyConflictError(delivery_stream, "stable version", "busy")

    def _terminal_failure(self, message: Message) -> tuple[int, str] | None:
        message_id = self._message_id(message)
        if self._event_store is None:
            attempts = self._volatile_attempts.get(message_id, 0)
            return None if attempts < self._max_delivery_attempts else (attempts, "failed")
        history = self._event_store.read_stream(f"delivery:{self._agent_name}:{message_id}")
        failures = [event for event in history.events if event.type == "delivery.failed"]
        if len(failures) < self._max_delivery_attempts:
            return None
        latest = failures[-1].data if isinstance(failures[-1].data, Mapping) else {}
        return (
            len(failures),
            f"{latest.get('error_type', 'Error')}: {latest.get('error_message', 'failed')}",
        )

    def _publish_outputs(self, outputs: Sequence[Message]) -> bool:
        try:
            for output in outputs:
                if isinstance(output, Event) and not output.metadata.target:
                    continue
                self._transport.publish_message(output)
            return True
        except Exception as error:  # noqa: BLE001 - counted, and the message stays unacknowledged
            self._increment_metrics(publish_failures=1)
            logger.warning(
                "distributed_service_publish_failed",
                agent_name=self._agent_name,
                error_type=type(error).__name__,
                error_message=str(error),
            )
            return False

    def _publish_dead_letter(
        self,
        stream: str,
        entry_id: str,
        record: object,
        error: str,
        *,
        attempts: int,
    ) -> bool:
        publish = getattr(self._transport, "publish_dead_letter", None)
        if not callable(publish):
            logger.warning(
                "dead_letter_transport_not_supported",
                agent_name=self._agent_name,
                stream=stream,
                entry_id=entry_id,
            )
            return True
        try:
            publish(
                target=self._agent_name,
                source_stream=stream,
                group=self._group,
                entry_id=entry_id,
                record=record,
                error=error,
                attempts=attempts,
            )
            return True
        except Exception as publish_error:  # noqa: BLE001 - counted, and the message stays pending
            self._increment_metrics(publish_failures=1)
            logger.warning(
                "dead_letter_publish_failed",
                agent_name=self._agent_name,
                error_type=type(publish_error).__name__,
                error_message=str(publish_error),
            )
            return False

    def _ack(self, stream: str, entry_id: str) -> None:
        self._transport.ack(stream, self._group, entry_id)

    def _workflow_stream_name(self, message: Message) -> str:
        domain = (message.metadata.domain or "distributed").strip() or "distributed"
        workflow_id = (
            message.metadata.turn_id
            or message.metadata.correlation_id
            or message.metadata.runtime_id
            or self._message_id(message)
        )
        return f"workflow:{domain}:{workflow_id}"

    @staticmethod
    def _normalize_outputs(
        *,
        message: Message,
        responses: Sequence[Message],
    ) -> tuple[Message, ...]:
        return normalize_outputs(message=message, responses=responses)

    def _error_messages(self, message: Message, error: Exception) -> tuple[Message, ...]:
        return failure_replies(self._agent_name, message, error)

    @staticmethod
    def _resolve_reply_target(message: Message) -> str:
        return reply_target_of(message)

    @staticmethod
    def _message_id(message: Message) -> str:
        return getattr(message.metadata, "message_id", "") or message.metadata.idempotency_key

    def _increment_metrics(
        self,
        *,
        handled: int = 0,
        replayed: int = 0,
        retried: int = 0,
        dead_lettered: int = 0,
        publish_failures: int = 0,
    ) -> None:
        self._metrics = DeliveryMetrics(
            handled=self._metrics.handled + handled,
            replayed=self._metrics.replayed + replayed,
            retried=self._metrics.retried + retried,
            dead_lettered=self._metrics.dead_lettered + dead_lettered,
            publish_failures=self._metrics.publish_failures + publish_failures,
        )
