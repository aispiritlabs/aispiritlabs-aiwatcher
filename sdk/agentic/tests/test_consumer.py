from __future__ import annotations

from collections.abc import Sequence

import pytest

from aiwatcher_agentic.workflow.consumer import ConsumerConfig, MessageConsumer
from aiwatcher_agentic.workflow.errors import StepLimitExceeded
from aiwatcher_agentic.workflow.message_stream import InMemoryMessageStream
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Event,
    Message,
    RecordedMessageMetadata,
    UserMessage,
)
from aiwatcher_agentic.workflow.reactor import Reactor

# --- Fakes ---


class FakeReactor:
    """Reactor that returns a canned AssistantMessage."""

    def __init__(self, response_text: str = "done") -> None:
        self._response_text = response_text
        self.invocations: list[Message] = []

    def can_handle(self, command: Message) -> bool:
        return True

    def invoke(self, command: Message) -> Message:
        self.invocations.append(command)
        return AssistantMessage(
            data=ConversationData(role="assistant", text=self._response_text),
            metadata=RecordedMessageMetadata(
                domain=command.metadata.domain,
                source="fake-reactor",
            ),
        )


class FailingReactor:
    """Reactor that fails N times, then succeeds."""

    def __init__(self, failures: int, error_type: type[Exception] = ConnectionError) -> None:
        self._failures = failures
        self._error_type = error_type
        self._call_count = 0

    def can_handle(self, command: Message) -> bool:
        return True

    def invoke(self, command: Message) -> Message:
        self._call_count += 1
        if self._call_count <= self._failures:
            raise self._error_type(f"fail #{self._call_count}")
        return AssistantMessage(
            data=ConversationData(role="assistant", text="recovered"),
            metadata=RecordedMessageMetadata(source="fake"),
        )


# --- Helpers ---


def _user(text: str) -> UserMessage:
    return UserMessage(data=ConversationData(role="user", text=text))


def _event(name: str) -> Event:
    return Event(type=name)


def _noop_decider(msg: Message) -> Sequence[Message]:
    """Message router that produces no commands and terminates immediately."""
    return []


def _echo_decider(msg: Message) -> Sequence[Message]:
    """Message router that echoes UserMessage as an Event command and ignores the rest."""
    if isinstance(msg, UserMessage):
        return [
            Event(
                type="process",
                data={"text": msg.data.text},
                metadata=RecordedMessageMetadata(domain=msg.metadata.domain),
            )
        ]
    return []


def _noop_routing(command: Message) -> Reactor | None:
    return None


# --- Tests ---


class TestMessageConsumerBasicFlow:
    def test_empty_stream_does_nothing(self) -> None:
        stream = InMemoryMessageStream()
        consumer = MessageConsumer()
        consumer.consume(stream, _noop_decider, _noop_routing)

        assert stream.is_empty()
        assert len(stream.all_messages()) == 0

    def test_single_message_no_commands(self) -> None:
        stream = InMemoryMessageStream()
        stream.append(_user("hello"))
        consumer = MessageConsumer()

        consumer.consume(stream, _noop_decider, _noop_routing)

        assert stream.is_empty()
        assert len(stream.all_messages()) == 1  # only the original

    def test_decider_produces_command_no_reactor(self) -> None:
        stream = InMemoryMessageStream()
        stream.append(_user("hello"))
        consumer = MessageConsumer()

        consumer.consume(stream, _echo_decider, _noop_routing)

        messages = stream.all_messages()
        # original UserMessage + Event command (no reactor output since routing returns None)
        # But the Event goes back to stream → decider sees it → returns [] → done
        assert len(messages) == 2
        assert isinstance(messages[0], UserMessage)
        assert isinstance(messages[1], Event)

    def test_full_cycle_decider_routing_reactor(self) -> None:
        reactor = FakeReactor(response_text="result")
        stream = InMemoryMessageStream()
        stream.append(_user("input"))

        def routing(command: Message) -> Reactor | None:
            if isinstance(command, Event):
                return reactor
            return None

        consumer = MessageConsumer()
        consumer.consume(stream, _echo_decider, routing)

        messages = stream.all_messages()
        # 1. UserMessage("input") — initial
        # 2. AssistantMessage("result") — reactor output (Event command NOT in stream)
        # MessageRouter sees AssistantMessage → [] → done
        assert len(messages) == 2
        assert isinstance(messages[0], UserMessage)
        assert isinstance(messages[1], AssistantMessage)
        assert messages[1].data.text == "result"

    def test_reactor_receives_correct_command(self) -> None:
        reactor = FakeReactor()
        stream = InMemoryMessageStream()
        stream.append(_user("data"))

        def routing(command: Message) -> Reactor | None:
            return reactor if isinstance(command, Event) else None

        consumer = MessageConsumer()
        consumer.consume(stream, _echo_decider, routing)

        assert len(reactor.invocations) == 1
        assert reactor.invocations[0].data == {"text": "data"}


class TestMessageConsumerStepLimit:
    def test_raises_on_step_limit(self) -> None:
        """Message router that always produces a command creates infinite loop → step limit."""

        def infinite_decider(msg: Message) -> Sequence[Message]:
            return [Event(type="loop")]

        config = ConsumerConfig(max_steps=5)
        consumer = MessageConsumer(config=config)
        stream = InMemoryMessageStream()
        stream.append(_user("start"))

        with pytest.raises(StepLimitExceeded) as exc_info:
            consumer.consume(stream, infinite_decider, _noop_routing)

        assert exc_info.value.max_steps == 5


class TestMessageConsumerRetry:
    def test_retries_on_retryable_error(self) -> None:
        reactor = FailingReactor(failures=1, error_type=ConnectionError)
        stream = InMemoryMessageStream()
        stream.append(_user("test"))

        def routing(command: Message) -> Reactor | None:
            return reactor if isinstance(command, Event) else None

        config = ConsumerConfig(max_retries=2)
        consumer = MessageConsumer(config=config)
        consumer.consume(stream, _echo_decider, routing)

        messages = stream.all_messages()
        assert any(isinstance(m, AssistantMessage) and m.data.text == "recovered" for m in messages)

    def test_raises_non_retryable_error(self) -> None:
        reactor = FailingReactor(failures=1, error_type=ValueError)
        stream = InMemoryMessageStream()
        stream.append(_user("test"))

        def routing(command: Message) -> Reactor | None:
            return reactor if isinstance(command, Event) else None

        consumer = MessageConsumer()

        with pytest.raises(ValueError, match="fail #1"):
            consumer.consume(stream, _echo_decider, routing)

    def test_raises_after_max_retries_exceeded(self) -> None:
        reactor = FailingReactor(failures=5, error_type=ConnectionError)
        stream = InMemoryMessageStream()
        stream.append(_user("test"))

        def routing(command: Message) -> Reactor | None:
            return reactor if isinstance(command, Event) else None

        config = ConsumerConfig(max_retries=2)
        consumer = MessageConsumer(config=config)

        with pytest.raises(ConnectionError):
            consumer.consume(stream, _echo_decider, routing)


class TestMessageConsumerMultiStep:
    def test_multi_step_chain(self) -> None:
        """Message router produces a chain: step1 → step2 → done."""
        step_count = 0

        def chain_decider(msg: Message) -> Sequence[Message]:
            nonlocal step_count
            if isinstance(msg, UserMessage):
                return [Event(type="step1")]
            if isinstance(msg, Event) and msg.type == "step1":
                return [Event(type="step2")]
            if isinstance(msg, Event) and msg.type == "step2":
                return [Event(type="done")]
            return []

        stream = InMemoryMessageStream()
        stream.append(_user("start"))

        consumer = MessageConsumer()
        consumer.consume(stream, chain_decider, _noop_routing)

        messages = stream.all_messages()
        names = [m.type for m in messages if isinstance(m, Event)]
        assert names == ["step1", "step2", "done"]

    def test_decider_fans_out_multiple_commands(self) -> None:
        """Message router produces multiple commands from one message."""
        reactor = FakeReactor(response_text="processed")

        def fan_out_decider(msg: Message) -> Sequence[Message]:
            if isinstance(msg, UserMessage):
                return [
                    Event(type="task_a", data={"text": "a"}),
                    Event(type="task_b", data={"text": "b"}),
                ]
            return []

        def routing(command: Message) -> Reactor | None:
            return reactor if isinstance(command, Event) else None

        stream = InMemoryMessageStream()
        stream.append(_user("go"))

        consumer = MessageConsumer()
        consumer.consume(stream, fan_out_decider, routing)

        messages = stream.all_messages()
        # Events routed to reactor → NOT in stream (only reactor outputs are)
        results = [m for m in messages if isinstance(m, AssistantMessage)]
        assert len(results) == 2
        assert reactor.invocations[0].type == "task_a"
        assert reactor.invocations[1].type == "task_b"
