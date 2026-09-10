from __future__ import annotations

from aiwatcher_agentic.workflow.types import (
    EventId,
    GlobalPosition,
    MessageId,
    RuntimeId,
    SessionId,
    StreamName,
    TurnId,
)


class TestNominalTypes:
    def test_newtypes_are_runtime_transparent(self) -> None:
        """NewType is zero-cost at runtime — values are just str/int."""
        event_id = EventId("evt-123")
        message_id = MessageId("msg-456")
        session_id = SessionId("sess-789")
        runtime_id = RuntimeId("rt-001")
        turn_id = TurnId("turn-002")
        stream_name = StreamName("cart-alice")

        assert event_id == "evt-123"
        assert message_id == "msg-456"
        assert session_id == "sess-789"
        assert runtime_id == "rt-001"
        assert turn_id == "turn-002"
        assert stream_name == "cart-alice"

    def test_position_types(self) -> None:
        from aiwatcher_agentic.workflow.types import StreamPosition

        sp = StreamPosition(42)
        gp = GlobalPosition(100)

        assert sp == 42
        assert gp == 100
        assert sp + 1 == 43

    def test_newtypes_are_distinct_for_type_checker(self) -> None:
        """At runtime they're equal, but mypy treats them as different types."""
        session_id = SessionId("abc")
        turn_id = TurnId("abc")

        # Runtime: same value
        assert session_id == turn_id  # type: ignore[comparison-overlap]

        # But the type checker should flag: SessionId != TurnId
        # This test documents the intent — mypy enforces it
