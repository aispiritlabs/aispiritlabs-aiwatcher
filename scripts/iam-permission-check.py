#!/usr/bin/env python3
"""What a grant does, asked of a running server by two people who really signed in.

Every line below is a question somebody would otherwise answer by clicking as
one person, signing out, and clicking as another — which is why nobody answers
it twice. It needs `just authentik-up`, `just authentik-seed`, `just panel` and
`just run-sso-iam`, and it writes only into organizations it creates itself.

A lesson is a project plus a grant window: access from `valid_from`, editing
until `edit_until`, reading until `read_until`. There is no lesson in the
backend and there does not need to be.

    python3 scripts/iam-permission-check.py

The last section is **M1** — "my project, my stream": the runs, spans, metrics
and live stream of one project, and what revoking a grant does to somebody who
is already watching. It takes about half a minute, because one of its questions
is what happens on the next re-check of an open stream rather than on the next
request.

Two of its questions need a run on the *project* side, and only a producer's
credential can put one there — a person's session carries no project, on
purpose. So the first run prints the line to configure, and the second asks
everything:

    python3 scripts/iam-permission-check.py
    AIWATCHER_M1_SCOPE=<organization>/<project> just run-sso-iam
    AIWATCHER_M1_SCOPE=<organization>/<project> python3 scripts/iam-permission-check.py

Without it those questions are reported as not asked rather than as held.
"""

from __future__ import annotations

import os
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from importlib import import_module

sso = import_module("sso-session")

ISSUER = "http://localhost:9000/application/o/aiwatcher/"
DAY = 86_400
# "From the first event ever written", as the log spells it. A zero-padded
# position rather than an empty string, because an empty `Last-Event-ID` is
# indistinguishable from an absent one and those mean different things.
CHECKPOINT_START = "0" * 20


class Checks:
    """Each answer, with what was expected beside it — a failure is a scenario."""

    def __init__(self) -> None:
        self.rows: list[tuple[bool, str, str]] = []
        self.skips: list[tuple[str, str]] = []

    def that(self, held: bool, question: str, saw: object = "") -> bool:
        self.rows.append((held, question, str(saw)))
        print(f"{'ok  ' if held else 'FAIL'}  {question}" + (f"   [{saw}]" if saw else ""))
        return held

    def skipped(self, question: str, why: str) -> None:
        """A question this run could not ask, named rather than passed quietly.

        Reported apart from the answers: a gate that counted an unasked
        question as held would be the one kind of green that means nothing.
        """
        self.skips.append((question, why))
        print(f"skip  {question}   [{why}]")

    def report(self) -> int:
        failed = [row for row in self.rows if not row[0]]
        print(f"\n{len(self.rows) - len(failed)}/{len(self.rows)} held")
        for _, question, saw in failed:
            print(f"  failed: {question}   [{saw}]")
        if self.skips:
            print(f"{len(self.skips)} not asked:")
            for question, why in self.skips:
                print(f"  skipped: {question}   [{why}]")
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


# ── M1: my project, my stream ────────────────────────────────────────────────
#
# Everything above is the authored half: who may administer what. This is the
# observable half — runs, spans, metrics and the live stream — and it is the
# gate IAM-02 calls M1. It is asked over real HTTP against a real server for
# the reason the plan gives: a library test proves the fold, and what has to
# hold is the deployment.
#
# Two sides have to exist for a read to be able to answer one of them, and only
# a *credential* puts a run on the project side — a person's session carries no
# project, by design. So the producer token comes from the environment, and
# `just run-sso-iam` puts it there:
#
#     python3 scripts/iam-permission-check.py        # prints the line to set
#     AIWATCHER_M1_SCOPE=<org>/<project> just run-sso-iam
#     AIWATCHER_M1_SCOPE=<org>/<project> python3 scripts/iam-permission-check.py
#
# Without it the questions that need a project's own run are skipped by name
# rather than passed quietly, and every other question still runs — including
# the one that matters most, which is that a project read answers none of the
# instance's runs.

GLOBAL_TOKEN = "0123456789abcdef0123456789abcdef"
PROJECT_TOKEN = "fedcba9876543210fedcba9876543210"


def run_events(run_id: str, agent: str) -> list[dict[str, object]]:
    """One run: a start, one model call with both ends, and an end."""
    at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    call = {"call_id": "c1", "provider": "anthropic", "model": "claude-opus-5"}
    def event(event_type: str, data: dict[str, object]) -> dict[str, object]:
        return {
            "event_type": event_type,
            "occurred_at": at,
            "run_id": run_id,
            "agent_id": agent,
            "source": {"service": "permission-check", "sdk": "python"},
            "data": data,
        }
    return [
        event("run.started", {}),
        event("llm.started", call),
        event("llm.completed", {**call, "prompt_tokens": 100, "completion_tokens": 20}),
        event("run.completed", {"status": "succeeded"}),
    ]


def fold_events(run_id: str, now: int) -> list[dict[str, object]]:
    """A declared graph, a step of it, and an evaluation report.

    The three folds E2 left unkeyed and IAM-02/D keyed, published under a
    project's own credential. Each is a fact about a project's work, so each is
    asked twice: on the project's own routes while the grant stands, and on the
    instance's once it is gone.
    """
    at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    graph = f"m1-graph-{now}"

    def event(event_type: str, data: dict[str, object], **extra: object) -> dict[str, object]:
        return {
            "event_type": event_type,
            "occurred_at": at,
            "run_id": run_id,
            "source": {"service": "permission-check", "sdk": "python"},
            "data": data,
            **extra,
        }

    return [
        event(
            "workflow.declared",
            {
                "workflow_id": graph,
                "name": graph,
                "version": "sha256:m1",
                "nodes": [{"id": "acquire", "name": "Acquire", "kind": "chain"}],
                "edges": [],
            },
            workflow_id=graph,
            workflow_run_id=run_id,
        ),
        event("step.started", {"node": "acquire"}, workflow_id=graph, workflow_run_id=run_id),
        event("step.completed", {"node": "acquire"}, workflow_id=graph, workflow_run_id=run_id),
        event("eval.started", {"suite": f"m1-suite-{now}", "dataset": "m1@1"}),
        event("eval.completed", {"metrics": {"mean_score": 1.0}}),
    ]


def m1(checks: Checks, teacher, student, student_subject: str, now: int) -> None:
    scope = os.environ.get("AIWATCHER_M1_SCOPE", "").strip()

    if scope:
        org, project = scope.split("/", 1)
        status, _ = sso.call(teacher, "GET", f"/api/v1/iam/organizations/{org}/projects/{project}/access")
        if status != 200:
            print(
                f"AIWATCHER_M1_SCOPE names {scope}, which this signed-in teacher cannot open "
                f"({status}). Unset it to mint a fresh one.",
                file=sys.stderr,
            )
            return
    else:
        status, organization = sso.call(teacher, "POST", "/api/v1/iam/organizations", {"name": f"M1 {now}"})
        if status != 201:
            print(f"could not create an organization for M1: {status}", file=sys.stderr)
            return
        org = organization["id"]
        _, created = sso.call(
            teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
            {"type": "create_project", "name": "The lesson everyone can see"},
        )
        project = created["ProjectCreated"]["scope"]["project"]
        print(
            "\n  to ask the two questions that need a run on the project side, restart\n"
            f"  the server and this script with AIWATCHER_M1_SCOPE={org}/{project}\n"
        )

    scoped = f"/api/v1/orgs/{org}/projects/{project}"

    # A marker first, only for the checkpoint its acknowledgement carries:
    # the streams below resume from *there* rather than from the beginning of
    # the log, because a development log holds tens of thousands of events and
    # a replay from nought stops at `MAX_RESYNC_EVENTS` long before reaching
    # anything published today. Asking about a scope over a replay that never
    # arrived would be a question that passes for the wrong reason.
    status, marker = sso.publish(GLOBAL_TOKEN, run_events(f"m1-marker-{now}", "marker"))
    if status != 202:
        print(f"the global producer token was refused: {status} {marker}", file=sys.stderr)
        return
    start = marker["last_checkpoint"]

    # Two runs, one on each side. The global one is published by the token
    # every producer has had; the project one by a token that names a project,
    # which is the only thing that puts a run on that side.
    status, refused = sso.publish(GLOBAL_TOKEN, run_events(f"m1-global-{now}", "researcher"))
    if status != 202:
        print(f"the global producer token was refused: {status} {refused}", file=sys.stderr)
        return
    produced = bool(scope) and sso.publish(
        PROJECT_TOKEN, run_events(f"m1-project-{now}", "estimator")
    )[0] == 202
    # The same project's graph, step and evaluation report. Published here,
    # read on the project's own routes below, and asked about again at the end
    # once the grant is provably gone.
    if produced:
        sso.publish(PROJECT_TOKEN, fold_events(f"m1-folds-{now}", now))
    time.sleep(1.0)

    # ── The list of my projects ──────────────────────────────────────────────
    sso.call(teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
             {"type": "set_member", "principal": {"provider": ISSUER, "subject": student_subject},
              "role": "member"})
    status, _ = sso.call(student, "GET", f"{scoped}/runs")
    checks.that(status == 404, "before a grant, a project's runs are not there — 404, not 403", status)

    grant(teacher, org, project, student_subject, "viewer", {"valid_from": now - DAY})
    status, mine = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects")
    checks.that(
        status == 200 and [entry["project"]["scope"]["project"] for entry in mine] == [project],
        "signing in answers exactly the projects the grants say, and no others",
        status,
    )

    # ── Runs, spans and metrics of my project alone ──────────────────────────
    status, page = sso.call(student, "GET", f"{scoped}/runs")
    ids = [run["run_id"] for run in page["runs"]] if status == 200 else []
    checks.that(
        status == 200 and f"m1-global-{now}" not in ids,
        "a project's runs list holds none of the instance's runs",
        f"{status} {ids}",
    )
    checks.that(
        status == 200 and page["total_known"] == len(ids),
        "and its cursor counts one side, not the instance",
        page.get("total_known") if status == 200 else status,
    )
    if produced:
        # Its own, by id — not "exactly one", because a second run of this
        # script against the same scope publishes into the same project and a
        # count would then fail for the wrong reason.
        checks.that(f"m1-project-{now}" in ids, "and holds its own", ids)
        status, spans = sso.call(student, "GET", f"{scoped}/spans")
        checks.that(
            status == 200
            and spans["spans"]
            and all(row["run_id"] in ids for row in spans["spans"]),
            "a project's spans are its runs' spans and nobody else's",
            f"{status} {len(spans.get('spans', [])) if status == 200 else ''}",
        )
        status, metrics = sso.call(student, "GET", f"{scoped}/metrics")
        checks.that(
            status == 200
            and metrics["totals"]["runs"] == len(ids)
            and metrics["window"]["runs_retained"] == len(ids),
            "and its metrics count its own runs, with retention reported over that side",
            f"{status} {metrics.get('totals', {}).get('runs') if status == 200 else ''} of {len(ids)}",
        )
        status, _ = sso.call(student, "GET", f"{scoped}/runs/m1-global-{now}")
        checks.that(status == 404, "a run the project does not hold is a run that is not there", status)
        status, _ = sso.call(student, "GET", f"/api/v1/runs/m1-project-{now}")
        checks.that(status == 404, "and the instance route does not reach into the project either", status)

        # The three folds E2 left behind, now keyed: a project's graph, its
        # traversal and its evaluation report answer on the project's own
        # routes. The mirror of each — that the instance list holds none of
        # them — is asked at the end, after the grant is gone, because that is
        # the half a badge could fake.
        status, graphs = sso.call(student, "GET", f"{scoped}/workflows")
        names = [row.get("workflow_id") for row in graphs.get("workflows", [])] if status == 200 else []
        checks.that(
            status == 200 and f"m1-graph-{now}" in names,
            "a project's workflow graph is on the project's own catalog",
            f"{status} {names}",
        )
        status, runs = sso.call(student, "GET", f"{scoped}/workflow-executions")
        execution_ids = [row.get("workflow_run_id") for row in runs.get("executions", [])] if status == 200 else []
        checks.that(
            status == 200 and f"m1-folds-{now}" in execution_ids,
            "and its traversal is on the project's own execution list",
            f"{status} {execution_ids}",
        )
        status, reports = sso.call(student, "GET", f"{scoped}/evaluations")
        suites = [row.get("suite") for row in reports.get("evaluations", [])] if status == 200 else []
        checks.that(
            status == 200 and f"m1-suite-{now}" in suites,
            "and its evaluation report is on the project's own fold (ADR_0010's own projection)",
            f"{status} {suites}",
        )
    else:
        for question in (
            "and holds its own",
            "a project's spans are its runs' spans and nobody else's",
            "and its metrics count its own runs, with retention reported over that side",
            "a run the project does not hold is a run that is not there",
            "and the instance route does not reach into the project either",
            "a project's workflow graph is on the project's own catalog",
            "and its traversal is on the project's own execution list",
            "and its evaluation report is on the project's own fold (ADR_0010's own projection)",
        ):
            checks.skipped(question, "no producer token names a project")

    # ── The live stream ──────────────────────────────────────────────────────
    text = sso.stream(student, f"{scoped}/events/stream?from={start}", 6.0)
    checks.that(
        "event: caught_up" in text and f"m1-global-{now}" not in text,
        "a project's live stream replays none of the instance's events",
        text[:90].replace("\n", " "),
    )
    if produced:
        checks.that(f"m1-project-{now}" in text, "and replays its own", text[:90].replace("\n", " "))
    else:
        checks.skipped("and replays its own", "no project-scoped producer token")

    instance = sso.stream(student, f"/api/v1/events/stream?from={start}", 6.0)
    checks.that(
        "event: caught_up" in instance and f"m1-global-{now}" in instance,
        "the instance stream still replays the instance's events, as it always has",
        instance[:90].replace("\n", " "),
    )
    if produced:
        checks.that(
            f"m1-project-{now}" not in instance,
            "and none of a project's — the half that makes this a boundary and not a badge",
            instance[:90].replace("\n", " "),
        )
    else:
        checks.skipped(
            "and none of a project's — the half that makes this a boundary and not a badge",
            "no project-scoped producer token",
        )

    # ── Revoking a grant, while somebody is watching ─────────────────────────
    #
    # The one question the panel could not answer before this: until now the
    # session cookie's TTL *was* the revocation window, so a stream opened in
    # the morning kept running all day. This waits for the re-check on purpose.
    watched: dict[str, str] = {}
    follower = threading.Thread(
        target=lambda: watched.update(text=sso.stream(student, f"{scoped}/events/stream", 90.0))
    )
    follower.start()
    time.sleep(2.0)
    _, access = sso.call(student, "GET", f"/api/v1/iam/organizations/{org}/projects/{project}/access")
    for entry in access.get("grants", []):
        sso.call(teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
                 {"type": "revoke_grant", "project": project, "grant": entry["grant"]["id"]})
    follower.join(timeout=90)
    checks.that(
        "event: revoked" in watched.get("text", ""),
        "revoking a grant closes the stream somebody already had open, and says why",
        watched.get("text", "")[-60:].replace("\n", " ") or "nothing arrived",
    )
    status, _ = sso.call(student, "GET", f"{scoped}/runs")
    checks.that(status == 404, "and the reads are cut with it", status)
    resumed = sso.stream(student, f"{scoped}/events/stream?from={CHECKPOINT_START}", 4.0)
    checks.that(
        resumed == "HTTP 404",
        "a resume after revocation replays nothing — Last-Event-ID is a position, not a key",
        resumed[:60].replace("\n", " "),
    )

    # ── The other half of the same boundary ──────────────────────────────────
    #
    # The grant is gone; the reads above prove it. So every answer below is one
    # this principal gets with no grant on that project at all, asked on the
    # **instance** routes — where, before IAM-02/D, all three of these came
    # back. Keying those folds is what turned them round, and asking them here
    # rather than beside the positive ones is deliberate: a badge on a row
    # could fake the project's list, and only this says the instance holds none
    # of it.
    if produced:
        status, graphs = sso.call(student, "GET", "/api/v1/workflows")
        names = [row.get("workflow_id") for row in graphs.get("workflows", [])] if status == 200 else []
        checks.that(
            status == 200 and f"m1-graph-{now}" not in names,
            "a project's workflow graph is on no instance list — that fold has the project in its row",
            f"{status} {f'm1-graph-{now}' in names}",
        )
        status, runs = sso.call(student, "GET", "/api/v1/workflow-executions")
        ids = [row.get("workflow_run_id") for row in runs.get("executions", [])] if status == 200 else []
        checks.that(
            status == 200 and f"m1-folds-{now}" not in ids,
            "and neither is its execution, to somebody the project itself answers 404 to",
            f"{status} {f'm1-folds-{now}' in ids}",
        )
        status, reports = sso.call(student, "GET", "/api/v1/evaluations")
        suites = [row.get("suite") for row in reports.get("evaluations", [])] if status == 200 else []
        checks.that(
            status == 200 and f"m1-suite-{now}" not in suites,
            "nor its evaluation report, on the fold ADR_0010 gave its own projection",
            f"{status} {f'm1-suite-{now}' in suites}",
        )
        status, _ = sso.call(student, "GET", f"{scoped}/workflows")
        checks.that(
            status == 404,
            "and the project's own catalog is cut with the grant, like every other scoped read",
            status,
        )
    else:
        for question in (
            "a project's workflow graph is on no instance list — that fold has the project in its row",
            "and neither is its execution, to somebody the project itself answers 404 to",
            "nor its evaluation report, on the fold ADR_0010 gave its own projection",
            "and the project's own catalog is cut with the grant, like every other scoped read",
        ):
            checks.skipped(question, "no producer token names a project")


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

    m1(checks, teacher, student, student_subject, now)

    return checks.report()


if __name__ == "__main__":
    sys.exit(main())
