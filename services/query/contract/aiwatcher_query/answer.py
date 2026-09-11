"""The answer: rows capped and reported, in the shape Flow's `QueryRunner::run` has.

A cap rather than a stream, because the panel renders a table and a managed step's
rows are a bounded artifact. Hitting it is reported rather than hidden: silently
returning the first thousand of something is how people draw conclusions from a slice
they did not know was a slice.

Every value leaves as something JSON carries exactly, or not at all. An integer beyond
what the reactor's decoder reads as an integer would come back rounded to a float, so
it is refused naming the column — the spec's "reported rather than rounded" in its
honest form; a float that is not a number is null, because JSON has no word for it.
"""

from __future__ import annotations

import base64
import datetime as dt
import math
from decimal import Decimal
from typing import Any

import pyarrow as pa

from aiwatcher_query.errors import QueryRefusedError

MAX_ROWS = 1_000
#: A simulation proves the transformation on a reviewable sample only.
SIMULATION_ROWS = 25

#: What `serde_json` reads as an integer: an `i64`, or a `u64` above it.
_SMALLEST, _LARGEST = -(2**63), 2**64 - 1


def rows_of(table: pa.Table) -> tuple[list[str], list[dict[str, Any]]]:
    """A table's column names, and its rows as JSON-ready records."""
    columns: list[str] = list(table.column_names)
    seen: set[str] = set()
    for name in columns:
        if name in seen:
            # A record has one value per name, so the second would silently replace the
            # first — the usual cause is a join, and the fix is an alias.
            raise QueryRefusedError(
                f'The answer has two columns named "{name}". Alias one of them.'
            )
        seen.add(name)
    return columns, [
        {name: json_safe(value, name) for name, value in row.items()}
        for row in _to_microseconds(table).to_pylist()
    ]


def _to_microseconds(table: pa.Table) -> pa.Table:
    """The table with every nanosecond time cast down to microseconds.

    A Python `datetime`, `time` or `timedelta` holds microseconds, and pyarrow refuses to
    make one of a nanosecond value unless its last three digits are zero — which they are
    from a clock that ticks in microseconds, as macOS's does, and are not from Linux's, so
    DataFusion's `now()` failed the whole answer there and nowhere else. Microseconds are
    the answer's precision anyway: `isoformat` writes no more, and Flow's `DateTime` holds
    no more. So the extra digits are cut, the same on every clock.
    """
    schema = pa.schema(
        [field.with_type(_microseconds(field.type)) for field in table.schema],
        metadata=table.schema.metadata,
    )
    return table if schema.equals(table.schema) else table.cast(schema, safe=False)


def _microseconds(kind: pa.DataType) -> pa.DataType:
    if isinstance(kind, pa.TimestampType) and kind.unit == "ns":
        return pa.timestamp("us", kind.tz)
    if isinstance(kind, pa.Time64Type) and kind.unit == "ns":
        return pa.time64("us")
    if isinstance(kind, pa.DurationType) and kind.unit == "ns":
        return pa.duration("us")
    if isinstance(kind, pa.ListType):
        return pa.list_(kind.value_field.with_type(_microseconds(kind.value_type)))
    if isinstance(kind, pa.LargeListType):
        return pa.large_list(kind.value_field.with_type(_microseconds(kind.value_type)))
    if isinstance(kind, pa.StructType):
        return pa.struct(
            [
                kind.field(i).with_type(_microseconds(kind.field(i).type))
                for i in range(kind.num_fields)
            ]
        )
    return kind


def json_safe(value: Any, column: str) -> Any:
    match value:
        case None | bool() | str():
            return value
        case int():
            if not _SMALLEST <= value <= _LARGEST:
                raise QueryRefusedError(
                    f'Column "{column}" holds {value}, which does not fit in 64 bits, and the '
                    "answer carries whole numbers exactly or not at all. Cast it to a float "
                    "in the query if an approximation will do."
                )
            return value
        case float():
            return value if math.isfinite(value) else None
        case Decimal():
            if value.is_finite() and value == value.to_integral_value():
                return json_safe(int(value), column)
            return float(value) if value.is_finite() else None
        case dt.datetime() | dt.date() | dt.time():
            return value.isoformat()
        case dt.timedelta():
            return value.total_seconds()
        case bytes():
            return base64.b64encode(value).decode("ascii")
        case list() | tuple():
            return [json_safe(item, column) for item in value]
        case dict():
            return {str(key): json_safe(item, column) for key, item in value.items()}
        case _:
            return str(value)


def window_applied(span: tuple[int, int] | None, windowed: bool) -> str:
    """Whether the rows were read through the exact bounds a plan pinned.

    `span` when a plan pinned bounds and the dataset takes a window — the API's windowed
    routes read `as_of` and `window_seconds` together, which is exactly that span —
    and `none` otherwise: a dataset with no window cannot be read through one, and
    saying `span` about it would be a claim nobody could act on.
    """
    return "span" if span is not None and windowed else "none"
