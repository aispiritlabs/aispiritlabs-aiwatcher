from __future__ import annotations

import pytest
from starlette.testclient import TestClient

from ml_pipeline.config import Config
from ml_pipeline.service import create_app
from ml_pipeline.staging import SLUG, Staging

NOTEBOOK = """
import marimo

app = marimo.App()


@app.cell
def _():
    from ml_pipeline import Block

    return (Block,)


@app.cell
def _(Block):
    _block = Block.for_notebook(__file__)
    rows = _block.get_rows()
    params = _block.get_params()
    return params, rows


@app.cell
def _(rows):
    output = [{**row, "seen": True} for row in rows]
    return (output,)


if __name__ == "__main__":
    app.run()
"""


@pytest.fixture
def client(config: Config) -> TestClient:
    return TestClient(create_app(config))


def test_health_says_what_it_holds_and_where_the_apps_are(client: TestClient) -> None:
    body = client.get("/ml-pipeline/healthz").json()

    assert body["status"] == "ok"
    assert body["notebooks"] >= 1
    assert body["app_path"] == "/ml-pipeline/app"


def test_the_demo_notebook_is_listed_with_its_revision(client: TestClient) -> None:
    listed = client.get("/ml-pipeline/notebooks").json()["notebooks"]
    names = [notebook["name"] for notebook in listed]

    assert "pii_detection" in names
    one = client.get("/ml-pipeline/notebooks/pii_detection").json()
    assert one["source"].startswith('"""')
    assert one["app_url"] == "/ml-pipeline/app/pii_detection/"
    assert len(one["revision"]) == 64


def test_a_source_that_is_not_a_notebook_is_refused_rather_than_written(
    scratch: Config,
) -> None:
    with TestClient(create_app(scratch)) as client:
        refused = client.put("/ml-pipeline/notebooks/demo", json={"source": "print('hi')\n"})

        assert refused.status_code == 422
        assert "marimo app" in refused.json()["error"]["message"]
        assert client.get("/ml-pipeline/notebooks/demo").status_code == 404


def test_running_a_block_hands_the_next_one_its_rows_and_leaves_them_staged(
    scratch: Config,
) -> None:
    """Both halves of one run: what the next block reads, and what the live app
    will show when somebody opens this one."""
    with TestClient(create_app(scratch)) as client:
        client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK})

        result = client.post(
            "/ml-pipeline/run", json={"notebook": "demo", "rows": [{"text": "a"}]}
        ).json()

        assert result["rows"] == [{"text": "a", "seen": True}]
        assert result["columns"] == ["text", "seen"]
        assert result["truncated"] is False
        # Asserted through the reader rather than through a path: what matters
        # is that the live app, which knows only the notebook's name, opens on
        # the rows this run staged.
        assert Staging(scratch.data).get_input("demo").rows == [{"text": "a"}]


def test_two_runs_of_one_notebook_do_not_overwrite_each_others_rows(
    scratch: Config,
) -> None:
    """The reason a context exists. Two pipelines sharing one notebook used to
    share one staged file, so each preview read the other's table."""
    with TestClient(create_app(scratch)) as client:
        client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK})
        for context, text in [("exec-1/clean/1", "first"), ("exec-2/clean/1", "second")]:
            client.post(
                "/ml-pipeline/run",
                json={"notebook": "demo", "rows": [{"text": text}], "context": context},
            )

        staging = Staging(scratch.data)
        assert staging.get_input("demo", "exec-1/clean/1").rows == [{"text": "first"}]
        assert staging.get_input("demo", "exec-2/clean/1").rows == [{"text": "second"}]
        # And the editor opens on the one that ran last, which is the whole
        # reason `latest` is written at all.
        assert staging.get_input("demo").rows == [{"text": "second"}]


def test_a_context_may_not_be_a_path(scratch: Config) -> None:
    """A context id holds separators, so it is hashed rather than sanitised —
    and the directory it names is inside the staging root by construction."""
    with TestClient(create_app(scratch)) as client:
        client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK})
        client.post(
            "/ml-pipeline/run",
            json={"notebook": "demo", "rows": [{"text": "a"}], "context": "../../etc/passwd"},
        )

        written = sorted(path.name for path in (scratch.data / "blocks" / "demo").iterdir())
        assert all(SLUG.match(name) for name in written if name != "latest.json"), written


def test_more_rows_than_this_service_accepts_is_a_refusal_not_a_silent_slice(
    scratch: Config,
) -> None:
    small = Config(notebooks=scratch.notebooks, data=scratch.data, max_rows=2)
    with TestClient(create_app(small)) as client:
        client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK})

        refused = client.post(
            "/ml-pipeline/run",
            json={"notebook": "demo", "rows": [{"n": index} for index in range(3)]},
        )

        assert refused.status_code == 400
        assert "3 rows were sent" in refused.json()["error"]["message"]


def test_a_notebook_nobody_wrote_is_a_404(client: TestClient) -> None:
    assert client.get("/ml-pipeline/notebooks/absent").status_code == 404
    assert client.post("/ml-pipeline/run", json={"notebook": "absent"}).status_code == 404


def test_the_live_app_is_served_under_the_path_health_advertises(client: TestClient) -> None:
    """The iframe's src. A 404 here is the whole interactive half missing."""
    page = client.get("/ml-pipeline/app/pii_detection/")

    assert page.status_code == 200
    assert "text/html" in page.headers["content-type"]
