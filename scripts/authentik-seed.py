#!/usr/bin/env python3
"""The two people a grant needs, in the local authentik.

A permission is a relation between two principals, so testing one takes two
accounts that have each completed a real sign-in. `akadmin` is authentik's
superuser and stays out of it; these two are ordinary users:

    teacher   in aiwatcher-admins → aiwatcher's `admin`, may create an organization
    student   in no group         → aiwatcher's `viewer`, sees nothing until granted

Idempotent: it creates what is missing and resets both passwords, so running it
twice is how you recover from having forgotten one.

Uses the bootstrap API token the compose file sets, which exists only because
this authentik is a development one.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request

AUTHENTIK = "http://localhost:9000/api/v3"
TOKEN = "aiwatcher-dev-bootstrap"

PEOPLE = [
    ("teacher", "Workshop teacher", "teacher@localhost", "aiwatcher-admins"),
    ("student", "Workshop student", "student@localhost", None),
]


def call(method: str, path: str, body: dict | None = None):
    request = urllib.request.Request(
        AUTHENTIK + path,
        data=json.dumps(body).encode() if body is not None else None,
        method=method,
        headers={"Authorization": f"Bearer {TOKEN}", "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request) as answer:
        raw = answer.read()
        return json.loads(raw) if raw else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--suffix",
        default="-dev",
        help="what each password is: the username plus this (default: -dev)",
    )
    arguments = parser.parse_args()

    try:
        groups = {group["name"]: group["pk"] for group in call("GET", "/core/groups/?page_size=50")["results"]}
    except urllib.error.HTTPError as refused:
        # The blueprint is applied a few seconds after the containers report
        # healthy, and until it is there is no token and no group to find.
        print(
            f"authentik answered {refused.code}: it is up but not seeded yet — "
            "wait a few seconds and run this again",
            file=sys.stderr,
        )
        return 1
    except urllib.error.URLError as unreachable:
        print(f"authentik is not answering on :9000 ({unreachable.reason}); just authentik-up", file=sys.stderr)
        return 1

    for username, name, email, group in PEOPLE:
        found = call("GET", f"/core/users/?username={username}")["results"]
        user = found[0] if found else call(
            "POST",
            "/core/users/",
            {
                "username": username,
                "name": name,
                "email": email,
                "is_active": True,
                "type": "internal",
                "groups": [groups[group]] if group else [],
            },
        )
        call("POST", f"/core/users/{user['pk']}/set_password/", {"password": f"{username}{arguments.suffix}"})
        fresh = call("GET", f"/core/users/{user['pk']}/")
        held = ", ".join(entry["name"] for entry in fresh["groups_obj"]) or "no groups"
        print(f"{username}: password {username}{arguments.suffix}, {held}")
        print(f"  subject {fresh['uid']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
