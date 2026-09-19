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

Two sections at the end are gates. **M1** — "my project, my stream": the runs,
spans, metrics and live stream of one project, and what revoking a grant does to
somebody who is already watching. It takes about half a minute, because one of
its questions is what happens on the next re-check of an open stream rather than
on the next request. **M0** — "two clients, one organization": whether a
question asked as one client ever answers with the other's row, or with the
deployment's.

Against a deployment rather than a laptop, point all three at it:

    AIWATCHER_URL=https://aiwatcher.example \
    AIWATCHER_AUTHENTIK_URL=https://auth.example \
    python3 scripts/iam-permission-check.py

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

import hashlib
import json
import os
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from importlib import import_module

sso = import_module("sso-session")

# The issuer aiwatcher is configured with — which is the `provider` half of
# every principal, so a grant made against the wrong one names nobody. Read
# from the deployment rather than assumed, because the application slug differs
# between a laptop and a cluster that already had an `aiwatcher` application.
ISSUER = os.environ.get("AIWATCHER_ISSUER", "http://localhost:9000/application/o/aiwatcher/")
# What each fixture's password is: the username plus this, as
# `scripts/authentik-seed.py --suffix` writes them.
SUFFIX = os.environ.get("AIWATCHER_FIXTURE_SUFFIX", "-dev")
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

# What `just run-sso-iam` puts in AIWATCHER_AUTH_INGEST_TOKENS. Overridable,
# because a deployment's producer secrets are its own and the questions that
# need a run on each side cannot be asked without them.
GLOBAL_TOKEN = os.environ.get("AIWATCHER_GLOBAL_TOKEN", "0123456789abcdef0123456789abcdef")
PROJECT_TOKEN = os.environ.get("AIWATCHER_PROJECT_TOKEN", "fedcba9876543210fedcba9876543210")


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


def m1(
    checks: Checks,
    teacher,
    student,
    observer,
    student_subject: str,
    now: int,
) -> None:
    """The project half, and the instance half asked by somebody who holds one.

    `observer` is an instance viewer with no grant anywhere. Before IAM-03's D4
    that was `student` — everybody signed in held `viewer` — and the questions
    below that are about what an **instance** read answers now need a principal
    who can make one at all. That is the change, and it is why they are two
    people rather than one.
    """
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
        status, _ = sso.call(observer, "GET", f"/api/v1/runs/m1-project-{now}")
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

    instance = sso.stream(observer, f"/api/v1/events/stream?from={start}", 6.0)
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
        status, graphs = sso.call(observer, "GET", "/api/v1/workflows")
        names = [row.get("workflow_id") for row in graphs.get("workflows", [])] if status == 200 else []
        checks.that(
            status == 200 and f"m1-graph-{now}" not in names,
            "a project's workflow graph is on no instance list — that fold has the project in its row",
            f"{status} {f'm1-graph-{now}' in names}",
        )
        status, runs = sso.call(observer, "GET", "/api/v1/workflow-executions")
        ids = [row.get("workflow_run_id") for row in runs.get("executions", [])] if status == 200 else []
        checks.that(
            status == 200 and f"m1-folds-{now}" not in ids,
            "and neither is its execution, to somebody the project itself answers 404 to",
            f"{status} {f'm1-folds-{now}' in ids}",
        )
        status, reports = sso.call(observer, "GET", "/api/v1/evaluations")
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


# ── M0: two clients, two projects, one organization ──────────────────────────
#
# The gate IAM-03 calls M0, and it asks one thing in two directions: **no
# question a client asks answers with somebody else's row**. Somebody else is
# the other client, and it is also the deployment — a client on a shared
# instance holds no instance role at all (D4), so the unassigned side is as much
# not-theirs as the other project is.
#
# The instance half is walked from `contracts/openapi.json` rather than from a
# list written here, for the reason the panel's `SCOPED_ROUTES` is: a
# hand-written list goes stale in the direction nobody notices, which is the
# route somebody added last week. Against a deployment the checkout's contract
# may be ahead of what is running; a path that is not there answers 404, which
# is the same "no data" this is asking about.
#
# Like M1, two of the questions need a run on each client's side, and only a
# *credential* puts one there. So the first run prints the line and the second
# asks everything:
#
#     python3 scripts/iam-permission-check.py
#     AIWATCHER_M0_SCOPES=client-a=<org>/<a>,client-b=<org>/<b> just run-sso-iam
#     AIWATCHER_M0_SCOPES=… python3 scripts/iam-permission-check.py

CONTRACT = Path(__file__).resolve().parent.parent / "contracts" / "openapi.json"


def instance_reads() -> list[str]:
    """Every instance path in the contract a GET can be asked of as it stands.

    Parameterless, because a path parameter this made up would be refused for
    being made up and would say nothing about the boundary. What is left is the
    lists — which is what a client would land on.
    """
    if not CONTRACT.exists():
        return []
    document = json.loads(CONTRACT.read_text())
    return sorted(
        path
        for path, item in document["paths"].items()
        if "get" in item
        and "{" not in path
        and path.startswith("/api/v1/")
        and not path.startswith("/api/v1/auth/")
        and not path.startswith("/api/v1/iam/")
    )


def client_token(name: str) -> str:
    """The same secret `just run-sso-iam` derives, so the two agree by rule."""
    return hashlib.sha256(name.encode()).hexdigest()[:32]


def public(path: str, body: dict[str, object]) -> tuple[int, dict]:
    """A call with no session at all, which is what the two open routes take."""
    request = urllib.request.Request(
        sso.PANEL + path,
        method="POST",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request) as answer:
            return answer.status, json.loads(answer.read() or b"{}")
    except urllib.error.HTTPError as refused:
        return refused.code, json.loads(refused.read() or b"{}")


def newcomer(checks: Checks, teacher, org: str, project: str, now: int) -> None:
    """Somebody with a link, no account, and no way to be given one by hand.

    The whole of D5, asked end to end, because the half that decides it is
    **not in this repository**: an enrolment flow that put a new account in
    `aiwatcher-viewers` would give every client a role on the whole deployment
    and nothing here would fail. So this makes a real account through the
    provider's own flow and asks what it came out holding.
    """
    status, offer = sso.call(
        teacher,
        "POST",
        f"/api/v1/iam/organizations/{org}/projects/{project}/invitations",
        {"role": "viewer", "window": {"valid_from": now - DAY},
         "expires_at": now + DAY, "label": f"newcomer-{now}@example.test"},
    )
    if status != 201:
        checks.skipped("somebody with no account is enrolled by their invitation",
                       f"the offer was refused: {status}")
        return
    status, enrolment = public("/api/v1/iam/invitations/enrollment", {"token": offer["token"]})
    if status == 501:
        for question in (
            "somebody with no account is enrolled by their invitation",
            "and the account it made holds no instance role",
            "and the grant it redeems is the one the offer declared",
        ):
            checks.skipped(question, "AIWATCHER_AUTH_PROVISION_URL is not configured")
        return
    checks.that(
        status == 200 and enrolment.get("url", "").startswith(
            os.environ.get("AIWATCHER_AUTHENTIK_URL", "http://localhost:9000").rstrip("/")
        ),
        "somebody with no account is enrolled by their invitation",
        f"{status} {enrolment.get('url', enrolment)[:70]}",
    )
    if status != 200:
        return

    username = f"newcomer-{now}"
    try:
        session = sso.enrol(enrolment["url"], username, f"{username}@example.test", "a-long-enough-password")
    except Exception as refused:  # noqa: BLE001 - reported, not raised
        checks.skipped("and the account it made holds no instance role", str(refused)[:120])
        return
    status, identity = sso.call(session, "GET", "/api/v1/auth/me")
    checks.that(
        # `groups` is absent rather than empty when there are none, which is
        # the answer this is looking for either way.
        status == 200 and identity["roles"] == [] and not identity.get("groups"),
        "and the account it made holds no instance role and no group of this deployment's",
        f"{status} roles={identity.get('roles')} groups={identity.get('groups', [])}",
    )
    status, redeemed = sso.call(session, "POST", "/api/v1/iam/invitations/redeem",
                                {"token": offer["token"]})
    checks.that(
        status == 200 and redeemed["role"] == "viewer"
        and redeemed["project"]["scope"]["project"] == project,
        "and the grant it redeems is the one the offer declared",
        f"{status} {redeemed.get('role')}",
    )


def m0(checks: Checks, teacher, now: int) -> None:
    scopes = {}
    for entry in os.environ.get("AIWATCHER_M0_SCOPES", "").split(","):
        if "=" in entry:
            name, scope = entry.split("=", 1)
            scopes[name.strip()] = scope.strip()

    clients = ["client-a", "client-b"]
    sessions = {}
    for name in clients:
        try:
            sessions[name] = sso.sign_in(name, name + SUFFIX)
        except Exception as refused:  # noqa: BLE001 - reported, not raised
            checks.skipped(f"{name} signs in", f"{refused}")
            return
    subjects = {
        name: sso.call(session, "GET", "/api/v1/auth/me")[1]["subject"]
        for name, session in sessions.items()
    }

    # Every client holds no instance role. This is D4's whole premise, and it
    # is configuration outside this repository — an enrolment flow that put a
    # new account in `aiwatcher-viewers` would undo the isolation with nothing
    # here failing — so it is asked rather than assumed.
    roles = {
        name: sso.call(session, "GET", "/api/v1/auth/me")[1]["roles"]
        for name, session in sessions.items()
    }
    checks.that(
        all(not held for held in roles.values()),
        "a client signs in holding no instance role at all",
        roles,
    )

    if scopes:
        org = next(iter(scopes.values())).split("/", 1)[0]
        status, _ = sso.call(teacher, "GET", f"/api/v1/iam/organizations/{org}/roster")
        if status != 200:
            print(f"AIWATCHER_M0_SCOPES names {org}, which this teacher cannot administer "
                  f"({status}). Unset it to mint fresh ones.", file=sys.stderr)
            return
        projects = {name: scope.split("/", 1)[1] for name, scope in scopes.items()}
    else:
        status, organization = sso.call(
            teacher, "POST", "/api/v1/iam/organizations", {"name": f"AI Spirit {now}"}
        )
        if status != 201:
            print(f"could not create the shared organization: {status}", file=sys.stderr)
            return
        org = organization["id"]
        projects = {}
        for name in clients:
            _, created = sso.call(
                teacher, "POST", f"/api/v1/iam/organizations/{org}/commands",
                {"type": "create_project", "name": f"Demo for {name}"},
            )
            projects[name] = created["ProjectCreated"]["scope"]["project"]
        line = ",".join(f"{name}={org}/{projects[name]}" for name in clients)
        print(f"\n  to ask the questions that need each client's own run, restart\n"
              f"  the server and this script with AIWATCHER_M0_SCOPES={line}\n")

    # An invitation, because that is how a client who has never signed in gets
    # in — and a guest, because a client is not a colleague (D3).
    for name in clients:
        status, offer = sso.call(
            teacher,
            "POST",
            f"/api/v1/iam/organizations/{org}/projects/{projects[name]}/invitations",
            {"role": "editor", "window": {"valid_from": now - DAY},
             "expires_at": now + DAY, "label": f"{name}@localhost"},
        )
        if status != 201:
            checks.skipped(f"{name} is invited as a guest", f"the offer was refused: {status}")
            continue
        if name == clients[0]:
            # What a stranger sees before they have an account: the terms, with
            # no session at all, and looking costs the offer nothing.
            status, offered = public("/api/v1/iam/invitations/preview", {"token": offer["token"]})
            checks.that(
                status == 200 and offered.get("standing") == "guest"
                and offered.get("project", {}).get("scope", {}).get("project") == projects[name],
                "an invitation reads its own terms to somebody with no session and no account",
                f"{status} {offered.get('standing')}",
            )
        status, redeemed = sso.call(
            sessions[name], "POST", "/api/v1/iam/invitations/redeem", {"token": offer["token"]}
        )
        checks.that(
            status == 200 and redeemed["role"] == "editor",
            f"{name} redeems an invitation and holds an editor grant on their project",
            status,
        )

    newcomer(checks, teacher, org, projects[clients[0]], now)

    roster_status, roster = sso.call(teacher, "GET", f"/api/v1/iam/organizations/{org}/roster")
    guests = {entry["principal"]["subject"] for entry in roster.get("guests", [])}
    members = {entry["principal"]["subject"] for entry in roster.get("members", [])}
    checks.that(
        roster_status == 200
        and all(subjects[name] in guests for name in clients)
        and not any(subjects[name] in members for name in clients),
        "and is a guest in the roster, not a member of it",
        f"{roster_status} {len(guests)} guest(s), {len(members)} member(s)",
    )

    # One run per client, each published by that client's own token.
    produced = {}
    for name in clients:
        if name not in scopes:
            continue
        status, _ = sso.publish(client_token(name), run_events(f"m0-{name}-{now}", name))
        produced[name] = status == 202
        if not produced[name]:
            print(f"the {name} producer token was refused: {status}", file=sys.stderr)
    time.sleep(1.0)

    # ── Each client's own project, and the other's ───────────────────────────
    for name, other in [(clients[0], clients[1]), (clients[1], clients[0])]:
        session = sessions[name]
        mine = f"/api/v1/orgs/{org}/projects/{projects[name]}"
        theirs = f"/api/v1/orgs/{org}/projects/{projects[other]}"

        status, listed = sso.call(session, "GET", f"/api/v1/iam/organizations/{org}/projects")
        checks.that(
            status == 200
            and [entry["project"]["scope"]["project"] for entry in listed] == [projects[name]],
            f"{name} sees exactly one project in the shared organization",
            f"{status} {len(listed) if status == 200 else ''}",
        )
        status, _ = sso.call(session, "GET", f"/api/v1/iam/organizations/{org}/roster")
        checks.that(
            status == 403,
            f"{name} cannot read the roster — a guest administers nothing",
            status,
        )

        if produced.get(name) and produced.get(other):
            status, page = sso.call(session, "GET", f"{mine}/runs")
            ids = [run["run_id"] for run in page["runs"]] if status == 200 else []
            checks.that(
                status == 200 and f"m0-{name}-{now}" in ids and f"m0-{other}-{now}" not in ids,
                f"{name}'s runs are {name}'s, and hold none of {other}'s",
                f"{status} {ids}",
            )
            status, _ = sso.call(session, "GET", f"{mine}/runs/m0-{other}-{now}")
            checks.that(status == 404, f"{other}'s run is not in {name}'s project", status)
        else:
            for question in (
                f"{name}'s runs are {name}'s, and hold none of {other}'s",
                f"{other}'s run is not in {name}'s project",
            ):
                checks.skipped(question, "no producer token names each client's project")

        # Every scoped family, asked about the other client's project. Not the
        # whole contract: one route per store is what proves the resolver, and
        # the resolver is one.
        for family in ["runs", "spans", "metrics", "workflows", "evaluations",
                       "prompts", "datasets", "training-runs", "labs",
                       "evaluation-results", "annotation-projects"]:
            status, _ = sso.call(session, "GET", f"{theirs}/{family}")
            checks.that(
                status == 404,
                f"{name} asking for {other}'s {family} is told there is no such project",
                status,
            )

        # ── The deployment's own side ────────────────────────────────────────
        answered = []
        for path in instance_reads():
            status, _ = sso.call(session, "GET", path)
            if 200 <= status < 300:
                answered.append(f"{status} {path}")
        checks.that(
            not answered,
            f"and no instance route answers {name} at all "
            f"({len(instance_reads())} asked from the contract)",
            answered[:3] or "none answered",
        )

        status, stream_text = sso.call(session, "GET", "/api/v1/live/snapshot")
        checks.that(
            status != 200,
            f"{name} does not reach the instance's live snapshot either",
            status,
        )


def main() -> int:
    checks = Checks()
    now = int(time.time())

    teacher = sso.sign_in("teacher", "teacher" + SUFFIX)
    student = sso.sign_in("student", "student" + SUFFIX)
    observer = sso.sign_in("observer", "observer" + SUFFIX)
    _, teacher_identity = sso.call(teacher, "GET", "/api/v1/auth/me")
    _, student_identity = sso.call(student, "GET", "/api/v1/auth/me")
    student_subject = student_identity["subject"]
    checks.that(
        teacher_identity["roles"] == ["admin"],
        "a group this deployment maps is the instance role somebody holds",
        teacher_identity["roles"],
    )
    # And its other half, which is what AIWATCHER_AUTH_DEFAULT_ROLE decides.
    # Asked as a consequence rather than as a value, so the same question holds
    # against a deployment that chose either: whoever holds no instance role
    # reaches no instance route, and whoever holds `viewer` reaches them all.
    unmapped = student_identity["roles"]
    status, _ = sso.call(student, "GET", "/api/v1/runs")
    checks.that(
        (status == 200) == bool(unmapped),
        "and somebody in no mapped group reaches the deployment's own runs only if "
        "AIWATCHER_AUTH_DEFAULT_ROLE gave them a role",
        f"{unmapped or 'no role'} → {status}",
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
    stranger = sso.sign_in("student", "student" + SUFFIX)
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

    m1(checks, teacher, student, observer, student_subject, now)
    m0(checks, teacher, now)

    return checks.report()


if __name__ == "__main__":
    sys.exit(main())
