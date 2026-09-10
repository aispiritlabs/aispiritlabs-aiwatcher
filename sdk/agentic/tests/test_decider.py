from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass

import pytest

from aiwatcher_agentic.workflow.decider import (
    Decider,
    DeciderSpecification,
    handle_command,
)
from aiwatcher_agentic.workflow.errors import ConcurrencyConflictError, IllegalStateError
from aiwatcher_agentic.workflow.event_store import (
    InMemoryEventStore,
)
from aiwatcher_agentic.workflow.messages import Event

# ---------------------------------------------------------------------------
# Domain model: Shopping Cart
# ---------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class EmptyCart:
    status: str = "empty"


@dataclass(frozen=True, slots=True)
class OpenedCart:
    status: str = "opened"
    items: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class ClosedCart:
    status: str = "closed"


type CartState = EmptyCart | OpenedCart | ClosedCart


# --- Commands (plain frozen dataclasses) ---


@dataclass(frozen=True, slots=True)
class AddItem:
    cart_id: str
    item: str


@dataclass(frozen=True, slots=True)
class RemoveItem:
    cart_id: str
    item: str


@dataclass(frozen=True, slots=True)
class ConfirmCart:
    cart_id: str


# --- Events ---


def _event(event_type: str, **data: object) -> Event:
    return Event(type=event_type, data=dict(data))


# --- Evolve ---


def evolve(state: CartState, event: Event) -> CartState:
    match event.type:
        case "item_added":
            items = state.items if isinstance(state, OpenedCart) else ()
            return OpenedCart(items=(*items, event.data["item"]))
        case "item_removed":
            if not isinstance(state, OpenedCart):
                return state
            remaining = tuple(i for i in state.items if i != event.data["item"])
            return OpenedCart(items=remaining)
        case "cart_confirmed":
            return ClosedCart()
        case _:
            return state


# --- Decide ---


def decide(command: AddItem | RemoveItem | ConfirmCart, state: CartState) -> Sequence[Event]:
    match command:
        case AddItem(item=item):
            if isinstance(state, ClosedCart):
                raise IllegalStateError("Cannot add items to a closed cart")
            return [_event("item_added", item=item)]
        case RemoveItem(item=item):
            if not isinstance(state, OpenedCart):
                raise IllegalStateError("Cannot remove items from a non-opened cart")
            if item not in state.items:
                raise IllegalStateError(f"Item '{item}' not in cart")
            return [_event("item_removed", item=item)]
        case ConfirmCart():
            if not isinstance(state, OpenedCart):
                raise IllegalStateError("Cannot confirm a non-opened cart")
            return [_event("cart_confirmed")]


cart_decider: Decider[
    AddItem | RemoveItem | ConfirmCart,
    CartState,
    Event,
] = Decider(
    decide=decide,
    evolve=evolve,
    initial_state=EmptyCart,
)


# ---------------------------------------------------------------------------
# Tests: Decider structure
# ---------------------------------------------------------------------------


class TestDecider:
    def test_decide_returns_events(self) -> None:
        events = cart_decider.decide(AddItem("c1", "shoes"), EmptyCart())
        assert len(events) == 1
        assert events[0].type == "item_added"

    def test_evolve_transitions_state(self) -> None:
        state = cart_decider.evolve(EmptyCart(), _event("item_added", item="shoes"))
        assert isinstance(state, OpenedCart)
        assert state.items == ("shoes",)

    def test_initial_state_is_empty(self) -> None:
        assert isinstance(cart_decider.initial_state(), EmptyCart)

    def test_decide_rejects_invalid_transition(self) -> None:
        with pytest.raises(IllegalStateError, match="closed cart"):
            cart_decider.decide(AddItem("c1", "shoes"), ClosedCart())


# ---------------------------------------------------------------------------
# Tests: handle_command (full event sourcing loop)
# ---------------------------------------------------------------------------


class TestHandleCommand:
    def test_first_command_creates_stream(self) -> None:
        store = InMemoryEventStore()

        result = handle_command(store, "cart-1", cart_decider, AddItem("c1", "shoes"))

        assert len(result.new_events) == 1
        assert result.new_events[0].type == "item_added"
        assert isinstance(result.new_state, OpenedCart)
        assert result.new_state.items == ("shoes",)
        assert result.next_version == 1

    def test_subsequent_commands_accumulate_state(self) -> None:
        store = InMemoryEventStore()

        handle_command(store, "cart-1", cart_decider, AddItem("c1", "shoes"))
        result = handle_command(store, "cart-1", cart_decider, AddItem("c1", "hat"))

        assert isinstance(result.new_state, OpenedCart)
        assert result.new_state.items == ("shoes", "hat")
        assert result.next_version == 2

    def test_confirm_closes_cart(self) -> None:
        store = InMemoryEventStore()

        handle_command(store, "cart-1", cart_decider, AddItem("c1", "shoes"))
        result = handle_command(store, "cart-1", cart_decider, ConfirmCart("c1"))

        assert isinstance(result.new_state, ClosedCart)
        assert result.next_version == 2

    def test_rejects_add_to_closed_cart(self) -> None:
        store = InMemoryEventStore()

        handle_command(store, "cart-1", cart_decider, AddItem("c1", "shoes"))
        handle_command(store, "cart-1", cart_decider, ConfirmCart("c1"))

        with pytest.raises(IllegalStateError, match="closed cart"):
            handle_command(store, "cart-1", cart_decider, AddItem("c1", "hat"))

    def test_remove_item(self) -> None:
        store = InMemoryEventStore()

        handle_command(store, "cart-1", cart_decider, AddItem("c1", "shoes"))
        handle_command(store, "cart-1", cart_decider, AddItem("c1", "hat"))
        result = handle_command(store, "cart-1", cart_decider, RemoveItem("c1", "shoes"))

        assert isinstance(result.new_state, OpenedCart)
        assert result.new_state.items == ("hat",)

    def test_remove_nonexistent_item_raises(self) -> None:
        store = InMemoryEventStore()

        handle_command(store, "cart-1", cart_decider, AddItem("c1", "shoes"))

        with pytest.raises(IllegalStateError, match="not in cart"):
            handle_command(store, "cart-1", cart_decider, RemoveItem("c1", "hat"))

    def test_streams_are_isolated(self) -> None:
        store = InMemoryEventStore()

        handle_command(store, "cart-alice", cart_decider, AddItem("a", "shoes"))
        handle_command(store, "cart-bob", cart_decider, AddItem("b", "hat"))

        r_alice = handle_command(store, "cart-alice", cart_decider, AddItem("a", "jacket"))
        r_bob = handle_command(store, "cart-bob", cart_decider, AddItem("b", "scarf"))

        assert r_alice.new_state.items == ("shoes", "jacket")  # type: ignore[union-attr]
        assert r_bob.new_state.items == ("hat", "scarf")  # type: ignore[union-attr]

    def test_returns_unchanged_state_when_no_events_produced(self) -> None:
        store = InMemoryEventStore()

        noop_decider: Decider[str, list[str], Event] = Decider(
            decide=lambda cmd, state: [],
            evolve=lambda s, e: s,
            initial_state=list,
        )

        result = handle_command(store, "s-1", noop_decider, "noop")
        assert result.new_events == ()
        assert result.new_state == []
        assert result.next_version == 0


class TestHandleCommandConcurrencyRetry:
    def test_retries_on_concurrency_conflict(self) -> None:
        store = InMemoryEventStore()
        handle_command(store, "cart-1", cart_decider, AddItem("c1", "shoes"))

        # Simulate a concurrent write between load and append
        call_count = 0
        original_append = store.append_to_stream

        def intercepting_append(stream_name, events, *, expected_version=None):  # type: ignore[no-untyped-def]
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                # Simulate another writer sneaking in
                original_append(stream_name, [_event("item_added", item="sneaky")])
            return original_append(stream_name, events, expected_version=expected_version)

        store.append_to_stream = intercepting_append  # type: ignore[method-assign]

        result = handle_command(store, "cart-1", cart_decider, AddItem("c1", "hat"))

        # Should succeed on retry: shoes + sneaky + hat
        assert isinstance(result.new_state, OpenedCart)
        assert "hat" in result.new_state.items

    def test_raises_after_max_retries_exhausted(self) -> None:
        store = InMemoryEventStore()
        handle_command(store, "cart-1", cart_decider, AddItem("c1", "shoes"))

        original_append = store.append_to_stream

        def always_conflict(stream_name, events, *, expected_version=None):  # type: ignore[no-untyped-def]
            # Always sneak in a write before the real append
            original_append(stream_name, [_event("item_added", item="sneaky")])
            return original_append(stream_name, events, expected_version=expected_version)

        store.append_to_stream = always_conflict  # type: ignore[method-assign]

        with pytest.raises(ConcurrencyConflictError):
            handle_command(store, "cart-1", cart_decider, AddItem("c1", "hat"), max_retries=2)


# ---------------------------------------------------------------------------
# Tests: DeciderSpecification (BDD)
# ---------------------------------------------------------------------------


class TestDeciderSpecification:
    def setup_method(self) -> None:
        self.spec = DeciderSpecification.for_decider(cart_decider)

    def test_given_empty_when_add_then_item_added(self) -> None:
        self.spec.given().when(AddItem("c1", "shoes")).then(_event("item_added", item="shoes"))

    def test_given_item_when_confirm_then_confirmed(self) -> None:
        self.spec.given(
            _event("item_added", item="shoes"),
        ).when(ConfirmCart("c1")).then(_event("cart_confirmed"))

    def test_given_confirmed_when_add_then_throws(self) -> None:
        self.spec.given(
            _event("item_added", item="shoes"),
            _event("cart_confirmed"),
        ).when(AddItem("c1", "hat")).then_throws(IllegalStateError, match="closed cart")

    def test_given_empty_when_confirm_then_throws(self) -> None:
        self.spec.given().when(ConfirmCart("c1")).then_throws(IllegalStateError, match="non-opened")

    def test_given_items_when_remove_then_removed(self) -> None:
        self.spec.given(
            _event("item_added", item="shoes"),
            _event("item_added", item="hat"),
        ).when(RemoveItem("c1", "shoes")).then(_event("item_removed", item="shoes"))

    def test_given_empty_when_remove_then_throws(self) -> None:
        self.spec.given().when(RemoveItem("c1", "shoes")).then_throws(
            IllegalStateError, match="non-opened"
        )
