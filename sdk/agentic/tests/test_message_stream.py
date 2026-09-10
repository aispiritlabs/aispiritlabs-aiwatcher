from __future__ import annotations

from collections.abc import Sequence

from aiwatcher_agentic.workflow.message_stream import InMemoryMessageStream, project
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Message,
    UserMessage,
)


def _user(text: str) -> UserMessage:
    return UserMessage(data=ConversationData(role="user", text=text))


def _assistant(text: str) -> AssistantMessage:
    return AssistantMessage(data=ConversationData(role="assistant", text=text))


class TestInMemoryMessageStream:
    def test_empty_stream_returns_none(self) -> None:
        stream = InMemoryMessageStream()
        assert stream.read_next() is None

    def test_is_empty_when_new(self) -> None:
        stream = InMemoryMessageStream()
        assert stream.is_empty() is True

    def test_append_and_read_single(self) -> None:
        stream = InMemoryMessageStream()
        msg = _user("hello")
        stream.append(msg)

        assert stream.is_empty() is False
        assert stream.read_next() == msg
        assert stream.is_empty() is True

    def test_fifo_order(self) -> None:
        stream = InMemoryMessageStream()
        first = _user("first")
        second = _user("second")
        third = _user("third")

        stream.append(first)
        stream.append(second)
        stream.append(third)

        assert stream.read_next() == first
        assert stream.read_next() == second
        assert stream.read_next() == third
        assert stream.read_next() is None

    def test_read_next_returns_none_after_drain(self) -> None:
        stream = InMemoryMessageStream()
        stream.append(_user("one"))
        stream.read_next()

        assert stream.read_next() is None
        assert stream.is_empty() is True

    def test_all_messages_returns_full_history(self) -> None:
        stream = InMemoryMessageStream()
        first = _user("first")
        second = _assistant("response")

        stream.append(first)
        stream.append(second)
        stream.read_next()  # consume first

        history = stream.all_messages()
        assert len(history) == 2
        assert history[0] == first
        assert history[1] == second

    def test_all_messages_is_independent_of_read_position(self) -> None:
        stream = InMemoryMessageStream()
        stream.append(_user("a"))
        stream.append(_user("b"))

        stream.read_next()
        stream.read_next()

        assert len(stream.all_messages()) == 2

    def test_append_after_drain_works(self) -> None:
        stream = InMemoryMessageStream()
        stream.append(_user("first"))
        stream.read_next()
        assert stream.is_empty() is True

        stream.append(_user("second"))
        assert stream.is_empty() is False
        result = stream.read_next()
        assert result is not None
        assert result.data.text == "second"


class TestProject:
    def test_project_conversation_context(self) -> None:
        stream = InMemoryMessageStream()
        stream.append(_user("hello"))
        stream.append(_assistant("hi"))
        stream.append(_user("how are you"))

        def conversation_context(messages: Sequence[Message]) -> list[dict[str, str]]:
            return [
                {"role": m.data.role, "text": m.data.text or ""}
                for m in messages
                if hasattr(m.data, "role") and m.data.role
            ]

        result = project(stream, conversation_context)
        assert len(result) == 3
        assert result[0] == {"role": "user", "text": "hello"}
        assert result[1] == {"role": "assistant", "text": "hi"}
        assert result[2] == {"role": "user", "text": "how are you"}

    def test_project_message_count(self) -> None:
        stream = InMemoryMessageStream()
        stream.append(_user("a"))
        stream.append(_user("b"))

        count = project(stream, lambda msgs: len(msgs))
        assert count == 2

    def test_project_empty_stream(self) -> None:
        stream = InMemoryMessageStream()
        result = project(stream, lambda msgs: list(msgs))
        assert result == []
