#!/usr/bin/env python3
"""Move training pairs off the event log and into the governed archive.

The build before ADR_0021 kept conversation content by convention: a producer
that wanted a turn retained put ``input`` and ``output`` in the data of an
``llm.completed`` event, and an exporter paged the log for the events that had
both. That convention is gone, and this script is the way out of it.

It reads those events, records each pair as two governed turns — a user turn
and an assistant turn, joined by ``parent_message_id`` — and attaches the
consent record the operator supplies on the command line. Nothing is inferred:
if the pairs were kept on a lawful basis, whoever knows what it was has to say
so here, and the archive records it verbatim.

Two things it deliberately does not do.

**It does not delete anything from the log.** The log is append-only, which is
why the content should never have been there and why this cannot fix it.
Rotating the affected log segments is a retention operation, not a script.

**It does not approve anything.** Every imported turn arrives pending, because
nobody has read it. That is the whole point of the review gate, and an importer
that pre-approved its own output would defeat it on the first day.

    ./scripts/import-conversation-turns.py training-demo \\
        --subject tenant-17 --basis consent --reference ticket-4102
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

DEFAULT_URL = "http://127.0.0.1:8080"
BATCH = 64

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "sdk", "python"))
from aiwatcher_sdk.conversations import (  # noqa: E402
    ArchiveError,
    Consent,
    ConversationArchive,
    PatternRedactor,
    Retention,
    Turn,
)


def request(base: str, path: str) -> Any:
    headers = {"content-type": "application/json"}
    token = os.environ.get("AIWATCHER_TOKEN")
    if token:
        headers["authorization"] = f"Bearer {token}"
    call = urllib.request.Request(f"{base.rstrip('/')}{path}", headers=headers, method="GET")
    try:
        with urllib.request.urlopen(call, timeout=30) as response:  # noqa: S310
            return json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read().decode(errors="replace")
        raise RuntimeError(f"aiwatcher returned HTTP {error.code}: {detail}") from error
    except urllib.error.URLError as error:
        raise RuntimeError(f"cannot reach aiwatcher at {base}: {error.reason}") from error


def conversation_runs(base: str, conversation: str) -> list[dict[str, Any]]:
    runs: list[dict[str, Any]] = []
    before: str | None = None
    while True:
        query: dict[str, Any] = {"conversation_id": conversation, "limit": 500}
        if before:
            query["before"] = before
        page = request(base, f"/api/v1/runs?{urllib.parse.urlencode(query)}")
        runs.extend(page.get("runs", []))
        before = page.get("next_cursor")
        if not before:
            return runs


def completed_events(base: str, run_id: str) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    after: int | None = None
    while True:
        query: dict[str, Any] = {"event_type": "llm.completed", "limit": 500}
        if after is not None:
            query["after"] = after
        path = (
            f"/api/v1/runs/{urllib.parse.quote(run_id, safe='')}/events?"
            f"{urllib.parse.urlencode(query)}"
        )
        page = request(base, path)
        events.extend(page.get("events", []))
        after = page.get("next_cursor")
        if after is None:
            return events


def turns_from(conversation: str, run_id: str, events: list[dict[str, Any]]) -> list[Turn]:
    """Two turns per legacy pair, joined so the shape survives the move."""
    turns: list[Turn] = []
    for ordinal, event in enumerate(events):
        data = event.get("data", {})
        if "input" not in data or "output" not in data:
            continue
        metadata = event.get("metadata", {})
        call_id = str(data.get("call_id") or f"call-{ordinal}")
        # Derived from the run and the call rather than generated, so running
        # this twice imports one turn rather than two — the same reason every
        # id in this system is derived.
        question = f"{run_id}:{call_id}:in"
        answer = f"{run_id}:{call_id}:out"
        provenance = {
            "run_id": run_id,
            "trace_id": str(metadata.get("trace_id") or ""),
            "span_id": str(metadata.get("span_id") or ""),
            "agent_id": str(metadata.get("agent_id") or ""),
            "call_id": call_id,
            "model": str(data.get("model") or ""),
        }
        turns.append(
            Turn(
                conversation_id=conversation,
                message_id=question,
                role="user",
                parts=[{"kind": "text", "text": str(data["input"])}],
                ordinal=ordinal * 2,
                provenance=provenance,
            )
        )
        turns.append(
            Turn(
                conversation_id=conversation,
                message_id=answer,
                role="assistant",
                parent_message_id=question,
                parts=[{"kind": "text", "text": str(data["output"])}],
                ordinal=ordinal * 2 + 1,
                provenance=provenance,
            )
        )
    return turns


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("conversation", help="exact conversation_id to import")
    parser.add_argument("--api", default=os.environ.get("AIWATCHER_URL", DEFAULT_URL))
    parser.add_argument("--subject", required=True, help="whose data this is")
    parser.add_argument(
        "--basis",
        required=True,
        choices=["consent", "contract", "legitimate_interest", "synthetic"],
        help="what makes keeping it lawful",
    )
    parser.add_argument("--reference", required=True, help="where that record lives")
    parser.add_argument("--scope", default="train", help="comma-separated: train,evaluate,share")
    parser.add_argument("--ttl-days", type=int, default=30)
    parser.add_argument("--policy-id", default="")
    arguments = parser.parse_args()

    archive = ConversationArchive(
        arguments.api,
        redactor=PatternRedactor(),
        consent=Consent(
            subject=arguments.subject,
            basis=arguments.basis,
            reference=arguments.reference,
            scope=[part.strip() for part in arguments.scope.split(",") if part.strip()],
        ),
        retention=Retention(ttl_days=arguments.ttl_days, policy_id=arguments.policy_id),
    )

    runs = conversation_runs(arguments.api, arguments.conversation)
    if not runs:
        raise RuntimeError(f"conversation {arguments.conversation!r} has no runs")

    turns: list[Turn] = []
    for run in runs:
        run_id = str(run["run_id"])
        turns.extend(turns_from(arguments.conversation, run_id, completed_events(arguments.api, run_id)))
    if not turns:
        raise RuntimeError(
            "no legacy pairs found; this only reads llm.completed events that carry "
            "both data.input and data.output"
        )

    imported = 0
    findings = 0
    for start in range(0, len(turns), BATCH):
        recorded = archive.record_turns(turns[start : start + BATCH])
        imported += len(recorded)
        findings += sum(len(turn.get("findings", [])) for turn in recorded)
        print(f"  {imported}/{len(turns)} turns", end="\r", file=sys.stderr)

    print(
        f"imported {imported} turns from {len(runs)} runs into {arguments.conversation}; "
        f"{findings} findings; all pending review"
    )
    print(
        "The event log still holds the original bodies. Rotating those segments is a "
        "retention operation, not something this script can do."
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ArchiveError, KeyError, RuntimeError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
