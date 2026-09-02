"""The producer half of the conversation archive.

The redactor is the part worth testing here, because it is the only place in
this system where a secret can be *removed* rather than reported: everything
after it has already seen the content.
"""

from __future__ import annotations

import email.message
import io
import json
import urllib.error
from typing import Any

import pytest

from aiwatcher_sdk.conversations import (
    ArchiveError,
    Consent,
    ConversationArchive,
    NullRedactor,
    PatternRedactor,
    Retention,
    ToolResult,
    Turn,
    _from_http_error,
)


class RecordingArchive(ConversationArchive):
    """An archive whose transport records instead of sending."""

    def __init__(self, **kwargs: Any) -> None:
        super().__init__("http://aiwatcher.invalid", **kwargs)
        self.sent: list[dict[str, Any]] = []
        self.paths: list[str] = []
        self.replies: list[dict[str, Any]] = []

    def _request(self, method: str, path: str, body: Any = None) -> dict[str, Any]:
        self.paths.append(f"{method} {path}")
        self.sent.append(dict(body or {}))
        if self.replies:
            return self.replies.pop(0)
        return {"turns": [{"turn_id": "abc", "created": True}]}

    def first_turn(self) -> dict[str, Any]:
        turns: Any = self.sent[0]["turns"]
        first: dict[str, Any] = turns[0]
        return first


def _http_error(status: int, body: dict[str, Any]) -> urllib.error.HTTPError:
    """A real ``HTTPError`` with a readable body, rather than a patched one."""
    return urllib.error.HTTPError(
        "http://aiwatcher.invalid/api/v1/conversation-turns",
        status,
        "refused",
        email.message.Message(),
        io.BytesIO(json.dumps(body).encode()),
    )


def archive(**kwargs: Any) -> RecordingArchive:
    return RecordingArchive(
        redactor=PatternRedactor(),
        consent=Consent(
            subject="tenant-17", basis="consent", reference="ticket-4102", scope=["train"]
        ),
        retention=Retention(ttl_days=30, policy_id="training-v2"),
        **kwargs,
    )


# ── Redaction ────────────────────────────────────────────────────────────────


def test_a_credential_never_leaves_the_process() -> None:
    client = archive()
    client.record(
        conversation_id="c1",
        message_id="m1",
        role="user",
        text="my key is AKIAIOSFODNN7EXAMPLE, do not lose it",
    )
    body = json.dumps(client.sent[0])
    assert "AKIAIOSFODNN7EXAMPLE" not in body
    assert "[redacted]" in body
    # And the record says which rule fired, so the server can tell a hook that
    # ran from one that was never wired.
    turn = client.first_turn()
    assert turn["policy"]["redaction"]["rules"] == ["aws-access-key-id"]
    assert turn["policy"]["redaction"]["redactor"].startswith("aiwatcher-sdk-pattern@")


def test_tool_output_is_redacted_too() -> None:
    # The realistic leak: a hook wired only to what the model said passes
    # through whatever the environment returned.
    client = archive()
    client.record_turns(
        [
            Turn(
                conversation_id="c1",
                message_id="m1",
                role="tool",
                parts=[{"kind": "text", "text": "read the config"}],
                tool_results=[
                    ToolResult(
                        call_id="1",
                        name="read_file",
                        ok=True,
                        content="AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE",
                    )
                ],
            )
        ]
    )
    body = json.dumps(client.sent[0])
    assert "AKIAIOSFODNN7EXAMPLE" not in body
    assert "read_file" in body


def test_a_hook_that_found_nothing_still_says_it_ran() -> None:
    # An empty rule list is meaningful and is not the same as no record: a
    # protected deployment refuses the second and accepts the first.
    client = archive()
    client.record(conversation_id="c1", message_id="m1", role="user", text="hello")
    redaction = client.first_turn()["policy"]["redaction"]
    assert redaction["rules"] == []
    assert redaction["redactor"]


def test_the_null_redactor_has_to_be_named() -> None:
    # There is no default redactor. "Send everything verbatim" is expressible
    # and is never the path of least resistance.
    client = RecordingArchive(redactor=NullRedactor())
    client.record(conversation_id="c1", message_id="m1", role="user", text="ada@example.com")
    turn = client.first_turn()
    assert turn["content"]["parts"][0]["text"] == "ada@example.com"
    assert turn["policy"]["redaction"]["redactor"] == "null"


@pytest.mark.parametrize(
    ("text", "rule"),
    [
        ("write to ada@example.com", "email"),
        ("card 4111 1111 1111 1111", "payment-card"),
        ("call +44 20 7946 0958", "phone-number"),
        ("-----BEGIN RSA PRIVATE KEY-----", "private-key"),
        ("key sk-abcdefghijklmnopqrstuvwxyz0123456789", "api-key"),
    ],
)
def test_each_rule_matches_the_shape_the_server_looks_for(text: str, rule: str) -> None:
    # The two sides deliberately match: a hook catching more would leave
    # findings the review queue never shows, and one catching less would make
    # every turn arrive with a finding.
    _, fired = PatternRedactor().redact(text)
    assert fired == [rule]


@pytest.mark.parametrize(
    "text",
    [
        "the task-sk-report is ready",
        "user@localhost",
        "@mention",
        "order 4111111111111112",
        "build 20260902114500123",
        "ref +123456789",
    ],
)
def test_the_conservative_shapes_do_not_fire_on_prose(text: str) -> None:
    redacted, fired = PatternRedactor().redact(text)
    assert fired == []
    assert redacted == text


def test_redacting_twice_is_stable() -> None:
    # A retried flush must send the same bytes: the content digest is what
    # makes a re-send one turn rather than two.
    once, _ = PatternRedactor().redact("mail ada@example.com now")
    twice, _ = PatternRedactor().redact(once)
    assert once == twice


# ── What is sent ─────────────────────────────────────────────────────────────


def test_reasoning_is_a_part_of_its_own() -> None:
    # Most training shapes must not include it, and a plain text part carrying
    # it would be indistinguishable from the answer.
    client = archive()
    client.record(
        conversation_id="c1",
        message_id="m2",
        role="assistant",
        reasoning="the user probably means Berlin",
        text="nine degrees",
    )
    parts = client.first_turn()["content"]["parts"]
    assert [part["kind"] for part in parts] == ["reasoning", "text"]


def test_provenance_travels_in_the_clear_so_the_join_outlives_the_words() -> None:
    client = archive()
    client.record(
        conversation_id="c1",
        message_id="m1",
        role="assistant",
        text="nine degrees",
        run_id="run-1",
        model="provider-model",
        prompt="planner.system@abc",
    )
    provenance = client.first_turn()["provenance"]
    assert provenance == {
        "run_id": "run-1",
        "model": "provider-model",
        "prompt": "planner.system@abc",
    }


def test_the_consent_and_retention_ride_with_every_turn() -> None:
    client = archive()
    client.record(conversation_id="c1", message_id="m1", role="user", text="hello")
    policy = client.first_turn()["policy"]
    assert policy["consent"]["basis"] == "consent"
    assert policy["consent"]["scope"] == ["train"]
    assert policy["retention"] == {"ttl_days": 30, "policy_id": "training-v2"}


def test_an_empty_scope_is_sent_as_empty_rather_than_filled_in() -> None:
    # Empty means nothing is permitted, and an export excludes it by name. A
    # client that helpfully defaulted it would be inventing a permission.
    client = RecordingArchive(redactor=NullRedactor())
    client.record(conversation_id="c1", message_id="m1", role="user", text="hello")
    assert client.first_turn()["policy"]["consent"]["scope"] == []
    assert client.first_turn()["policy"]["consent"]["basis"] == "unknown"


def test_a_review_never_names_its_own_reviewer() -> None:
    client = archive()
    client.review("c1", "abc", state="approved", preference="chosen")
    body = client.sent[0]
    assert body["review"] == {"state": "approved", "note": "", "preference": "chosen"}
    assert "reviewer" not in json.dumps(body)


def test_an_erasure_names_exactly_one_target() -> None:
    client = archive()
    with pytest.raises(ArchiveError):
        client.erase()
    with pytest.raises(ArchiveError):
        client.erase(subject="tenant-17", conversation_id="c1")
    client.erase(subject="tenant-17")
    assert client.sent[0] == {"subject": "tenant-17"}


def test_iterating_a_corpus_follows_the_cursor() -> None:
    client = archive()
    client.replies = [
        {"rows": [{"n": 1}, {"n": 2}], "next_offset": 2, "total": 3},
        {"rows": [{"n": 3}], "total": 3},
    ]
    assert list(client.iter_rows("training/agent-turns", "a" * 64, page=2)) == [
        {"n": 1},
        {"n": 2},
        {"n": 3},
    ]


def test_a_disabled_archive_says_which_decision_is_missing() -> None:
    translated = _from_http_error(_http_error(501, {"code": "registry_disabled", "message": "…"}))
    assert "AIWATCHER_CONVERSATION_ARCHIVE" in str(translated)
    # And not worth retrying: it is a decision somebody has to make.
    assert not translated.is_retryable


def test_a_refusal_carries_every_reason_at_once() -> None:
    translated = _from_http_error(
        _http_error(
            422,
            {
                "code": "turn_rejected",
                "message": "the turn was refused",
                "details": [
                    "policy.consent.basis is required",
                    "policy.redaction is required",
                ],
            },
        )
    )
    assert len(translated.details) == 2
    assert translated.code == "turn_rejected"
