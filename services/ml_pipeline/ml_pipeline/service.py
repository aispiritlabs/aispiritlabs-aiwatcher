"""The service: six control routes, and marimo's own app host under them.

Shaped after `services/query/flow`, deliberately. Both are optional, both are talked
to by the panel directly rather than through the Rust API, and neither is known
to the aiwatcher binary — so a deployment that runs neither loses two blocks
from one screen and nothing else (ADR_0008, ADR_0024).

    GET  /ml-pipeline/healthz             is it up, and which notebooks does it hold
    GET  /ml-pipeline/notebooks           every notebook, by name and revision
    GET  /ml-pipeline/notebooks/{name}    one notebook's source, as it is now
    PUT  /ml-pipeline/notebooks/{name}    write it, if it parses and is a marimo app
    GET  .../notebooks/{name}/revisions/{revision}   one exact source, forever
    POST /ml-pipeline/run                 run one notebook over rows, return its rows
    POST /ml-pipeline/staging             put rows under a context, run nothing
    GET  /ml-pipeline/staging             how much scratch is here, and how old
    GET  /ml-pipeline/executions/{key}    did this key run here, and is it still going
    ANY  /ml-pipeline/app/{name}/         the notebook itself, live, for an iframe

SECURITY: this runs notebook code, with no sandbox, in this process (the app
host) and in child processes (a run). It binds to localhost and has no
authentication of its own — exactly like the Flow service, and for the same
reason: the panel proxies it, and it is a development surface. Never expose it
on a public interface.
"""

from __future__ import annotations

from typing import Any

import anyio.to_thread
import marimo
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Mount, Route

from ml_pipeline.config import Config
from ml_pipeline.log import logger
from ml_pipeline.memory import ExecutionMemory
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
    settings.revisions.mkdir(parents=True, exist_ok=True)
    settings.data.mkdir(parents=True, exist_ok=True)
    directory = NotebookDirectory(root=settings.notebooks, revisions=settings.revisions)
    # An upgrade keeps the sources that are here. It cannot keep the ones that
    # were here yesterday — those were never written down — so this is stated
    # rather than logged as a success.
    kept = directory.keep_current()
    if kept:
        log.info("notebook.history.backfilled", notebooks=kept)
    staging = Staging(root=settings.data)
    memory = ExecutionMemory()

    async def healthz(_: Request) -> JSONResponse:
        return JSONResponse(
            {
                "status": "ok",
                "marimo": marimo.__version__,
                "notebooks": len(directory.get_notebooks()),
                "notebook_directory": str(settings.notebooks),
                "revision_directory": str(settings.revisions),
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

    async def get_revision(request: Request) -> JSONResponse:
        """One exact source, by the digest a plan pinned.

        A managed step asks this before it reads its rows, so a revision this
        runtime no longer holds costs one GET rather than a run. It never falls
        back to the head: that would run something else under a pinned run's
        name.
        """
        notebook = directory.get_revision(
            request.path_params["name"], request.path_params["revision"]
        )
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
        name = _named(body)
        # A managed run pins a revision and gets exactly that; the panel's
        # editor test sends none and gets the head, which is the unsaved code
        # somebody is looking at. Section 16.3's `ad_hoc`, expressed as an
        # absent field rather than a flag that could disagree with it.
        revision = _revision(body)
        notebook = (
            directory.get_revision(name, revision)
            if revision is not None
            else directory.get_notebook(name)
        )
        rows = _rows(body, settings.max_rows)
        params = _params(body)
        context = _context(body)

        if context is not None:
            memory.started(context)
        try:
            # Off the event loop. `run_notebook` is a blocking `subprocess.run`,
            # and called directly it held every other request for the length of
            # the notebook — measured at 657 ms of a 704 ms run, on a notebook
            # that finishes in under a second. Two things depend on this: the
            # lookup below has to be answerable *while* a notebook runs, which
            # is the only case it exists for, and marimo's live app is served
            # by this same process for the panel's iframe.
            result = await anyio.to_thread.run_sync(
                run_notebook, notebook, rows, params, staging, settings, context
            )
        except BaseException:
            if context is not None:
                memory.failed(context)
            raise
        if context is not None:
            memory.finished(context, result.revision, result.row_count)

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
                # Whether a managed chain may remember this. Declared by the
                # notebook, because nothing outside it can tell a pure
                # transform from one that read the clock — the same reason the
                # query service reports it rather than the caller assuming it.
                "deterministic": result.deterministic,
                "app_url": app_url(notebook.name),
            }
        )

    async def staged(_: Request) -> JSONResponse:
        """How much scratch this service is holding, and how old the oldest is.

        Asked for rather than polled. Nothing here cleans up: a stage overwrites
        its own context and leaves every other one where it is, so this
        directory only grows — one context per attempt of every managed step
        that reached a notebook. Workflow retention runs in another process,
        prunes executions rather than disks, and could not reach this one if it
        wanted to.

        This service is the only process that can see it, which is why the
        number lives here and not beside the Rust binary's own storage figures.
        Together they are the two halves of "what is piling up that nothing will
        delete", which is the question a reference-aware collector is the answer
        to — and whether that is worth building is a question about a curve
        nobody had.

        A walk rather than a running total, because a total kept in memory is
        wrong the moment this process restarts and this directory does not.
        """
        totals = await anyio.to_thread.run_sync(staging.measure)
        return JSONResponse(
            {
                "contexts": totals.contexts,
                "files": totals.files,
                "bytes": totals.bytes,
                # Absent rather than zero when nothing is staged: "empty" and
                # "everything here is from today" are different states.
                "oldest_days": totals.oldest_days,
                "directory": str(settings.data),
            }
        )

    async def stage(request: Request) -> JSONResponse:
        """Put rows where a notebook's live app will read them, and run nothing.

        The editor half of what `run` does as a side effect. aiwatcher calls
        this to open a block on what an *old* execution actually read: the rows
        are in its object store, the live app knows only a notebook's name, and
        `latest` is the join between them.

        It stages and stops. Running the notebook to fill its editor would
        execute somebody's code because somebody clicked "open", and would
        overwrite the output of the run being looked at.

        The **head** is what marimo serves, so a session opened on an old run's
        rows shows those rows under the code that is there now. The code that
        ran is read separately, by its digest, through the revision route — a
        live app per revision would mean putting the history under the notebook
        root, which is the one place it is kept out of.
        """
        body = await _body(request)
        name = _named(body)
        # It has to exist: staging for a notebook nobody has written puts rows
        # under a name that will never be read, and answers 200.
        directory.get_notebook(name)
        staged = staging.stage(
            name,
            _rows(body, settings.max_rows),
            params=_params(body),
            context=_context(body),
        )
        log.info(
            "notebook.staged",
            notebook=staged.notebook,
            context=staged.context,
            rows=len(staged.rows),
        )
        return JSONResponse(
            {
                "notebook": staged.notebook,
                "context": staged.context,
                "rows": len(staged.rows),
                "columns": staged.columns,
                "staged_at": staged.staged_at,
                "app_url": app_url(staged.notebook),
            }
        )

    async def seen(request: Request) -> JSONResponse:
        """What this service knows about one idempotency key.

        A reactor asks after a timeout, before it runs the same key again — a
        timeout says the caller stopped waiting and nothing about whether this
        service stopped working. `absent` is the ordinary answer and the safe
        one, so an older build that does not serve this route at all is read the
        same way.
        """
        return JSONResponse(memory.seen(request.path_params["key"]))

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
            Route(
                "/ml-pipeline/notebooks/{name}/revisions/{revision}",
                get_revision,
                methods=["GET"],
            ),
            Route("/ml-pipeline/run", run, methods=["POST"]),
            Route("/ml-pipeline/staging", stage, methods=["POST"]),
            Route("/ml-pipeline/staging", staged, methods=["GET"]),
            # `:path` because the key is `<execution>/<step>/<attempt>` and
            # carries its own separators. Sent as it is rather than encoded, so
            # a person reading a log sees the key they would grep for.
            Route("/ml-pipeline/executions/{key:path}", seen, methods=["GET"]),
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


def _context(body: dict[str, Any]) -> str | None:
    """Which run's rows these are, when the caller is a run.

    A managed step sends `<execution>/<step>/<attempt>`; the panel sends
    nothing and shares the ad-hoc directory. Bounded because it becomes a hash
    and a hash of an unbounded string is an unbounded read.
    """
    context = body.get("context")
    if context is None or context == "":
        return None
    if not isinstance(context, str) or len(context) > 512:
        raise ValueError('"context" is a string of at most 512 characters')
    return context


def _revision(body: dict[str, Any]) -> str | None:
    """Which source to run, when the caller pinned one.

    Absent is the editor's answer and is not a default the managed path can
    fall into: the reactor always sends it, and a plan with no revision is one
    the compiler refuses.
    """
    revision = body.get("code_revision")
    if revision is None or revision == "":
        return None
    if not isinstance(revision, str):
        raise ValueError('"code_revision" is a sha256 digest as a string')
    return revision


def _params(body: dict[str, Any]) -> dict[str, Any]:
    params = body.get("params", {})
    if not isinstance(params, dict):
        raise ValueError('"params" is an object')
    return params
