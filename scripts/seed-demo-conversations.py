#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["aiwatcher-sdk"]
#
# [tool.uv.sources]
# aiwatcher-sdk = { path = "../sdk/python", editable = true }
# ///
"""Seed the governed conversation archive: record, review, export, read back.

The whole of ADR_0021 in one run, against a server started with
``just run-conversations``. It records a short exchange with a real consent
record and a real redaction pass, shows what the server's own scanner finds in
the one turn that carries a credential, approves what may be trained on,
queues an export, waits for the worker, and prints the immutable reference.

Nothing here is mocked and nothing is inserted behind the API. The point is
that the guardrails are the ones a producer will actually hit: a turn with no
consent record is refused with every reason at once, a turn nobody reviewed is
excluded from the corpus by name, and the content that carries a credential is
excluded whatever anybody clicks.
"""

from __future__ import annotations

import argparse
import os
import sys
import time
from typing import Any

DEFAULT_URL = "http://127.0.0.1:8080"
CONVERSATION = "training-demo"
CORPUS = "training/agent-turns"

# The producer's own hook. Deliberately the SDK's, so this script demonstrates
# the supported path rather than a second one written for a demo.
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "sdk", "python"))
from aiwatcher_sdk.conversations import (  # noqa: E402
    ArchiveError,
    Consent,
    ConversationArchive,
    NullRedactor,
    PatternRedactor,
    Retention,
    ToolResult,
)

#: One exchange. The third turn is recorded through a producer whose redaction
#: hook is *not* wired to tool output — the realistic misconfiguration — so the
#: credential in it reaches the server, the server's own scan finds it, and the
#: review gate is what keeps it out of the corpus. Every other turn goes through
#: a working hook and arrives clean.
EXCHANGE: list[dict[str, Any]] = [
    {
        "message_id": "m1",
        "role": "user",
        "text": "What is deployed to staging right now? My address is 10 Downing Street.",
    },
    {
        "message_id": "m2",
        "role": "assistant",
        "parent": "m1",
        "reasoning": "They probably mean the web service rather than the workers.",
        "text": "Checking the deployment.",
    },
    {
        "message_id": "m3",
        "role": "tool",
        "parent": "m2",
        "text": "reading the deployment manifest",
        "unredacted": True,
        "tool": ToolResult(
            call_id="c1",
            name="read_file",
            ok=True,
            content="image: web:2026.09.01\nAWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE",
        ),
    },
    {
        "message_id": "m4",
        "role": "assistant",
        "parent": "m3",
        "text": "Staging is on web:2026.09.01, deployed this morning.",
    },
]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api", default=os.environ.get("AIWATCHER_URL", DEFAULT_URL))
    parser.add_argument("--conversation", default=CONVERSATION)
    parser.add_argument("--corpus", default=CORPUS)
    arguments = parser.parse_args()

    consent = Consent(
        subject="tenant-17",
        basis="consent",
        reference="https://example.invalid/policies/training#2026-09",
        scope=["train", "evaluate"],
    )
    retention = Retention(ttl_days=30, policy_id="training-v2")
    archive = ConversationArchive(
        arguments.api, redactor=PatternRedactor(), consent=consent, retention=retention
    )

    try:
        policy = archive.get_policy()
    except ArchiveError as error:
        print(f"error: {error}", file=sys.stderr)
        if error.code == "registry_disabled":
            print(
                "\nThe archive is off, which is the default. Start the server with\n"
                "  just run-conversations\n"
                "or set AIWATCHER_CONVERSATION_ARCHIVE=on and AIWATCHER_CONVERSATION_KEYS.",
                file=sys.stderr,
            )
        return 1

    print(
        f"archive: {policy['mode']}, at most {policy['max_ttl_days']} days, "
        f"keys {', '.join(policy['key_ids'])}"
    )

    # 1. The refusal a producer with no consent record gets, before anything
    #    useful happens. Shown rather than described, because it is the gate.
    if policy["mode"] == "protected":
        bare = ConversationArchive(arguments.api, redactor=PatternRedactor())
        try:
            bare.record(
                conversation_id=arguments.conversation,
                message_id="refused",
                role="user",
                text="this will not be stored",
            )
            print("warning: a turn with no consent record was accepted", file=sys.stderr)
        except ArchiveError as error:
            print(f"\nrefused, as it should be — {len(error.details)} problems:")
            for problem in error.details:
                print(f"  · {problem}")

    # A second producer, whose hook removes nothing and says so. Standing in
    # for the one whose redaction was never wired to tool output.
    unhooked = ConversationArchive(
        arguments.api, redactor=NullRedactor(), consent=consent, retention=retention
    )

    # 2. Record the exchange. Redaction runs in this process.
    print("\nrecording:")
    recorded: list[dict[str, Any]] = []
    for message in EXCHANGE:
        client = unhooked if message.get("unredacted") else archive
        turn = client.record(
            conversation_id=arguments.conversation,
            message_id=message["message_id"],
            role=message["role"],
            text=message.get("text", ""),
            reasoning=message.get("reasoning"),
            parent_message_id=message.get("parent"),
            tool_results=[message["tool"]] if "tool" in message else [],
            run_id="conversation-run-001",
            model="provider-model",
        )
        findings = ", ".join(f"{f['kind']}:{f['rule']}" for f in turn.get("findings", []))
        print(
            f"  {message['message_id']:>4} {message['role']:<9} "
            f"{turn['turn_id'][:12]}…  {findings or 'no findings'}"
        )
        recorded.append(turn)

    # 3. Review. The turn the server found a credential in is rejected with a
    #    reason; the rest are approved.
    print("\nreviewing:")
    for message, turn in zip(EXCHANGE, recorded, strict=True):
        risky = any(
            finding["kind"] in {"pii", "secret"} for finding in turn.get("findings", [])
        )
        state = "rejected" if risky else "approved"
        note = "the server's scan found a credential the producer's hook missed" if risky else ""
        archive.review(
            arguments.conversation, turn["turn_id"], state=state, note=note or "looks fine"
        )
        print(f"  {message['message_id']:>4} {state}")

    # 4. Export. A job, not a corpus.
    print("\nqueueing an export…")
    job = archive.export(arguments.corpus, fmt="chat", required_scope="train")
    print(f"  job {job['job_id'][:12]}… over {len(job['conversations'])} conversation(s)")

    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        job = archive.get_job(job["job_id"])
        if job["state"] in {"completed", "failed", "cancelled"}:
            break
        time.sleep(0.5)

    if job["state"] != "completed":
        print(f"error: the export ended as {job['state']}: {job.get('error')}", file=sys.stderr)
        return 1

    counts = job["counts"]
    print(
        f"  {job['state']}: {counts['rows']} rows from {counts['turns_included']} of "
        f"{counts['turns_considered']} turns"
    )
    for reason, count in sorted(job.get("exclusions", {}).items()):
        print(f"    left out: {count} {reason.replace('_', ' ')}")

    reference = f"{arguments.corpus}@{job['version']}"
    print(f"\nimmutable reference: {reference}")

    # 5. Read one row back, so the audit trail is visible rather than claimed.
    try:
        page = archive.get_rows(arguments.corpus, job["version"], limit=1)
    except ArchiveError as error:
        if error.code == "forbidden":
            print("(reading rows needs the admin role on an authenticated instance)")
            return 0
        raise
    if page["rows"]:
        eligibility = page["rows"][0].get("eligibility", [])
        first = eligibility[0] if isinstance(eligibility, list) else eligibility
        print("why the first row was eligible:")
        for field in ("consent_basis", "consent_reference", "reviewer", "run_id", "model"):
            print(f"  {field}: {first.get(field)}")

    print(
        "\nOpen Conversations → Review for the queue, and Conversations → Corpora "
        f"for {reference}."
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ArchiveError, RuntimeError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
