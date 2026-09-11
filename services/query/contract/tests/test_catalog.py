from __future__ import annotations

import json
from pathlib import Path

import pytest

from aiwatcher_query.catalog import Catalog, CatalogError, read_catalog
from aiwatcher_query.config import DEFAULT_CATALOG


def test_the_shipped_catalog_declares_every_dataset_flow_serves() -> None:
    assert list(read_catalog(DEFAULT_CATALOG)) == [
        "runs",
        "spans",
        "events",
        "hub_datasets",
        "hub_rows",
        "hub_columns",
        "annotation_images",
        "corpus_spans",
    ]


def test_a_corpus_dataset_is_offered_only_where_a_corpus_root_is_configured(tmp_path: Path) -> None:
    assert "corpus_spans" not in Catalog.load(DEFAULT_CATALOG, corpus_root=None).names()
    assert "corpus_spans" in Catalog.load(DEFAULT_CATALOG, corpus_root=tmp_path).names()


def test_default_is_runs() -> None:
    catalog = Catalog.load(DEFAULT_CATALOG, corpus_root=None)
    runs = catalog.resolve("default")
    assert runs is not None
    assert runs.name == "runs"
    assert runs.describe()["aliases"] == ["default"]


def test_a_dataset_is_described_in_the_shape_flow_lists_it() -> None:
    catalog = Catalog.load(DEFAULT_CATALOG, corpus_root=None)
    hub = catalog.resolve("hub_datasets")
    assert hub is not None
    described = hub.describe()
    assert list(described) == [
        "name",
        "aliases",
        "grain",
        "description",
        "requires_run",
        "parameters",
        "columns",
    ]
    assert described["parameters"][1] == {
        "name": "hub",
        "required": False,
        "description": "One hub instead of both.",
        "values": ["kaggle", "huggingface"],
    }
    assert described["columns"][0] == {"name": "hub", "type": "string"}


def _catalog_file(tmp_path: Path, dataset: dict[str, object]) -> Path:
    path = tmp_path / "catalog.json"
    path.write_text(json.dumps({"version": 1, "datasets": [dataset]}))
    return path


def test_a_near_miss_key_is_refused_naming_the_dataset_and_the_key(tmp_path: Path) -> None:
    path = _catalog_file(
        tmp_path, {"name": "runs", "grain": "g", "description": "d", "columns": {}, "window": True}
    )
    with pytest.raises(CatalogError, match=r'dataset "runs" .* has a key "window"'):
        read_catalog(path)


def test_a_comment_is_skipped_at_every_level(tmp_path: Path) -> None:
    path = _catalog_file(
        tmp_path,
        {
            "$comment": "why",
            "name": "runs",
            "grain": "g",
            "description": "d",
            "columns": {"$comment": "why", "run_id": "string"},
        },
    )
    assert read_catalog(path)["runs"].columns == {"run_id": "string"}


def test_a_flag_that_is_not_a_boolean_is_refused(tmp_path: Path) -> None:
    path = _catalog_file(
        tmp_path, {"name": "runs", "grain": "g", "description": "d", "columns": {}, "windowed": 1}
    )
    with pytest.raises(CatalogError, match='"windowed" is true or false'):
        read_catalog(path)


def test_a_missing_catalog_names_the_variable_that_moves_it(tmp_path: Path) -> None:
    with pytest.raises(CatalogError, match="AIWATCHER_QUERY_CATALOG"):
        read_catalog(tmp_path / "nowhere.json")
