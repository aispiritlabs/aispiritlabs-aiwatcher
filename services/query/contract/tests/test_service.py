"""The six routes, through the real fork server and a real child per query."""

from __future__ import annotations

import os
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import anyio
import httpx
import pytest
from starlette.testclient import TestClient

from aiwatcher_query import child
from aiwatcher_query.catalog import Catalog
from aiwatcher_query.child import Limits
from aiwatcher_query.config import Admission, Config
from aiwatcher_query.sandbox import Sandbox
from aiwatcher_query.service import create_app
from arrow_engine import ENGINE, REFERENCE
from conftest import CORPUS_ROWS

TIMEOUT_SECONDS = 2
FLOW_ANSWER_FIELDS = [
    "columns",
    "rows",
    "row_count",
    "truncated",
    "truncate_cells",
    "dataset",
    "grain",
    "source",
    "window_seconds",
    "window_applied",
    "deterministic",
    "took_ms",
    "digest",
]


@pytest.fixture(scope="module")
def sandbox() -> Sandbox:
    # Set before the fork server starts, as a deployment's credential would be.
    os.environ["AIWATCHER_AUTH_INGEST_TOKENS"] = "producer=secret"
    started = Sandbox(REFERENCE, Limits(cpu_seconds=TIMEOUT_SECONDS, memory_mb=0))
    started.start()
    return started


@pytest.fixture
def app(sandbox: Sandbox, corpus: Path) -> Any:
    config = Config(aiwatcher="http://127.0.0.1:9", corpus=corpus, timeout_seconds=TIMEOUT_SECONDS)
    catalog = Catalog.load(config.catalog, corpus_root=corpus)
    return create_app(config, engine=ENGINE, engine_ref=REFERENCE, catalog=catalog, runner=sandbox)


@pytest.fixture
def client(app: Any) -> Iterator[TestClient]:
    with TestClient(app) as client:
        yield client


def _query(client: TestClient, text: str, route: str = "query", **body: object) -> httpx.Response:
    response: httpx.Response = client.post(f"/query/{route}", json={"pipeline": text, **body})
    return response


def test_healthz_names_the_engine_and_its_language(client: TestClient) -> None:
    answer = client.get("/query/healthz").json()
    assert (answer["engine"], answer["language"], answer["admission"]) == (
        "arrow",
        "arrow-python",
        "open",
    )
    assert answer["aiwatcher_reachable"] is False


def test_datasets_lists_the_catalog_with_the_engine(client: TestClient) -> None:
    answer = client.get("/query/datasets").json()
    assert [dataset["name"] for dataset in answer["datasets"]][-1] == "corpus_spans"
    assert (answer["max_rows"], answer["engine"], answer["language"]) == (
        1000,
        "arrow",
        "arrow-python",
    )


def test_a_query_answers_with_every_field_flow_answers_with(client: TestClient) -> None:
    response = _query(client, 'read("corpus_spans").filter(field("status") == "error")')
    assert response.status_code == 200, response.text
    answer = response.json()
    assert list(answer) == FLOW_ANSWER_FIELDS
    assert answer["row_count"] == CORPUS_ROWS // 3
    assert answer["dataset"] == "corpus_spans"
    assert answer["deterministic"] is False
    assert answer["window_applied"] == "none"


def test_a_preview_reads_a_sample_and_says_there_was_more(client: TestClient) -> None:
    answer = _query(client, 'read("corpus_spans")', route="simulate").json()
    assert (answer["row_count"], answer["truncated"]) == (25, True)


def test_a_refusal_is_a_422_with_where(client: TestClient) -> None:
    response = _query(client, "x = 1\n42")
    assert response.status_code == 422
    assert response.json()["error"] == {
        "message": "A query ends in a pyarrow Table, and this one ended in int 42.",
        "line": 2,
        "column": 1,
    }


def test_a_child_that_ends_without_answering_is_not_mistaken_for_the_clock(
    client: TestClient,
) -> None:
    # Its pipe closes as it exits, a moment before it can be reaped; killing it in that
    # moment used to report the timeout, thirty seconds early.
    response = _query(client, 'import os\nos._exit(3)\nread("corpus_spans")')
    assert response.status_code == 502
    assert response.json()["error"]["message"] == (
        "The query's process ended with status 3 without answering."
    )


def test_a_body_without_a_pipeline_is_a_400(client: TestClient) -> None:
    assert client.post("/query/query", json={"query": "read()"}).status_code == 400
    assert client.post("/query/check", content=b"not json").status_code == 400


def test_a_route_that_is_not_there_answers_in_the_contracts_shape(client: TestClient) -> None:
    response = client.get("/flow/healthz")
    assert response.status_code == 404
    assert response.json()["error"]["message"] == "No route GET /flow/healthz."


def test_check_reads_without_running(client: TestClient) -> None:
    answer = client.post("/query/check", json={"pipeline": 'read("spanz")'}).json()
    assert answer["ok"] is False
    assert 'no dataset "spanz"' in answer["diagnostics"][0]["message"]


def test_a_managed_run_is_remembered_by_its_key_and_a_failed_one_is_not(client: TestClient) -> None:
    key = "exec-1/step-1/1"
    answer = _query(client, 'read("corpus_spans")', execution_id=key).json()
    seen = client.get(f"/query/executions/{key}").json()
    assert seen == {"state": "done", "digest": answer["digest"], "rows": CORPUS_ROWS}

    _query(client, "42", execution_id="exec-1/step-2/1")
    assert client.get("/query/executions/exec-1/step-2/1").json() == {"state": "absent"}


def test_open_mode_holds_no_credentials(client: TestClient) -> None:
    answer = _query(
        client,
        "import os, pyarrow as pa\n"
        'names = [name for name in os.environ if name.startswith("AIWATCHER_")]\n'
        'pa.table({"name": pa.array(names, pa.string())})',
    ).json()
    assert answer["rows"] == []


def test_a_query_runs_in_a_scratch_directory_of_its_own(client: TestClient) -> None:
    answer = _query(client, 'import os, pyarrow as pa\npa.table({"cwd": [os.getcwd()]})').json()
    assert Path(answer["rows"][0]["cwd"]).name.startswith("aiwatcher-query-")
    assert not Path(answer["rows"][0]["cwd"]).exists()


def test_the_child_reads_aiwatcher_with_nothing_from_its_environment() -> None:
    # Not proxies, not ~/.netrc — and on macOS, not the system proxy lookup, which is
    # CoreFoundation in a forked child and crashed whole runs of this file at random.
    with child.api_client() as client:
        assert client.trust_env is False


def test_the_service_process_keeps_no_credential_after_starting(sandbox: Sandbox) -> None:
    assert not [name for name in os.environ if name.startswith("AIWATCHER_")]


@pytest.mark.anyio
async def test_open_mode_stops_a_query_at_its_ceiling_while_answering_others(app: Any) -> None:
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://query.test") as client:
        spinning: dict[str, httpx.Response] = {}

        async def spin() -> None:
            spinning["response"] = await client.post(
                "/query/query", json={"pipeline": "while True:\n    pass\nread('corpus_spans')"}
            )

        async with anyio.create_task_group() as group:
            group.start_soon(spin)
            await anyio.sleep(0.3)
            started = time.monotonic()
            healthy = await client.get("/query/healthz")
            assert healthy.status_code == 200
            assert time.monotonic() - started < 1.0
            assert "response" not in spinning

        stopped = spinning["response"]
        assert stopped.status_code == 422
        assert "AIWATCHER_QUERY_TIMEOUT_SECONDS" in stopped.json()["error"]["message"]


@pytest.fixture
def anyio_backend() -> str:
    return "asyncio"


@pytest.fixture
def strict_client(sandbox: Sandbox, corpus: Path) -> Iterator[TestClient]:
    config = Config(
        aiwatcher="http://127.0.0.1:9",
        corpus=corpus,
        timeout_seconds=TIMEOUT_SECONDS,
        admission=Admission.STRICT,
    )
    catalog = Catalog.load(config.catalog, corpus_root=corpus)
    app = create_app(config, engine=ENGINE, engine_ref=REFERENCE, catalog=catalog, runner=sandbox)
    with TestClient(app) as client:
        yield client


def test_strict_mode_refuses_an_import_before_anything_runs(strict_client: TestClient) -> None:
    response = _query(strict_client, 'import os\nread("corpus_spans")')
    assert response.status_code == 422
    assert response.json()["error"]["message"].startswith("A strict query imports nothing")


def test_strict_mode_runs_what_it_admits(strict_client: TestClient) -> None:
    response = _query(strict_client, 'read("corpus_spans").filter(field("status") == "error")')
    assert response.status_code == 200, response.text
    assert response.json()["row_count"] == CORPUS_ROWS // 3


def test_a_strict_check_names_the_refusal_and_says_it_was_strict(strict_client: TestClient) -> None:
    answer = strict_client.post(
        "/query/check", json={"pipeline": 'read("runs").to_pandas()'}
    ).json()
    assert answer["checked_by"] == ["python", "aiwatcher", "strict"]
    (found,) = answer["diagnostics"]
    assert found["message"].startswith("to_pandas is not admitted: it leaves the engine")
