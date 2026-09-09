"""`agentic.workflow.EventStore` over one hosted execution.

Written against a stand-in for `agentic.workflow.Message` rather than the real
one, which is the point of the codec: this SDK does not depend on the agent's
packages, and a test that imported them would quietly make it do so.

The stand-in also has to be *shaped* like the real message, because that is the
whole contract — `kind`, `type`, `data`, and a metadata dataclass.
"""

from __future__ import annotations

import dataclasses
import json
from dataclasses import dataclass, field
from typing import Any

import httpx
import pytest

from aiwatcher_sdk.api import ApiError, Transport
from aiwatcher_sdk.integrations.agentic import (
    AiwatcherEventStore,
    ConcurrencyConflictError,
    FilePayloadStore,
    JoinTimers,
    MemoryPayloadStore,
    SagaTimers,
    dataclass_codec,
    digest_of,
    event_store,
)

EXECUTION = "graph-1"


@dataclass(frozen=True, slots=True, kw_only=True)
class Metadata:
    message_id: str = ""
    causation_id: str | None = None
    correlation_id: str = ""
    turn_id: str = ""


@dataclass(frozen=True, slots=True, kw_only=True)
class Message:
    kind: str = "message"
    type: str = "message"
    data: Any = field(default_factory=dict)
    metadata: Metadata = field(default_factory=Metadata)


def turn(message_id: str, text: str = "hello", type_: str = "TurnCompleted") -> Message:
    return Message(
        kind="event",
        type=type_,
        data={"role": "assistant", "text": text},
        metadata=Metadata(message_id=message_id, turn_id="t-1"),
    )


class Server:
    """A hosted execution's two routes, with the rules that matter kept.

    A stub rather than a mock: the compare-and-append and the inbox are what
    this client is written against, and a double that answered 200 to everything
    would prove nothing about either.
    """

    def __init__(self) -> None:
        self.rows: list[dict[str, Any]] = []
        self.seen: dict[str, int] = {}
        self.requests: list[httpx.Request] = []
        #: Who holds the decider lease, when anybody does.
        self.lease_holder: str | None = None

    def handle(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        if request.method == "GET":
            return self._history(request)
        return self._append(request)

    def _history(self, request: httpx.Request) -> httpx.Response:
        after = int(request.url.params.get("after", 0))
        limit = int(request.url.params.get("limit", 100))
        page = [row for row in self.rows if row["stream_version"] > after][: limit + 1]
        more = len(page) > limit
        page = page[:limit]
        return httpx.Response(
            200,
            json={
                "messages": page,
                "next_after": page[-1]["stream_version"] if more and page else None,
                "version": len(self.rows),
            },
        )

    def _append(self, request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        if self.lease_holder not in (None, body["holder"]):
            return httpx.Response(
                409,
                json={"code": "lease_held", "message": f"{self.lease_holder} is deciding this"},
            )
        key = request.headers.get("idempotency-key", "")
        if key in self.seen:
            return httpx.Response(200, json={"version": self.seen[key], "created": False})
        if body["expected_version"] != len(self.rows):
            return httpx.Response(
                409,
                json={"code": "version_conflict", "message": "somebody else appended first"},
            )
        self._record({"kind": "hosted", "message_type": "HostedAppend", "metadata": {}})
        for message in body["messages"]:
            self._record({"kind": "hosted", **message})
        self.seen[key] = len(self.rows)
        return httpx.Response(200, json={"version": len(self.rows), "created": True})

    def _record(self, message: dict[str, Any]) -> None:
        self.rows.append({"stream_version": len(self.rows) + 1, "message": message})


def store(server: Server, payloads: Any = None, timers: Any = None) -> AiwatcherEventStore:
    return AiwatcherEventStore(
        Transport(
            "http://aiwatcher.invalid",
            error=ApiError,
            client=httpx.Client(transport=httpx.MockTransport(server.handle)),
        ),
        EXECUTION,
        payloads=payloads or MemoryPayloadStore(),
        codec=dataclass_codec(Message, Metadata),
        holder="worker-a",
        timers=timers,
    )


def test_a_message_written_through_this_store_comes_back_as_the_caller_s_own_type() -> None:
    # The codec's whole job: `agentic`'s `Message` is a type this SDK is handed,
    # never one it imports, and a round trip has to be indistinguishable from
    # the store the executor had before.
    server = Server()
    subject = store(server)
    sent = turn("m-1")

    subject.append_to_stream(EXECUTION, (sent,), expected_version=0)
    read = subject.read_stream(EXECUTION)

    assert read.events == (sent,)
    assert read.current_version == 2, "one marker and one message"
    assert read.stream_exists


def test_the_words_go_to_the_payload_store_and_the_stream_carries_a_reference() -> None:
    # Section 40.4. Every hop in an agent graph carries text, and it is
    # conversation content: the stream gets a reference, a plaintext digest and
    # a size, and never the words.
    server = Server()
    payloads = MemoryPayloadStore()
    subject = store(server, payloads)
    sent = turn("m-1", text="the model said something private")

    subject.append_to_stream(EXECUTION, (sent,), expected_version=0)

    wire = json.dumps([row["message"] for row in server.rows])
    assert "something private" not in wire, wire
    payload = server.rows[-1]["message"]["payload"]
    assert payload["policy"] == "external"
    assert payload["digest"] == digest_of(sent.data)
    assert payload["size"] > 0
    # And the words are where the worker put them.
    assert payloads.get_payload(payload["reference"]) == sent.data


def test_a_payload_that_is_not_the_one_that_was_appended_is_refused() -> None:
    # The prompt registry's rule, in a payload store: a graph replayed from
    # somebody else's words with nothing to say so is the corruption no metric
    # catches.
    server = Server()
    payloads = MemoryPayloadStore()
    subject = store(server, payloads)
    subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=0)

    reference = server.rows[-1]["message"]["payload"]["reference"]
    payloads.store_payload(reference.removeprefix("memory://"), {"text": "something else"})

    with pytest.raises(ValueError, match="not the payload that was appended"):
        subject.read_stream(EXECUTION)


def test_a_lost_race_raises_the_error_the_executor_retries_on() -> None:
    # `DurableWorkflowExecutor` catches `ConcurrencyConflictError` by class to
    # drive its OCC loop, so a 409 that surfaced as anything else would break
    # the retry this store exists to serve. `agentic` is not installed here, so
    # what it raises is this package's own.
    server = Server()
    subject = store(server)
    subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=0)

    with pytest.raises(ConcurrencyConflictError) as refused:
        subject.append_to_stream(EXECUTION, (turn("m-2"),), expected_version=0)
    assert refused.value.stream_name == EXECUTION
    assert refused.value.expected == 0

    # And the loser reloads and succeeds, which is the loop in miniature.
    version = subject.read_stream(EXECUTION).current_version
    subject.append_to_stream(EXECUTION, (turn("m-2"),), expected_version=version)
    assert len(subject.read_stream(EXECUTION).events) == 2


def test_a_run_somebody_else_is_deciding_is_not_a_conflict_to_retry() -> None:
    # Both are 409 and only one is worth retrying. `version_conflict` means
    # re-read and decide again, which is the loop `DurableWorkflowExecutor`
    # runs; `lease_held` means another decider owns this run, and raising it as
    # a conflict would make that loop spin exactly where it has to stop. Found
    # against a real server rather than this stub, which is why the stub now
    # keeps a lease at all.
    server = Server()
    server.lease_holder = "worker-b"
    subject = store(server)

    with pytest.raises(ApiError) as refused:
        subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=0)
    assert refused.value.status == 409
    assert refused.value.code == "lease_held"
    assert not isinstance(refused.value, ConcurrencyConflictError)


def test_a_repeated_batch_is_one_append_because_its_key_names_its_messages() -> None:
    # Section 43.10, and the kickoff's first trap. A key derived from the
    # execution alone would make the second batch a redelivery of the first.
    server = Server()
    subject = store(server)
    subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=0)
    subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=0)

    assert len(subject.read_stream(EXECUTION).events) == 1, "the same batch, once"

    version = subject.read_stream(EXECUTION).current_version
    subject.append_to_stream(EXECUTION, (turn("m-2"),), expected_version=version)
    assert len(subject.read_stream(EXECUTION).events) == 2, "a different batch is a second one"


def test_this_engine_s_own_rows_are_not_handed_to_a_fold_written_for_the_agent() -> None:
    # A hosted stream holds the marker aiwatcher writes per append, and the
    # run's own start. Replaying those through `agentic`'s vocabulary would be
    # handing a fold somebody else's events.
    server = Server()
    server.rows.append(
        {
            "stream_version": 1,
            "message": {"kind": "command", "command": "start_execution"},
        }
    )
    subject = store(server)
    subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=1)

    read = subject.read_stream(EXECUTION)
    assert [message.type for message in read.events] == ["TurnCompleted"]
    assert read.current_version == 3, "the version counts every row, not just the agent's"


def test_a_long_stream_is_walked_a_page_at_a_time(monkeypatch: Any) -> None:
    # A graph that ran for a day is a stream neither side can hold, so the read
    # pages. A page of two is what finds an off-by-one in `after` — with one
    # page the cursor is never used at all.
    monkeypatch.setattr(event_store, "PAGE", 2)
    server = Server()
    subject = store(server)
    for index in range(5):
        version = subject.read_stream(EXECUTION).current_version
        subject.append_to_stream(EXECUTION, (turn(f"m-{index}"),), expected_version=version)

    server.requests.clear()
    read = subject.read_stream(EXECUTION)

    assert [message.metadata.message_id for message in read.events] == [
        f"m-{index}" for index in range(5)
    ]
    assert read.current_version == 10, "five markers and five messages"
    assert len(server.requests) > 1, "one call would not have paged at all"

    server.requests.clear()
    subject.read_stream(EXECUTION)
    whole = len(server.requests)

    # And `max_count` stops as soon as it has enough rather than walking the
    # rest. Not one request: a page of two holds one marker and one message, so
    # two messages take two pages — which is the point of paging by rows and
    # counting messages.
    server.requests.clear()
    assert len(subject.read_stream(EXECUTION, max_count=2).events) == 2
    assert 0 < len(server.requests) < whole


def test_aggregate_stream_folds_here_because_the_engine_never_reads_these() -> None:
    # aiwatcher folds none of a hosted run's messages on purpose — an engine
    # that read them would be a second decider. The fold is the caller's.
    server = Server()
    subject = store(server)
    subject.append_to_stream(EXECUTION, (turn("m-1"), turn("m-2")), expected_version=0)

    result = subject.aggregate_stream(
        EXECUTION,
        evolve=lambda state, message: [*state, message.metadata.message_id],
        initial_state=list,
    )
    assert result.state == ["m-1", "m-2"]
    assert result.stream_exists


def test_a_store_open_on_one_execution_refuses_another_stream_s_name() -> None:
    # One store, one execution. Routing by name would let a decider that
    # wandered onto another graph's stream write there without noticing.
    server = Server()
    subject = store(server)
    with pytest.raises(ValueError, match="One store, one execution"):
        subject.read_stream("some-other-graph")


def test_there_is_no_read_across_executions_and_asking_says_why() -> None:
    # The thing that spans executions is the event log, and it carries facts
    # about work rather than content. Answering one execution's messages to a
    # prefix that names many would read as an empty result for every other one.
    server = Server()
    subject = store(server)
    subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=0)

    assert len(subject.read_all().events) == 1
    with pytest.raises(ValueError, match="no read across executions"):
        subject.read_all(stream_prefix="every-graph")


def test_a_file_payload_store_survives_the_process_that_wrote_it(tmp_path: Any) -> None:
    # The restart this whole phase is about. A store that lost its payloads on
    # restart would have kept a history it cannot replay, which is why
    # `MemoryPayloadStore` is not a default.
    server = Server()
    subject = store(server, FilePayloadStore(tmp_path))
    sent = turn("m-1", text="worth keeping")
    subject.append_to_stream(EXECUTION, (sent,), expected_version=0)

    revived = store(server, FilePayloadStore(tmp_path))
    assert revived.read_stream(EXECUTION).events == (sent,)


def test_the_wire_never_carries_a_second_meaning_for_kind() -> None:
    # aiwatcher's `kind` is its own three-way split — command, event, hosted —
    # and `agentic`'s is the message's. Two under one name would be read wrong
    # by whichever side looked first.
    server = Server()
    subject = store(server)
    subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=0)

    message = server.rows[-1]["message"]
    assert message["kind"] == "hosted"
    assert message["metadata"]["agentic_kind"] == "event"
    assert subject.read_stream(EXECUTION).events[0].kind == "event"


def test_a_metadata_field_this_build_does_not_know_is_dropped_rather_than_fatal() -> None:
    # A stream written by a newer build of the agent still replays under an
    # older one: the same forwards-compatibility every reader here keeps.
    server = Server()
    subject = store(server)
    subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=0)
    server.rows[-1]["message"]["metadata"]["a_field_from_next_year"] = 1

    assert subject.read_stream(EXECUTION).events[0].metadata.message_id == "m-1"


def timeout(timeout_id: str, due_at_ns: int, type_: str) -> Message:
    return Message(
        kind="event",
        type=type_,
        data={"timeout_id": timeout_id, "due_at_ns": due_at_ns},
        metadata=Metadata(message_id=f"{type_}:{timeout_id}"),
    )


def test_a_saga_s_timeout_becomes_a_row_the_engine_can_wake_up_for() -> None:
    # The whole of what makes `agentic`'s sagas fire with no change to
    # `agentic`. `schedule_timeout`, `due_timeouts` and `fire_timeout` have all
    # worked since before any of this existed; what has never existed is
    # something that wakes up and looks, because a worker holding its own SQLite
    # is not running when the timeout comes due. The event still goes into the
    # stream exactly as it did — the row goes beside it, in the same append.
    server = Server()
    subject = store(server, timers=SagaTimers())
    subject.append_to_stream(
        EXECUTION,
        (timeout("reply", 1_700_000_000_000_000_000, SagaTimers.SCHEDULED),),
        expected_version=0,
    )

    body = json.loads(server.requests[-1].content)
    assert len(body["messages"]) == 1, "the event is appended as it always was"
    schedule = body["timers"][0]["schedule"]
    assert schedule["timer_id"] == "reply"
    assert schedule["due_at"] == "2023-11-14T22:13:20Z"
    # The message the engine hands back is the one being appended now, so a saga
    # reading its own history and a saga receiving the timeout see the same
    # words.
    assert schedule["message"] == body["messages"][0]


def test_a_timeout_that_was_handled_withdraws_its_row() -> None:
    # A saga appends `saga.timeout_fired` when it has dealt with one. Leaving
    # the row would have a tick deliver it again to a saga that moved on.
    server = Server()
    subject = store(server, timers=SagaTimers())
    subject.append_to_stream(
        EXECUTION,
        (timeout("reply", 0, SagaTimers.FIRED),),
        expected_version=0,
    )
    body = json.loads(server.requests[-1].content)
    assert body["timers"] == [{"cancel": {"timer_id": "reply"}}]


def test_a_store_with_no_policy_asks_for_no_timers_rather_than_guessing() -> None:
    # `saga.timeout_scheduled` is `agentic.workflow.Saga`'s word, not this
    # store's. A store that recognised it without being told would be reading a
    # caller's vocabulary it was never handed.
    server = Server()
    subject = store(server)
    subject.append_to_stream(
        EXECUTION,
        (timeout("reply", 1_700_000_000_000_000_000, SagaTimers.SCHEDULED),),
        expected_version=0,
    )
    assert "timers" not in json.loads(server.requests[-1].content)


def test_a_message_that_is_not_a_timeout_asks_for_no_row() -> None:
    server = Server()
    subject = store(server, timers=SagaTimers())
    subject.append_to_stream(EXECUTION, (turn("m-1"),), expected_version=0)
    assert "timers" not in json.loads(server.requests[-1].content)


def test_the_codec_takes_a_message_apart_without_naming_its_class() -> None:
    codec = dataclass_codec(Message, Metadata)
    record = codec.as_record(turn("m-1"))
    assert record.type == "TurnCompleted"
    assert record.kind == "event"
    assert record.metadata["message_id"] == "m-1"
    assert dataclasses.is_dataclass(codec.build_message(record))


def join_event(type_: str, turn_id: str, due_at: float | None = None) -> Message:
    data: dict[str, Any] = {"turn_id": turn_id, "summarizer_node_id": "summarizer-1"}
    if due_at is not None:
        data["due_at"] = due_at
    return Message(
        kind="event",
        type=type_,
        data=data,
        metadata=Metadata(message_id=f"{type_}:{turn_id}"),
    )


def test_a_graph_join_s_deadline_becomes_a_row_the_engine_can_wake_up_for() -> None:
    # The silence a fan-in could not break for itself: a node that never
    # completes leaves the join waiting for ever, and noticing that needs
    # something that wakes up and looks.
    server = Server()
    subject = store(server, timers=JoinTimers())
    subject.append_to_stream(
        EXECUTION,
        (join_event(JoinTimers.SCHEDULED, "turn-1", due_at=1_700_000_000.0),),
        expected_version=0,
    )

    body = json.loads(server.requests[-1].content)
    assert len(body["messages"]) == 1, "the event is appended as it always was"
    schedule = body["timers"][0]["schedule"]
    # Derived from the turn and the summarizer, so the append that schedules it
    # and the one that withdraws it name the same row without either having to
    # remember an id the other minted.
    assert schedule["timer_id"] == "join:turn-1:summarizer-1"
    assert schedule["due_at"] == "2023-11-14T22:13:20Z"
    assert schedule["message"] == body["messages"][0]


def test_a_summary_that_returned_withdraws_its_deadline() -> None:
    server = Server()
    subject = store(server, timers=JoinTimers())
    subject.append_to_stream(
        EXECUTION,
        (join_event(JoinTimers.COMPLETED, "turn-1"),),
        expected_version=0,
    )
    body = json.loads(server.requests[-1].content)
    assert body["timers"] == [{"cancel": {"timer_id": "join:turn-1:summarizer-1"}}]


def test_a_claim_does_not_withdraw_a_join_s_deadline() -> None:
    # A claim is somebody saying they are running the summarizer, and a worker
    # that says that and then dies is precisely the case the deadline is for.
    # Only a summary that finished makes it moot.
    server = Server()
    subject = store(server, timers=JoinTimers())
    subject.append_to_stream(
        EXECUTION,
        (join_event("graph.summary_claimed", "turn-1"),),
        expected_version=0,
    )
    assert "timers" not in json.loads(server.requests[-1].content)


def test_a_deadline_with_no_turn_asks_for_no_row_rather_than_a_guessed_one() -> None:
    server = Server()
    subject = store(server, timers=JoinTimers())
    subject.append_to_stream(
        EXECUTION,
        (
            Message(
                kind="event",
                type=JoinTimers.SCHEDULED,
                data={"due_at": 1_700_000_000.0},
                metadata=Metadata(message_id="no-turn"),
            ),
        ),
        expected_version=0,
    )
    assert "timers" not in json.loads(server.requests[-1].content)
