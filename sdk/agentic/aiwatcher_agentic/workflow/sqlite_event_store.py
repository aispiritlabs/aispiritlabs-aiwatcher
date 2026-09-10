from __future__ import annotations

import logging
import sqlite3
import time
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Any, Protocol

from aiwatcher_agentic.workflow.errors import DuplicateMessageError
from aiwatcher_agentic.workflow.event_store import (
    NO_CONCURRENCY_CHECK,
    AggregateStreamResult,
    AppendResult,
    ExpectedVersion,
    GlobalPosition,
    InlineProjection,
    ReadAllResult,
    ReadStreamResult,
    StreamPosition,
    Upcaster,
    _apply_upcasters,
    _check_expected_version,
)
from aiwatcher_agentic.workflow.messages import Message, normalize_recorded_message
from aiwatcher_agentic.workflow.processor import CheckpointStore
from aiwatcher_agentic.workflow.serialization import deserialize_record, serialize_record

logger = logging.getLogger(__name__)

_SCHEMA = """
CREATE TABLE IF NOT EXISTS event_streams (
    stream_name TEXT PRIMARY KEY,
    current_version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS event_messages (
    global_position INTEGER PRIMARY KEY,
    stream_name TEXT NOT NULL,
    stream_position INTEGER NOT NULL,
    message_id TEXT,
    event_id TEXT,
    kind TEXT,
    event_type TEXT,
    contract_name TEXT,
    schema_version INTEGER,
    correlation_id TEXT,
    causation_id TEXT,
    idempotency_key TEXT,
    payload_json BLOB NOT NULL,
    created_at_ns INTEGER NOT NULL,
    UNIQUE(stream_name, stream_position)
);

CREATE INDEX IF NOT EXISTS idx_event_messages_stream_position
ON event_messages(stream_name, stream_position);

CREATE TABLE IF NOT EXISTS event_message_ids (
    stream_name TEXT NOT NULL,
    message_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    global_position INTEGER NOT NULL,
    PRIMARY KEY(stream_name, message_id),
    UNIQUE(event_id)
);

CREATE TABLE IF NOT EXISTS event_idempotency_keys (
    stream_name TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    event_id TEXT NOT NULL,
    global_position INTEGER NOT NULL,
    PRIMARY KEY(stream_name, idempotency_key),
    UNIQUE(event_id)
);

CREATE TABLE IF NOT EXISTS processor_checkpoints (
    processor_id TEXT PRIMARY KEY,
    position INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS processor_locks (
    processor_key TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    lease_until_ns INTEGER NOT NULL
);
"""

_EVENT_MESSAGE_COLUMNS = {
    "message_id": "TEXT",
    "event_id": "TEXT",
    "kind": "TEXT",
    "event_type": "TEXT",
    "contract_name": "TEXT",
    "schema_version": "INTEGER",
    "correlation_id": "TEXT",
    "causation_id": "TEXT",
    "idempotency_key": "TEXT",
}


class SQLiteTransactionalProjection(Protocol):
    """Projection that writes through the event store's open SQLite transaction."""

    def project(
        self,
        connection: sqlite3.Connection,
        stream_name: str,
        events: Sequence[Message],
    ) -> None: ...


class EventPayloadCodec(Protocol):
    """Application-supplied encryption/compression boundary for stored payloads."""

    def encode(self, payload: bytes) -> bytes: ...

    def decode(self, payload: bytes) -> bytes: ...


def _default_path() -> Path:
    # Under the working directory, never beside this file. Derived from where the
    # module is installed, this named site-packages in a wheel and the root of
    # the aiwatcher checkout in a source tree — not the application's `data/`
    # it named before the engine moved (AW-2).
    return Path.cwd() / ".data" / "workflow_event_store.sqlite3"


def _resolve_path(path: str | Path | None) -> Path:
    if path is not None:
        return Path(path).expanduser()
    default = _default_path()
    default.parent.mkdir(parents=True, exist_ok=True)
    return default


def _connect(path: Path) -> sqlite3.Connection:
    connection = sqlite3.connect(path)
    connection.execute("PRAGMA journal_mode=WAL;")
    connection.execute("PRAGMA synchronous=NORMAL;")
    connection.execute("PRAGMA busy_timeout=5000;")
    return connection


def _ensure_schema(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with _connect(path) as connection:
        connection.executescript(_SCHEMA)
        existing_columns = {
            str(row[1]) for row in connection.execute("PRAGMA table_info(event_messages)")
        }
        for name, sql_type in _EVENT_MESSAGE_COLUMNS.items():
            if name not in existing_columns:
                connection.execute(f"ALTER TABLE event_messages ADD COLUMN {name} {sql_type}")
        connection.execute(
            """
            CREATE INDEX IF NOT EXISTS idx_event_messages_contract_position
            ON event_messages(contract_name, global_position)
            """
        )
        connection.execute(
            """
            INSERT OR IGNORE INTO event_idempotency_keys (
                stream_name, idempotency_key, event_id, global_position
            )
            SELECT stream_name, message_id, event_id, global_position
            FROM event_message_ids
            """
        )
        connection.commit()


class SQLiteEventStore:
    """SQLite-backed event store for durable workflow and domain streams."""

    def __init__(
        self,
        path: str | Path | None = None,
        *,
        after_commit_hooks: Sequence[Callable[[str, Sequence[Message]], None]] = (),
        inline_projections: Sequence[InlineProjection] = (),
        transactional_projections: Sequence[SQLiteTransactionalProjection] = (),
        upcasters: Sequence[Upcaster] = (),
        payload_codec: EventPayloadCodec | None = None,
    ) -> None:
        self._path = _resolve_path(path)
        _ensure_schema(self._path)
        self._after_commit_hooks = list(after_commit_hooks)
        self._inline_projections = list(inline_projections)
        self._transactional_projections = list(transactional_projections)
        self._upcasters = list(upcasters)
        self._payload_codec = payload_codec

    @property
    def path(self) -> Path:
        return self._path

    def add_after_commit_hook(self, hook: Callable[[str, Sequence[Message]], None]) -> None:
        self._after_commit_hooks.append(hook)

    def add_inline_projection(self, projection: InlineProjection) -> None:
        self._inline_projections.append(projection)

    def add_transactional_projection(self, projection: SQLiteTransactionalProjection) -> None:
        self._transactional_projections.append(projection)

    def add_upcaster(self, upcaster: Upcaster) -> None:
        self._upcasters.append(upcaster)

    def read_stream(
        self,
        stream_name: str,
        *,
        from_position: StreamPosition = 0,
        max_count: int | None = None,
    ) -> ReadStreamResult[Message]:
        with _connect(self._path) as connection:
            version = self._current_version(connection, stream_name)
            stream_exists = version > 0
            if max_count is None:
                rows = connection.execute(
                    """
                    SELECT stream_position, global_position, created_at_ns, payload_json
                    FROM event_messages
                    WHERE stream_name = ? AND stream_position >= ?
                    ORDER BY stream_position
                    """,
                    (stream_name, from_position),
                ).fetchall()
            else:
                rows = connection.execute(
                    """
                    SELECT stream_position, global_position, created_at_ns, payload_json
                    FROM event_messages
                    WHERE stream_name = ? AND stream_position >= ?
                    ORDER BY stream_position
                    LIMIT ?
                    """,
                    (stream_name, from_position, max_count),
                ).fetchall()

        messages = tuple(
            self._bind_positions(
                message=self._deserialize_message(payload_json),
                stream_name=stream_name,
                stream_position=stream_position,
                global_position=global_position,
                recorded_at_ns=created_at_ns,
            )
            for stream_position, global_position, created_at_ns, payload_json in rows
        )
        return ReadStreamResult(
            events=_apply_upcasters(messages, self._upcasters),
            current_version=version,
            stream_exists=stream_exists,
        )

    def read_all(
        self,
        *,
        from_position: GlobalPosition = 0,
        max_count: int | None = None,
        stream_prefix: str | None = None,
    ) -> ReadAllResult[Message]:
        clauses = ["global_position >= ?"]
        parameters: list[object] = [from_position]
        if stream_prefix is not None:
            clauses.append("stream_name LIKE ?")
            parameters.append(f"{stream_prefix}%")
        query = f"""
            SELECT stream_name, stream_position, global_position, created_at_ns, payload_json
            FROM event_messages
            WHERE {" AND ".join(clauses)}
            ORDER BY global_position
        """  # noqa: S608 - the clauses are fixed text; every value is a bound parameter
        if max_count is not None:
            query += " LIMIT ?"
            parameters.append(max_count)

        with _connect(self._path) as connection:
            rows = connection.execute(query, parameters).fetchall()
            end_row = connection.execute(
                "SELECT COALESCE(MAX(global_position), -1) + 1 FROM event_messages"
            ).fetchone()

        messages = tuple(
            self._bind_positions(
                message=self._deserialize_message(payload_json),
                stream_name=stream_name,
                stream_position=stream_position,
                global_position=global_position,
                recorded_at_ns=created_at_ns,
            )
            for stream_name, stream_position, global_position, created_at_ns, payload_json in rows
        )
        end_position = int(end_row[0]) if end_row is not None else 0
        next_position = end_position
        if messages:
            next_position = (messages[-1].metadata.global_position or 0) + 1
        return ReadAllResult(
            events=_apply_upcasters(messages, self._upcasters),
            next_position=next_position,
            end_position=end_position,
        )

    def aggregate_stream(
        self,
        stream_name: str,
        *,
        evolve: Callable[[Any, Any], Any],
        initial_state: Callable[[], Any],
        from_position: StreamPosition = 0,
    ) -> AggregateStreamResult[Any]:
        result = self.read_stream(stream_name, from_position=from_position)
        state = initial_state()
        for message in result.events:
            state = evolve(state, message)
        return AggregateStreamResult(
            state=state,
            current_version=result.current_version,
            stream_exists=result.stream_exists,
        )

    def append_to_stream(
        self,
        stream_name: str,
        events: Sequence[Message],
        *,
        expected_version: ExpectedVersion = NO_CONCURRENCY_CHECK,
    ) -> AppendResult:
        if not events:
            with _connect(self._path) as connection:
                return AppendResult(next_version=self._current_version(connection, stream_name))

        with _connect(self._path) as connection:
            connection.execute("BEGIN IMMEDIATE")
            current_version = self._current_version(connection, stream_name)
            _check_expected_version(stream_name, current_version, expected_version)

            next_global_position = self._next_global_position(connection)
            created_at_ns = time.time_ns()
            recorded = tuple(
                normalize_recorded_message(
                    event,
                    stream_name=stream_name,
                    stream_position=current_version + index,
                    global_position=next_global_position + index,
                    recorded_at_ns=created_at_ns,
                )
                for index, event in enumerate(events)
            )
            serialized = tuple(self._serialize_message(message) for message in recorded)

            try:
                for projection in self._inline_projections:
                    projection.project(stream_name, recorded)
            except Exception:
                connection.rollback()
                raise

            try:
                connection.executemany(
                    """
                    INSERT INTO event_idempotency_keys (
                        stream_name, idempotency_key, event_id, global_position
                    ) VALUES (?, ?, ?, ?)
                    """,
                    [
                        (
                            stream_name,
                            message.metadata.idempotency_key,
                            getattr(message.metadata, "event_id", ""),
                            next_global_position + index,
                        )
                        for index, message in enumerate(recorded)
                        if message.metadata.idempotency_key
                        and getattr(message.metadata, "event_id", "")
                    ],
                )
            except sqlite3.IntegrityError as error:
                connection.rollback()
                raise DuplicateMessageError(stream_name) from error
            connection.executemany(
                """
                INSERT INTO event_messages (
                    global_position,
                    stream_name,
                    stream_position,
                    message_id,
                    event_id,
                    kind,
                    event_type,
                    contract_name,
                    schema_version,
                    correlation_id,
                    causation_id,
                    idempotency_key,
                    payload_json,
                    created_at_ns
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    (
                        next_global_position + index,
                        stream_name,
                        current_version + index,
                        getattr(message.metadata, "message_id", ""),
                        getattr(message.metadata, "event_id", ""),
                        message.kind,
                        message.type,
                        message.metadata.contract_name,
                        message.metadata.schema_version,
                        message.metadata.correlation_id,
                        message.metadata.causation_id,
                        message.metadata.idempotency_key,
                        payload,
                        created_at_ns,
                    )
                    for index, (message, payload) in enumerate(
                        zip(recorded, serialized, strict=True)
                    )
                ],
            )

            try:
                for transactional in self._transactional_projections:
                    transactional.project(connection, stream_name, recorded)
            except Exception:
                connection.rollback()
                raise

            next_version = current_version + len(events)
            connection.execute(
                """
                INSERT INTO event_streams (stream_name, current_version)
                VALUES (?, ?)
                ON CONFLICT(stream_name) DO UPDATE SET current_version = excluded.current_version
                """,
                (stream_name, next_version),
            )
            connection.commit()

        for hook in self._after_commit_hooks:
            try:
                hook(stream_name, recorded)
            except Exception:
                logger.exception("After-commit hook failed for stream '%s'", stream_name)

        return AppendResult(next_version=next_version)

    def stream_exists(self, stream_name: str) -> bool:
        with _connect(self._path) as connection:
            return self._current_version(connection, stream_name) > 0

    @staticmethod
    def _current_version(connection: sqlite3.Connection, stream_name: str) -> int:
        row = connection.execute(
            "SELECT current_version FROM event_streams WHERE stream_name = ?",
            (stream_name,),
        ).fetchone()
        if row is None:
            return 0
        return int(row[0])

    @staticmethod
    def _next_global_position(connection: sqlite3.Connection) -> int:
        row = connection.execute(
            "SELECT COALESCE(MAX(global_position), -1) + 1 FROM event_messages"
        ).fetchone()
        return int(row[0]) if row is not None else 0

    def _serialize_message(self, message: Message) -> bytes:
        payload = serialize_record(message).encode("utf-8")
        return self._payload_codec.encode(payload) if self._payload_codec is not None else payload

    def _deserialize_message(self, payload_json: bytes | str) -> Message:
        payload = payload_json.encode("utf-8") if isinstance(payload_json, str) else payload_json
        if self._payload_codec is not None:
            payload = self._payload_codec.decode(payload)
        message = deserialize_record(payload)
        if not isinstance(message, Message):
            raise TypeError(f"Expected serialized workflow message, got {type(message)!r}")
        return message

    @staticmethod
    def _bind_positions(
        *,
        message: Message,
        stream_name: str,
        stream_position: int,
        global_position: int,
        recorded_at_ns: int,
    ) -> Message:
        return normalize_recorded_message(
            message,
            stream_name=stream_name,
            stream_position=stream_position,
            global_position=global_position,
            recorded_at_ns=recorded_at_ns,
        )


class SQLiteCheckpointStore(CheckpointStore):
    """Persistent checkpoint store sharing the SQLite event-store file."""

    def __init__(self, path: str | Path | None = None) -> None:
        self._path = _resolve_path(path)
        _ensure_schema(self._path)

    def read(self, processor_id: str) -> StreamPosition | None:
        with _connect(self._path) as connection:
            row = connection.execute(
                "SELECT position FROM processor_checkpoints WHERE processor_id = ?",
                (processor_id,),
            ).fetchone()
        if row is None:
            return None
        return int(row[0])

    def store(self, processor_id: str, position: StreamPosition) -> None:
        with _connect(self._path) as connection:
            connection.execute(
                """
                INSERT INTO processor_checkpoints (processor_id, position)
                VALUES (?, ?)
                ON CONFLICT(processor_id) DO UPDATE SET position = excluded.position
                """,
                (processor_id, int(position)),
            )
            connection.commit()

    def compare_and_store(
        self,
        processor_id: str,
        expected_position: StreamPosition | None,
        position: StreamPosition,
    ) -> bool:
        with _connect(self._path) as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT position FROM processor_checkpoints WHERE processor_id = ?",
                (processor_id,),
            ).fetchone()
            current = None if row is None else int(row[0])
            if current != expected_position:
                connection.rollback()
                return False
            connection.execute(
                """
                INSERT INTO processor_checkpoints (processor_id, position)
                VALUES (?, ?)
                ON CONFLICT(processor_id) DO UPDATE SET position = excluded.position
                """,
                (processor_id, int(position)),
            )
            connection.commit()
            return True


class SQLiteProcessorLock:
    """Lease-based processor fencing shared by all processes using the SQLite file."""

    def __init__(self, path: str | Path | None = None) -> None:
        self._path = _resolve_path(path)
        _ensure_schema(self._path)

    def acquire(self, processor_key: str, instance_id: str, lease_seconds: float) -> bool:
        now_ns = time.time_ns()
        lease_until_ns = now_ns + int(lease_seconds * 1_000_000_000)
        with _connect(self._path) as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT instance_id, lease_until_ns FROM processor_locks WHERE processor_key = ?",
                (processor_key,),
            ).fetchone()
            if row is not None and str(row[0]) != instance_id and int(row[1]) > now_ns:
                connection.rollback()
                return False
            connection.execute(
                """
                INSERT INTO processor_locks (processor_key, instance_id, lease_until_ns)
                VALUES (?, ?, ?)
                ON CONFLICT(processor_key) DO UPDATE SET
                    instance_id = excluded.instance_id,
                    lease_until_ns = excluded.lease_until_ns
                """,
                (processor_key, instance_id, lease_until_ns),
            )
            connection.commit()
            return True

    def refresh(self, processor_key: str, instance_id: str, lease_seconds: float) -> bool:
        lease_until_ns = time.time_ns() + int(lease_seconds * 1_000_000_000)
        with _connect(self._path) as connection:
            cursor = connection.execute(
                """
                UPDATE processor_locks
                SET lease_until_ns = ?
                WHERE processor_key = ? AND instance_id = ?
                """,
                (lease_until_ns, processor_key, instance_id),
            )
            connection.commit()
            return cursor.rowcount == 1

    def release(self, processor_key: str, instance_id: str) -> None:
        with _connect(self._path) as connection:
            connection.execute(
                "DELETE FROM processor_locks WHERE processor_key = ? AND instance_id = ?",
                (processor_key, instance_id),
            )
            connection.commit()
