"""The transport the four registry clients share.

Worth its own file because the decision in it is not "does a request work" but
"what may be sent twice". Every test below is a case where the four
hand-rolled clients this replaced would have done something different from
each other.

Driven through `httpx.MockTransport` rather than a socket: what is being
tested is the policy, and a real server would only add the ways a socket can
be slow.
"""

from __future__ import annotations

from collections.abc import Callable
from datetime import UTC, datetime, timedelta
from email.utils import format_datetime
from typing import Any

import httpx
import pytest

from aiwatcher_sdk.api import ApiError, Transport, _retry_after


class RefusalError(ApiError):
    """A client's own error type, which is what the transport must raise."""


def transport(
    handler: Callable[[httpx.Request], httpx.Response],
    *,
    base_url: str = "http://aiwatcher.invalid",
    token: str | None = None,
    attempts: int = 3,
    hints: dict[str, str] | None = None,
) -> Transport:
    return Transport(
        base_url,
        token=token,
        attempts=attempts,
        error=RefusalError,
        subject="the registry",
        hints=hints,
        client=httpx.Client(transport=httpx.MockTransport(handler)),
    )


def counting(*responses: httpx.Response | Exception) -> tuple[Callable[..., Any], list[str]]:
    """A handler that answers each of `responses` in turn, and records paths."""
    seen: list[str] = []
    queue = list(responses)

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(str(request.url))
        answer = queue.pop(0) if queue else httpx.Response(200, json={})
        if isinstance(answer, Exception):
            raise answer
        return answer

    return handler, seen


# ── What may be repeated ─────────────────────────────────────────────────────


def test_a_503_is_retried_because_it_applied_nothing() -> None:
    handler, seen = counting(
        httpx.Response(503, json={"code": "unavailable", "message": "starting"}),
        httpx.Response(200, json={"ok": True}),
    )
    assert transport(handler).json("POST", "/api/v1/prompts", {}) == {"ok": True}
    assert len(seen) == 2


def test_a_refusal_is_not_retried_and_carries_every_problem() -> None:
    # A 422 is a decision the server will make identically forever. Retrying it
    # is what a pipeline does instead of telling somebody what is wrong.
    handler, seen = counting(
        httpx.Response(
            422,
            json={
                "code": "annotation_rejected",
                "message": "the annotation was refused",
                "details": ["door_1: missing the keypoint hinge", "wall_3: thickness_px"],
            },
        )
    )
    with pytest.raises(RefusalError) as failure:
        transport(handler).json("POST", "/api/v1/annotation-revisions", {})

    assert len(seen) == 1
    assert failure.value.status == 422
    assert failure.value.code == "annotation_rejected"
    assert len(failure.value.details) == 2
    assert failure.value.is_retryable is False


def test_a_read_timeout_is_repeated_for_a_get_and_not_for_a_bare_post() -> None:
    # The distinction the clients this replaced did not make. A read that timed
    # out was *sent*: the server may have applied it and the answer is what went
    # missing, so repeating it is safe only where applying it twice is.
    handler, seen = counting(httpx.ReadTimeout("slow"), httpx.Response(200, json={"ok": True}))
    assert transport(handler).json("GET", "/api/v1/prompts") == {"ok": True}
    assert len(seen) == 2

    handler, seen = counting(httpx.ReadTimeout("slow"))
    with pytest.raises(RefusalError, match="unreachable"):
        transport(handler).json("POST", "/api/v1/conversation-turns", {})
    assert len(seen) == 1


def test_a_post_that_says_it_is_idempotent_is_repeated() -> None:
    # Every write in this API is content addressed or keyed, so the ones that
    # are say so and get the same treatment as a GET.
    handler, seen = counting(httpx.ReadTimeout("slow"), httpx.Response(201, json={"ok": True}))
    sent = transport(handler).json(
        "POST", "/api/v1/annotation-blobs", content=b"a plan", idempotent=True
    )
    assert sent == {"ok": True}
    assert len(seen) == 2


def test_a_connection_that_never_landed_is_repeated_whatever_the_method() -> None:
    # Nothing reached the server, so there is nothing to apply twice.
    handler, seen = counting(httpx.ConnectError("refused"), httpx.Response(200, json={"ok": True}))
    assert transport(handler).json("POST", "/api/v1/training-runs", {}) == {"ok": True}
    assert len(seen) == 2


def test_attempts_are_bounded_and_the_last_failure_is_what_is_raised() -> None:
    handler, seen = counting(*[httpx.Response(503, json={"message": "still starting"})] * 5)
    with pytest.raises(RefusalError) as failure:
        transport(handler, attempts=3).json("GET", "/api/v1/prompts")

    assert len(seen) == 3
    assert failure.value.status == 503
    assert failure.value.is_retryable is True


# ── Retry-After ──────────────────────────────────────────────────────────────


def test_retry_after_is_read_in_both_the_forms_it_is_written() -> None:
    assert _retry_after("2") == 2.0
    assert _retry_after(None) is None
    assert _retry_after("whenever") is None
    later = _retry_after(format_datetime(datetime.now(UTC) + timedelta(seconds=30)))
    assert later is not None
    assert 25.0 < later <= 30.0


def test_a_throttled_request_carries_what_the_server_asked_for() -> None:
    handler, _ = counting(
        httpx.Response(429, json={"message": "slow down"}, headers={"retry-after": "7"})
    )
    with pytest.raises(RefusalError) as failure:
        transport(handler, attempts=1).json("GET", "/api/v1/prompts")
    assert failure.value.retry_after == 7.0


# ── Where the token goes ─────────────────────────────────────────────────────


def test_the_token_reaches_this_registry_and_nowhere_else() -> None:
    # An image registered by reference is fetched from somebody else's host.
    # Sending this deployment's bearer token there would hand it to whoever
    # runs it.
    headers: list[str | None] = []

    def handler(request: httpx.Request) -> httpx.Response:
        headers.append(request.headers.get("authorization"))
        return httpx.Response(200, content=b"pixels")

    client = transport(handler, token="secret")
    client.read("/api/v1/annotation-blobs/abc")
    client.read("https://someone-elses-mirror.invalid/plan.png")

    assert headers == ["Bearer secret", None]


def test_a_redirect_is_not_followed() -> None:
    # A redirect is how a request that passed an origin check arrives somewhere
    # else, with the header the check permitted.
    handler, seen = counting(httpx.Response(302, headers={"location": "http://169.254.169.254/"}))
    with pytest.raises(RefusalError) as failure:
        transport(handler, token="secret").read("/api/v1/annotation-blobs/abc")

    assert len(seen) == 1
    assert failure.value.status == 302
    # And says so, rather than handing back the empty body a 3xx carries.
    assert "169.254.169.254" in str(failure.value)


# ── The JSON contract ────────────────────────────────────────────────────────


def test_an_empty_body_is_an_answer_and_a_non_object_is_not() -> None:
    handler, _ = counting(httpx.Response(204))
    assert transport(handler).json("POST", "/api/v1/prompts", {}) == {}

    handler, _ = counting(httpx.Response(200, json=[1, 2, 3]))
    with pytest.raises(RefusalError, match="expected an object"):
        transport(handler).json("GET", "/api/v1/prompts")


def test_html_from_a_proxy_names_what_arrived_rather_than_failing_three_frames_later() -> None:
    handler, _ = counting(
        httpx.Response(200, content=b"<html>gateway</html>", headers={"content-type": "text/html"})
    )
    with pytest.raises(RefusalError, match="text/html"):
        transport(handler).json("GET", "/api/v1/prompts")


def test_a_hint_replaces_the_message_for_the_code_it_names() -> None:
    # "this instance has no store" is a deployment decision, and the message
    # that helps names the variable rather than restating the status.
    handler, _ = counting(
        httpx.Response(501, json={"code": "registry_disabled", "message": "not configured"})
    )
    client = transport(handler, hints={"registry_disabled": "set AIWATCHER_PROMPT_STORE"})
    with pytest.raises(RefusalError, match="AIWATCHER_PROMPT_STORE"):
        client.json("GET", "/api/v1/prompts")


def test_a_query_is_encoded_by_the_client_and_a_none_is_not_sent() -> None:
    handler, seen = counting(httpx.Response(200, json={}))
    transport(handler).json(
        "GET",
        "/api/v1/annotation-export",
        params={"project": "floor-plans/dom", "export": "9f", "split": None},
    )
    assert seen == [
        "http://aiwatcher.invalid/api/v1/annotation-export?project=floor-plans%2Fdom&export=9f"
    ]


def test_a_transport_closes_the_pool_it_opened_and_leaves_a_borrowed_one() -> None:
    borrowed = httpx.Client(transport=httpx.MockTransport(lambda _: httpx.Response(200, json={})))
    with Transport("http://aiwatcher.invalid", error=RefusalError, client=borrowed):
        pass
    assert not borrowed.is_closed

    owned = Transport("http://aiwatcher.invalid", error=RefusalError)
    with owned:
        pass
    assert owned._client.is_closed
