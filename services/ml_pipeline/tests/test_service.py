from __future__ import annotations

import anyio
import pytest
from httpx import ASGITransport, AsyncClient
from starlette.testclient import TestClient

from ml_pipeline.config import Config
from ml_pipeline.memory import ABSENT, DONE, RUNNING
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
    small = Config(
        notebooks=scratch.notebooks,
        revisions=scratch.revisions,
        data=scratch.data,
        max_rows=2,
    )
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


def test_a_key_this_service_never_ran_is_absent_rather_than_a_404(client: TestClient) -> None:
    # `absent` is the safe answer and the ordinary one, so the route says it
    # rather than making the caller read a status code as a state.
    body = client.get("/ml-pipeline/executions/01a0/detect/1").json()

    assert body == {"state": ABSENT}


def test_a_finished_run_is_remembered_under_the_key_the_reactor_will_ask_with(
    scratch: Config,
) -> None:
    # The context *is* the idempotency key — `<execution>/<step>/<attempt>`,
    # the same string the reactor derives — so a run needs no second field for
    # this and a lookup needs no translation.
    client = TestClient(create_app(scratch))
    client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK})
    key = "01a0/detect/1"

    ran = client.post(
        "/ml-pipeline/run",
        json={"notebook": "demo", "rows": [{"a": 1}], "params": {}, "context": key},
    )
    assert ran.status_code == 200, ran.text

    body = client.get(f"/ml-pipeline/executions/{key}").json()
    assert body["state"] == DONE
    # The revision it ran, which is what a receipt records too. Two answers
    # that disagree mean the stored rows came from a different notebook.
    assert body["revision"] == ran.json()["revision"]
    assert body["rows"] == ran.json()["row_count"]


def test_a_run_with_no_context_is_not_remembered_at_all(scratch: Config) -> None:
    # An ad-hoc preview from the panel is not a managed attempt: it is not
    # keyed, not resumed and not deduplicated. Remembering it would put notes
    # in this dict for every keystroke somebody previews.
    client = TestClient(create_app(scratch))
    client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK})

    ran = client.post(
        "/ml-pipeline/run", json={"notebook": "demo", "rows": [{"a": 1}], "params": {}}
    )
    assert ran.status_code == 200, ran.text

    assert client.get("/ml-pipeline/executions/adhoc").json() == {"state": ABSENT}


def test_a_notebook_that_raised_leaves_no_note_saying_it_is_still_running(
    scratch: Config,
) -> None:
    # Otherwise a reactor waits for something that stopped, and only the lease
    # expiring frees it.
    (scratch.notebooks / "boom.py").write_text(
        NOTEBOOK.replace('{**row, "seen": True} for row in rows', "1 / 0 for row in rows")
    )
    client = TestClient(create_app(scratch), raise_server_exceptions=False)
    key = "01a0/boom/1"

    failed = client.post(
        "/ml-pipeline/run",
        json={"notebook": "boom", "rows": [{"a": 1}], "params": {}, "context": key},
    )
    assert failed.status_code == 422, failed.text

    assert client.get(f"/ml-pipeline/executions/{key}").json() == {"state": ABSENT}


def test_a_running_notebook_does_not_hold_every_other_request_behind_it(
    scratch: Config,
) -> None:
    """The reason `run_notebook` is handed to a worker thread.

    Called straight from the async handler it blocked the event loop for the
    whole subprocess — measured at 657 ms of a 704 ms run on a notebook that
    finishes in under a second. Two things depend on it not doing that: the
    lookup below has to be answerable *while* a notebook runs, which is the only
    case it exists for, and marimo's live app is served by this same process for
    the panel's iframe.

    Asserted through the lookup rather than through a stopwatch: a timing test on
    a shared runner measures the runner. What this needs to be true is that the
    key reads `running` from outside while the run is in flight, which is only
    possible if the loop is free to answer.
    """
    slow = NOTEBOOK.replace(
        "    from ml_pipeline import Block",
        "    import time\n    from ml_pipeline import Block",
    ).replace(
        '{**row, "seen": True} for row in rows',
        '{**row, "seen": not time.sleep(0.05)} for row in rows',
    )
    (scratch.notebooks / "slow.py").write_text(slow)
    key = "01a0/slow/1"

    seen: list[str] = []

    async def watch() -> None:
        # Two clients on one app: `run` blocks its own request by design, so the
        # question has to come from somewhere that is not waiting on it.
        while not seen or seen[-1] == RUNNING:
            async with AsyncClient(transport=ASGITransport(app=app), base_url="http://t") as client:
                answer = await client.get(f"/ml-pipeline/executions/{key}")
            seen.append(answer.json()["state"])
            await anyio.sleep(0.02)

    async def exercise() -> None:
        async with (
            anyio.create_task_group() as group,
            AsyncClient(transport=ASGITransport(app=app), base_url="http://t") as client,
        ):
            await client.put("/ml-pipeline/notebooks/slow", json={"source": slow})
            group.start_soon(watch)
            ran = await client.post(
                "/ml-pipeline/run",
                json={
                    "notebook": "slow",
                    "rows": [{"a": index} for index in range(20)],
                    "params": {},
                    "context": key,
                },
                timeout=120,
            )
            assert ran.status_code == 200, ran.text

    app = create_app(scratch)
    anyio.run(exercise)

    assert RUNNING in seen, f"the service never answered while a notebook ran: {seen}"
    assert seen[-1] == DONE, seen


def test_a_run_that_pins_a_revision_executes_that_source_after_the_head_moved(
    scratch: Config,
) -> None:
    """Work 5's exit, in one test.

    Save a notebook, note the revision a pipeline would pin, edit the notebook,
    then run the pinned revision. What comes back is the *first* notebook's
    output. Before the revision store this was a refusal — the run was told the
    notebook had been edited and to save the pipeline again, which is not a
    thing anybody can do to a run that already happened.
    """
    edited = NOTEBOOK.replace('"seen": True', '"seen": "edited"')
    with TestClient(create_app(scratch)) as client:
        pinned = client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK}).json()
        moved = client.put("/ml-pipeline/notebooks/demo", json={"source": edited}).json()
        assert pinned["revision"] != moved["revision"]

        result = client.post(
            "/ml-pipeline/run",
            json={
                "notebook": "demo",
                "rows": [{"text": "a"}],
                "code_revision": pinned["revision"],
            },
        ).json()

        assert result["rows"] == [{"text": "a", "seen": True}]
        # And it says which source ran, which is what the reactor compares
        # against the plan's pin after the fact.
        assert result["revision"] == pinned["revision"]


def test_a_run_that_pins_nothing_gets_the_head_somebody_is_editing(scratch: Config) -> None:
    """The editor's own path, section 16.3's `ad_hoc`.

    Expressed as an absent field rather than a flag, so there is no way for the
    flag and the revision to disagree.
    """
    edited = NOTEBOOK.replace('"seen": True', '"seen": "edited"')
    with TestClient(create_app(scratch)) as client:
        client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK})
        moved = client.put("/ml-pipeline/notebooks/demo", json={"source": edited}).json()

        result = client.post(
            "/ml-pipeline/run", json={"notebook": "demo", "rows": [{"text": "a"}]}
        ).json()

        assert result["rows"] == [{"text": "a", "seen": "edited"}]
        assert result["revision"] == moved["revision"]


def test_a_pinned_revision_this_runtime_never_held_is_a_404_and_not_the_head(
    scratch: Config,
) -> None:
    """The one outcome content addressing exists to prevent.

    Falling back to the head here would run *something else* under a pinned
    run's name, and the rows would look exactly like a successful run.
    """
    with TestClient(create_app(scratch)) as client:
        client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK})

        absent = client.post(
            "/ml-pipeline/run",
            json={"notebook": "demo", "rows": [{"text": "a"}], "code_revision": "0" * 64},
        )

        assert absent.status_code == 404
        assert "saving the notebook again" in absent.json()["error"]["message"]


def test_one_exact_source_is_readable_by_its_digest_before_anything_runs(
    scratch: Config,
) -> None:
    """What a managed step asks first, so a lost revision costs one GET."""
    with TestClient(create_app(scratch)) as client:
        pinned = client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK}).json()
        client.put(
            "/ml-pipeline/notebooks/demo",
            json={"source": NOTEBOOK.replace('"seen": True', '"seen": "edited"')},
        )

        found = client.get(f"/ml-pipeline/notebooks/demo/revisions/{pinned['revision']}")
        missing = client.get(f"/ml-pipeline/notebooks/demo/revisions/{'0' * 64}")
        refused = client.get("/ml-pipeline/notebooks/demo/revisions/not-a-digest")

        assert found.json()["source"] == NOTEBOOK
        assert found.json()["name"] == "demo"
        assert missing.status_code == 404
        assert refused.status_code == 422


def test_staging_puts_rows_where_the_live_app_reads_them_and_runs_nothing(
    scratch: Config,
) -> None:
    """The editor half of a run, without the run.

    aiwatcher calls this to open a block on what an old execution read. Running
    the notebook to fill its editor would execute somebody's code because
    somebody clicked "open", and would overwrite the output being looked at.
    """
    with TestClient(create_app(scratch)) as client:
        client.put("/ml-pipeline/notebooks/demo", json={"source": NOTEBOOK})

        staged = client.post(
            "/ml-pipeline/staging",
            json={
                "notebook": "demo",
                "rows": [{"text": "what that run read"}],
                "params": {"threshold": 0.8},
                "context": "exec-9/detect/2",
            },
        ).json()

        assert staged["rows"] == 1
        assert staged["context"] == "exec-9/detect/2"
        assert staged["app_url"] == "/ml-pipeline/app/demo/"
        staging = Staging(scratch.data)
        assert staging.get_input("demo", "exec-9/detect/2").rows == [{"text": "what that run read"}]
        # And the pointer the live app follows, which knows only the name.
        assert staging.get_input("demo").rows == [{"text": "what that run read"}]
        assert staging.get_input("demo").params == {"threshold": 0.8}
        # Nothing ran: no output beside the input it staged.
        assert not (scratch.data / "blocks" / "demo" / "adhoc" / "output.json").exists()


def test_staging_for_a_notebook_nobody_wrote_is_a_404(scratch: Config) -> None:
    """Rows under a name that will never be read, answered 200, is worse than
    a refusal: the editor opens on an empty table and nothing says why."""
    with TestClient(create_app(scratch)) as client:
        absent = client.post(
            "/ml-pipeline/staging", json={"notebook": "absent", "rows": [{"a": 1}]}
        )

        assert absent.status_code == 404
