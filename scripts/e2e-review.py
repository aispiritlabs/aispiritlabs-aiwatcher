#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Feedback into a regression case, through people (FTI C4), against a server of its own.

A curation dataset of four capitals is what runs are measured on. Somebody
notices the application named Mombasa for Kenya's capital in a trace, and:

1. proposes it as a case — and proposing it again lands on that review, not a
   second one;
2. nothing publishes before a person writes the expected answer and another
   approves it: publishing with nothing approved is refused;
3. writes the expected answer, approves it, and publishes: the dataset gains a
   version holding the four cases and the reviewed one, named for the review;
4. publishing the same approved set again lands on that version;
5. a cohort derived from the new version holds five cases, the reviewed one
   with its question and the answer people wrote;
6. the review says which version it was published in, and refuses to change.

    cargo build --bin aiwatcher   # once
    just e2e-review
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("AIWATCHER_BINARY", ROOT / "target" / "debug" / "aiwatcher"))
BASE = ""


def serve(home: Path) -> subprocess.Popen[bytes]:
    global BASE
    if not BINARY.exists():
        raise SystemExit(f"no server binary at {BINARY}: `cargo build --bin aiwatcher`")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    home.mkdir(parents=True)
    env = {name: value for name, value in os.environ.items() if not name.startswith("AIWATCHER_")}
    env |= {
        "AIWATCHER_LISTEN": f"127.0.0.1:{port}",
        "AIWATCHER_DATA_DIR": str(home / ".data"),
        "AIWATCHER_BUS": "wal",
        "AIWATCHER_SEED_FILE": "none",
        "AIWATCHER_LOG": "warn",
    }
    log = (home / "server.log").open("wb")
    process = subprocess.Popen(  # noqa: S603 — the binary this repository builds
        [str(BINARY)], cwd=home, env=env, stdout=log, stderr=subprocess.STDOUT
    )
    BASE = f"http://127.0.0.1:{port}"
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if process.poll() is not None:
            break
        if call("GET", "/readyz")[0] == 200 and call("GET", "/api/v1/datasets")[0] == 200:
            return process
        time.sleep(0.2)
    process.kill()
    raise SystemExit(f"aiwatcher did not come up on {BASE}")


def call(method: str, path: str, body: Any = None) -> tuple[int, Any]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method)  # noqa: S310 — our own server
    request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310 — our own server
            content, status = response.read(), response.status
    except urllib.error.HTTPError as error:
        content, status = error.read(), error.code
    except (urllib.error.URLError, ConnectionError):
        return 0, None
    try:
        return status, json.loads(content) if content else None
    except json.JSONDecodeError:
        return status, None


def main() -> int:
    failures: list[str] = []

    def check(number: int, claim: str, holds: bool, detail: object = "") -> None:
        print(
            f"  {'✓' if holds else '✗'} {number}. {claim}"
            + (f" — {detail}" if detail not in ("", None) else "")
        )
        if not holds:
            failures.append(claim)

    home = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-review-"))
    server = serve(home / "server")
    query = "?dataset=" + urllib.parse.quote("capitals", safe="")
    try:
        rows = [
            {
                "case_id": f"capital-{country.lower()}",
                "input": {"question": f"What is the capital of {country}?"},
                "expected": {"answer": capital},
            }
            for country, capital in (
                ("France", "Paris"),
                ("Japan", "Tokyo"),
                ("Peru", "Lima"),
                ("Chile", "Santiago"),
            )
        ]
        status, first = call(
            "POST",
            "/api/v1/datasets",
            {
                "name": "capitals",
                "pipeline": "data_frame()->read(capitals)",
                "columns": ["case_id", "input", "expected"],
                "items": rows,
                "source": hashlib.sha256(json.dumps(rows).encode()).hexdigest(),
            },
        )
        if status not in (200, 201):
            raise SystemExit(f"publishing the cases answered {status}: {first}")
        proposal = {
            "dataset": "capitals",
            "target": {"kind": "trace", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"},
            "question": "What is the capital of Kenya?",
            "answer": "Mombasa",
            "note": "a user said the answer was wrong",
            "content": "written",
        }
        print("\na trace, a review and a new version of the cases:")
        proposed = call("POST", "/api/v1/evaluation-reviews", proposal)
        again = call("POST", "/api/v1/evaluation-reviews", proposal | {"note": "said again"})
        check(
            1,
            "proposing what is under review lands on that review",
            proposed[0] == 201
            and again[0] == 200
            and again[1]["review"]["id"] == proposed[1]["review"]["id"],
            {"first": proposed[0], "again": again[0]},
        )
        review = proposed[1]["review"]

        early = call("POST", "/api/v1/evaluation-reviews/publish" + query)
        check(
            2,
            "nothing publishes before a person approves",
            early[0] == 422,
            (early[1] or {}).get("message"),
        )

        call(
            "POST",
            f"/api/v1/evaluation-reviews/{review['id']}/actions{query}",
            {"action": "expect", "expected": "Nairobi"},
        )
        approved = call(
            "POST",
            f"/api/v1/evaluation-reviews/{review['id']}/actions{query}",
            {"action": "approve"},
        )
        published = call("POST", "/api/v1/evaluation-reviews/publish" + query)
        latest = (published[1] or {}).get("dataset", {}).get("dataset", {}).get("latest", {})
        check(
            3,
            "an approved case publishes as a new version of the cases, named for the review",
            approved[1]["state"] == "approved"
            and published[0] == 200
            and latest.get("row_count") == 5
            and latest.get("produced_by") == "evaluation-reviews/capitals"
            and latest.get("version") != first["dataset"]["latest"]["version"],
            {"rows": latest.get("row_count"), "produced_by": latest.get("produced_by")},
        )

        # The review is published now, so publishing again has nothing approved;
        # the version the cases are in is what the dataset still names.
        repeat = call("POST", "/api/v1/evaluation-reviews/publish" + query)
        heads = call("GET", "/api/v1/datasets")[1]["datasets"]
        head = next(dataset for dataset in heads if dataset["name"] == "capitals")
        check(
            4,
            "publishing again adds no version",
            repeat[0] == 422
            and head["latest"]["version"] == latest.get("version")
            and len(head["versions"]) == 2,
            {"repeat": repeat[0], "versions": len(head["versions"])},
        )

        dataset = {"kind": "curation", "name": "capitals", "version": latest.get("version")}
        cohort = call("POST", "/api/v1/evaluation-cohorts", {"dataset": dataset, "split": "test"})
        rows_now = call(
            "GET", f"/api/v1/dataset-rows?name=capitals&version={latest.get('version')}&limit=10"
        )[1]
        reviewed = [
            entry["row"]
            for entry in (rows_now or {}).get("rows", [])
            if str(entry["row"].get("case_id", "")).startswith("review-")
        ]
        check(
            5,
            "a cohort derived from the new version holds the reviewed case",
            cohort[0] == 200
            and cohort[1]["cohort"]["case_count"] == 5
            and len(reviewed) == 1
            and reviewed[0]["expected"] == {"answer": "Nairobi"},
            {
                "case_count": (cohort[1] or {}).get("cohort", {}).get("case_count"),
                "reviewed": reviewed[:1],
            },
        )

        page = call("GET", "/api/v1/evaluation-reviews" + query)[1]
        item = page["items"][0]
        changed = call(
            "POST",
            f"/api/v1/evaluation-reviews/{review['id']}/actions{query}",
            {"action": "reject", "reason": "on second thought"},
        )
        check(
            6,
            "the review names its version and a published case does not change",
            item["state"] == "published"
            and item["published_in"] == latest.get("version")
            and changed[0] == 400,
            {"state": item["state"], "refused": changed[0]},
        )
    finally:
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
        if failures:
            print(f"\nkept {home} for the server log")
        else:
            shutil.rmtree(home, ignore_errors=True)

    if failures:
        print(f"\n✗ {len(failures)} claim(s) did not hold")
        return 1
    print("\n✓ a trace became a case people wrote and approved, in a new version of the cases")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
