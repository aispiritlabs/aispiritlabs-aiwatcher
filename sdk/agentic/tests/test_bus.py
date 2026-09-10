from __future__ import annotations

import pytest

from aiwatcher_agentic.workflow.bus import InMemoryCommandBus, InMemoryEventBus
from aiwatcher_agentic.workflow.messages import Event, UserCommand


def _command(cmd_type: str, **data: object) -> UserCommand:
    return UserCommand(type=cmd_type, data=dict(data))


def _event(event_type: str, **data: object) -> Event:
    return Event(type=event_type, data=dict(data))


class TestInMemoryCommandBus:
    def test_send_dispatches_to_registered_handler(self) -> None:
        bus = InMemoryCommandBus()
        received: list[UserCommand] = []
        bus.register("create_cart", lambda cmd: received.append(cmd))

        cmd = _command("create_cart", customer="alice")
        bus.send(cmd)

        assert len(received) == 1
        assert received[0].data["customer"] == "alice"

    def test_send_raises_when_no_handler(self) -> None:
        bus = InMemoryCommandBus()

        with pytest.raises(ValueError, match="No handler registered"):
            bus.send(_command("unknown"))

    def test_register_duplicate_raises(self) -> None:
        bus = InMemoryCommandBus()
        bus.register("create_cart", lambda cmd: None)

        with pytest.raises(ValueError, match="already has a registered handler"):
            bus.register("create_cart", lambda cmd: None)

    def test_different_command_types_route_correctly(self) -> None:
        bus = InMemoryCommandBus()
        create_calls: list[str] = []
        delete_calls: list[str] = []

        bus.register("create", lambda cmd: create_calls.append(cmd.type))
        bus.register("delete", lambda cmd: delete_calls.append(cmd.type))

        bus.send(_command("create"))
        bus.send(_command("delete"))
        bus.send(_command("create"))

        assert create_calls == ["create", "create"]
        assert delete_calls == ["delete"]


class TestInMemoryEventBus:
    def test_publish_fans_out_to_all_subscribers(self) -> None:
        bus = InMemoryEventBus()
        calls_a: list[str] = []
        calls_b: list[str] = []

        bus.subscribe("order_placed", lambda e: calls_a.append(e.type))
        bus.subscribe("order_placed", lambda e: calls_b.append(e.type))

        bus.publish(_event("order_placed"))

        assert calls_a == ["order_placed"]
        assert calls_b == ["order_placed"]

    def test_publish_with_no_subscribers_is_silent(self) -> None:
        bus = InMemoryEventBus()
        # Should not raise
        bus.publish(_event("unhandled"))

    def test_subscriber_failure_does_not_block_others(self) -> None:
        bus = InMemoryEventBus()
        calls: list[str] = []

        bus.subscribe("evt", lambda e: (_ for _ in ()).throw(RuntimeError("boom")))
        bus.subscribe("evt", lambda e: calls.append("ok"))

        bus.publish(_event("evt"))

        assert calls == ["ok"]

    def test_publish_many(self) -> None:
        bus = InMemoryEventBus()
        received: list[str] = []
        bus.subscribe("a", lambda e: received.append("a"))
        bus.subscribe("b", lambda e: received.append("b"))

        bus.publish_many([_event("a"), _event("b"), _event("a")])

        assert received == ["a", "b", "a"]

    def test_different_event_types_isolated(self) -> None:
        bus = InMemoryEventBus()
        placed: list[str] = []
        shipped: list[str] = []

        bus.subscribe("placed", lambda e: placed.append(e.type))
        bus.subscribe("shipped", lambda e: shipped.append(e.type))

        bus.publish(_event("placed"))

        assert placed == ["placed"]
        assert shipped == []
