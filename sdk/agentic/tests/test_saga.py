from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass

from aiwatcher_agentic.workflow.event_store import InMemoryEventStore
from aiwatcher_agentic.workflow.messages import Event, Message, RecordedMessageMetadata, UserCommand
from aiwatcher_agentic.workflow.saga import (
    DurableSagaCoordinator,
    Saga,
    SagaAction,
    SagaStep,
    replay_saga,
    run_saga_step,
)

# ---------------------------------------------------------------------------
# Domain: Order Fulfillment Saga
#
# Coordinates: OrderPlaced → ReserveInventory → ChargePayment → ShipOrder
# Compensates: PaymentFailed → ReleaseInventory
# ---------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class OrderSagaState:
    status: str = "idle"
    order_id: str = ""
    inventory_reserved: bool = False
    payment_charged: bool = False


def _event(event_type: str, **data: object) -> Event:
    return Event(type=event_type, data=dict(data))


def _command(cmd_type: str, **data: object) -> UserCommand:
    return UserCommand(type=cmd_type, data=dict(data))


def order_evolve(state: OrderSagaState, event: Event) -> OrderSagaState:
    match event.type:
        case "order_placed":
            return OrderSagaState(
                status="awaiting_inventory",
                order_id=event.data.get("order_id", ""),
            )
        case "inventory_reserved":
            return OrderSagaState(
                status="awaiting_payment",
                order_id=state.order_id,
                inventory_reserved=True,
            )
        case "payment_charged":
            return OrderSagaState(
                status="awaiting_shipment",
                order_id=state.order_id,
                inventory_reserved=True,
                payment_charged=True,
            )
        case "payment_failed":
            return OrderSagaState(
                status="compensating",
                order_id=state.order_id,
                inventory_reserved=state.inventory_reserved,
                payment_charged=False,
            )
        case "inventory_released":
            return OrderSagaState(
                status="cancelled",
                order_id=state.order_id,
                inventory_reserved=False,
            )
        case "order_shipped":
            return OrderSagaState(
                status="completed",
                order_id=state.order_id,
                inventory_reserved=True,
                payment_charged=True,
            )
        case _:
            return state


def order_decide(event: Event, state: OrderSagaState) -> Sequence[SagaStep[UserCommand | Event]]:
    match state.status:
        case "awaiting_inventory":
            return [
                SagaStep(
                    action=SagaAction.SENT,
                    message=_command("reserve_inventory", order_id=state.order_id),
                )
            ]
        case "awaiting_payment":
            return [
                SagaStep(
                    action=SagaAction.SENT,
                    message=_command("charge_payment", order_id=state.order_id),
                )
            ]
        case "awaiting_shipment":
            return [
                SagaStep(
                    action=SagaAction.SENT,
                    message=_command("ship_order", order_id=state.order_id),
                )
            ]
        case "compensating":
            if state.inventory_reserved:
                return [
                    SagaStep(
                        action=SagaAction.COMPENSATED,
                        message=_command("release_inventory", order_id=state.order_id),
                    )
                ]
            return []
        case _:
            return []


order_saga: Saga[Event, OrderSagaState, UserCommand | Event] = Saga(
    decide=order_decide,
    evolve=order_evolve,
    initial_state=OrderSagaState,
)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestSagaStructure:
    def test_initial_state(self) -> None:
        state = order_saga.initial_state()
        assert state.status == "idle"

    def test_evolve_transitions_state(self) -> None:
        state = order_saga.evolve(OrderSagaState(), _event("order_placed", order_id="o-1"))
        assert state.status == "awaiting_inventory"
        assert state.order_id == "o-1"


class TestRunSagaStep:
    def test_order_placed_sends_reserve_inventory(self) -> None:
        result = run_saga_step(
            order_saga,
            OrderSagaState(),
            _event("order_placed", order_id="o-1"),
        )

        assert result.new_state.status == "awaiting_inventory"
        assert len(result.steps) == 1
        assert result.steps[0].action == SagaAction.SENT
        assert result.steps[0].message.type == "reserve_inventory"

    def test_inventory_reserved_sends_charge_payment(self) -> None:
        state = OrderSagaState(status="awaiting_inventory", order_id="o-1")

        result = run_saga_step(order_saga, state, _event("inventory_reserved"))

        assert result.new_state.status == "awaiting_payment"
        assert result.steps[0].message.type == "charge_payment"

    def test_payment_charged_sends_ship_order(self) -> None:
        state = OrderSagaState(
            status="awaiting_payment",
            order_id="o-1",
            inventory_reserved=True,
        )

        result = run_saga_step(order_saga, state, _event("payment_charged"))

        assert result.new_state.status == "awaiting_shipment"
        assert result.steps[0].message.type == "ship_order"

    def test_payment_failed_compensates_inventory(self) -> None:
        state = OrderSagaState(
            status="awaiting_payment",
            order_id="o-1",
            inventory_reserved=True,
        )

        result = run_saga_step(order_saga, state, _event("payment_failed"))

        assert result.new_state.status == "compensating"
        assert len(result.steps) == 1
        assert result.steps[0].action == SagaAction.COMPENSATED
        assert result.steps[0].message.type == "release_inventory"

    def test_completed_saga_emits_nothing(self) -> None:
        state = OrderSagaState(
            status="awaiting_shipment",
            order_id="o-1",
            inventory_reserved=True,
            payment_charged=True,
        )

        result = run_saga_step(order_saga, state, _event("order_shipped"))

        assert result.new_state.status == "completed"
        assert result.steps == ()


class TestReplaySaga:
    def test_replay_full_happy_path(self) -> None:
        events = [
            _event("order_placed", order_id="o-1"),
            _event("inventory_reserved"),
            _event("payment_charged"),
            _event("order_shipped"),
        ]

        state = replay_saga(order_saga, events)

        assert state.status == "completed"
        assert state.order_id == "o-1"
        assert state.inventory_reserved is True
        assert state.payment_charged is True

    def test_replay_compensation_path(self) -> None:
        events = [
            _event("order_placed", order_id="o-1"),
            _event("inventory_reserved"),
            _event("payment_failed"),
            _event("inventory_released"),
        ]

        state = replay_saga(order_saga, events)

        assert state.status == "cancelled"
        assert state.inventory_reserved is False

    def test_replay_empty_events(self) -> None:
        state = replay_saga(order_saga, [])
        assert state.status == "idle"


class TestDurableSagaCoordinator:
    @staticmethod
    def coordinator(store: InMemoryEventStore) -> DurableSagaCoordinator[OrderSagaState]:
        durable_saga: Saga[Message, OrderSagaState, Message] = Saga(
            decide=lambda event, state: order_decide(event, state),  # type: ignore[arg-type, return-value]
            evolve=lambda state, event: order_evolve(state, event),  # type: ignore[arg-type]
            initial_state=OrderSagaState,
        )
        return DurableSagaCoordinator(
            name="orders",
            saga=durable_saga,
            event_store=store,
        )

    def test_persists_inbox_and_outbox_and_replays_duplicate(self) -> None:
        store = InMemoryEventStore()
        coordinator = self.coordinator(store)
        incoming = Event(
            type="order_placed",
            data={"order_id": "o-1"},
            metadata=RecordedMessageMetadata(message_id="input-1"),
        )

        first = coordinator.handle("o-1", incoming)
        duplicate = coordinator.handle("o-1", incoming)

        assert first.duplicate is False
        assert duplicate.duplicate is True
        assert [message.type for message in duplicate.outputs] == ["reserve_inventory"]
        assert duplicate.outputs[0].metadata.causation_id == "input-1"
        assert len(store.read_stream("saga:orders:o-1").events) == 2
        assert coordinator.state("o-1").status == "awaiting_inventory"

    def test_timeout_is_due_once_and_uses_deterministic_inbox_identity(self) -> None:
        store = InMemoryEventStore()
        coordinator = self.coordinator(store)
        coordinator.schedule_timeout(
            "o-1",
            timeout_id="payment",
            due_at_ns=100,
            data={"order_id": "o-1"},
        )

        assert coordinator.fire_timeout("o-1", timeout_id="payment", now_ns=99) is None
        fired = coordinator.fire_timeout("o-1", timeout_id="payment", now_ns=100)
        duplicate = coordinator.fire_timeout("o-1", timeout_id="payment", now_ns=101)

        assert fired is not None
        assert fired.duplicate is False
        assert duplicate is None
        events = store.read_stream("saga:orders:o-1").events
        timeout_inputs = [event for event in events if event.type == "saga.timeout_fired"]
        assert len(timeout_inputs) == 1
        metadata = timeout_inputs[0].metadata
        assert isinstance(metadata, RecordedMessageMetadata)
        assert metadata.message_id == "saga-timeout:orders:o-1:payment"
