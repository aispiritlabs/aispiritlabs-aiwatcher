"""The query contract every Python engine serves: six routes under `/query`.

    GET  /query/healthz            is it up, which engine and language, can it see aiwatcher
    GET  /query/datasets           what a query may read, and the columns of each
    POST /query/check              {"pipeline": …} -> what is wrong with it, without running it
    POST /query/simulate           {"pipeline": …} -> a 25-row preview, input bounded
    POST /query/query              {"pipeline": …, "window_seconds": …, "window_from": …,
                                    "window_to": …, "execution_id": …}
                                   -> a table, saying which window it used
    GET  /query/executions/{id}    did this service already run that key

The same request and answer fields as `services/query/flow`, which is what lets the
panel and the reactor talk to "the query engine" rather than to one of them — and the
conformance suite is what proves it. Only `/query`: Flow also answers under `/flow` for
the release that renamed it, and no Python engine ever had that prefix.

SECURITY: in `open` admission this runs the Python a query is written in — in a child
with ceilings, no credentials and a scratch directory, but Python. It binds to localhost
by default and has no authentication of its own, exactly like Flow and the notebook
runtime. Never expose it on a public interface (ADR_0028).
"""

from __future__ import annotations

from typing import Any, Protocol

import anyio
import anyio.to_thread
import httpx
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.routing import Route

from aiwatcher_query.answer import MAX_ROWS, SIMULATION_ROWS, window_applied
from aiwatcher_query.catalog import Catalog
from aiwatcher_query.check import check
from aiwatcher_query.config import Admission, Config
from aiwatcher_query.engine import Engine
from aiwatcher_query.errors import Failure
from aiwatcher_query.evaluate import Job, Outcome
from aiwatcher_query.log import logger
from aiwatcher_query.memory import ExecutionMemory, digest_of

log = logger(__name__)

PREFIX = "/query"
_NO_PIPELINE = 'Send {"pipeline": "<the query>"}.'


class Runner(Protocol):
    """What runs a job: the sandbox in a deployment."""

    def run(self, job: Job) -> Outcome | Failure: ...


def create_app(
    config: Config, *, engine: Engine, engine_ref: str, catalog: Catalog, runner: Runner
) -> Starlette:
    memory = ExecutionMemory()
    limiter = anyio.CapacityLimiter(config.concurrency)
    identity = {"engine": engine.name, "language": engine.language}
    strict = config.admission is Admission.STRICT
    # Derived once, here, for /query/check; the child derives its own before it runs.
    vocabulary = engine.vocabulary() if strict else None

    async def healthz(_: Request) -> JSONResponse:
        return JSONResponse(
            {
                "status": "ok",
                **identity,
                "version": engine.version(),
                "admission": config.admission.value,
                "aiwatcher": config.aiwatcher,
                # Whether *this* service is up is rarely the question; whether it can
                # reach aiwatcher is, so a panel showing an empty table can say which.
                "aiwatcher_reachable": await _reachable(config.aiwatcher),
            }
        )

    async def datasets(_: Request) -> JSONResponse:
        return JSONResponse(
            {
                "datasets": [dataset.describe() for dataset in catalog.all()],
                "source": config.aiwatcher,
                "max_rows": MAX_ROWS,
                **identity,
            }
        )

    async def check_route(request: Request) -> JSONResponse:
        text = _pipeline(await _body(request))
        if text is None:
            return _refused(400, _NO_PIPELINE)
        return JSONResponse(check(text, catalog, vocabulary))

    async def run(request: Request, max_rows: int, input_limit: int | None) -> JSONResponse:
        body = await _body(request)
        text = _pipeline(body)
        if text is None:
            return _refused(400, _NO_PIPELINE)
        span = _span(body)
        # Only a managed run is remembered; an ad-hoc query is not keyed, resumed or
        # deduplicated, so there is nothing to note about it.
        key = _execution_id(body) if max_rows == MAX_ROWS else None
        job = Job(
            engine=engine_ref,
            text=text,
            max_rows=max_rows,
            aiwatcher=config.aiwatcher,
            catalog=catalog,
            corpus=config.corpus,
            window_seconds=_window(body),
            as_of=span[1] if span else None,
            input_limit=input_limit,
            strict=strict,
        )
        if key:
            memory.started(key)
        try:
            outcome = await anyio.to_thread.run_sync(runner.run, job, limiter=limiter)
        except Exception as error:
            log.exception("query.crashed")
            outcome = Failure(502, f"The query could not be run: {error}")
        if isinstance(outcome, Failure):
            if key:
                memory.failed(key)
            log.info("query.refused", status=outcome.status, message=outcome.message)
            return JSONResponse(outcome.body(), status_code=outcome.status)

        digest = digest_of(outcome.rows)
        if key:
            memory.finished(key, digest, len(outcome.rows))
        log.info(
            "query.run", dataset=outcome.dataset, rows=len(outcome.rows), took_ms=outcome.took_ms
        )
        return JSONResponse(
            {
                "columns": outcome.columns,
                "rows": outcome.rows,
                "row_count": len(outcome.rows),
                "truncated": outcome.truncated,
                "truncate_cells": True,
                "dataset": outcome.dataset,
                "grain": outcome.grain,
                "source": config.aiwatcher,
                "window_seconds": outcome.window_seconds,
                "window_applied": window_applied(span, outcome.windowed),
                "deterministic": outcome.deterministic,
                "took_ms": outcome.took_ms,
                "digest": digest,
            }
        )

    async def query(request: Request) -> JSONResponse:
        return await run(request, MAX_ROWS, None)

    async def simulate(request: Request) -> JSONResponse:
        # One input row over the sample, so the output cap can still report truncation.
        return await run(request, SIMULATION_ROWS, SIMULATION_ROWS + 1)

    async def executions(request: Request) -> JSONResponse:
        return JSONResponse(memory.seen(request.path_params["key"]))

    async def missing(request: Request, _: Exception) -> Response:
        # Flow's shape for a route that is not there, so a client reading `error.message`
        # reads this too. A 404 from a query route is also what tells the reactor that
        # something else holds this port.
        return _refused(404, f"No route {request.method} {request.url.path}.")

    return Starlette(
        routes=[
            Route(f"{PREFIX}/healthz", healthz, methods=["GET"]),
            Route(f"{PREFIX}/datasets", datasets, methods=["GET"]),
            Route(f"{PREFIX}/check", check_route, methods=["POST"]),
            Route(f"{PREFIX}/query", query, methods=["POST"]),
            Route(f"{PREFIX}/simulate", simulate, methods=["POST"]),
            Route(f"{PREFIX}/executions/{{key:path}}", executions, methods=["GET"]),
        ],
        exception_handlers={404: missing, 405: missing},
    )


async def _body(request: Request) -> dict[str, Any]:
    try:
        body = await request.json()
    except ValueError:
        return {}
    return body if isinstance(body, dict) else {}


def _pipeline(body: dict[str, Any]) -> str | None:
    value = body.get("pipeline")
    return value if isinstance(value, str) else None


def _whole(value: object) -> int | None:
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def _window(body: dict[str, Any]) -> int | None:
    """The panel's window. Anything but a positive whole number reads as "everything":
    the window is a view control, and a malformed one should widen the answer."""
    value = _whole(body.get("window_seconds"))
    return value if value is not None and value > 0 else None


def _span(body: dict[str, Any]) -> tuple[int, int] | None:
    """The exact bounds a managed plan pinned, when it pinned any."""
    start, end = _whole(body.get("window_from")), _whole(body.get("window_to"))
    return (start, end) if start is not None and end is not None and end > start else None


def _execution_id(body: dict[str, Any]) -> str | None:
    value = body.get("execution_id")
    return value if isinstance(value, str) and value else None


def _refused(status: int, message: str) -> JSONResponse:
    return JSONResponse({"error": {"message": message, "line": 0, "column": 0}}, status_code=status)


async def _reachable(aiwatcher: str) -> bool:
    try:
        async with httpx.AsyncClient(timeout=2.0) as client:
            return (await client.get(f"{aiwatcher}/livez")).status_code < 400
    except httpx.HTTPError:
        return False
