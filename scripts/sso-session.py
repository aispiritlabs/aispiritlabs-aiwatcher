#!/usr/bin/env python3
"""Sign in to a local `just run-sso` the way a browser does, without one.

Two people cannot share one browser profile, and a permission matrix wants to
be re-run rather than re-clicked — so this drives the whole authorization-code
flow with PKCE through authentik's own flow-executor API and hands back an
opener holding the aiwatcher session cookie.

It is a development tool against a development identity provider, and the
passwords it takes are the fixtures `just authentik-seed` writes. Nothing here
belongs anywhere near a real one.

    python3 scripts/sso-session.py teacher     # prints /api/v1/auth/me

Standard library only, so it runs with no virtualenv in a checkout that has
never installed anything.
"""

from __future__ import annotations

import argparse
import http.cookiejar
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

# The panel's dev server, not :8080 — the redirect URI on the provider is a
# :5173 one, because that is the origin a browser sees when /api is proxied.
#
# Overridable, because the same questions have to be askable of a deployment:
# the M0 gate is run against `vps` as well as against a laptop, and a matrix
# that could only be asked locally would be proving the wrong thing.
PANEL = os.environ.get("AIWATCHER_URL", "http://localhost:5173").rstrip("/")
AUTHENTIK = os.environ.get("AIWATCHER_AUTHENTIK_URL", "http://localhost:9000").rstrip("/")
FLOW = os.environ.get("AIWATCHER_AUTHENTIK_FLOW", "default-authentication-flow")

Session = urllib.request.OpenerDirector


class SignInFailed(RuntimeError):
    """The flow did not end at aiwatcher's callback."""


def _open(session: Session, url: str, headers: dict[str, str] | None = None):
    return session.open(urllib.request.Request(url, headers=headers or {}))


def _post(session: Session, url: str, body: dict[str, object], referer: str) -> dict:
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json",
            "Referer": referer,
        },
    )
    return json.loads(session.open(request).read())


def sign_in(username: str, password: str, next_path: str = "/account") -> Session:
    """One person's session, from the login route to the callback."""
    return authorize(
        urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
        ),
        username,
        password,
        next_path,
    )


def authorize(
    session: Session,
    username: str | None = None,
    password: str | None = None,
    next_path: str = "/account",
) -> Session:
    """Take whatever `session` already is through aiwatcher's sign-in.

    Two callers, and the second is why this is not just [`sign_in`]: somebody
    who has this moment made an account through the provider's own enrolment is
    **already signed in there**, so the authorization leg answers with a
    redirect rather than a login form, and a helper that insisted on the form
    would fail on the one path D5 exists to make work.
    """
    landed = _open(session, f"{PANEL}/api/v1/auth/login?next={urllib.parse.quote(next_path)}")
    flow_url = landed.geturl()
    if "/if/flow/" not in flow_url:
        # Already authenticated at the provider: the authorization code came
        # straight back and the cookie is set.
        return session
    if username is None or password is None:
        raise SignInFailed(f"the provider asked for a login and none was given: {flow_url}")

    # The executor wants the flow page's own query string as one `query`
    # parameter. Passing `next=` straight through loses it, and the flow then
    # ends at authentik's user page instead of coming back here — with no
    # error, because nothing failed.
    executor = f"{AUTHENTIK}/api/v3/flows/executor/{FLOW}/?" + urllib.parse.urlencode(
        {"query": urllib.parse.urlparse(flow_url).query}
    )

    stage = json.loads(_open(session, executor, {"Accept": "application/json"}).read())
    for _ in range(8):
        match stage.get("component"):
            case "ak-stage-identification":
                stage = _post(session, executor, {"uid_field": username}, flow_url)
            case "ak-stage-password":
                stage = _post(session, executor, {"password": password}, flow_url)
            case "xak-flow-redirect":
                target = stage["to"]
                _open(session, target if target.startswith("http") else AUTHENTIK + target)
                return session
            case other:
                raise SignInFailed(f"unexpected stage {other}: {json.dumps(stage)[:300]}")
    raise SignInFailed("the flow did not finish")


def enrol(enrolment_url: str, username: str, email: str, password: str) -> Session:
    """Make an account through the provider's own enrolment, as a person would.

    The other half of [`sign_in`] and the reason the invitation flow is worth
    driving from here: what makes a client of one project isolated is that the
    account they end up with holds **no aiwatcher group** (IAM-03 D4, D5), and
    that is configuration in the identity provider rather than anything this
    repository can assert. So the gate makes a real account and asks.
    """
    session = urllib.request.build_opener(
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
    )
    parsed = urllib.parse.urlparse(enrolment_url)
    slug = parsed.path.rstrip("/").rsplit("/", 1)[-1]
    executor = f"{AUTHENTIK}/api/v3/flows/executor/{slug}/?" + urllib.parse.urlencode(
        {"query": parsed.query}
    )
    stage = json.loads(_open(session, executor, {"Accept": "application/json"}).read())
    for _ in range(8):
        match stage.get("component"):
            case "ak-stage-prompt":
                stage = _post(
                    session,
                    executor,
                    {
                        "username": username,
                        "email": email,
                        "password": password,
                        "password_repeat": password,
                    },
                    enrolment_url,
                )
            case "xak-flow-redirect":
                target = stage["to"]
                _open(session, target if target.startswith("http") else AUTHENTIK + target)
                # The account exists and this session is signed in *there*. What
                # it does not yet have is an aiwatcher cookie, which is the
                # ordinary authorization-code leg with no form to fill in.
                return authorize(session)
            case other:
                raise SignInFailed(f"unexpected enrolment stage {other}: {json.dumps(stage)[:300]}")
    raise SignInFailed("the enrolment flow did not finish")


def call(
    session: Session,
    method: str,
    path: str,
    body: dict[str, object] | None = None,
    *,
    mutation_header: bool = True,
) -> tuple[int, object]:
    """One API call, with the status kept — a 403 here is an answer, not a crash."""
    request = urllib.request.Request(
        PANEL + path,
        method=method,
        data=json.dumps(body).encode() if body is not None else None,
    )
    request.add_header("Content-Type", "application/json")
    # Not a simple header, so a cross-origin form cannot carry it. Required on
    # every IAM mutation and harmless on a read; `mutation_header=False` is for
    # asking what happens without it.
    if mutation_header:
        request.add_header("X-AIWatcher-IAM", "1")
    try:
        with session.open(request) as answer:
            raw = answer.read()
            return answer.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as refused:
        raw = refused.read()
        try:
            return refused.code, json.loads(raw)
        except ValueError:
            return refused.code, raw.decode(errors="replace")[:300]


def publish(token: str, events: list[dict[str, object]]) -> tuple[int, object]:
    """Publish a batch as a producer would: a shared secret, no session.

    The one thing a browser cannot do here, and the reason the M1 gate needs
    it: which project an event belongs to is the credential's word, never the
    body's, and a person's session carries no project at all.
    """
    request = urllib.request.Request(
        PANEL + "/api/v1/events",
        method="POST",
        data=json.dumps({"events": events}).encode(),
    )
    request.add_header("Content-Type", "application/json")
    request.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(request) as answer:
            raw = answer.read()
            return answer.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as refused:
        return refused.code, refused.read().decode(errors="replace")[:300]


def stream(session: Session, path: str, seconds: float) -> str:
    """Follow an SSE body for a while and hand back what arrived.

    A live stream does not end, so this reads until the clock runs out — or
    until the server closes it, which is the answer the revocation question is
    asking for. The socket timeout is shorter than the deadline so a quiet
    stream still comes back on time; the server sends a keep-alive every
    fifteen seconds, which is what keeps a quiet one from looking dead.
    """
    request = urllib.request.Request(PANEL + path)
    request.add_header("Accept", "text/event-stream")
    try:
        answer = session.open(request, timeout=min(seconds, 20.0))
    except urllib.error.HTTPError as refused:
        return f"HTTP {refused.code}"
    except (TimeoutError, OSError) as unreachable:
        return f"unreachable: {unreachable}"
    deadline = time.monotonic() + seconds
    text = ""
    try:
        while time.monotonic() < deadline:
            line = answer.readline()
            if not line:
                break
            text += line.decode(errors="replace")
    except (TimeoutError, OSError):
        pass
    finally:
        answer.close()
    return text


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("username", nargs="?", default="teacher")
    parser.add_argument("--password", help="defaults to <username>-dev, the seeded fixture")
    arguments = parser.parse_args()

    session = sign_in(arguments.username, arguments.password or f"{arguments.username}-dev")
    status, identity = call(session, "GET", "/api/v1/auth/me")
    print(json.dumps(identity, indent=2))
    return 0 if status == 200 else 1


if __name__ == "__main__":
    sys.exit(main())
