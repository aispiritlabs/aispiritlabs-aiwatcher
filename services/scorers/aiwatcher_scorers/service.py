"""The two routes, and the rules every adapter is held to behind them.

``GET /scorers/catalog`` describes every adapter this process loaded.
``POST /scorers/score`` holds a request to what the card pinned before any
case is scored: the adapter's release, and — for a graded metric — the model.
A card pinned against another release is a 409 naming both, never a number
from a framework the card did not measure with. Then the parameters, every
problem at once, and then each case, in order.

A framework's metric blocks while it asks its model, so each case runs in a
worker thread and the catalog stays answerable while a batch is scored.

Given a token, both routes want it as a bearer credential — the one aiwatcher's
work role sends as ``AIWATCHER_SCORER_TOKEN``. ``/health`` never does: a probe
carries no credential, and it answers nothing but which adapters loaded.
"""

from __future__ import annotations

import hmac
import json
from collections.abc import Awaitable, Callable, Sequence

from starlette.applications import Starlette
from starlette.concurrency import run_in_threadpool
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Route

from aiwatcher_scorers.adapter import Adapter, scored
from aiwatcher_scorers.contract import (
    CONTRACT,
    Case,
    ContractError,
    JsonValue,
    Scored,
    ScoreRequest,
    parse_score_request,
)


def catalog(adapters: Sequence[Adapter]) -> dict[str, JsonValue]:
    described: list[JsonValue] = []
    for adapter in adapters:
        entry: dict[str, JsonValue] = {
            "name": adapter.name,
            "version": adapter.version,
            "metrics": [
                implemented.metric.to_json() for _, implemented in sorted(adapter.metrics().items())
            ],
        }
        if adapter.model is not None:
            entry["model"] = adapter.model.to_json()
        described.append(entry)
    return {"contract": CONTRACT, "adapters": described}


def missing_side(case: Case, reads: Sequence[str]) -> str | None:
    if "input" in reads and not case.has_input:
        return "the case carries no input, and this metric reads it"
    if "expected" in reads and not case.has_expected:
        return "the case carries no expectation, and this metric reads it"
    return None


async def score(adapters: Sequence[Adapter], request: ScoreRequest) -> list[Scored]:
    adapter = next((held for held in adapters if held.name == request.adapter), None)
    if adapter is None:
        raise ContractError(404, f"this service runs no adapter `{request.adapter}`")
    implemented = adapter.metrics().get(request.metric)
    if implemented is None:
        raise ContractError(404, f"{adapter.name} implements no metric `{request.metric}` here")
    metric = implemented.metric
    grades_with = adapter.model if metric.model_graded else None
    if request.declared.version != adapter.version or request.declared.model != grades_with:
        raise ContractError(
            409,
            f"the card pinned {adapter.name} {request.declared.version}"
            f"{_with(request.declared.model)} and this service runs {adapter.version}"
            f"{_with(grades_with)}; publish the card again to measure with what runs here",
        )
    problems = metric.refusals(request.parameters)
    if problems:
        raise ContractError(422, f"{adapter.name}.{metric.metric} {'; '.join(problems)}")
    replies: list[Scored] = []
    for case in request.cases:
        missing = missing_side(case, metric.reads)
        if missing is not None:
            replies.append(Scored(failed=missing))
            continue
        replies.append(await run_in_threadpool(scored, implemented, request.parameters, case))
    return replies


def _with(model: object) -> str:
    name = getattr(model, "name", None)
    version = getattr(model, "version", None)
    return f" grading with {name} {version}" if name else ""


type Handler = Callable[[Request], Awaitable[JSONResponse]]


def guarded(token: str | None, handler: Handler) -> Handler:
    """The handler, behind the bearer token when this service was given one."""
    if token is None:
        return handler
    expected = f"Bearer {token}".encode()

    async def checked(request: Request) -> JSONResponse:
        presented = request.headers.get("authorization", "").encode()
        # Compared in constant time: the difference between a wrong first byte and a wrong
        # last one is not something to hand whoever is guessing.
        if not hmac.compare_digest(presented, expected):
            return JSONResponse(
                {
                    "message": "this scorer service wants its bearer token "
                    "(AIWATCHER_SCORER_TOKEN, on aiwatcher's work role)"
                },
                status_code=401,
                headers={"www-authenticate": "Bearer"},
            )
        return await handler(request)

    return checked


def create_app(adapters: Sequence[Adapter], token: str | None = None) -> Starlette:
    async def health(_: Request) -> JSONResponse:
        return JSONResponse({"status": "ok", "adapters": [held.name for held in adapters]})

    async def read_catalog(_: Request) -> JSONResponse:
        return JSONResponse(catalog(adapters))

    async def score_cases(request: Request) -> JSONResponse:
        try:
            body: JsonValue = json.loads(await request.body())
        except ValueError:
            return JSONResponse({"message": "the request is not JSON"}, status_code=400)
        try:
            replies = await score(adapters, parse_score_request(body))
        except ContractError as refused:
            return JSONResponse({"message": refused.message}, status_code=refused.status)
        return JSONResponse({"replies": [reply.to_json() for reply in replies]})

    return Starlette(
        routes=[
            Route("/health", health),
            Route("/scorers/catalog", guarded(token, read_catalog)),
            Route("/scorers/score", guarded(token, score_cases), methods=["POST"]),
        ]
    )
