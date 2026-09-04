"""The service: five control routes, and marimo's own app host under them.

Shaped after `services/flow`, deliberately. Both are optional, both are talked
to by the panel directly rather than through the Rust API, and neither is known
to the aiwatcher binary — so a deployment that runs neither loses two blocks
from one screen and nothing else (ADR_0008, ADR_0024).

    GET  /ml-pipeline/healthz             is it up, and which notebooks does it hold
    GET  /ml-pipeline/notebooks           every notebook, by name and revision
    GET  /ml-pipeline/notebooks/{name}    one notebook's source
    PUT  /ml-pipeline/notebooks/{name}    write it, if it parses and is a marimo app
    POST /ml-pipeline/run                 run one notebook over rows, return its rows
    ANY  /ml-pipeline/app/{name}/         the notebook itself, live, for an iframe

SECURITY: this runs notebook code, with no sandbox, in this process (the app
host) and in child processes (a run). It binds to localhost and has no
authentication of its own — exactly like the Flow service, and for the same
reason: the panel proxies it, and it is a development surface. Never expose it
on a public interface.
"""

from __future__ import annotations

from typing import Any

import marimo
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Mount, Route

from ml_pipeline.config import Config
from ml_pipeline.log import logger
from ml_pipeline.notebooks import (
    NotebookDirectory,
    NotebookNotFoundError,
    NotebookRejectedError,
)
from ml_pipeline.runner import NotebookFailedError, run_notebook
from ml_pipeline.staging import Row, Staging, StagingError

log = logger(__name__)

APP_PATH = "/ml-pipeline/app"


def app_url(notebook: str) -> str:
    """Where the live app for one notebook is served.

    The trailing slash is load-bearing rather than tidy: marimo's page
    references its own assets relatively, so without it every one of them
    resolves a directory too high and the iframe renders an empty frame with
    a page of 404s behind it. The dynamic directory redirects to this form,
    and there is no reason to make a browser find that out.
    """
    return f"{APP_PATH}/{notebook}/"


def create_app(config: Config | None = None) -> Starlette:
    """The whole service, as one ASGI application."""
    settings = config or Config.from_env()
    settings.notebooks.mkdir(parents=True, exist_ok=True)
    settings.data.mkdir(parents=True, exist_ok=True)
    directory = NotebookDirectory(root=settings.notebooks)
    staging = Staging(root=settings.data)

    async def healthz(_: Request) -> JSONResponse:
        return JSONResponse(
            {
                "status": "ok",
                "marimo": marimo.__version__,
                "notebooks": len(directory.get_notebooks()),
                "notebook_directory": str(settings.notebooks),
                "app_path": APP_PATH,
                "max_rows": settings.max_rows,
                "timeout_seconds": settings.timeout_seconds,
            }
        )

    async def list_notebooks(_: Request) -> JSONResponse:
        return JSONResponse(
            {
                "notebooks": [summary.__dict__ for summary in directory.get_notebooks()],
                "directory": str(settings.notebooks),
            }
        )

    async def get_notebook(request: Request) -> JSONResponse:
        notebook = directory.get_notebook(request.path_params["name"])
        return JSONResponse(
            {
                **notebook.summary.__dict__,
                "source": notebook.source,
                "app_url": app_url(notebook.name),
            }
        )

    async def save_notebook(request: Request) -> JSONResponse:
        body = await _body(request)
        source = body.get("source")
        if not isinstance(source, str) or not source.strip():
            return _refused('send {"source": "import marimo\\napp = marimo.App()…"}', status=400)
        notebook = directory.save_notebook(request.path_params["name"], source)
        log.info(
            "notebook.saved",
            notebook=notebook.name,
            revision=notebook.revision[:12],
            size=notebook.summary.size,
        )
        return JSONResponse(
            {
                **notebook.summary.__dict__,
                "source": notebook.source,
                "app_url": app_url(notebook.name),
            }
        )

    async def run(request: Request) -> JSONResponse:
        body = await _body(request)
        notebook = directory.get_notebook(_named(body))
        result = run_notebook(
            notebook,
            _rows(body, settings.max_rows),
            _params(body),
            staging,
            settings,
        )
        return JSONResponse(
            {
                "notebook": result.notebook,
                "revision": result.revision,
                "columns": result.columns,
                "rows": result.rows,
                "row_count": result.row_count,
                "truncated": result.truncated,
                "stdout": result.stdout,
                "took_ms": result.took_ms,
                "app_url": app_url(notebook.name),
            }
        )

    # marimo's public embedding API. The dynamic directory turns every notebook
    # in the directory into a live app; `include_code=False` is not decoration
    # — the panel is where a notebook is edited, and a second editor in the
    # iframe would be a second source for the same file.
    host = (
        marimo.create_asgi_app(quiet=True, include_code=False)
        .with_dynamic_directory(path=APP_PATH, directory=str(settings.notebooks))
        .build()
    )

    return Starlette(
        routes=[
            Route("/ml-pipeline/healthz", healthz),
            Route("/ml-pipeline/notebooks", list_notebooks),
            Route("/ml-pipeline/notebooks/{name}", get_notebook, methods=["GET"]),
            Route("/ml-pipeline/notebooks/{name}", save_notebook, methods=["PUT"]),
            Route("/ml-pipeline/run", run, methods=["POST"]),
            # Last, and mounted at the root rather than at `APP_PATH`: marimo's
            # app serves its own assets from paths it chooses, and its dynamic
            # directory matches the full path itself.
            Mount("/", app=host),
        ],
        exception_handlers={
            NotebookNotFoundError: _handler(404),
            NotebookRejectedError: _handler(422),
            NotebookFailedError: _handler(422),
            StagingError: _handler(422),
            ValueError: _handler(400),
        },
    )


def _handler(status: int) -> Any:
    async def handle(_: Request, error: Exception) -> JSONResponse:
        body: dict[str, Any] = {"message": str(error)}
        if isinstance(error, NotebookFailedError):
            body["stdout"] = error.stdout
            body["stderr"] = error.stderr
        return JSONResponse({"error": body}, status_code=status)

    return handle


def _refused(message: str, status: int) -> JSONResponse:
    return JSONResponse({"error": {"message": message}}, status_code=status)


async def _body(request: Request) -> dict[str, Any]:
    try:
        body = await request.json()
    # Any decoding failure at all is the same 400; there is no second thing to do.
    except Exception as error:
        raise ValueError(f"the request body is not JSON: {error}") from error
    if not isinstance(body, dict):
        raise ValueError("the request body is not a JSON object")
    return body


def _named(body: dict[str, Any]) -> str:
    name = body.get("notebook")
    if not isinstance(name, str) or not name:
        raise ValueError('send {"notebook": "<name>", "rows": [...]}')
    return name


def _rows(body: dict[str, Any], limit: int) -> list[Row]:
    rows = body.get("rows", [])
    if not isinstance(rows, list):
        raise ValueError('"rows" is a list of objects')
    kept = [row for row in rows if isinstance(row, dict)]
    if len(kept) > limit:
        raise ValueError(f"{len(kept)} rows were sent; this service accepts {limit}")
    return kept


def _params(body: dict[str, Any]) -> dict[str, Any]:
    params = body.get("params", {})
    if not isinstance(params, dict):
        raise ValueError('"params" is an object')
    return params
