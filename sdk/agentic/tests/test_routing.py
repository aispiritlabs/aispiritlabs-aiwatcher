from __future__ import annotations

from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Event,
    Message,
    UserMessage,
)
from aiwatcher_agentic.workflow.reactor import LLMResponse
from aiwatcher_agentic.workflow.routing import make_llm_routing


class FakeReactor:
    def can_handle(self, command: Message) -> bool:
        return True

    def invoke(self, command: Message) -> Message:
        return AssistantMessage(data=ConversationData(role="assistant", text="ok"))


class TestMakeLLMRouting:
    def test_user_message_routes_to_reactor(self) -> None:
        reactor = FakeReactor()
        routing = make_llm_routing(reactor)
        result = routing(UserMessage(data=ConversationData(role="user", text="hello")))
        assert result is reactor

    def test_assistant_message_returns_none(self) -> None:
        routing = make_llm_routing(FakeReactor())
        assert routing(AssistantMessage(data=ConversationData(role="assistant", text="hi"))) is None

    def test_event_returns_none(self) -> None:
        routing = make_llm_routing(FakeReactor())
        assert routing(Event(type="something")) is None

    def test_llm_response_returns_none(self) -> None:
        routing = make_llm_routing(FakeReactor())
        assert routing(LLMResponse(data=ConversationData(role="assistant", text="done"))) is None

    def test_base_message_returns_none(self) -> None:
        routing = make_llm_routing(FakeReactor())
        assert routing(Message()) is None
