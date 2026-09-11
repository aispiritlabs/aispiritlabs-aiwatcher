from __future__ import annotations

import pytest

from aiwatcher_query.catalog import Catalog
from aiwatcher_query.errors import QueryRefusedError
from aiwatcher_query.reading import parse_period, plan_read


@pytest.mark.parametrize(
    ("period", "seconds"),
    [("15m", 900), ("1h", 3_600), ("7d", 604_800), ("2w", 1_209_600), (3600, 3_600), ("all", None)],
)
def test_a_period_is_flows_period(period: object, seconds: int | None) -> None:
    assert parse_period(period) == seconds


@pytest.mark.parametrize("period", ["1y", "0h", "h", 0, -5, True, 1.5])
def test_a_period_that_is_not_a_duration_is_refused(period: object) -> None:
    with pytest.raises(QueryRefusedError, match="period="):
        parse_period(period)


def test_a_period_is_capped_at_a_year() -> None:
    with pytest.raises(QueryRefusedError, match="capped at 365d"):
        parse_period("53w")


def test_a_period_replaces_the_requests_window(catalog: Catalog) -> None:
    assert plan_read(catalog, "spans", {"period": "1h"}, 900).window_seconds == 3_600
    assert plan_read(catalog, "spans", {}, 900).window_seconds == 900


def test_an_unknown_dataset_names_the_ones_there_are(catalog: Catalog) -> None:
    with pytest.raises(QueryRefusedError, match=r'no dataset "spanz"\. Available: runs, spans'):
        plan_read(catalog, "spanz", {}, None)


def test_a_per_run_dataset_without_a_run_says_how_to_write_one(catalog: Catalog) -> None:
    with pytest.raises(QueryRefusedError, match=r'read\("events", run="run-1"\)'):
        plan_read(catalog, "events", {}, None)


def test_a_period_on_a_dataset_with_no_window_is_refused(catalog: Catalog) -> None:
    with pytest.raises(QueryRefusedError, match="does not take a period"):
        plan_read(catalog, "events", {"run": "run-1", "period": "1h"}, None)


def test_an_argument_the_route_does_not_declare_is_refused_naming_what_it_takes(
    catalog: Catalog,
) -> None:
    with pytest.raises(
        QueryRefusedError, match=r'no read\(\) argument "search"\. It takes: q=, hub=, limit='
    ):
        plan_read(catalog, "hub_datasets", {"search": "plans"}, None)


def test_a_value_outside_the_declared_ones_is_refused(catalog: Catalog) -> None:
    with pytest.raises(QueryRefusedError, match="hub= takes one of 'kaggle', 'huggingface'"):
        plan_read(catalog, "hub_datasets", {"hub": "roboflow"}, None)


def test_a_required_argument_is_named_with_its_reason(catalog: Catalog) -> None:
    with pytest.raises(QueryRefusedError, match=r"needs project= — Which annotation project"):
        plan_read(catalog, "annotation_images", {}, None)


def test_a_whole_number_argument_is_carried_as_its_digits(catalog: Catalog) -> None:
    plan = plan_read(catalog, "hub_rows", {"dataset": "owner/name", "limit": 891}, None)
    assert plan.arguments == {"dataset": "owner/name", "limit": "891"}


def test_a_dataset_name_that_is_not_text_is_refused(catalog: Catalog) -> None:
    with pytest.raises(QueryRefusedError, match="takes a dataset name"):
        plan_read(catalog, None, {}, None)
