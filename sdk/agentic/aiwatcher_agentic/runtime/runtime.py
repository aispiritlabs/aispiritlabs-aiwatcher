"""Generic agent orchestration runtime.

Accepts workflows, a router, and output handlers as constructor parameters.
No application-specific agents are hardcoded here; concrete applications
subclass :class:`AgenticRuntime` and supply their own pieces (see
``personal_assistant.runtime.PARuntime``).

What it used to read from `ai_spirit_agent` it is now handed (AW-2): a tracer,
through the `LLMTracer` port, and its settings, through `RuntimeSettings`. The
one thing it did with a model client — configure it — is `_configure_providers`,
a hook an application overrides, because a model client is not something this
distribution knows about.
"""

from __future__ import annotations

import threading
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Any, Protocol

from structlog import get_logger

from aiwatcher_agentic.runtime.config import RuntimeConfig, RuntimeSettings
from aiwatcher_agentic.runtime.storage.sqlite_store import MessageStore, SQLiteMessageStore
from aiwatcher_agentic.runtime.workflow_descriptors import render_workflow_descriptors
from aiwatcher_agentic.tracer import LLMTracer, NoopLLMTracer
from aiwatcher_agentic.workflow import (
    DurableMessageBus,
    SQLiteCheckpointStore,
    SQLiteEventStore,
    WorkflowRuntime,
)
from aiwatcher_agentic.workflow._workflow import AgenticWorkflow
from aiwatcher_agentic.workflow.execution import WorkflowExecution
from aiwatcher_agentic.workflow.ids import uuid7
from aiwatcher_agentic.workflow.message_bus import InMemoryMessageBus
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Message,
    PromptSnapshot,
    RecordedMessageMetadata,
    UserCommand,
    UserMessage,
)
from aiwatcher_agentic.workflow.output_handler import WorkflowOutputHandler
from aiwatcher_agentic.workflow.turn_execution import TurnExecutor, TurnPlan, coerce_reply_text

logger = get_logger(__name__)

ShutdownStep = tuple[str, Callable[[], None]]


class RouterProtocol(Protocol):
    """Minimal protocol for a workflow router."""

    def route(self, message: str, available_workflows_summary: str) -> str: ...

    def start(self) -> str: ...

    def close(self) -> None: ...


@dataclass(frozen=True, slots=True)
class RuntimeServices:
    """Infrastructure a runtime hands to workflow and handler factories."""

    bus: InMemoryMessageBus
    tracer: LLMTracer
    settings: RuntimeSettings


type WorkflowSource = (
    Sequence[AgenticWorkflow] | Callable[[RuntimeServices], Sequence[AgenticWorkflow]]
)
type OutputHandlerSource = (
    Sequence[WorkflowOutputHandler] | Callable[[RuntimeServices], Sequence[WorkflowOutputHandler]]
)


class AgenticRuntime:
    """Generic agent orchestration runtime.

    Accepts workflows (or a factory that builds them from :class:`RuntimeServices`),
    a router, and output handlers. Application-specific logic belongs in a
    subclass — override :meth:`_run_general_fallback` for a custom no-route
    answer and :meth:`_extra_shutdown_steps` to release extra resources.
    """

    def __init__(
        self,
        *,
        workflows: WorkflowSource,
        router: RouterProtocol,
        output_handlers: OutputHandlerSource | None = None,
        workflow_filter: Callable[[AgenticWorkflow], bool] | None = None,
        on_stop: Callable[[], None] | None = None,
        session_id: str = "",
        settings: RuntimeSettings | None = None,
        tracer: LLMTracer | None = None,
    ) -> None:
        self._settings: RuntimeSettings = settings if settings is not None else RuntimeConfig()
        self._stop_lock = threading.Lock()
        self._stopped = False
        self._tracer: LLMTracer = tracer if tracer is not None else NoopLLMTracer()
        self.runtime_id = self._new_runtime_id()
        self.session_id = session_id
        self.event_store = self._create_event_store()
        self.store = self._create_message_store()
        self.bus = self._create_bus(self.store)
        self._workflow_runtime = WorkflowRuntime(
            bus=self.bus,
            tracer=self._tracer,
            max_inline_bytes=self._settings.message_stream_inline_bytes,
            chunk_bytes=self._settings.message_stream_chunk_bytes,
        )
        self._turn_executor: TurnExecutor = self._workflow_runtime.turn_executor
        self.router = router
        self._workflow_filter = workflow_filter
        self._on_stop = on_stop

        services = RuntimeServices(bus=self.bus, tracer=self._tracer, settings=self._settings)

        built_workflows = workflows(services) if callable(workflows) else workflows
        for workflow in built_workflows:
            self._workflow_runtime.register_workflow(workflow.description.agent_name, workflow)
        self.workflows: dict[str, AgenticWorkflow] = self._workflow_runtime.workflows

        built_handlers = output_handlers(services) if callable(output_handlers) else output_handlers
        for handler in built_handlers or ():
            self._workflow_runtime.register_output_handler(handler)

        self._configure_providers()
        replay_pending = getattr(self.bus, "replay_pending", None)
        if callable(replay_pending):
            replay_pending()

    # ------------------------------------------------------------------
    # Infrastructure factories — override to swap the backing services
    # ------------------------------------------------------------------

    def _create_message_store(self) -> MessageStore:
        return SQLiteMessageStore(
            path=self._settings.message_store_path,
            batch_size=self._settings.message_store_batch_size,
            flush_interval_seconds=self._settings.message_store_flush_interval_seconds,
        )

    def _create_event_store(self) -> SQLiteEventStore:
        return SQLiteEventStore(path=self._settings.event_store_path)

    def _create_bus(self, store: MessageStore) -> InMemoryMessageBus:
        stream_name = f"runtime-messages:{self.session_id or 'default'}"
        return DurableMessageBus(
            event_store=self.event_store,
            checkpoint_store=SQLiteCheckpointStore(self.event_store.path),
            store=store,
            stream_name=stream_name,
            consumer_id="runtime-dispatch-v1",
        )

    def _configure_providers(self) -> None:
        """Where an application configures the model clients its workflows call.

        Run once, after the workflows and output handlers are registered and
        before the bus replays what it had not delivered, so that a replayed
        turn never reaches a client nobody configured. Nothing, here.
        """

    @staticmethod
    def _new_runtime_id() -> str:
        return str(uuid7())

    @staticmethod
    def _new_turn_id() -> str:
        return str(uuid7())

    def _available_workflows_summary(self) -> str:
        return render_workflow_descriptors(list(self._routable_workflows().values()))

    def _routable_workflows(self) -> dict[str, AgenticWorkflow]:
        routable: dict[str, AgenticWorkflow] = {}
        for workflow in self.workflows.values():
            if self._workflow_filter and not self._workflow_filter(workflow):
                continue
            routable[workflow.description.agent_name] = workflow
        return routable

    def _resolve_workflow(self, text: str) -> AgenticWorkflow | None:
        available_summary = self._available_workflows_summary()
        workflow_name = self.router.route(text, available_summary)
        logger.debug("workflow_resolved", workflow=workflow_name)
        return self._routable_workflows().get(workflow_name)

    @staticmethod
    def _unknown_workflow_message(available_workflows_summary: str) -> str:
        return (
            "No matching agent found for this message.\n"
            "Available agents:\n"
            f"{available_workflows_summary}"
        )

    def _get_turn_executor(self) -> TurnExecutor:
        return self._workflow_runtime.turn_executor

    def _publish_workflow_execution(
        self,
        *,
        incoming: UserMessage,
        workflow_name: str,
        execution: WorkflowExecution,
    ) -> str | None:
        return self._workflow_runtime.publish_workflow_execution(
            incoming=incoming,
            workflow_name=workflow_name,
            execution=execution,
        )

    def start(self) -> str:
        return self.router.start()

    def get_initial_greeting(self) -> str:
        return self.start()

    def run(self, text: str) -> str:
        return self.handle(
            UserMessage(
                data=ConversationData(role="user", text=text),
                metadata=RecordedMessageMetadata(
                    runtime_id=self.runtime_id,
                    session_id=self.session_id,
                    domain="general",
                    source="user",
                ),
            )
        )

    # ------------------------------------------------------------------
    # Shutdown
    # ------------------------------------------------------------------

    def _extra_shutdown_steps(self) -> Sequence[ShutdownStep]:
        """Resources a subclass wants closed after workflows, before the bus."""
        return ()

    def _shutdown_steps(self) -> list[ShutdownStep]:
        steps: list[ShutdownStep] = []
        if self._on_stop is not None:
            steps.append(("on_stop", self._on_stop))
        steps.append(("router", self.router.close))
        for workflow_name, workflow in self.workflows.items():
            close = getattr(workflow, "close", None)
            if callable(close):
                steps.append((f"workflow:{workflow_name}", close))
        steps.extend(self._extra_shutdown_steps())
        steps.append(("bus", self.bus.close))
        steps.append(("tracer", self._tracer.shutdown))
        return steps

    def stop(self) -> None:
        """Release every runtime resource, best effort.

        Every step runs even if an earlier one failed; the collected failures are
        raised together so none is silently dropped.
        """
        with self._stop_lock:
            if self._stopped:
                return
            self._stopped = True

        errors: list[Exception] = []
        for name, action in self._shutdown_steps():
            try:
                action()
            except Exception as error:  # noqa: BLE001 - every step runs; the failures are raised together
                logger.warning("runtime_shutdown_step_failed", step=name, error=str(error))
                errors.append(error)

        if errors:
            raise ExceptionGroup("runtime shutdown failed", errors)

    def close(self) -> None:
        self.stop()

    # ------------------------------------------------------------------
    # Turn planning
    # ------------------------------------------------------------------

    def _build_router_messages(
        self,
        *,
        message: UserMessage,
        turn_id: str,
        workflow_name: str,
        router_response: Any | None,
    ) -> tuple[Message, ...]:
        if router_response is None:
            return ()

        messages: list[Message] = []
        if router_response.result.prompt_snapshot is not None:
            snapshot = router_response.result.prompt_snapshot
            messages.append(
                PromptSnapshot(
                    data=ConversationData(
                        role="system",
                        text=snapshot.text,
                        payload={"tool_schema": list(snapshot.tool_schema)},
                    ),
                    metadata=RecordedMessageMetadata(
                        runtime_id=message.metadata.runtime_id,
                        session_id=message.metadata.session_id,
                        turn_id=turn_id,
                        domain="routing",
                        source="router",
                        target=message.metadata.source,
                        prompt_name=snapshot.prompt_name,
                        prompt_hash=snapshot.prompt_hash,
                        agent_run_id=router_response.result.run_id,
                    ),
                )
            )
        messages.append(
            AssistantMessage(
                data=ConversationData(role="assistant", text=workflow_name),
                metadata=RecordedMessageMetadata(
                    runtime_id=message.metadata.runtime_id,
                    session_id=message.metadata.session_id,
                    turn_id=turn_id,
                    domain="routing",
                    source="router",
                    target=message.metadata.source,
                    agent_run_id=router_response.result.run_id,
                ),
            )
        )
        return tuple(messages)

    def _run_general_fallback(self, message: UserMessage) -> WorkflowExecution:
        """Answer a message the router could not route. Subclasses may override."""
        del message
        return WorkflowExecution(
            text=self._unknown_workflow_message(self._available_workflows_summary())
        )

    def _plan_general_turn(self, message: UserMessage) -> TurnPlan:
        available_summary = self._available_workflows_summary()
        route_response = getattr(self.router, "route_response", None)
        message_text = message.data.text if isinstance(message.data, ConversationData) else ""
        if callable(route_response):
            router_response = route_response(message_text, available_summary)
            workflow_name = router_response.output.strip()
        else:
            workflow_name = self.router.route(message_text or "", available_summary)
            router_response = None
        logger.debug("workflow_resolved", workflow=workflow_name)
        workflow = self._routable_workflows().get(workflow_name)
        turn_id = self._new_turn_id()
        pre_messages = self._build_router_messages(
            message=message,
            turn_id=turn_id,
            workflow_name=workflow_name,
            router_response=router_response,
        )

        if workflow is None:
            return TurnPlan(
                incoming=UserMessage(
                    data=ConversationData(role="user", text=message_text),
                    metadata=RecordedMessageMetadata(
                        runtime_id=message.metadata.runtime_id,
                        session_id=message.metadata.session_id,
                        turn_id=turn_id,
                        domain="general",
                        source=message.metadata.source,
                    ),
                ),
                handler=self._run_general_fallback,
                trace_name="general",
                lifecycle_domain="general",
                lifecycle_target=None,
                lifecycle_workflow_name="general",
                output_agent_name="assistant",
                pre_messages=pre_messages,
            )

        return TurnPlan(
            incoming=UserMessage(
                data=ConversationData(role="user", text=message_text),
                metadata=RecordedMessageMetadata(
                    runtime_id=message.metadata.runtime_id,
                    session_id=message.metadata.session_id,
                    turn_id=turn_id,
                    domain=workflow_name,
                    source=message.metadata.source,
                    target=workflow_name,
                ),
            ),
            handler=workflow.handle,
            trace_name=workflow_name,
            lifecycle_domain=workflow_name,
            lifecycle_target=workflow_name,
            lifecycle_workflow_name=workflow_name,
            output_agent_name=workflow_name,
            selected_workflow=workflow_name,
            pre_messages=pre_messages,
        )

    def _handle_general_user_message(self, message: UserMessage) -> str:
        return self._get_turn_executor().execute(self._plan_general_turn(message))

    def _plan_targeted_turn(self, message: UserMessage) -> TurnPlan:
        target = message.metadata.target
        if target is None:
            raise ValueError("Targeted turns require metadata.target.")
        turn_id = message.metadata.turn_id or self._new_turn_id()
        workflow = self.workflows[target]
        return TurnPlan(
            incoming=message.with_metadata(turn_id=turn_id, domain=target),
            handler=workflow.handle,
            trace_name=target,
            lifecycle_domain=target,
            lifecycle_target=target,
            lifecycle_workflow_name=target,
            output_agent_name=target,
        )

    def _handle_targeted_message(self, message: UserMessage) -> str:
        return self._get_turn_executor().execute(self._plan_targeted_turn(message))

    def handle(self, message: Message) -> str:
        if (
            isinstance(message, UserMessage)
            and message.metadata.domain == "general"
            and message.metadata.source == "user"
        ):
            return self._handle_general_user_message(message)

        if isinstance(message, UserMessage):
            target = message.metadata.target
            if target and target in self.workflows:
                return self._handle_targeted_message(message)

        if isinstance(message, UserCommand):
            self.bus.publish(message)
            target = message.metadata.domain or message.metadata.target
            if target and target in self.workflows:
                reply = self.workflows[target].handle(message)
                return coerce_reply_text(reply)

        replies = self.bus.publish(message)
        return coerce_reply_text(replies[-1] if replies else "")

    def handle_message(self, text: str) -> str:
        return self.run(text)
