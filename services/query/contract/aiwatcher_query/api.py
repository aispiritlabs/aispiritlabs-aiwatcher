"""The aiwatcher API, paged into Arrow — route for route the way Flow pages it.

Flow's measurement stands (`Catalog.php`): grain decides the cost, not transport, and
the API has already folded events into the grain a question asks about. So there is no
export and no second copy. The cursors the API serves — `next_cursor` in the body,
replayed as the dataset's `cursor_param` — are replayed here; the window reaches every
page as `window_seconds` and, when a managed plan pinned one, `as_of`; and only the
arguments a dataset declares are forwarded, because the API refuses one it does not
know and a typo would come back as a 400 about the whole read.

The status is read before the body. By the time a missing `rows` key is noticed there
is no status left to branch on, and the message a person needed — often a 501 naming
an unset variable — is the one that got lost.

What comes out is a table typed from the catalog's columns, whatever the page held, so
an empty read is an empty table *with* its columns, and every engine is handed types
rather than left to infer them from one page.
"""

from __future__ import annotations

import json
from collections.abc import Iterator, Mapping
from typing import Any
from urllib.parse import quote

import httpx
import pyarrow as pa

from aiwatcher_query.catalog import Dataset
from aiwatcher_query.errors import AiwatcherUnreachableError, QueryRefusedError, UpstreamFailedError

#: Rows per request. Fewer, larger pages beat many small ones; Flow's number.
PAGE_SIZE = 500
#: The hub reads a sample in pages of at most a hundred, by offset rather than cursor.
HUB_PAGE = 100
HUB_DEFAULT_LIMIT = 100
HUB_MAX_LIMIT = 1_000

_INTEGER_DTYPES = frozenset(
    {"int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64"}
)
_FLOAT_DTYPES = frozenset({"float16", "float32", "float64", "float", "double"})


def fetch(
    client: httpx.Client,
    base: str,
    dataset: Dataset,
    *,
    run: str | None,
    window_seconds: int | None,
    arguments: Mapping[str, str],
    as_of: int | None,
    input_limit: int | None,
) -> pa.Table:
    """Every row one `read()` asks the API for, as a table of the dataset's columns."""
    path = (
        dataset.path.replace("{run}", quote(run or "", safe=""))
        if dataset.requires_run
        else dataset.path
    )
    url = f"{base}{path}"
    params = query_parameters(dataset, window_seconds, arguments, as_of)
    if dataset.name == "hub_rows":
        records, declared = _hub_rows(client, url, params, arguments, input_limit)
        return table_of(dataset, records, row_type=_row_type(declared))
    records = [record for page in _cursor_pages(client, url, params, dataset) for record in page]
    return table_of(dataset, records)


def query_parameters(
    dataset: Dataset,
    window_seconds: int | None,
    arguments: Mapping[str, str],
    as_of: int | None,
) -> dict[str, str | int]:
    """The query string one page of a dataset is read with. `Catalog::query`, in Python."""
    params: dict[str, str | int] = {"limit": PAGE_SIZE}
    if dataset.windowed and window_seconds is not None and window_seconds > 0:
        params["window_seconds"] = window_seconds
    # Absent for every panel query, which keeps a pasted link meaning "the last hour"
    # when it is opened; present, the window is a closed span a retry reads again.
    if dataset.windowed and as_of is not None:
        params["as_of"] = as_of
    for name in dataset.parameters:
        value = arguments.get(name)
        if value:
            params[name] = value
    return params


def _cursor_pages(
    client: httpx.Client, url: str, params: dict[str, str | int], dataset: Dataset
) -> Iterator[list[Any]]:
    cursor: object = None
    replayed: set[str] = set()
    while True:
        page = params if cursor is None else {**params, dataset.cursor_param: str(cursor)}
        body = _get(client, url, page)
        yield _rows_at(body, dataset.rows_path, url)
        cursor = body.get("next_cursor") if isinstance(body, dict) else None
        if cursor is None or cursor == "":
            return
        # A cursor that comes back unchanged would page for ever; Flow's extractor
        # would too, and a query that never ends is a worse answer than a refusal.
        if str(cursor) in replayed:
            raise AiwatcherUnreachableError(
                f"aiwatcher returned the cursor {cursor!r} twice for {url}; "
                "paging it would never end"
            )
        replayed.add(str(cursor))


def _hub_rows(
    client: httpx.Client,
    url: str,
    params: dict[str, str | int],
    arguments: Mapping[str, str],
    input_limit: int | None,
) -> tuple[list[Any], list[Any]]:
    """A bounded sample, by offset: `read("hub_rows", limit=891)` must not return 100.

    The page's envelope says what the corpus declares its columns are, which is what
    `row` is typed from — the route `hub_columns` reads for its envelope.
    """
    limit = _whole(arguments.get("limit", str(HUB_DEFAULT_LIMIT)), 1, HUB_MAX_LIMIT)
    offset = _whole(arguments.get("offset", "0"), 0, None)
    if limit is None or offset is None:
        raise QueryRefusedError(
            f"hub_rows takes limit= from 1 to {HUB_MAX_LIMIT} and a whole, nonnegative offset=."
        )
    if input_limit is not None:
        limit = min(limit, input_limit)
    page = min(HUB_PAGE, limit)
    records: list[Any] = []
    declared: list[Any] = []
    while len(records) < limit:
        body = _get(client, url, {**params, "offset": offset + len(records), "limit": page})
        if not declared and isinstance(body, dict) and isinstance(body.get("columns"), list):
            declared = body["columns"]
        rows = _rows_at(body, "rows", url)
        if not rows:
            break
        records.extend(rows)
    return records[:limit], declared


def _get(client: httpx.Client, url: str, params: Mapping[str, str | int]) -> Any:
    try:
        response = client.get(url, params=params)
    except httpx.HTTPError as error:
        raise AiwatcherUnreachableError(f"aiwatcher did not answer at {url}: {error}") from error
    if not 200 <= response.status_code < 300:
        raise UpstreamFailedError(
            response.status_code, _detail(response), str(response.request.url)
        )
    try:
        return response.json()
    except ValueError as error:
        raise AiwatcherUnreachableError(
            f"aiwatcher answered {url} with something that is not JSON"
        ) from error


def _detail(response: httpx.Response) -> str:
    """aiwatcher's own words, in whichever shape this route answers with.

    `{"error":{"message":…}}` is the API's error body and `{"message":…}` what a couple
    of older routes send. The reason phrase is the fallback rather than the body: a body
    that is not JSON is usually a proxy's HTML, and quoting it helps nobody.
    """
    try:
        body = response.json()
    except ValueError:
        body = None
    if isinstance(body, dict):
        error = body.get("error")
        if isinstance(error, dict) and isinstance(error.get("message"), str):
            return str(error["message"])
        if isinstance(body.get("message"), str):
            return str(body["message"])
    return response.reason_phrase or "no message"


def _rows_at(body: Any, path: str, url: str) -> list[Any]:
    value = body
    for key in path.split("."):
        value = value.get(key) if isinstance(value, dict) else None
    if not isinstance(value, list):
        raise AiwatcherUnreachableError(f"aiwatcher's answer for {url} has no list at '{path}'")
    return value


def table_of(
    dataset: Dataset, records: list[Any], *, row_type: pa.DataType | None = None
) -> pa.Table:
    """Records projected onto the dataset's columns, in its order and with its types.

    The API omits a null field rather than sending it as null, so a missing field is
    null here — a run that succeeded has no `error` key at all.
    """
    fields = []
    arrays = []
    for column, declared in dataset.columns.items():
        kind = row_type if row_type is not None and column == "row" else arrow_type(declared)
        source = _source(dataset, column)
        values = [_cell(_dig(record, source), kind, column) for record in records]
        fields.append(pa.field(column, kind))
        arrays.append(pa.array(values, type=kind))
    return pa.Table.from_arrays(arrays, schema=pa.schema(fields))


def arrow_type(declared: str) -> pa.DataType:
    """A catalog type as Arrow. `array` — an event's `data` — is JSON text.

    Text rather than a struct inferred from the rows, because events of different types
    carry different payloads and one table has one type per column: inference would be
    a guess that the next page contradicts.
    """
    match declared.removesuffix("|null"):
        case "int":
            return pa.int64()
        case "list<string>":
            return pa.list_(pa.string())
        case _:
            return pa.string()


def _row_type(declared: list[Any]) -> pa.DataType | None:
    """`hub_rows`' `row`, as a struct of the columns the corpus declares.

    Typed from the hub's declaration rather than inferred from one page, because a mixed
    corpus infers wrongly — Titanic's `Age` is null and float — and `col("row")["Age"]`
    is how a query addresses a field in either engine. No declaration, no struct: the
    row is then JSON text, as an event's `data` is.
    """
    fields = [
        pa.field(column["name"], _hub_type(column))
        for column in declared
        if isinstance(column, dict) and isinstance(column.get("name"), str)
    ]
    return pa.struct(fields) if fields else None


def _hub_type(column: Mapping[str, Any]) -> pa.DataType:
    kind, dtype = column.get("kind", ""), column.get("dtype", "")
    if kind == "Image":
        # The rows route hands a picture over as its address and its size, which
        # is all an import reads — a struct, so a query reaches `image.src` by
        # field rather than by parsing text.
        return pa.struct([("src", pa.string()), ("height", pa.int64()), ("width", pa.int64())])
    if kind == "ClassLabel" or (kind == "Value" and dtype in _INTEGER_DTYPES):
        return pa.int64()
    if kind == "Value" and dtype in _FLOAT_DTYPES:
        return pa.float64()
    if kind == "Value" and dtype == "bool":
        return pa.bool_()
    return pa.string()


def _cell(value: Any, kind: pa.DataType, column: str) -> Any:
    if value is None:
        return None
    if pa.types.is_struct(kind):
        if not isinstance(value, dict):
            return None
        return {field.name: _cell(value.get(field.name), field.type, field.name) for field in kind}
    if pa.types.is_list(kind):
        items = value if isinstance(value, list) else [value]
        return [_cell(item, kind.value_type, column) for item in items]
    if pa.types.is_string(kind):
        return value if isinstance(value, str) else json.dumps(value, separators=(",", ":"))
    if pa.types.is_integer(kind):
        if isinstance(value, float) and value.is_integer():
            return int(value)
        if isinstance(value, int) and not isinstance(value, bool):
            return value
    elif pa.types.is_floating(kind):
        if isinstance(value, int | float) and not isinstance(value, bool):
            return float(value)
    elif pa.types.is_boolean(kind) and isinstance(value, bool):
        return value
    # The same bytes answer the same way on every retry, so this is the query's 422
    # rather than an outage: the column's declared type and its data disagree.
    raise QueryRefusedError(f'Column "{column}" is declared {kind}, and the data holds {value!r}.')


def _source(dataset: Dataset, column: str) -> str:
    """Where a column lives inside one record — `Catalog::source`, in Python.

    Runs and spans are flat. A recorded event nests everything but `event_type` and
    `data` under `metadata`, and its producer one level deeper; an image head nests the
    image under `image`. Flattening here is what lets a query say `col("service")`.
    """
    if dataset.name == "annotation_images":
        return column if column == "review" else f"image.{column}"
    if dataset.name != "events":
        return column
    match column:
        case "event_type" | "data":
            return column
        case "service" | "sdk":
            return f"metadata.source.{column}"
        case _:
            return f"metadata.{column}"


def _dig(record: Any, path: str) -> Any:
    value = record
    for key in path.split("."):
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def _whole(raw: str, minimum: int, maximum: int | None) -> int | None:
    try:
        value = int(raw)
    except ValueError:
        return None
    if value < minimum or (maximum is not None and value > maximum):
        return None
    return value
