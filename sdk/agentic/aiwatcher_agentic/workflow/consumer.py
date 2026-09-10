from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field

from aiwatcher_agentic.workflow.errors import StepLimitExceeded
from aiwatcher_agentic.workflow.message_stream import MessageStream
from aiwatcher_agentic.workflow.messages import Message
from aiwatcher_agentic.workflow.reactor import MessageRouter, Reactor, TechnicalRoutingFn
from aiwatcher_agentic.workflow.tracer import NoopWorkflowTracer, WorkflowTracer


def _default_is_retryable(error: Exception) -> bool:
    return isinstance(error, (TimeoutError, ConnectionError, OSError))


@dataclass(frozen=True, slots=True)
class ConsumerConfig:
    max_steps: int = 100
    max_retries: int = 2
    is_retryable: Callable[[Exception], bool] = field(default_factory=lambda: _default_is_retryable)


class MessageConsumer:
    """Consumer: polluje stream, przekazuje do processorow (MessageRouter, Reactor)."""

    def __init__(
        self,
        config: ConsumerConfig | None = None,
        tracer: WorkflowTracer | None = None,
    ) -> None:
        self._config = config or ConsumerConfig()
        self._tracer = tracer or NoopWorkflowTracer()

    def consume(
        self,
        stream: MessageStream,
        decider: MessageRouter,
        routing_fn: TechnicalRoutingFn,
    ) -> None:
        """Poll stream az pusty. Wiadomosc -> decider -> routing -> reactor -> output -> stream."""
        step = 0

        while (msg := stream.read_next()) is not None:
            step += 1
            if step > self._config.max_steps:
                raise StepLimitExceeded(max_steps=self._config.max_steps)

            commands = decider(msg)

            for command in commands:
                reactor = routing_fn(command)
                if reactor is None:
                    # No reactor — command goes to stream as-is (e.g., domain events)
                    stream.append(command)
                    continue

                # Reactor handles the command — only reactor output goes to stream
                with self._tracer.step(
                    name=f"reactor.{type(command).__name__}",
                    span_type="TOOL",
                    attributes={"step": step, "command_type": type(command).__name__},
                ) as span:
                    output, attempts = self._invoke_with_retry(reactor, command)
                    output = self._bind_output_context(
                        command=command,
                        output=output,
                        step=step,
                        attempts=attempts,
                    )
                    span.update(
                        output={"message_type": type(output).__name__},
                        metadata={"attempt_no": attempts, "loop_iteration": step},
                    )

                stream.append(output)

    @staticmethod
    def _bind_output_context(
        *,
        command: Message,
        output: Message,
        step: int,
        attempts: int,
    ) -> Message:
        return output.with_metadata(
            session_id=(
                output.metadata.session_id
                or command.metadata.session_id
                or command.metadata.runtime_id
            ),
            trace_id=output.metadata.trace_id or command.metadata.trace_id,
            span_id=output.metadata.span_id or command.metadata.span_id,
            parent_span_id=(output.metadata.parent_span_id or command.metadata.parent_span_id),
            span_name=output.metadata.span_name or command.metadata.span_name,
            span_type=output.metadata.span_type or command.metadata.span_type,
            attempt_no=output.metadata.attempt_no or attempts,
            loop_iteration=output.metadata.loop_iteration or step,
        )

    def _invoke_with_retry(self, reactor: Reactor, command: Message) -> tuple[Message, int]:
        for attempt in range(self._config.max_retries + 1):
            try:
                with self._tracer.step(
                    name=f"reactor.{type(command).__name__}.attempt",
                    span_type="CHAIN",
                    attributes={"attempt_no": attempt + 1},
                ):
                    return reactor.invoke(command), attempt + 1
            except Exception as error:
                if attempt >= self._config.max_retries or not self._config.is_retryable(error):
                    raise
        raise AssertionError("unreachable: retry loop must return or raise")
