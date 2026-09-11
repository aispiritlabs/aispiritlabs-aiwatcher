from __future__ import annotations

import pyarrow as pa
import pytest

from aiwatcher_query.api import fetch
from aiwatcher_query.catalog import Catalog, Dataset
from aiwatcher_query.errors import AiwatcherUnreachableError, UpstreamFailedError
from aiwatcher_query.reading import plan_read
from conftest import FakeApi

BASE = "http://aiwatcher.test"


def _dataset(catalog: Catalog, name: str) -> Dataset:
    dataset = catalog.resolve(name)
    assert dataset is not None
    return dataset


def _read(
    api: FakeApi, catalog: Catalog, name: str, as_of: int | None = None, **arguments: object
) -> pa.Table:
    plan = plan_read(catalog, name, arguments, None)
    with api.client() as client:
        return fetch(
            client,
            BASE,
            plan.dataset,
            run=plan.run,
            window_seconds=plan.window_seconds,
            arguments=plan.arguments,
            as_of=as_of,
            input_limit=None,
        )


def test_a_window_reaches_every_page_it_requests(api: FakeApi, catalog: Catalog) -> None:
    api.answer(
        "/api/v1/spans",
        {"spans": [{"span_id": "a"}], "next_cursor": "c1"},
        {"spans": [{"span_id": "b"}], "next_cursor": None},
    )
    table = _read(api, catalog, "spans", period="1h")
    assert table.column("span_id").to_pylist() == ["a", "b"]
    assert [page["window_seconds"] for page in api.params()] == ["3600", "3600"]
    assert "after" not in api.params()[0]
    assert api.params()[1]["after"] == "c1"


def test_a_pinned_window_end_reaches_a_windowed_route_and_no_other(
    api: FakeApi, catalog: Catalog
) -> None:
    api.answer("/api/v1/runs", {"runs": []})
    api.answer("/api/v1/runs/run-1/events", {"events": []})
    _read(api, catalog, "runs", as_of=1_790_000_000)
    _read(api, catalog, "events", as_of=1_790_000_000, run="run-1")
    assert api.params()[0]["as_of"] == "1790000000"
    assert "as_of" not in api.params()[1]


def test_only_the_arguments_a_route_declares_are_forwarded(api: FakeApi, catalog: Catalog) -> None:
    api.answer("/api/v1/dataset-hubs/search", {"results": []})
    _read(api, catalog, "hub_datasets", q="floor plans", hub="huggingface")
    assert api.params()[0] == {"limit": "500", "q": "floor plans", "hub": "huggingface"}


def test_an_empty_read_is_a_table_with_the_datasets_columns(api: FakeApi, catalog: Catalog) -> None:
    api.answer("/api/v1/runs", {"runs": []})
    table = _read(api, catalog, "runs")
    assert table.num_rows == 0
    assert table.column_names == list(_dataset(catalog, "runs").columns)
    assert table.schema.field("agents").type == pa.list_(pa.string())
    assert table.schema.field("duration_ms").type == pa.int64()


def test_a_field_the_api_omitted_is_null(api: FakeApi, catalog: Catalog) -> None:
    api.answer("/api/v1/runs", {"runs": [{"run_id": "r", "status": "succeeded"}]})
    row = _read(api, catalog, "runs").to_pylist()[0]
    assert row["run_id"] == "r"
    assert row["error"] is None


def test_an_events_columns_are_read_from_where_the_record_nests_them(
    api: FakeApi, catalog: Catalog
) -> None:
    api.answer(
        "/api/v1/runs/run 1/events",
        {
            "events": [
                {
                    "event_type": "llm.completed",
                    "data": {"model": "gpt-5", "tokens": 3},
                    "metadata": {
                        "run_id": "run 1",
                        "sequence": 4,
                        "source": {"service": "svc", "sdk": "py"},
                    },
                }
            ]
        },
    )
    row = _read(api, catalog, "events", run="run 1").to_pylist()[0]
    assert b"/runs/run%201/events" in api.requests[0].url.raw_path
    assert (row["event_type"], row["run_id"], row["sequence"]) == ("llm.completed", "run 1", 4)
    assert (row["service"], row["sdk"]) == ("svc", "py")
    assert row["data"] == '{"model":"gpt-5","tokens":3}'


def test_a_hub_sample_is_read_by_offset_to_the_limit_it_asked_for(
    api: FakeApi, catalog: Catalog
) -> None:
    page = {
        "columns": [
            {"name": "Name", "kind": "Value", "dtype": "string"},
            {"name": "Age", "kind": "Value", "dtype": "float64"},
            {"name": "Survived", "kind": "ClassLabel"},
        ],
        "rows": [
            {"row_index": i, "row": {"Name": f"n{i}", "Age": None, "Survived": 1}}
            for i in range(100)
        ],
    }
    api.answer("/api/v1/dataset-hubs/rows", page)
    table = _read(api, catalog, "hub_rows", dataset="owner/titanic", limit=150)
    assert table.num_rows == 150
    assert [(p["offset"], p["limit"]) for p in api.params()] == [("0", "100"), ("100", "100")]
    assert table.schema.field("row").type == pa.struct(
        [("Name", pa.string()), ("Age", pa.float64()), ("Survived", pa.int64())]
    )
    assert table.to_pylist()[0]["row"] == {"Name": "n0", "Age": None, "Survived": 1}


def test_a_hub_picture_is_a_struct_of_its_address_and_size(api: FakeApi, catalog: Catalog) -> None:
    picture = {"src": "https://hub.test/0.png", "height": 64, "width": 32}
    api.answer(
        "/api/v1/dataset-hubs/rows",
        {
            "columns": [{"name": "image", "kind": "Image"}],
            "rows": [{"row_index": 0, "row": {"image": picture}}],
        },
    )
    table = _read(api, catalog, "hub_rows", dataset="owner/pictures", limit=1)
    assert table.to_pylist()[0]["row"]["image"] == picture


def test_aiwatchers_own_words_and_status_come_back(api: FakeApi, catalog: Catalog) -> None:
    api.answer(
        "/api/v1/dataset-hubs/search",
        {"error": {"message": "AIWATCHER_HUBS is not set"}},
        status=501,
    )
    with pytest.raises(
        UpstreamFailedError, match=r"answered 501 .*AIWATCHER_HUBS is not set"
    ) as raised:
        _read(api, catalog, "hub_datasets")
    assert raised.value.permanent


def test_a_cursor_that_comes_back_unchanged_stops_rather_than_paging_for_ever(
    api: FakeApi, catalog: Catalog
) -> None:
    api.answer("/api/v1/runs", {"runs": [{"run_id": "r"}], "next_cursor": "same"})
    with pytest.raises(AiwatcherUnreachableError, match="cursor 'same' twice"):
        _read(api, catalog, "runs")
