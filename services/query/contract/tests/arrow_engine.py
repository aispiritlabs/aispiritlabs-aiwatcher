"""An engine made of pyarrow alone, so the contract is tested without DataFusion or DuckDB.

Its frame is a `pyarrow.Table` and its expressions are `pyarrow.compute`'s, which is
enough to read, filter and answer — everything the contract does around an engine.
It is never served: `serve` is only ever handed a real engine's reference.
"""

from __future__ import annotations

import datetime as dt
from collections.abc import Mapping, Sequence
from pathlib import Path

import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.csv as pv
import pyarrow.parquet as pq

from aiwatcher_query import FileFormat, Session
from aiwatcher_query.admission import Vocabulary


class ArrowSession:
    def namespace(self) -> Mapping[str, object]:
        return {"pc": pc, "field": pc.field, "now": lambda: dt.datetime.now(dt.UTC)}

    def from_arrow(self, table: pa.Table) -> object:
        return table

    def read_files(self, paths: Sequence[Path], fmt: FileFormat, schema: pa.Schema) -> object:
        if fmt == "parquet":
            return pa.concat_tables([pq.read_table(path) for path in paths])
        options = pv.ConvertOptions(column_types=schema, strings_can_be_null=True)
        return pa.concat_tables([pv.read_csv(path, convert_options=options) for path in paths])

    def limit(self, frame: object, rows: int) -> object:
        assert isinstance(frame, pa.Table)
        return frame.slice(0, rows)

    def is_frame(self, value: object) -> bool:
        return isinstance(value, pa.Table)

    def collect(self, frame: object, rows: int) -> pa.Table:
        assert isinstance(frame, pa.Table)
        return frame.slice(0, rows)


class ArrowEngine:
    name = "arrow"
    language = "arrow-python"
    frame = "a pyarrow Table"
    volatile = frozenset({"now"})
    function_by_name: str | None = None

    def version(self) -> str:
        return str(pa.__version__)

    def vocabulary(self) -> Vocabulary:
        leaves = "it leaves the engine, and a query answers with a frame"
        return Vocabulary.of(
            "pyarrow",
            names=ArrowSession().namespace().keys(),
            members=[pa.Table, pc, pc.Expression],
            declined={
                "register_scalar_function": "it registers a Python function with the engine",
                "to_pandas": leaves,
                "to_pydict": leaves,
                "to_pylist": leaves,
            },
        )

    def open(self, corpus: Path | None = None) -> Session:
        return ArrowSession()


ENGINE = ArrowEngine()
REFERENCE = "arrow_engine:ENGINE"
