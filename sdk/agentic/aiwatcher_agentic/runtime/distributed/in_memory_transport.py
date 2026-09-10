"""In-memory transport with consumer-group semantics, for testing without a broker."""

from __future__ import annotations

import threading
import time

from aiwatcher_agentic.runtime.distributed.contracts import AgentHeartbeat, AgentRegistration
from aiwatcher_agentic.runtime.distributed.registry import AgentSnapshot
from aiwatcher_agentic.runtime.distributed.serialization import deserialize_record, serialize_record
from aiwatcher_agentic.runtime.distributed.transport import (
    ConsumedRecord,
    MalformedRecord,
    normalize_distributed_message,
)
from aiwatcher_agentic.workflow.messages import Message


class InMemoryTransport:
    """Thread-safe in-memory transport that mimics the durable one.

    Supports consumer groups, pending entries, and reclaiming what an idle
    consumer never acked — enough to run ``DistributedService`` with no server.
    Entry ids keep the two-part shape Redis Streams gave them.
    """

    def __init__(self, *, prefix: str = "test") -> None:
        self._prefix = prefix
        self._lock = threading.Lock()
        self._condition = threading.Condition(self._lock)
        self._entry_counter = 0
        # stream_name -> [(entry_id, {"payload": serialized})]
        self._streams: dict[str, list[tuple[str, dict[str, str]]]] = {}
        # stream_name -> {group -> set of acked entry_ids}
        self._acked: dict[str, dict[str, set[str]]] = {}
        # stream_name -> {group -> {consumer -> set of delivered entry_ids}}
        self._delivered: dict[str, dict[str, dict[str, set[str]]]] = {}
        # stream_name -> {group -> {entry_id -> delivery_time_ns}}
        self._delivery_times: dict[str, dict[str, dict[str, int]]] = {}
        self._dead_letters: list[dict[str, object]] = []
        self._closed = False

    @property
    def prefix(self) -> str:
        return self._prefix

    @property
    def dead_letters(self) -> tuple[dict[str, object], ...]:
        with self._lock:
            return tuple(dict(item) for item in self._dead_letters)

    def reply_address(self, name: str) -> str:
        """Every address is a topic here, a client's included."""
        return name

    def message_stream(self, target: str) -> str:
        resolved_target = target.strip()
        if not resolved_target:
            raise ValueError("Distributed message target must not be empty")
        return f"{self._prefix}:messages:{resolved_target}"

    def publish_message(self, message: Message) -> str:
        normalized = normalize_distributed_message(message)
        if not normalized.metadata.target:
            raise ValueError("Distributed messages must have a target")
        serialized = serialize_record(normalized)
        stream = self.message_stream(normalized.metadata.target)
        with self._condition:
            self._entry_counter += 1
            entry_id = f"{self._entry_counter}-0"
            self._streams.setdefault(stream, []).append((entry_id, {"payload": serialized}))
            self._condition.notify_all()
            return entry_id

    def last_message_id(self, target: str) -> str:
        stream = self.message_stream(target)
        with self._lock:
            entries = self._streams.get(stream, [])
            if not entries:
                return "0-0"
            return entries[-1][0]

    def read_messages(
        self,
        target: str,
        *,
        after_id: str = "0-0",
        block_ms: int = 1_000,
        count: int = 10,
    ) -> list[ConsumedRecord]:
        stream = self.message_stream(target)
        after_seq = self._parse_entry_seq(after_id)
        deadline = time.monotonic() + block_ms / 1000.0

        with self._condition:
            while True:
                entries = self._streams.get(stream, [])
                result: list[ConsumedRecord] = []
                for entry_id, payload in entries:
                    if self._parse_entry_seq(entry_id) <= after_seq:
                        continue
                    serialized = payload.get("payload")
                    if not isinstance(serialized, str):
                        continue
                    result.append(
                        ConsumedRecord(
                            stream=stream,
                            entry_id=entry_id,
                            record=self._deserialize_record(serialized),
                        )
                    )
                    if len(result) >= count:
                        break
                if result:
                    return result
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return []
                self._condition.wait(timeout=remaining)

    def ensure_consumer_group(self, target: str, group: str) -> None:
        stream = self.message_stream(target)
        with self._lock:
            self._streams.setdefault(stream, [])
            self._acked.setdefault(stream, {}).setdefault(group, set())
            self._delivered.setdefault(stream, {}).setdefault(group, {})
            self._delivery_times.setdefault(stream, {}).setdefault(group, {})

    def consume_target(
        self,
        target: str,
        *,
        group: str,
        consumer: str,
        block_ms: int = 1_000,
        count: int = 10,
    ) -> list[ConsumedRecord]:
        """Return undelivered messages, mimicking xreadgroup with '>'."""
        self.ensure_consumer_group(target, group)
        stream = self.message_stream(target)
        deadline = time.monotonic() + block_ms / 1000.0

        with self._condition:
            while True:
                entries = self._streams.get(stream, [])
                acked = self._acked[stream][group]
                all_delivered = self._delivered[stream][group]
                all_delivered_ids: set[str] = set()
                for consumer_ids in all_delivered.values():
                    all_delivered_ids |= consumer_ids

                result: list[ConsumedRecord] = []
                now_ns = time.time_ns()
                for entry_id, payload in entries:
                    if entry_id in acked or entry_id in all_delivered_ids:
                        continue
                    serialized = payload.get("payload")
                    if not isinstance(serialized, str):
                        continue
                    all_delivered.setdefault(consumer, set()).add(entry_id)
                    self._delivery_times[stream][group][entry_id] = now_ns
                    result.append(
                        ConsumedRecord(
                            stream=stream,
                            entry_id=entry_id,
                            record=self._deserialize_record(serialized),
                        )
                    )
                    if len(result) >= count:
                        break
                if result:
                    return result
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return []
                self._condition.wait(timeout=remaining)

    def autoclaim_pending(
        self,
        target: str,
        *,
        group: str,
        consumer: str,
        min_idle_ms: int = 5_000,
        count: int = 10,
    ) -> list[ConsumedRecord]:
        """Claim pending (delivered but unacked) messages from any consumer."""
        self.ensure_consumer_group(target, group)
        stream = self.message_stream(target)
        min_idle_ns = min_idle_ms * 1_000_000
        now_ns = time.time_ns()

        with self._lock:
            entries_map = dict(self._streams.get(stream, []))
            acked = self._acked[stream][group]
            all_delivered = self._delivered[stream][group]
            delivery_times = self._delivery_times[stream][group]

            pending_ids: list[str] = []
            for delivered_ids in all_delivered.values():
                for entry_id in delivered_ids:
                    if entry_id in acked:
                        continue
                    delivered_at = delivery_times.get(entry_id, 0)
                    if now_ns - delivered_at < min_idle_ns:
                        continue
                    pending_ids.append(entry_id)

            pending_ids = sorted(set(pending_ids), key=lambda eid: self._parse_entry_seq(eid))

            result: list[ConsumedRecord] = []
            for entry_id in pending_ids[:count]:
                # Transfer ownership to this consumer
                for delivered_ids in all_delivered.values():
                    delivered_ids.discard(entry_id)
                all_delivered.setdefault(consumer, set()).add(entry_id)
                delivery_times[entry_id] = now_ns

                payload = entries_map.get(entry_id)
                if payload is None:
                    continue
                serialized = payload.get("payload")
                if not isinstance(serialized, str):
                    continue
                result.append(
                    ConsumedRecord(
                        stream=stream,
                        entry_id=entry_id,
                        record=self._deserialize_record(serialized),
                    )
                )
            return result

    def ack(self, stream: str, group: str, entry_id: str) -> int:
        with self._lock:
            acked = self._acked.get(stream, {}).get(group)
            if acked is None:
                return 0
            if entry_id in acked:
                return 0
            acked.add(entry_id)
            return 1

    def publish_dead_letter(
        self,
        *,
        target: str,
        source_stream: str,
        group: str,
        entry_id: str,
        record: object,
        error: str,
        attempts: int,
    ) -> str:
        with self._lock:
            self._entry_counter += 1
            dead_letter_id = f"{self._entry_counter}-0"
            self._dead_letters.append(
                {
                    "id": dead_letter_id,
                    "target": target,
                    "source_stream": source_stream,
                    "group": group,
                    "entry_id": entry_id,
                    "record": record,
                    "error": error,
                    "attempts": attempts,
                }
            )
            return dead_letter_id

    def close(self) -> None:
        self._closed = True

    @staticmethod
    def _parse_entry_seq(entry_id: str) -> int:
        parts = entry_id.split("-", 1)
        try:
            return int(parts[0])
        except ValueError:
            return 0

    @staticmethod
    def _deserialize_record(serialized: str) -> object:
        try:
            return deserialize_record(serialized)
        except Exception as error:  # noqa: BLE001 - what will not decode is malformed, whatever broke
            return MalformedRecord(
                raw_payload=serialized,
                error_type=type(error).__name__,
                error_message=str(error),
            )


class InMemoryServiceRegistry:
    """In-memory agent registry for testing without a broker."""

    def __init__(self) -> None:
        self._registrations: dict[str, AgentRegistration] = {}
        self._heartbeats: dict[str, AgentHeartbeat] = {}
        self._lock = threading.Lock()

    def register(self, registration: AgentRegistration) -> None:
        with self._lock:
            self._registrations[registration.agent_name] = registration

    def heartbeat(self, heartbeat: AgentHeartbeat) -> None:
        with self._lock:
            self._heartbeats[heartbeat.agent_name] = heartbeat

    def live_agents(self, *, max_age_seconds: float) -> list[AgentSnapshot]:
        now_ns = time.time_ns()
        max_age_ns = int(max_age_seconds * 1_000_000_000)
        with self._lock:
            snapshots: list[AgentSnapshot] = []
            for agent_name, registration in self._registrations.items():
                hb = self._heartbeats.get(agent_name)
                if hb is None:
                    continue
                age_ns = now_ns - hb.emitted_at_ns
                if age_ns > max_age_ns:
                    continue
                snapshots.append(
                    AgentSnapshot(
                        agent_name=registration.agent_name,
                        capabilities=registration.capabilities,
                        role=registration.role,
                        consumer_group=registration.consumer_group,
                        status=hb.status,
                        last_seen_ns=hb.emitted_at_ns,
                    )
                )
            return sorted(snapshots, key=lambda s: s.agent_name)

    def find_by_capability(
        self, capability: str, *, max_age_seconds: float
    ) -> AgentSnapshot | None:
        for agent in self.live_agents(max_age_seconds=max_age_seconds):
            if capability in agent.capabilities:
                return agent
        return None
