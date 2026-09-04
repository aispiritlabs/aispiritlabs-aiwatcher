"""The governed conversation archive, from Python: redact, then record.

Two halves, and the order matters. :class:`Redactor` runs **in the producer's
process**, on content that has not left it yet, because that is the only place
a secret can be removed rather than merely reported. :class:`ConversationArchive`
then sends what is left, together with the consent and retention that permit
keeping it.

    from aiwatcher_sdk.conversations import (
        ConversationArchive, Consent, PatternRedactor, Retention,
    )

    archive = ConversationArchive(
        "http://aiwatcher:8080",
        redactor=PatternRedactor(),
        consent=Consent(subject=tenant, basis="consent", reference="ticket-4102",
                        scope=["train"]),
        retention=Retention(ttl_days=30, policy_id="training-v2"),
    )
    archive.record(conversation_id=session, message_id="m1", role="user", text=question)
    archive.record(conversation_id=session, message_id="m2", role="assistant",
                   text=answer, parent_message_id="m1")

Like :mod:`aiwatcher_sdk.prompts` and unlike the telemetry transport, **every
method here raises**. The transport swallows failures because an agent must not
fall over for want of a span; this is not telemetry. A write that silently
vanished would produce a corpus missing exactly the exchanges that were
interesting enough to break something, and nothing would say so.

The redaction record is a *claim*, and the server says so: it records the hook
and the rules that fired, and it runs its own scan regardless, because a hook
that was misconfigured reports the same record as one that worked. What the
claim buys is the ability to refuse content from a producer that has no hook at
all — which is what a protected deployment does.
"""

from __future__ import annotations

from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from types import TracebackType
from typing import Any, Literal, Protocol, Self, runtime_checkable
from urllib.parse import quote

import httpx

from aiwatcher_sdk.api import ApiError, Transport

__all__ = [
    "ArchiveError",
    "Consent",
    "ConversationArchive",
    "NullRedactor",
    "PatternRedactor",
    "Redactor",
    "Retention",
    "ToolResult",
    "Turn",
]

Role = Literal["system", "developer", "user", "assistant", "tool"]
Basis = Literal["unknown", "consent", "contract", "legitimate_interest", "synthetic"]
Scope = Literal["train", "evaluate", "share"]

#: This module's version, reported as part of the redaction record. A bare hook
#: name is accepted and is worth less: "which version of the scrubber ran" is
#: the question asked after something gets through.
REDACTOR_VERSION = "1"


_DISABLED = (
    "this aiwatcher instance keeps no conversation archive; "
    "set AIWATCHER_CONVERSATION_ARCHIVE=on and AIWATCHER_CONVERSATION_KEYS"
)


class ArchiveError(ApiError):
    """The archive refused, or could not be reached.

    ``code`` is the machine-readable discriminator the API returns; switch on
    it rather than on the message. Three are worth knowing:

    ``registry_disabled``
        This instance keeps no archive. A deployment decision, not a bug — see
        ``AIWATCHER_CONVERSATION_ARCHIVE``.
    ``turn_rejected``
        The content was refused, and ``details`` holds every reason at once.
    ``erased``
        The turn was here and its content is gone. An answer rather than a
        404, and the distinction an auditor asked for.

    :attr:`~aiwatcher_sdk.api.ApiError.is_retryable` is not simply "5xx"
    either: a 501 means this instance keeps no archive, and retrying that
    forever is what a job does instead of telling somebody to make a decision.
    """


# ── Producer-side redaction ──────────────────────────────────────────────────


@runtime_checkable
class Redactor(Protocol):
    """A producer's own hook, run before anything leaves the process.

    Returns the text to send and the rule ids that fired. Returning the text
    unchanged with an empty list is a valid, meaningful answer: the hook ran and
    found nothing, which is different from no hook having run.
    """

    #: ``name@version``. Recorded verbatim on every turn this hook touched.
    name: str

    def redact(self, text: str) -> tuple[str, list[str]]: ...


@dataclass(frozen=True, slots=True)
class NullRedactor:
    """A hook that removes nothing, and says so.

    For content that is already known to be safe — synthetic fixtures, a
    benchmark — where the honest record is "a hook ran and had nothing to do".
    Not a default: a producer that wants no redaction should have to name it.
    """

    name: str = "null"

    def redact(self, text: str) -> tuple[str, list[str]]:
        return text, []


@dataclass(slots=True)
class PatternRedactor:
    """Replaces the credential and identifier shapes the server also looks for.

    Deliberately the *same* conservative set as the server's scanner, and for a
    reason worth stating: a producer hook that caught things the server does not
    would leave findings the review queue never shows, and one that caught fewer
    would make every turn arrive with a finding. Matching the two means a clean
    scan on the server is evidence the hook worked.

    It is a floor, not a solution. A real deployment has a hook that knows its
    own product's identifiers — order numbers, account references, internal
    hostnames — and this class is what makes "we have not written that yet" a
    working state rather than a blocker.
    """

    placeholder: str = "[redacted]"
    name: str = f"aiwatcher-sdk-pattern@{REDACTOR_VERSION}"

    def redact(self, text: str) -> tuple[str, list[str]]:
        fired: list[str] = []
        for rule, finder in _RULES:
            spans = finder(text)
            if not spans:
                continue
            fired.append(rule)
            for start, end in reversed(spans):
                text = f"{text[:start]}{self.placeholder}{text[end:]}"
        return text, sorted(set(fired))


def _find_emails(text: str) -> list[tuple[int, int]]:
    spans: list[tuple[int, int]] = []
    for index, character in enumerate(text):
        if character != "@":
            continue
        start = index
        while start > 0 and (text[start - 1].isalnum() or text[start - 1] in "._%+-"):
            start -= 1
        end = index + 1
        while end < len(text) and (text[end].isalnum() or text[end] in ".-"):
            end += 1
        domain = text[index + 1 : end]
        # A dot with at least two letters after it. Without this, every
        # `@mention` in prose is a finding.
        dot = domain.rfind(".")
        tld = domain[dot + 1 :] if dot >= 0 else ""
        if start == index or not tld.isalpha() or len(tld) < 2:
            continue
        spans.append((start, end))
    return spans


def _luhn(digits: str) -> bool:
    total = 0
    for position, digit in enumerate(reversed(digits)):
        value = int(digit)
        if position % 2 == 1:
            value *= 2
            if value > 9:
                value -= 9
        total += value
    return total % 10 == 0


def _find_cards(text: str) -> list[tuple[int, int]]:
    spans: list[tuple[int, int]] = []
    index = 0
    while index < len(text):
        if not text[index].isdigit():
            index += 1
            continue
        start = index
        digits = ""
        end = index
        while end < len(text):
            if text[end].isdigit():
                digits += text[end]
                end += 1
            elif text[end] in " -" and end + 1 < len(text) and text[end + 1].isdigit():
                end += 1
            else:
                break
        if 13 <= len(digits) <= 19 and _luhn(digits):
            spans.append((start, end))
        index = max(end, start + 1)
    return spans


def _find_phones(text: str) -> list[tuple[int, int]]:
    spans: list[tuple[int, int]] = []
    index = 0
    while index < len(text):
        if text[index] != "+":
            index += 1
            continue
        start = index
        count = 0
        end = index + 1
        while end < len(text):
            if text[end].isdigit():
                count += 1
                end += 1
            elif text[end] in " -()" and count:
                end += 1
            else:
                break
        if 10 <= count <= 15:
            spans.append((start, end))
        index = max(end, start + 1)
    return spans


#: (prefix, minimum characters after it, rule) — the server's list.
_PREFIXES: tuple[tuple[str, int, str], ...] = (
    ("AKIA", 16, "aws-access-key-id"),
    ("ASIA", 16, "aws-access-key-id"),
    ("ghp_", 30, "github-token"),
    ("gho_", 30, "github-token"),
    ("ghs_", 30, "github-token"),
    ("ghu_", 30, "github-token"),
    ("ghr_", 30, "github-token"),
    ("github_pat_", 40, "github-token"),
    ("xoxb-", 20, "slack-token"),
    ("xoxp-", 20, "slack-token"),
    ("sk-", 32, "api-key"),
    ("sk_live_", 16, "api-key"),
    ("rk_live_", 16, "api-key"),
    ("AIza", 30, "google-api-key"),
    ("eyJ", 40, "json-web-token"),
)


def _is_token_character(character: str) -> bool:
    return character.isalnum() or character in "_-."


def _prefixed(rule: str) -> Any:
    def finder(text: str) -> list[tuple[int, int]]:
        spans: list[tuple[int, int]] = []
        for prefix, minimum, name in _PREFIXES:
            if name != rule:
                continue
            start = text.find(prefix)
            while start >= 0:
                after = start + len(prefix)
                # A prefix in the middle of a longer word is a word.
                if start == 0 or not _is_token_character(text[start - 1]):
                    end = after
                    while end < len(text) and _is_token_character(text[end]):
                        end += 1
                    if end - after >= minimum:
                        spans.append((start, end))
                start = text.find(prefix, after)
        return sorted(spans)

    return finder


def _find_private_keys(text: str) -> list[tuple[int, int]]:
    spans: list[tuple[int, int]] = []
    start = text.find("-----BEGIN ")
    while start >= 0:
        close = text.find("PRIVATE KEY-----", start, start + 64)
        if close >= 0:
            spans.append((start, close + len("PRIVATE KEY-----")))
        start = text.find("-----BEGIN ", start + 1)
    return spans


_RULES: tuple[tuple[str, Any], ...] = (
    ("private-key", _find_private_keys),
    ("aws-access-key-id", _prefixed("aws-access-key-id")),
    ("github-token", _prefixed("github-token")),
    ("slack-token", _prefixed("slack-token")),
    ("api-key", _prefixed("api-key")),
    ("google-api-key", _prefixed("google-api-key")),
    ("json-web-token", _prefixed("json-web-token")),
    ("email", _find_emails),
    ("payment-card", _find_cards),
    ("phone-number", _find_phones),
)


# ── What permits keeping it ──────────────────────────────────────────────────


@dataclass(frozen=True, slots=True)
class Consent:
    """Who this is about, on what basis, and what that permits.

    ``scope`` is empty by default and that means *nothing is permitted*, not
    everything: an export demanding ``train`` excludes an empty scope by name.
    """

    subject: str = ""
    basis: Basis = "unknown"
    reference: str = ""
    scope: Sequence[Scope] = ()
    granted_at: str | None = None

    def as_json(self) -> dict[str, Any]:
        body: dict[str, Any] = {
            "subject": self.subject,
            "basis": self.basis,
            "reference": self.reference,
            "scope": list(self.scope),
        }
        if self.granted_at:
            body["granted_at"] = self.granted_at
        return body


@dataclass(frozen=True, slots=True)
class Retention:
    """How long the content may be held, on a clock of its own.

    Unrelated to the event log's retention on purpose: that one is sized for
    volume and this one for what somebody was told. A deployment may shorten
    what is asked for here, and the turn records that it did.
    """

    ttl_days: int = 30
    policy_id: str = ""

    def as_json(self) -> dict[str, Any]:
        return {"ttl_days": self.ttl_days, "policy_id": self.policy_id}


@dataclass(frozen=True, slots=True)
class ToolResult:
    """What a tool handed back.

    Attached to the turn rather than folded into its text, because a failure is
    a training signal and a stringified error is not.
    """

    call_id: str
    name: str
    ok: bool
    content: str
    error: str = ""

    def as_json(self) -> dict[str, Any]:
        return {
            "call_id": self.call_id,
            "name": self.name,
            "ok": self.ok,
            "content": self.content,
            "error": self.error,
        }


@dataclass(frozen=True, slots=True)
class Turn:
    """One message, ready to send."""

    conversation_id: str
    message_id: str
    role: Role
    parts: Sequence[Mapping[str, Any]]
    parent_message_id: str | None = None
    ordinal: int = 0
    tool_results: Sequence[ToolResult] = ()
    provenance: Mapping[str, Any] = field(default_factory=dict)
    occurred_at: str | None = None

    def as_json(self, policy: Mapping[str, Any]) -> dict[str, Any]:
        body: dict[str, Any] = {
            "conversation_id": self.conversation_id,
            "message_id": self.message_id,
            "ordinal": self.ordinal,
            "role": self.role,
            "content": {
                "parts": list(self.parts),
                "tool_results": [result.as_json() for result in self.tool_results],
            },
            "provenance": dict(self.provenance),
            "policy": dict(policy),
        }
        if self.parent_message_id is not None:
            body["parent_message_id"] = self.parent_message_id
        if self.occurred_at is not None:
            body["occurred_at"] = self.occurred_at
        return body


# ── The client ───────────────────────────────────────────────────────────────


class ConversationArchive:
    """Records conversation content, having redacted it first.

    The redactor is not optional and has no default. A signature that let one be
    omitted would make "send everything verbatim" the path of least resistance,
    which is the outcome this whole module exists to avoid; :class:`NullRedactor`
    is how a caller says it meant to.
    """

    def __init__(
        self,
        base_url: str,
        *,
        redactor: Redactor,
        consent: Consent | None = None,
        retention: Retention | None = None,
        token: str | None = None,
        timeout: float = 10.0,
        attempts: int = 3,
        client: httpx.Client | None = None,
    ) -> None:
        self._redactor = redactor
        self._consent = consent or Consent()
        self._retention = retention or Retention()
        self._http = Transport(
            base_url,
            token=token,
            timeout=timeout,
            attempts=attempts,
            error=ArchiveError,
            subject="the conversation archive",
            hints={"registry_disabled": _DISABLED},
            client=client,
        )

    @property
    def base_url(self) -> str:
        return self._http.base_url

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    # ── Writes ───────────────────────────────────────────────────────────

    def record(
        self,
        *,
        conversation_id: str,
        message_id: str,
        role: Role,
        text: str = "",
        parts: Sequence[Mapping[str, Any]] | None = None,
        parent_message_id: str | None = None,
        ordinal: int = 0,
        tool_results: Sequence[ToolResult] = (),
        reasoning: str | None = None,
        consent: Consent | None = None,
        retention: Retention | None = None,
        **provenance: str,
    ) -> dict[str, Any]:
        """Redact one message and record it.

        ``provenance`` takes the ids that join this back to the telemetry that
        is still on the log — ``run_id``, ``trace_id``, ``span_id``,
        ``agent_id``, ``call_id``, ``model``, ``prompt``. They are kept out of
        the encrypted half on purpose: when the archive expires, they are what
        is left, and "this run used a model that answered badly" stays
        answerable after the words are gone.
        """
        built = list(parts) if parts is not None else []
        if reasoning:
            built.append({"kind": "reasoning", "text": reasoning})
        if text:
            built.append({"kind": "text", "text": text})
        turn = Turn(
            conversation_id=conversation_id,
            message_id=message_id,
            role=role,
            parts=built,
            parent_message_id=parent_message_id,
            ordinal=ordinal,
            tool_results=tuple(tool_results),
            provenance={key: value for key, value in provenance.items() if value},
        )
        recorded = self.record_turns([turn], consent=consent, retention=retention)
        return recorded[0]

    def record_turns(
        self,
        turns: Sequence[Turn],
        *,
        consent: Consent | None = None,
        retention: Retention | None = None,
    ) -> list[dict[str, Any]]:
        """Redact and record a whole exchange.

        Not a transaction, and the server does not pretend otherwise: each turn
        is written as it is validated. That is right for an at-least-once
        producer whose retry lands on the same message ids — a re-sent turn is
        the turn it already wrote, not a second one.
        """
        redacted: list[dict[str, Any]] = []
        for turn in turns:
            parts, tool_results, rules = self._apply(turn)
            policy = {
                "consent": (consent or self._consent).as_json(),
                "retention": (retention or self._retention).as_json(),
                "redaction": {
                    "redactor": self._redactor.name,
                    "rules": rules,
                    "replaced": len(rules),
                },
            }
            body = turn.as_json(policy)
            body["content"]["parts"] = parts
            body["content"]["tool_results"] = tool_results
            redacted.append(body)

        response = self._request("POST", "/api/v1/conversation-turns", {"turns": redacted})
        recorded: Any = response.get("turns", [])
        return list(recorded)

    def _apply(self, turn: Turn) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[str]]:
        """Run the hook over everything that carries prose.

        Tool output included, and that is the case worth stating: it is where a
        credential most often survives, because a hook wired only to what the
        model said passes through whatever the environment returned.
        """
        fired: list[str] = []
        parts: list[dict[str, Any]] = []
        for part in turn.parts:
            copy = dict(part)
            if isinstance(copy.get("text"), str):
                copy["text"], rules = self._redactor.redact(copy["text"])
                fired.extend(rules)
            parts.append(copy)
        results: list[dict[str, Any]] = []
        for result in turn.tool_results:
            body = result.as_json()
            body["content"], rules = self._redactor.redact(body["content"])
            fired.extend(rules)
            results.append(body)
        return parts, results, sorted(set(fired))

    def review(
        self,
        conversation_id: str,
        turn_id: str,
        *,
        state: Literal["pending", "approved", "rejected"],
        note: str = "",
        preference: Literal["chosen", "rejected"] | None = None,
    ) -> dict[str, Any]:
        """Approve or reject one turn.

        The reviewer is the authenticated caller and is never sent: a client
        that could name its own reviewer could file somebody else's approval.
        """
        review: dict[str, Any] = {"state": state, "note": note}
        if preference is not None:
            review["preference"] = preference
        return self._request(
            "POST",
            "/api/v1/conversation-turn-reviews",
            {"conversation_id": conversation_id, "turn_id": turn_id, "review": review},
        )

    def erase(
        self, *, subject: str | None = None, conversation_id: str | None = None
    ) -> dict[str, Any]:
        """Erase content, by consent subject or by conversation.

        Exactly one of the two. The heads, digests and review decisions stay —
        which is what lets an export that named a turn still explain it after
        the words are gone.
        """
        if (subject is None) == (conversation_id is None):
            raise ArchiveError("name exactly one of subject or conversation_id")
        body = {"subject": subject} if subject is not None else {"conversation_id": conversation_id}
        return self._request("POST", "/api/v1/conversation-erasures", body)

    # ── Exports ──────────────────────────────────────────────────────────

    def export(
        self,
        name: str,
        *,
        fmt: Literal["chat", "prompt_response", "sft", "dpo"] = "chat",
        conversations: Sequence[str] = (),
        required_scope: Scope = "train",
        require_human_review: bool = True,
        description: str = "",
    ) -> dict[str, Any]:
        """Queue an export. Returns the job, not a corpus.

        Idempotent: the job id is derived from the request and the selection it
        resolved to, so a retried call joins the job it already started.
        """
        body: dict[str, Any] = {
            "name": name,
            "description": description,
            "format": fmt,
            "required_scope": required_scope,
            "require_human_review": require_human_review,
        }
        if conversations:
            body["selection"] = {"conversations": list(conversations)}
        return self._request("POST", "/api/v1/conversation-exports", body)

    def get_job(self, job_id: str) -> dict[str, Any]:
        """One export job: where it is, and every row it left out, by reason."""
        return self._request("GET", f"/api/v1/conversation-exports/{_segment(job_id)}")

    def get_rows(
        self, name: str, version: str, *, offset: int = 0, limit: int = 100
    ) -> dict[str, Any]:
        """One page of an immutable corpus.

        Reading these needs the ``admin`` role: they are conversation content,
        and the gate is the same one that guards a single turn.
        """
        return self._request(
            "GET",
            "/api/v1/conversation-dataset-rows",
            params={"name": name, "version": version, "offset": offset, "limit": limit},
        )

    def iter_rows(self, name: str, version: str, *, page: int = 100) -> Iterator[dict[str, Any]]:
        """Every row of a corpus, a page at a time.

        A generator rather than a list: a corpus is the thing that did not fit
        in one response, which is why the export is asynchronous in the first
        place.
        """
        offset = 0
        while True:
            body = self.get_rows(name, version, offset=offset, limit=page)
            yield from body.get("rows", [])
            following = body.get("next_offset")
            if following is None:
                return
            offset = int(following)

    def get_policy(self) -> dict[str, Any]:
        """What this deployment demands, and which keys it can open with.

        Worth calling once at start-up: a producer that discovers the consent
        requirement from a 422 has already put a megabyte of content on the wire.
        """
        return self._request("GET", "/api/v1/conversation-policy")

    # ── Transport ────────────────────────────────────────────────────────

    def _request(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None = None,
        *,
        params: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        # Idempotent by construction: a turn is keyed by `message_id`, a review
        # is a state set to a value, and an export job's id is derived from the
        # request and the selection it resolved to — so a retried call joins
        # the job it already started rather than queueing a second one.
        return self._http.json(method, path, body, params=params, idempotent=True)


def _segment(value: str) -> str:
    """A path segment, encoded. httpx encodes a query; a path is the caller's."""
    return quote(value, safe="")
