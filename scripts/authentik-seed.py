#!/usr/bin/env python3
"""The people a grant needs, and the account aiwatcher enrols with.

A permission is a relation between two principals, so testing one takes
accounts that have each completed a real sign-in. `akadmin` is authentik's
superuser and stays out of it; these are ordinary users:

    teacher   in aiwatcher-admins → aiwatcher's `admin`, may create an organization
    student   in no group         → aiwatcher's `viewer` today, and `project`
                                    under AIWATCHER_AUTH_DEFAULT_ROLE=project
    client-a  in no group         → one demo client
    client-b  in no group         → the other, who must never see the first's rows

It also mints the service account aiwatcher provisions enrolments with
(IAM-03 D5) and prints its token: it may add an invitation to the enrolment
flow the blueprint creates, and it deliberately holds no other permission —
a token that could create users would make aiwatcher a way into this provider.

Idempotent: it creates what is missing and resets every password, so running it
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
    # An instance viewer with no grant anywhere: the principal the "an
    # instance read answers none of a project's rows" questions need, and the
    # one `student` used to be before a client stopped holding an instance
    # role by default (IAM-03 D4).
    ("observer", "Instance observer", "observer@localhost", "aiwatcher-viewers"),
    # The two clients of the M0 gate. In no group on purpose: what makes the
    # demo honest is that neither holds an instance role, so each sees their
    # own project and nothing of the deployment's.
    ("client-a", "Client A", "client-a@localhost", None),
    ("client-b", "Client B", "client-b@localhost", None),
]

# What aiwatcher presents to open an enrolment. Its permission is one line, and
# the point of the line is what it leaves out.
PROVISIONER = "aiwatcher-provisioner"
# Two, and the second is why the first is safe: adding an invitation, and
# reading flows so the invitation can be *bound* to the enrolment one. Without
# the read the invitation would have to be created unbound, which any invitation
# stage in this provider would then accept. Neither lets it touch a user.
PROVISION_PERMISSIONS = [
    "authentik_stages_invitation.add_invitation",
    "authentik_flows.view_flow",
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

    print()
    print(f"AIWATCHER_AUTH_PROVISION_TOKEN={provisioner()}")
    print("  (with AIWATCHER_AUTH_PROVISION_URL=http://localhost:9000 and")
    print("   AIWATCHER_AUTH_PROVISION_FLOW=aiwatcher-enrolment; `just run-sso-iam` sets all three)")
    return 0


def provisioner() -> str:
    """The service account aiwatcher enrols with, and its one permission.

    A service account rather than a user with a password: nobody signs in as
    this, and authentik's own `type: service_account` is what says so. The
    permission is assigned directly rather than through a group, because a
    group here is a role on the whole deployment and this account holds none.
    """
    found = call("GET", f"/core/users/?username={PROVISIONER}")["results"]
    user = found[0] if found else call(
        "POST",
        "/core/users/",
        {
            "username": PROVISIONER,
            "name": "aiwatcher enrolment",
            "type": "service_account",
            "is_active": True,
            "groups": [],
        },
    )
    call(
        "POST",
        f"/rbac/permissions/assigned_by_users/{user['pk']}/assign/",
        {"permissions": PROVISION_PERMISSIONS},
    )
    # One token, created once and read back afterwards. Not re-minted each run:
    # a second run of this script while a server is holding the first one would
    # leave that server presenting a token authentik had already forgotten, and
    # the symptom is a 502 from an enrolment somebody is looking at.
    if not call("GET", f"/core/tokens/?identifier={PROVISIONER}-token")["results"]:
        call(
            "POST",
            "/core/tokens/",
            {
                "identifier": f"{PROVISIONER}-token",
                "intent": "api",
                "user": user["pk"],
                "description": "aiwatcher opens the enrolment flow to one person",
                "expiring": False,
            },
        )
    return call("GET", f"/core/tokens/{PROVISIONER}-token/view_key/")["key"]


if __name__ == "__main__":
    sys.exit(main())
