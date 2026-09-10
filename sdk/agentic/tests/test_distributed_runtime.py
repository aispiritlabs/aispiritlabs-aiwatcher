from __future__ import annotations

from dataclasses import dataclass

from aiwatcher_agentic.runtime.distributed.client import DistributedChatClient
from aiwatcher_agentic.runtime.distributed.serialization import (
    deserialize_record,
    register_record_types,
    serialize_record,
)
from aiwatcher_agentic.runtime.distributed.transport import ConsumedRecord
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Event,
    Message,
    RecordedMessageMetadata,
)


@dataclass(frozen=True, slots=True, kw_only=True)
class SearchPlanned(Event):
    kind: str = "search_planned"
    type: str = "search_planned"
    question: str = ""
    queries: tuple[str, ...] = ()
    reply_target: str = "chat"

    def __post_init__(self) -> None:
        normalized_queries = tuple(str(query) for query in self.queries)
        object.__setattr__(self, "queries", normalized_queries)
        if not self.data:
            object.__setattr__(
                self,
                "data",
                {
                    "question": self.question,
                    "queries": list(normalized_queries),
                    "reply_target": self.reply_target,
                },
            )


register_record_types(SearchPlanned)


class _FakeTransport:
    def __init__(self) -> None:
        self.published_messages: list[Message] = []
        self._responses: list[list[ConsumedRecord]] = []

    def reply_address(self, name: str) -> str:
        return name

    def last_message_id(self, target: str) -> str:
        assert target == "chat"
        return "0-0"

    def publish_message(self, message: Message) -> str:
        self.published_messages.append(message)
        self._responses.append(
            [
                ConsumedRecord(
                    stream="chat",
                    entry_id="1-0",
                    record=AssistantMessage(
                        data=ConversationData(role="assistant", text="final answer"),
                        metadata=RecordedMessageMetadata(
                            runtime_id=message.metadata.runtime_id,
                            turn_id=message.metadata.turn_id,
                            domain=message.metadata.domain,
                            source="summary",
                            target="chat",
                        ),
                    ),
                )
            ]
        )
        return "0-1"

    def read_messages(
        self,
        target: str,
        *,
        after_id: str = "0-0",
        block_ms: int = 1_000,
        count: int = 10,
    ) -> list[ConsumedRecord]:
        del after_id, block_ms, count
        assert target == "chat"
        if not self._responses:
            return []
        return self._responses.pop(0)

    def close(self) -> None:
        return None


def test_distributed_chat_client_publishes_user_message_and_returns_assistant_text() -> None:
    transport = _FakeTransport()
    client = DistributedChatClient(
        transport, entry_agent="planner", source="chat", timeout_seconds=1.0
    )

    reply = client.ask("Find fresh info about Redis")

    assert reply == "final answer"
    assert len(transport.published_messages) == 1
    message = transport.published_messages[0]
    assert message.metadata.target == "planner"
    assert message.metadata.source == "chat"
    assert message.data.text == "Find fresh info about Redis"


def test_search_planned_round_trips_through_serializer() -> None:
    message = SearchPlanned(
        question="What changed in Redis 8.6?",
        queries=("Redis 8.6 release notes",),
        reply_target="chat",
        metadata=RecordedMessageMetadata(
            runtime_id="runtime-1",
            turn_id="turn-1",
            domain="lab6",
            source="planner",
            target="search",
        ),
    )

    serialized = serialize_record(message)
    restored = deserialize_record(serialized)

    assert isinstance(restored, SearchPlanned)
    assert restored.question == "What changed in Redis 8.6?"
    assert restored.queries == ("Redis 8.6 release notes",)
