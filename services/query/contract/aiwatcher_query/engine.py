"""What an engine is, to the contract: two protocols and a way to find one by name.

Everything a Python engine shares — the routes, the catalog, the API pager, the answer,
the child a query runs in — lives in this package, and an engine supplies only what is
its own: the names a query is written with and how rows become its frame and back.
DataFusion and DuckDB each implement this in their own workspace member, so an image
installs one engine's wheel and never the other's.

The split between `Engine` and `Session` is the fork server's. An `Engine` is imported
into the fork server and must do nothing heavy when it is: the measurement behind
AW-3's decision is that a child forked after `import datafusion` answers, and one
forked after the parent *ran* a DataFusion query panics in Tokio. So everything that
holds a runtime — a `SessionContext`, a DuckDB connection — belongs to the `Session`,
opened in the child by `Engine.open()` and gone when the child exits.
"""

from __future__ import annotations

import importlib
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Literal, Protocol, cast

import pyarrow as pa

from aiwatcher_query.admission import Vocabulary

type FileFormat = Literal["csv", "parquet"]


class Session(Protocol):
    """One query's hold on an engine, opened in the child that runs it."""

    def namespace(self) -> Mapping[str, object]:
        """The names a query is written with: `col`, `lit`, `f` for DataFusion."""
        ...

    def from_arrow(self, table: pa.Table) -> object:
        """Rows the contract read from the aiwatcher API, as this engine's frame."""
        ...

    def read_files(self, paths: Sequence[Path], fmt: FileFormat, schema: pa.Schema) -> object:
        """Part files on disk, as a frame the engine scans itself.

        Its own scan rather than Arrow read here and handed over, because a corpus is
        gigabytes and the engine's streaming reader is the whole reason it was chosen.
        """
        ...

    def limit(self, frame: object, rows: int) -> object:
        """The first `rows` of a frame, still lazy — a preview reads a sample."""
        ...

    def is_frame(self, value: object) -> bool:
        """Whether a query's last expression is something this engine can answer with."""
        ...

    def collect(self, frame: object, rows: int) -> pa.Table:
        """At most `rows` rows of a frame, executed."""
        ...


class Engine(Protocol):
    """An engine's facts, and the door to a `Session`."""

    @property
    def name(self) -> str:
        """The word a deployment chooses it by: `datafusion`."""
        ...

    @property
    def language(self) -> str:
        """What a query is written in: `datafusion-python`."""
        ...

    @property
    def frame(self) -> str:
        """What a query ends in, in a sentence: `a DataFusion DataFrame`."""
        ...

    @property
    def volatile(self) -> frozenset[str]:
        """Functions whose value is the moment they ran — a query naming one is honest
        work and a wrong cache entry, so its answer says it is not deterministic."""
        ...

    @property
    def function_by_name(self) -> str | None:
        """The constructor that calls a function by its name, as text — DuckDB's
        `FunctionExpression("now")` — or None where a function is called as itself, as
        DataFusion's `f.now()` is. `deterministic` reads a volatile call through it."""
        ...

    def version(self) -> str:
        """The library's version, for healthz. Read without executing anything."""
        ...

    def vocabulary(self) -> Vocabulary:
        """What a query may name under `strict` admission: derived from the engine's own
        classes and modules, with its `DECLINED`. Also read in the service's own process,
        for `/query/check`, so it never opens a session — DuckDB's reads its function
        catalog from an in-memory database of its own, which holds no rows and forks
        nothing."""
        ...

    def open(self, corpus: Path | None = None) -> Session:
        """One query's session, in the child. `corpus` is the corpus root, for an engine
        that confines what a session may read to it — DuckDB locks its connection there."""
        ...


def load_engine(reference: str) -> Engine:
    """The engine a `module:attribute` reference names.

    By reference rather than by object because the child that runs a query is forked by
    the fork server and is handed a pickled job: a name crosses that boundary, and the
    module it names is already imported there.
    """
    module, _, attribute = reference.partition(":")
    if not module or not attribute:
        raise ValueError(f"'{reference}' is not an engine reference; write module:attribute")
    return cast(Engine, getattr(importlib.import_module(module), attribute))


def engine_module(reference: str) -> str:
    return reference.partition(":")[0]
