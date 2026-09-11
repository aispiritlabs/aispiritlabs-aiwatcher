from __future__ import annotations

import datetime as dt
from decimal import Decimal

import pyarrow as pa
import pytest

from aiwatcher_query.answer import json_safe, rows_of, window_applied
from aiwatcher_query.errors import QueryRefusedError
from aiwatcher_query.memory import digest_of


def test_a_whole_number_beyond_64_bits_is_refused_naming_the_column() -> None:
    with pytest.raises(QueryRefusedError, match='Column "total" holds 36893488147419103232'):
        json_safe(2**65, "total")


def test_a_128_bit_sum_that_fits_comes_back_as_an_integer() -> None:
    assert json_safe(Decimal("9007199254740993"), "total") == 9_007_199_254_740_993


def test_a_float_json_has_no_word_for_is_null() -> None:
    assert json_safe(float("nan"), "mean") is None
    assert json_safe(float("inf"), "mean") is None


def test_a_timestamp_is_iso_8601() -> None:
    assert (
        json_safe(dt.datetime(2026, 9, 10, 12, 0, tzinfo=dt.UTC), "at")
        == "2026-09-10T12:00:00+00:00"
    )


def test_a_nanosecond_time_is_answered_to_the_microsecond_on_any_clock() -> None:
    # A Linux clock ticks in nanoseconds, and pyarrow makes no `datetime` of a value whose
    # last three digits are not zero: DataFusion's `now()` there, and CI's first red run.
    table = pa.table(
        {
            "at": pa.array([1_000_000_123_456_789], pa.timestamp("ns", "UTC")),
            "of_day": pa.array([45_296_123_456_789], pa.time64("ns")),
            "took": pa.array([1_500], pa.duration("ns")),
            "seen": pa.array([[1_000_000_123_456_789]], pa.list_(pa.timestamp("ns"))),
        }
    )
    assert rows_of(table) == (
        ["at", "of_day", "took", "seen"],
        [
            {
                "at": "1970-01-12T13:46:40.123456+00:00",
                "of_day": "12:34:56.123456",
                "took": 1e-06,
                "seen": ["1970-01-12T13:46:40.123456"],
            }
        ],
    )


def test_a_window_is_a_span_only_when_one_was_pinned_and_the_dataset_takes_one() -> None:
    assert window_applied((1, 2), windowed=True) == "span"
    assert window_applied((1, 2), windowed=False) == "none"
    assert window_applied(None, windowed=True) == "none"


def test_a_digest_is_of_compact_json_with_slashes_left_alone() -> None:
    # sha256 of `[{"uri":"a/b","n":1}]`, the bytes PHP's json_encode writes with
    # JSON_UNESCAPED_SLASHES — Flow's encoding, so the two agree where they can.
    assert digest_of([{"uri": "a/b", "n": 1}]) == (
        "78f5120c13dbb488b2a99474b1a16750c343bee108adcfc45805abb6088b681c"
    )
