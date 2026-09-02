#!/usr/bin/env python3
"""Seed a corpus-sized annotation import: stage, page, queue, watch, read.

The whole of ADR_0022 in one run, against a server started with ``just run``.
It stages a batch with its rights, its evidence and the Hub commit it was read
at; appends the rows in pages, re-sending one to show that a numbered append is
a retry rather than a second copy; queues the job; waits for the worker; and
prints what was registered along with every row that was refused and why.

Two of the rows are refused on purpose, because a job's refusals are the half
somebody actually has to read:

* one names ``169.254.169.254`` — the cloud metadata service. Under ``just
  run`` it is refused because this instance has no image source at all; under
  ``just run-hubs`` it is refused by name, because "download this address for
  me" from inside a cluster is a request-forgery primitive rather than a
  convenience. Either way it is never fetched.
* one carries bytes the registry cannot measure, so the row's zero dimensions
  stay zero and it is refused rather than registered as a picture of nothing.

Nothing here is mocked and nothing is inserted behind the API.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

BASE = os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080")
PROJECT = os.environ.get("AIWATCHER_IMPORT_PROJECT", "demo/staged-import")
PANEL = os.environ.get("AIWATCHER_PANEL_URL", "http://127.0.0.1:5173")
#: Pages, and rows per page. Small enough to read, more than one so the resume
#: point is a real thing rather than a field.
PAGES, PER_PAGE = 3, 4


class ApiError(RuntimeError):
    pass


def call(method: str, path: str, body: Any = None, raw: bytes | None = None) -> Any:
    data = raw if raw is not None else (None if body is None else json.dumps(body).encode())
    headers = {"content-type": "image/png" if raw is not None else "application/json"}
    token = os.environ.get("AIWATCHER_TOKEN")
    if token:
        headers["authorization"] = f"Bearer {token}"
    request = urllib.request.Request(  # noqa: S310 - the base URL is the caller's own server
        BASE + path, data=data, method=method, headers=headers
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310
            return json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as error:
        raise ApiError(f"{method} {path} → {error.code}: {error.read().decode()[:300]}") from error
    except urllib.error.URLError as error:
        raise ApiError(f"{method} {path}: {error.reason}. Is `just run` up?") from error


def png(width: int, height: int, salt: int) -> bytes:
    """A PNG header, which is all the registry reads and all a size check needs."""
    return (
        b"\x89PNG\r\n\x1a\n"
        + bytes([0, 0, 0, 13])
        + b"IHDR"
        + width.to_bytes(4, "big")
        + height.to_bytes(4, "big")
        + bytes([salt])
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", default=PROJECT)
    args = parser.parse_args()

    print("1. a project to import into")
    call(
        "POST",
        "/api/v1/annotation-projects",
        {
            "name": args.project,
            "description": "A corpus arriving in pages",
            # No vocabulary ships with aiwatcher; a project brings its own.
            "classes": [
                {
                    "name": "edge",
                    "geometry": "polyline",
                    "color": "#334155",
                    "description": "",
                    "attributes": [],
                    "keypoints": [],
                    "optional_keypoints": [],
                    "links": [],
                    "ignore": False,
                    "layer": 0,
                }
            ],
            "splits": {"train": 70, "validation": 15, "test": 15},
            "split_salt": "2026-09",
            "split_overrides": {},
        },
    )

    print("2. stage the batch")
    batch = call(
        "POST",
        "/api/v1/annotation-import-batches",
        {
            "project": args.project,
            "description": "twelve pictures, three pages",
            # A claim, with a person behind it — and the evidence that says
            # who read the licence, where, and when. The hub's own card never
            # becomes this; see ADR_0019.
            "rights": {"kind": "licensed", "license": "CC BY 4.0"},
            "evidence": {
                "primary_source_url": "https://example.invalid/corpus/paper",
                "reviewed_by": "seed-script",
                "note": "read at the original, not on the mirror",
            },
            "source": {
                "hub": "huggingface",
                "dataset_id": "someone/pictures",
                # The field that makes provenance a commit rather than a name
                # that moves.
                "revision": "c0ffee1234567890",
                "config": "default",
                "split": "train",
            },
        },
    )
    print(f"   batch {batch['batch_id'][:12]}")

    print("3. append the pages")
    for page in range(PAGES):
        rows = []
        for index in range(PER_PAGE):
            salt = page * PER_PAGE + index
            stored = call("POST", "/api/v1/annotation-blobs", raw=png(64, 48, salt))
            rows.append(
                {
                    "image_id": stored["image_id"],
                    "uri": stored["uri"],
                    "width": 64,
                    "height": 48,
                    # The building, not the file. Two pictures share each
                    # family, so the split is a split of subjects.
                    "group_id": f"house-{salt // 2}",
                    "view": "plan",
                }
            )
        report = call(
            "POST",
            "/api/v1/annotation-import-rows",
            {"batch": batch["batch_id"], "page": page, "rows": rows},
        )
        print(f"   page {report['page']}: {report['rows']} rows, {report['total_rows']} so far")
        if page == 0:
            again = call(
                "POST",
                "/api/v1/annotation-import-rows",
                {"batch": batch["batch_id"], "page": 0, "rows": rows},
            )
            if again["created"] or again["total_rows"] != report["total_rows"]:
                print("✗ a re-sent page was stored twice", file=sys.stderr)
                return 1
            print("   page 0 re-sent: acknowledged, not duplicated")

    print("4. two rows that will be refused")
    unreadable = call("POST", "/api/v1/annotation-blobs", raw=b"%PDF-1.7\nnot a picture\n")
    call(
        "POST",
        "/api/v1/annotation-import-rows",
        {
            "batch": batch["batch_id"],
            "rows": [
                {
                    # No content address, and an address nobody allowlisted.
                    # Nothing fetches it either way — see the module docstring.
                    "uri": "https://169.254.169.254/latest/meta-data/",
                    "width": 0,
                    "height": 0,
                    "group_id": "house-0",
                },
                {
                    # Stored bytes that are not a picture, so nothing can fill
                    # in the dimensions the registry requires.
                    "image_id": unreadable["image_id"],
                    "uri": unreadable["uri"],
                    "width": 0,
                    "height": 0,
                    "group_id": "house-0",
                },
            ],
        },
    )

    print("5. queue the job")
    job = call(
        "POST",
        "/api/v1/annotation-import-jobs",
        {"batch": batch["batch_id"], "dry_run": False},
    )
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        found = call("GET", f"/api/v1/annotation-import-job?job_id={job['job_id']}")
        if found["state"] in ("completed", "failed", "cancelled"):
            break
        print(f"   {found['state']}: page {found['cursor']} of {found['pages']}")
        time.sleep(1)
    else:
        print("✗ the worker did not finish in 60s", file=sys.stderr)
        return 1

    print(f"   {found['state']}: {found['counts']['accepted']} registered, "
          f"{found['counts']['rejected']} refused")
    for warning in found.get("warnings", []):
        print(f"   ! {warning}")

    if found["counts"]["rejected"]:
        print("6. what it refused")
        for reason, count in found["rejects"].items():
            print(f"   {count} × {reason}")
        refused = call(
            "GET",
            f"/api/v1/annotation-import-rejects?job_id={job['job_id']}&limit=10",
        )
        for row in refused["rows"]:
            print(f"   {row['uri']}\n     {row['detail']}")

    if found["state"] != "completed":
        print(f"✗ the job ended as {found['state']}", file=sys.stderr)
        return 1
    if not found.get("version"):
        print("✗ a completed import has a version", file=sys.stderr)
        return 1

    print()
    print(f"✓ {args.project}@{found['version'][:12]}")
    print(f"  jobs    {PANEL}/annotations/imports?job={job['job_id']}")
    print(f"  images  {PANEL}/annotations/label?project={urllib.parse.quote(args.project)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
