"""The DataFusion query engine (AW-3): a query is DataFusion's own Python API.

`col`, `lit` and `f` — DataFusion's `functions` — are what a query is written with, and
`read()` is the contract's: a catalog dataset by name, as a DataFusion `DataFrame`. There
is no `SessionContext` in a query's namespace and no SQL, because the session is this
module's rather than the query's: one per query, opened in the child that runs it.

Importing this module imports DataFusion and runs nothing. The fork server preloads it,
and AW-3 measured that a child forked after `import datafusion` answers while one forked
after the parent *ran* a query panics in Tokio's I/O driver — so a `SessionContext` is
made in `open()`, in the child, and nowhere else. `vocabulary()` is also read in the
service's own process, for `/query/check`, which is why it reads classes and never makes
a session.
"""

from __future__ import annotations

import functools
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import cast

import datafusion
import pyarrow as pa
from datafusion import DataFrame, Expr, SessionContext, col, lit
from datafusion import functions as f
from datafusion.expr import CaseBuilder, ExprFuncBuilder, SortExpr

from aiwatcher_query import FileFormat, Session
from aiwatcher_query.admission import Vocabulary, public_members

#: What a query is written with, besides `read`.
NAMESPACE: Mapping[str, object] = {"col": col, "lit": lit, "f": f}

#: What a strict query's values can be: what `read()` returns, what `col`, `lit` and `f`
#: build, and what those build in turn — a sort key, a `case`, a window's builder.
MEMBERS: tuple[object, ...] = (DataFrame, Expr, SortExpr, CaseBuilder, ExprFuncBuilder, f)

#: Functions whose value is the moment they ran. The one list here: the Python binding
#: says nothing about a function's volatility, where DuckDB's catalog does.
VOLATILE = frozenset({"now", "random", "uuid", "current_date", "current_time"})

_LEAVES = "it leaves the engine, and a query answers with a DataFrame"
_REGISTERS = "it registers a Python function with the engine"
_SESSION = (
    "it is the SessionContext's, which a query never holds; a dataset is reached through read()"
)
_SQL = (
    "it runs SQL, and a strict query is DataFusion's Python API; a dataset is reached "
    "through read()"
)

#: What no signature says. The file writers and every function taking a Python callable
#: are declined by their signatures (`admission.declined_by_signature`), and everything a
#: `SessionContext` offers is declined by `_vocabulary`, from the class.
DECLINED: Mapping[str, str] = {
    **dict.fromkeys(
        (
            "collect",
            "collect_column",
            "collect_partitioned",
            "execute_stream",
            "execute_stream_partitioned",
            "python_value",
            "to_arrow_table",
            "to_bytes",
            "to_pandas",
            "to_polars",
            "to_pydict",
            "to_pylist",
        ),
        _LEAVES,
    ),
    **dict.fromkeys(("udf", "udaf", "udwf", "udtf"), _REGISTERS),
    "from_bytes": (
        "it decodes an expression from bytes, which can name what a query reaches only "
        "through read()"
    ),
    "into_view": "it makes a table to register, and a query registers nothing",
    "parse_sql_expr": _SQL,
    "sql": _SQL,
    "sql_with_options": _SQL,
    "write_table": "it writes into a table, and a query answers with rows rather than storing them",
}


class DataFusionSession:
    """One query's `SessionContext`, made in the child that runs it."""

    def __init__(self) -> None:
        self.context = SessionContext()

    def namespace(self) -> Mapping[str, object]:
        return NAMESPACE

    def from_arrow(self, table: pa.Table) -> object:
        return self.context.from_arrow(table)

    def read_files(self, paths: Sequence[Path], fmt: FileFormat, schema: pa.Schema) -> object:
        if fmt == "csv":
            # The catalog's types rather than inferred ones; an empty field reads as null.
            return self.context.read_csv([str(path) for path in paths], schema=schema)
        # Parquet carries its own types. DataFusion takes one path for it, so this reads
        # the parts' directory — every `.parquet` file there, which for a generated
        # corpus is the parts the contract found.
        (directory,) = {path.parent for path in paths}
        return self.context.read_parquet(str(directory), file_extension=".parquet")

    def limit(self, frame: object, rows: int) -> object:
        return _frame(frame).limit(rows)

    def is_frame(self, value: object) -> bool:
        return isinstance(value, DataFrame)

    def collect(self, frame: object, rows: int) -> pa.Table:
        return _frame(frame).limit(rows).to_arrow_table()


class DataFusionEngine:
    name = "datafusion"
    language = "datafusion-python"
    frame = "a DataFusion DataFrame"
    volatile = VOLATILE
    function_by_name: str | None = None

    def version(self) -> str:
        return str(datafusion.__version__)

    def vocabulary(self) -> Vocabulary:
        return _vocabulary()

    def open(self, corpus: Path | None = None) -> Session:
        # DataFusion's Python binding has no setting that confines what a session reads,
        # so the corpus root is not this engine's to hold (DuckDB's lock is).
        return DataFusionSession()


@functools.cache
def _vocabulary() -> Vocabulary:
    """Once per process: DataFusion's classes do not change while it runs."""
    offered = {name for member in MEMBERS for name in public_members(member)}
    session = {name: _SESSION for name in public_members(SessionContext) if name not in offered}
    return Vocabulary.of(
        "DataFusion", names=NAMESPACE, members=MEMBERS, declined={**session, **DECLINED}
    )


def _frame(value: object) -> DataFrame:
    # `evaluate` asked `is_frame` before handing a value here.
    return cast(DataFrame, value)


ENGINE = DataFusionEngine()
REFERENCE = "query_datafusion:ENGINE"
