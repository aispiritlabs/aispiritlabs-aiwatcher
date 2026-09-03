"""The HTTP client the registry clients share.

Four clients in this SDK read and write aiwatcher's REST API —
:mod:`~aiwatcher_sdk.annotations`, :mod:`~aiwatcher_sdk.prompts`,
:mod:`~aiwatcher_sdk.training` and :mod:`~aiwatcher_sdk.conversations`. They had
four copies of the same forty lines: build a request, open it, catch
``HTTPError``, parse ``{"code", "message"}``, decide what a status means. Four
copies of one decision is four places for it to be made differently, and the
decision here is the retry policy — the thing most worth getting right once.

This module is deliberately **not** imported by :mod:`aiwatcher_sdk`. That
package is the telemetry client, it is imported into every instrumented agent
process, and it stays on ``urllib`` for the two reasons it always did: it must
never take an agent down, and it must not force an ``httpx`` version on a
process that already pinned one. A training job that reads an export has
neither problem.

What the registry clients get from being here:

* **One connection, kept.** An export of six hundred images is six hundred
  requests to one host; a new TCP and TLS handshake for each of them is the
  largest cost in the loop and nothing was reusing one.
* **One retry policy, and it knows what it may repeat.** A ``503`` is retried
  for any method, because nothing was applied. A read timeout is retried only
  for a request that is safe to send twice — see :data:`IDEMPOTENT_METHODS`,
  and ``idempotent=True`` for the content-addressed ``POST``\\ s that are.
* **``Retry-After`` is obeyed.** A server that says how long to wait is
  answering the question the backoff was guessing at.
* **A token goes to one origin.** :meth:`Transport.read` fetches image bytes
  from wherever a corpus points, which is frequently not this server, and the
  ``Authorization`` header is attached only to the registry's own origin.
  Redirects are not followed, so a redirect cannot move the request to an
  origin the check already passed.
"""

from __future__ import annotations

import email.utils
import os
import random
from collections.abc import Callable, Mapping, Sequence
from datetime import UTC, datetime
from types import TracebackType
from typing import Any, Final, Self

import httpx
from tenacity import RetryCallState, Retrying, retry_if_exception, stop_after_attempt

__all__ = [
    "IDEMPOTENT_METHODS",
    "RETRYABLE_STATUSES",
    "ApiError",
    "Transport",
]

#: Statuses worth coming back from. Everything else is a decision the server
#: has already made and will make again — a ``422`` is a drawing that does not
#: validate and a ``501`` is an instance built without the store, and retrying
#: either forever is what a pipeline does instead of telling somebody.
RETRYABLE_STATUSES: Final = frozenset({408, 429, 500, 502, 503, 504})

#: Methods a read timeout may be repeated on.
#:
#: The distinction the four hand-rolled clients did not make. A connection that
#: was never established can be retried whatever the method, because nothing
#: reached the server. A read that timed out is different: the request was
#: sent, the server may well have applied it, and the reply is what went
#: missing. Repeating that is safe for a ``GET`` and for a ``PUT`` that sets a
#: label to a value; it is not safe in general for a ``POST``, which is why the
#: ones that *are* safe say so — every write in this API that is content
#: addressed passes ``idempotent=True``.
IDEMPOTENT_METHODS: Final = frozenset({"GET", "HEAD", "OPTIONS", "PUT", "DELETE"})

_DEFAULT_ATTEMPTS: Final = 3
_BACKOFF_INITIAL: Final = 0.25
_BACKOFF_MAX: Final = 8.0
#: Longer than this and waiting is worse than reporting. A queue told to come
#: back in an hour is a queue whose caller should hear about it now.
_RETRY_AFTER_MAX: Final = 30.0


class ApiError(RuntimeError):
    """The API refused, or could not be reached.

    The base of every registry client's own error type, so a caller that wants
    to catch all of them can::

        from aiwatcher_sdk.api import ApiError

    ``code`` is the machine-readable discriminator the API returns; switch on
    it rather than on the message, which is prose and will change. ``status``
    is ``None`` when nothing was ever answered.
    """

    def __init__(
        self,
        message: str,
        *,
        status: int | None = None,
        code: str | None = None,
        details: Sequence[str] = (),
        retry_after: float | None = None,
    ) -> None:
        super().__init__(message)
        self.status = status
        self.code = code
        #: Every problem at once, when the server reported more than one. A
        #: drawing can be wrong in nine ways, and reporting the first would
        #: make fixing it nine round trips.
        self.details = list(details)
        #: What the server asked to be waited, in seconds, if it said.
        self.retry_after = retry_after

    @property
    def is_retryable(self) -> bool:
        """Whether coming back later could plausibly work.

        Not simply "5xx". A ``501`` means this instance was built without the
        store this client reads, and no amount of waiting adds one.
        """
        return self.status is None or self.status in RETRYABLE_STATUSES


class Transport:
    """One host, one connection pool, one retry policy.

    Not part of any client's public surface — the clients own it and expose
    their own methods — but constructible on its own by a caller that wants to
    reach a route this SDK has no method for yet.

    ``client`` takes an :class:`httpx.Client` the caller built, which is the
    extension point for a proxy, a client certificate, a custom transport or a
    test double. Its ``base_url`` is not used: every request here is built
    against ``base_url`` above, so what the caller supplies is the *how* and
    never the *where*.
    """

    def __init__(
        self,
        base_url: str,
        *,
        token: str | None = None,
        timeout: float = 30.0,
        attempts: int = _DEFAULT_ATTEMPTS,
        error: Callable[..., ApiError] = ApiError,
        subject: str = "the registry",
        hints: Mapping[str, str] | None = None,
        client: httpx.Client | None = None,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        # Sent as `Authorization: Bearer`, and needed only against an instance
        # with single sign-on on. Every method here raises, so a 401 arrives as
        # this module's error saying the server refused — which is the right
        # shape for "set AIWATCHER_TOKEN".
        self._token = token if token is not None else os.environ.get("AIWATCHER_TOKEN")
        self._error = error
        self._subject = subject
        self._hints = dict(hints or {})
        self._attempts = max(1, attempts)
        self._origin = _origin_of(self.base_url)
        self._timeout = timeout
        self._owned = client is None
        self._client = client if client is not None else _new_client(timeout)

    # ── Crossing a process boundary ──────────────────────────────────────

    def __getstate__(self) -> dict[str, Any]:
        """Everything but the connection pool.

        A `DataLoader` with `num_workers > 0` pickles its dataset under the
        `spawn` start method — the default on macOS and Windows — and that
        dataset holds a registry, which holds this. An `httpx.Client` owns a
        lock and a TLS context and pickles no better than a socket does, so it
        is dropped here and rebuilt in the worker, which is what a worker
        wanted anyway: a pool shared across processes is not a pool.
        """
        if not self._owned:
            raise TypeError(
                "this client was built on an httpx.Client the caller supplied, which cannot "
                "cross a process boundary; a worker would silently get a default one instead "
                "of the proxy, certificate or transport configured here. Construct the "
                "registry without `client=` to hand it to DataLoader workers"
            )
        return self.__dict__ | {"_client": None}

    def __setstate__(self, state: Mapping[str, Any]) -> None:
        self.__dict__.update(state)
        self._client = _new_client(self._timeout)

    # ── Lifetime ─────────────────────────────────────────────────────────

    def close(self) -> None:
        """Release the pool. A client the caller supplied is left alone."""
        if self._owned:
            self._client.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    # ── Requests ─────────────────────────────────────────────────────────

    def json(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None = None,
        *,
        params: Mapping[str, Any] | None = None,
        content: bytes | None = None,
        content_type: str | None = None,
        idempotent: bool | None = None,
    ) -> dict[str, Any]:
        """A request whose answer is a JSON object, or an exception.

        An empty body is ``{}`` rather than a failure — a ``204`` is a
        perfectly good answer to a write — but a body that parses to something
        other than an object is a broken contract and says so, because the
        alternative is an ``AttributeError`` three frames away.
        """
        response = self.send(
            method,
            path,
            body,
            params=params,
            content=content,
            content_type=content_type,
            idempotent=idempotent,
        )
        if not response.content:
            return {}
        try:
            parsed: Any = response.json()
        except ValueError as error:
            raise self._error(
                f"{path} answered {response.headers.get('content-type', 'no content type')} "
                "where this client expects JSON",
                status=response.status_code,
            ) from error
        if not isinstance(parsed, dict):  # pragma: no cover - server contract
            raise self._error(f"expected an object from {path}, got {type(parsed).__name__}")
        return parsed

    def read(self, target: str, *, params: Mapping[str, Any] | None = None) -> bytes:
        """Bytes. ``target`` is a path here, or an absolute URL anywhere.

        The absolute form is what an image registered by reference needs, and
        it is the reason the token is scoped: a corpus points at somebody
        else's host, and sending this deployment's bearer token there would
        hand it to whoever runs it.
        """
        return self.send("GET", target, params=params).content

    def send(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None = None,
        *,
        params: Mapping[str, Any] | None = None,
        content: bytes | None = None,
        content_type: str | None = None,
        idempotent: bool | None = None,
    ) -> httpx.Response:
        """One request, retried by the policy, raising this client's error.

        ``params`` are encoded by httpx rather than by the caller, which is
        what stops a project name with a ``/`` in it becoming a URL nobody
        meant. A ``None`` value is dropped rather than sent as the string
        ``"None"``.
        """
        url = path if "://" in path else f"{self.base_url}{path}"
        repeatable = method.upper() in IDEMPOTENT_METHODS if idempotent is None else idempotent
        query = {key: value for key, value in (params or {}).items() if value is not None}

        def attempt() -> httpx.Response:
            response = self._client.request(
                method,
                url,
                params=query or None,
                json=None if body is None else dict(body),
                content=content,
                headers=self._headers(url, content_type=content_type, body=body is not None),
            )
            # Not `is_error`, which is 4xx and 5xx only. A 3xx matters here
            # because redirects are not followed: left alone it would return a
            # response with no body, and a caller reading image bytes would get
            # zero of them with nothing raised.
            if response.status_code >= 300:
                raise self._refused(response)
            return response

        try:
            return self._retrying(repeatable)(attempt)
        except httpx.HTTPError as error:
            raise self._error(
                f"{self._subject} at {self.base_url} is unreachable: {error}"
            ) from error

    # ── Policy ───────────────────────────────────────────────────────────

    def _retrying(self, repeatable: bool) -> Retrying:
        return Retrying(
            stop=stop_after_attempt(self._attempts),
            wait=_wait,
            retry=retry_if_exception(lambda error: _worth_repeating(error, repeatable)),
            # The caller wants the failure that happened, not tenacity's
            # summary of how many times it happened.
            reraise=True,
        )

    def _headers(self, url: str, *, content_type: str | None, body: bool) -> dict[str, str] | None:
        headers: dict[str, str] = {}
        if content_type is not None:
            headers["content-type"] = content_type
        elif body:
            headers["content-type"] = "application/json"
        if self._token and _origin_of(url) == self._origin:
            headers["authorization"] = f"Bearer {self._token}"
        return headers or None

    def _refused(self, response: httpx.Response) -> ApiError:
        """The API's one error shape, as this client's one exception."""
        body: Any = None
        try:
            body = response.json()
        except ValueError:
            body = None
        fields: Mapping[str, Any] = body if isinstance(body, Mapping) else {}
        code = fields.get("code")
        if response.is_redirect:
            # Said in full, because the alternative is a caller staring at
            # "302 Found" from a URL that looked right. Following it is what
            # would carry the token in `_headers` to whatever answered.
            message = (
                f"{self._subject} redirected to "
                f"{response.headers.get('location', 'somewhere unnamed')}, which this client "
                "does not follow"
            )
        else:
            message = (
                fields.get("message")
                or response.reason_phrase
                or f"{self._subject} refused the request"
            )
        return self._error(
            self._hints.get(str(code), str(message)),
            status=response.status_code,
            code=code,
            details=fields.get("details", ()),
            retry_after=_retry_after(response.headers.get("retry-after")),
        )


def _new_client(timeout: float) -> httpx.Client:
    return httpx.Client(
        # A connect timeout much shorter than the read one, because they answer
        # different questions: "is anything there" is settled in a second on
        # any network worth using, while "has the export finished building"
        # legitimately takes the rest of it.
        timeout=httpx.Timeout(timeout, connect=min(timeout, 5.0)),
        # A redirect is how a request that passed an origin check arrives
        # somewhere else. Nothing in this API issues one.
        follow_redirects=False,
        headers={"accept": "application/json", "user-agent": _USER_AGENT},
    )


def _origin_of(url: str) -> tuple[str, str, int | None]:
    parsed = httpx.URL(url)
    return (parsed.scheme, parsed.host, parsed.port)


def _worth_repeating(error: BaseException, repeatable: bool) -> bool:
    """Whether this failure is worth another attempt.

    Split by what the failure says about the *server's* state rather than by
    exception family, which is the distinction that decides whether repeating
    is safe:

    * nothing reached the server — always repeat, whatever the method;
    * the request was sent and the answer went missing — repeat only what is
      safe to apply twice;
    * the server answered, and the answer was one of :data:`RETRYABLE_STATUSES`
      — repeat, because a ``503`` applied nothing.
    """
    if isinstance(error, httpx.ConnectError | httpx.ConnectTimeout | httpx.PoolTimeout):
        return True
    if isinstance(
        error,
        httpx.ReadTimeout | httpx.ReadError | httpx.WriteError | httpx.RemoteProtocolError,
    ):
        return repeatable
    if isinstance(error, ApiError):
        return error.status in RETRYABLE_STATUSES
    return False


def _wait(state: RetryCallState) -> float:
    """Exponential backoff with jitter, unless the server named a number.

    The jitter is not decoration: without it, twenty workers that started
    together against a queue that returned ``503`` come back together, which is
    the load that produced the ``503``.
    """
    attempt = max(1, state.attempt_number)
    backoff = min(_BACKOFF_INITIAL * 2 ** (attempt - 1), _BACKOFF_MAX)
    jittered: float = backoff * (0.5 + random.random() / 2)  # noqa: S311 - load, not a secret
    outcome = state.outcome
    if outcome is None or not outcome.failed:  # pragma: no cover - only reached on failure
        return jittered
    failure = outcome.exception()
    asked = failure.retry_after if isinstance(failure, ApiError) else None
    return max(jittered, min(asked, _RETRY_AFTER_MAX)) if asked else jittered


def _retry_after(header: str | None) -> float | None:
    """``Retry-After``, in seconds, in either of the two forms it is written.

    A count of seconds, or an HTTP date. Both are in the spec and both are sent
    in the wild; a client that reads only the first waits its own guess against
    a server that answered the question.
    """
    if not header:
        return None
    try:
        return max(0.0, float(header.strip()))
    except ValueError:
        pass
    try:
        when = email.utils.parsedate_to_datetime(header)
    except (TypeError, ValueError):
        return None
    if when.tzinfo is None:  # pragma: no cover - a server sending a naive date
        when = when.replace(tzinfo=UTC)
    return max(0.0, (when - datetime.now(UTC)).total_seconds())


def _user_agent() -> str:
    try:
        from importlib.metadata import version

        return f"aiwatcher-sdk/{version('aiwatcher-sdk')}"
    except Exception:  # noqa: BLE001 - a version is not worth an import error
        return "aiwatcher-sdk"


_USER_AGENT: Final = _user_agent()
