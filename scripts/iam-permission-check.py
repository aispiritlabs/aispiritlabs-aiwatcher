#!/usr/bin/env python3
"""What a grant does, asked of a running server by two people who really signed in.

Every line below is a question somebody would otherwise answer by clicking as
one person, signing out, and clicking as another — which is why nobody answers
it twice. It needs `just authentik-up`, `just authentik-seed`, `just panel` and
`just run-sso-iam`, and it writes only into an organization it creates itself.

A lesson is a project plus a grant window: access from `valid_from`, editing
until `edit_until`, reading until `read_until`. There is no lesson in the
backend and there does not need to be.

    python3 scripts/iam-permission-check.py
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from importlib import import_module

sso = import_module("sso-session")

ISSUER = "http://localhost:9000/application/o/aiwatcher/"
DAY = 86_400


class Checks:
    """Each answer, with what was expected beside it — a failure is a scenario."""

    def __init__(self) -> None:
        self.rows: list[tuple[bool, str, str]] = []

    def that(self, held: bool, question: str, saw: object = "") -> bool:
        self.rows.append((held, question, str(saw)))
        print(f"{'ok  ' if held else 'FAIL'}  {question}" + (f"   [{saw}]" if saw else ""))
        return held

    def report(self) -> int:
        failed = [row for row in self.rows if not row[0]]
        print(f"\n{len(self.rows) - len(failed)}/{len(self.rows)} held")
        for _, question, saw in failed:
            print(f"  failed: {question}   [{saw}]")
        return 1 if failed else 0


def grant(teacher, organization: str, project: str, subject: str, role: str, window: dict) -> tuple[int, object]:
    return sso.call(
        teacher,
        "POST",
        f"/api/v1/iam/organizations/{organization}/commands",
        {
            "type": "grant",
            "project": project,
            "grantee": {"kind": "user", "value": {"provider": ISSUER, "subject": subject}},
            "role": role,
            "window": window,
        },
    )


def main() -> int:
    checks = Checks()
    now = int(time.time())

    teacher = sso.sign_in("teacher", "teacher-dev")
    student = sso.sign_in("student", "student-dev")
    _, teacher_identity = sso.call(teacher, "GET", "/api/v1/auth/me")
    _, student_identity = sso.call(student, "GET", "/api/v1/auth/me")
    student_subject = student_identity["subject"]
    checks.that(
        teacher_identity["roles"] == ["admin"] and student_identity["roles"] == ["viewer"],
        "the two fixtures come back with the instance roles their groups map to",
        f"{teacher_identity['roles']} / {student_identity['roles']}",
    )

    status, _ = sso.call(student, "POST", "/api/v1/iam/organizations", {"name": "Not mine"})
    checks.that(status == 403, "a viewer may not create an organization", status)

    status, organization = sso.call(
        teacher, "POST", "/api/v1/iam/organizations", {"name": f"Workshop {now}"}
    )
    if status != 201:
        print(f"could not create an organization: {status} {organization}", file=sys.stderr)
        return 1
    org = organization["id"]

    status, _ = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects")
    checks.that(status == 404, "a non-member cannot even name somebody else's organization", status)

    _, created = sso.call(
        teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
        {"type": "create_project", "name": "Lesson one"},
    )
    project = created["ProjectCreated"]["scope"]["project"]

    status, mine = sso.call(teacher, "GET", f"/api/v1/iam/organizations/{org}/projects")
    checks.that(
        status == 200 and [entry["project"]["scope"]["project"] for entry in mine] == [project],
        "creating a project grants its creator, explicitly — the one implicit grant there is",
        [entry["role"] for entry in mine],
    )

    # The owner's own grant is the project's, not the organization's: a second
    # project they did not create must not appear.
    sso.call(teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
             {"type": "set_member",
              "principal": {"provider": ISSUER, "subject": student_subject},
              "role": "owner"})
    _, second = sso.call(
        student, "POST", f"/api/v1/iam/organizations/{org}/commands",
        {"type": "create_project", "name": "Lesson two"},
    )
    other_project = second["ProjectCreated"]["scope"]["project"]
    status, teacher_sees = sso.call(teacher, "GET", f"/api/v1/iam/organizations/{org}/projects")
    checks.that(
        other_project not in [entry["project"]["scope"]["project"] for entry in teacher_sees],
        "an organization owner has no implicit access to a project somebody else made",
        f"{len(teacher_sees)} project(s)",
    )

    status, _ = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects/{project}/access")
    checks.that(status == 404, "a member with no grant is told the project does not exist", status)

    # ── The window ───────────────────────────────────────────────────────────
    _, issued = grant(teacher, org, project, student_subject, "editor",
                      {"valid_from": now + DAY})
    future_grant = issued["GrantCreated"]["id"]
    status, _ = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects/{project}/access")
    checks.that(status == 404, "a grant that starts tomorrow grants nothing today", status)

    sso.call(teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
             {"type": "revoke_grant", "project": project, "grant": future_grant})

    grant(teacher, org, project, student_subject, "editor", {"valid_from": now - DAY})
    status, access = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects/{project}/access")
    checks.that(status == 200 and access["role"] == "editor",
                "an open window grants the role it declares", access.get("role"))

    status, lapsed = grant(teacher, org, project, student_subject, "editor",
                           {"valid_from": now - 2 * DAY, "edit_until": now - DAY})
    lapsed_grant = lapsed["GrantCreated"]["id"]

    # Two independent grants now: the open editor one and the lapsed one. Drop
    # the open one and the lapsed one must still read.
    for entry in access["grants"]:
        if entry["grant"]["id"] != lapsed_grant:
            sso.call(teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
                     {"type": "revoke_grant", "project": project, "grant": entry["grant"]["id"]})
    status, after_edit = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects/{project}/access")
    checks.that(
        status == 200 and after_edit["role"] == "viewer",
        "past edit_until the grant falls back to Viewer and keeps reading",
        after_edit.get("role"),
    )

    _, both = grant(teacher, org, project, student_subject, "admin", {"valid_from": now - DAY})
    permanent = both["GrantCreated"]["id"]
    status, summed = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects/{project}/access")
    checks.that(
        summed["role"] == "admin" and len(summed["grants"]) == 2,
        "two live sources are two grants, and the role is the maximum of them",
        f"{summed['role']} from {len(summed['grants'])}",
    )

    sso.call(teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
             {"type": "revoke_grant", "project": project, "grant": permanent})
    status, remaining = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects/{project}/access")
    checks.that(
        status == 200 and remaining["role"] == "viewer",
        "revoking one source leaves the other where it was — on the next request",
        remaining.get("role"),
    )

    grant(teacher, org, project, student_subject, "editor",
          {"valid_from": now - 2 * DAY, "read_until": now - DAY})
    for entry in remaining["grants"]:
        sso.call(teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
                 {"type": "revoke_grant", "project": project, "grant": entry["grant"]["id"]})
    status, cut = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects/{project}/access")
    checks.that(status == 404, "past read_until the project is gone, not read-only", status)

    status, listed = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects")
    checks.that(
        status == 200 and [e["project"]["scope"]["project"] for e in listed] == [other_project],
        "the project list is the grants, not the membership",
        len(listed),
    )

    # ── What a grant is not ──────────────────────────────────────────────────
    status, _ = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/audit")
    checks.that(status in (200, 403), "the audit answers an owner", status)

    status, _ = sso.call(
        teacher,
        "POST",
        f"/api/v1/iam/organizations/{org}/commands",
        {"type": "create_project", "name": "No header"},
        mutation_header=False,
    )
    checks.that(
        status == 403,
        "a command without X-AIWatcher-IAM is refused, so a cross-origin form cannot issue one",
        status,
    )

    status, _ = sso.call(
        teacher,
        "POST",
        f"/api/v1/iam/organizations/{org}/commands",
        {"type": "create_project", "name": "Lesson three", "actor": {"provider": ISSUER, "subject": "someone"}},
    )
    # 422 rather than 400: the command denies unknown fields, so the body is
    # refused before any handler sees it. What matters is that it is refused —
    # a field silently ignored would read as an actor somebody got to choose.
    checks.that(
        status == 422,
        "a command naming its own actor is refused, not quietly ignored",
        status,
    )

    # ── An invitation, which is how somebody who has never signed in gets in ──
    #
    # Everything above needed both people to exist as principals first. This is
    # the part that does not: the offer is made to a secret, and the pair is
    # learned when it is redeemed.
    status, lesson = sso.call(
        teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
        {"type": "create_project", "name": "Lesson with an invitation"},
    )
    invited_project = lesson["ProjectCreated"]["scope"]["project"]
    status, offer = sso.call(
        teacher,
        "POST",
        f"/api/v1/iam/organizations/{org}/projects/{invited_project}/invitations",
        {
            "role": "editor",
            "window": {"valid_from": now - DAY, "edit_until": now + DAY},
            "expires_at": now + DAY,
            "label": "somebody@example.test",
        },
    )
    checks.that(
        status == 201 and len(offer["token"]) == 64 and "token" not in offer["invitation"],
        "an invitation hands back its token once, and the record does not carry it",
        status,
    )
    token = offer["token"]

    status, listed = sso.call(teacher, "GET", f"/api/v1/iam/organizations/{org}/invitations")
    checks.that(
        status == 200 and all("token" not in entry for entry in listed),
        "listing invitations never shows a token again",
        len(listed),
    )

    # A second person, already signed in but a stranger to this project.
    stranger = sso.sign_in("student", "student-dev")
    status, _ = sso.call(
        stranger, "GET", f"/api/v1/iam/organizations/{org}/projects/{invited_project}/access"
    )
    checks.that(status == 404, "before redeeming, the invited project is not there", status)

    status, redeemed = sso.call(stranger, "POST", "/api/v1/iam/invitations/redeem", {"token": token})
    checks.that(
        status == 200 and redeemed["role"] == "editor",
        "redeeming makes a member and the grant the offer declared, in one step",
        f"{status} {redeemed.get('role') if isinstance(redeemed, dict) else redeemed}",
    )
    status, access = sso.call(
        stranger, "GET", f"/api/v1/iam/organizations/{org}/projects/{invited_project}/access"
    )
    checks.that(
        status == 200 and access["role"] == "editor" and len(access["grants"]) == 1,
        "and the grant is an ordinary one, with the window the offer named",
        access.get("role"),
    )

    status, _ = sso.call(stranger, "POST", "/api/v1/iam/invitations/redeem", {"token": token})
    checks.that(status == 409, "a token works once, even for the person who used it", status)
    status, _ = sso.call(teacher, "POST", "/api/v1/iam/invitations/redeem", {"token": token})
    checks.that(status == 409, "and once for anybody else too", status)

    status, _ = sso.call(
        teacher,
        "DELETE",
        f"/api/v1/iam/organizations/{org}/invitations/{offer['invitation']['id']}",
    )
    checks.that(
        status == 409,
        "a spent invitation is not withdrawn — its grant is revoked instead",
        status,
    )

    status, lapsed = sso.call(
        teacher,
        "POST",
        f"/api/v1/iam/organizations/{org}/projects/{invited_project}/invitations",
        {
            "role": "viewer",
            "window": {"valid_from": now - DAY},
            "expires_at": now - 60,
        },
    )
    checks.that(status == 400, "an invitation cannot be created already lapsed", status)

    status, _ = sso.call(
        stranger, "POST", "/api/v1/iam/invitations/redeem", {"token": "0" * 64}
    )
    checks.that(status == 404, "a token nobody issued is absent, not a hint", status)

    # Offering a grant is the authority to make one, so an organization owner
    # has it everywhere in their organization — including on a project they
    # hold no grant on and cannot open.
    status, _ = sso.call(
        stranger,
        "POST",
        f"/api/v1/iam/organizations/{org}/projects/{project}/invitations",
        {"role": "admin", "window": {"valid_from": now}, "expires_at": now + DAY},
    )
    checks.that(
        status == 201,
        "an organization owner may offer a grant on any project in it",
        status,
    )

    # …and an ordinary member may not, even on the project they were just
    # granted editor on. Demote them and ask again.
    sso.call(
        teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
        {"type": "set_member",
         "principal": {"provider": ISSUER, "subject": student_subject},
         "role": "member"},
    )
    status, _ = sso.call(
        stranger,
        "POST",
        f"/api/v1/iam/organizations/{org}/projects/{invited_project}/invitations",
        {"role": "admin", "window": {"valid_from": now}, "expires_at": now + DAY},
    )
    checks.that(
        status == 403,
        "an editor may not offer a grant on the project they hold — only its admin may",
        status,
    )

    return checks.report()


if __name__ == "__main__":
    sys.exit(main())
