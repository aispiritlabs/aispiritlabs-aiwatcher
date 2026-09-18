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
import sys
import urllib.error
import urllib.parse
import urllib.request

# The panel's dev server, not :8080 — the redirect URI on the provider is a
# :5173 one, because that is the origin a browser sees when /api is proxied.
PANEL = "http://localhost:5173"
AUTHENTIK = "http://localhost:9000"
FLOW = "default-authentication-flow"

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
    session = urllib.request.build_opener(
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
    )
    landed = _open(session, f"{PANEL}/api/v1/auth/login?next={urllib.parse.quote(next_path)}")
    flow_url = landed.geturl()
    if "/if/flow/" not in flow_url:
        raise SignInFailed(f"expected authentik's flow, landed on {flow_url}")

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
