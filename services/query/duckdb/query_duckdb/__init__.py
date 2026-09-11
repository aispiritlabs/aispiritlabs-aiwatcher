"""The DuckDB query engine (AW-3): a query is DuckDB's own relational Python API.

`ColumnExpression`, `ConstantExpression`, `FunctionExpression`, `CaseExpression` and
`StarExpression` are what a query is written with, and `read()` is the contract's: a
catalog dataset by name, as a DuckDB relation. There is no connection in a query's
namespace, no `sql()` and no `SQLExpression`, because the connection is this module's:
one per query, opened in the child that runs it.

That connection is locked before a query sees it. `allowed_directories` is the corpus
root and `enable_external_access` is off, so DuckDB itself refuses a file or a URL named
anywhere else — in a SQL string, a glob, an extension it would fetch — and neither
setting can be changed back while the database runs. AW-3 measured what still works
under it: the corpus's CSV and Parquet, rows handed over as Arrow, a spill to the scratch
directory. Under `strict` admission it is a second wall beneath the first; under `open`
it holds only the session's own connection, because a query that imports `duckdb` can
open another (ADR_0028).

A sum over a `BIGINT` is a 128-bit `HUGEINT`, which reaches Arrow as a `decimal128(38,
0)`; the contract's answer turns a whole decimal into an integer and refuses one wider
than 64 bits by its column, so nothing about it is here.

Importing this module imports DuckDB and runs nothing: the fork server preloads it, and
a child forked from a process that has run a query is the case AW-3 would not build on.
The function catalog a strict `FunctionExpression` is admitted against is read on first
use, from an in-memory database of its own — in the service's process for `/query/check`,
in the child for a strict run, and never in the fork server.
"""

from __future__ import annotations

import ast
import functools
import os
import re
from collections.abc import Iterator, Mapping, Sequence, Set
from dataclasses import dataclass
from pathlib import Path
from typing import cast

import duckdb
import pyarrow as pa
from duckdb import (
    CaseExpression,
    ColumnExpression,
    ConstantExpression,
    DuckDBPyConnection,
    DuckDBPyRelation,
    Expression,
    FunctionExpression,
    StarExpression,
)

from aiwatcher_query import FileFormat, Session
from aiwatcher_query.admission import Vocabulary, public_members

#: What a query is written with, besides `read`.
NAMESPACE: Mapping[str, object] = {
    "ColumnExpression": ColumnExpression,
    "ConstantExpression": ConstantExpression,
    "FunctionExpression": FunctionExpression,
    "CaseExpression": CaseExpression,
    "StarExpression": StarExpression,
}

#: What a strict query's values can be: what `read()` returns, and what the five build.
MEMBERS: tuple[object, ...] = (DuckDBPyRelation, Expression)

_LEAVES = "it leaves the engine, and a query answers with a relation"
_WRITES = "it writes a table or a file, and a query answers with rows rather than storing them"
_TEXT = "it hands back SQL text, which a strict query has nowhere to put"
_SQL = (
    "it runs SQL, and a strict query is DuckDB's Python API; a filter is written with "
    "ColumnExpression and ConstantExpression"
)
_CONNECTION = (
    "it is the connection's, which a query never holds; a dataset is reached through read()"
)
_MODULE = (
    "it is the duckdb module's, which a query never imports; a dataset is reached through read()"
)

#: What no signature says: DuckDB is pybind11, so `admission.declined_by_signature` reads
#: nothing off it, and this table carries every way out. Everything a connection and the
#: `duckdb` module offer is declined by `_vocabulary`, from the class and the module.
DECLINED: Mapping[str, str] = {
    **dict.fromkeys(
        (
            "arrow",
            "close",
            "df",
            "explain",
            "fetch_arrow_reader",
            "fetch_arrow_table",
            "fetch_df_chunk",
            "fetch_record_batch",
            "fetchall",
            "fetchdf",
            "fetchmany",
            "fetchnumpy",
            "fetchone",
            "pl",
            "show",
            "tf",
            "to_arrow_reader",
            "to_arrow_table",
            "to_df",
            "torch",
        ),
        _LEAVES,
    ),
    **dict.fromkeys(
        (
            "create",
            "create_view",
            "insert",
            "insert_into",
            "to_csv",
            "to_parquet",
            "to_table",
            "to_view",
            "update",
            "write_csv",
            "write_parquet",
        ),
        _WRITES,
    ),
    **dict.fromkeys(("SQLExpression", "apply", "executemany", "from_query", "query", "sql"), _SQL),
    **dict.fromkeys(("get_name", "sql_query"), _TEXT),
    "execute": "it runs the relation, which the service does once, when it collects the answer",
    "map": "it takes a Python function, and a strict query passes none",
}

#: Functions a strict query may not name as `FunctionExpression`'s text, with the reason.
#: The rest of DuckDB's catalog is admitted as it stands.
DECLINED_FUNCTIONS: Mapping[str, str] = {
    "current_setting": "it reads the database's configuration, which names paths on this machine",
    # Not in the Python build, which DuckDB leaves it out of; declined for the one that has it.
    "getenv": "it reads the process's environment",
    "write_log": "it writes to the database's log, and a query answers with rows",
}

#: Table functions DuckDB's binder also takes in a projection, which is where
#: `FunctionExpression` puts one. `duckdb_functions()` lists `unnest` as a table function
#: only, and AW-3 measured it expanding a list inside `select`.
IN_A_PROJECTION = frozenset({"unnest"})

#: Every method a relation offers. Text handed to one is SQL, except where `NOT_SQL` says.
RELATION_METHODS = frozenset(
    name
    for name in public_members(DuckDBPyRelation)
    if callable(getattr(DuckDBPyRelation, name)) and name not in public_members(Expression)
)

#: Where a relation method's text is not SQL: a name DuckDB quotes, or a join's kind.
#: By position and by keyword, as a call can pass either.
NOT_SQL: Mapping[str, frozenset[int | str | None]] = {
    "set_alias": frozenset({0, "alias"}),
    "join": frozenset({2, "how"}),
}

#: `aggregate`'s group key is SQL too, and the one place a strict query hands a relation
#: text: a column name, or several, which DuckDB parses as nothing but columns.
GROUP_KEY: frozenset[int | str | None] = frozenset({1, "group_expr"})
_COLUMN_NAMES = re.compile(r"^\s*[A-Za-z_][A-Za-z0-9_]*(\s*,\s*[A-Za-z_][A-Za-z0-9_]*)*\s*$")

#: The catalog's two column types, as DuckDB's CSV reader names them.
_CSV_TYPES: Mapping[pa.DataType, str] = {pa.int64(): "BIGINT", pa.string(): "VARCHAR"}


@dataclass(frozen=True)
class Functions:
    """DuckDB's function catalog, as a query calls into it through `FunctionExpression`."""

    #: What `FunctionExpression` may name: scalar, aggregate and macro functions.
    callable: frozenset[str]
    #: Whose value is the moment it ran: every scalar DuckDB does not call consistent.
    volatile: frozenset[str]


@functools.cache
def functions() -> Functions:
    """Once per process, from a database of its own: DuckDB's catalog does not change."""
    with duckdb.connect(":memory:") as connection:
        rows = connection.sql(
            "SELECT function_name, function_type, stability FROM duckdb_functions()"
        ).fetchall()
    return Functions(
        callable=frozenset(
            name for name, kind, _ in rows if kind in {"scalar", "aggregate", "macro"}
        )
        | IN_A_PROJECTION,
        volatile=frozenset(
            name for name, kind, stability in rows if kind == "scalar" and stability != "CONSISTENT"
        ),
    )


class DuckDBSession:
    """One query's connection, made in the child that runs it and locked before it answers."""

    def __init__(self, corpus: Path | None) -> None:
        self.connection = duckdb.connect(":memory:")
        lock(self.connection, corpus)

    def namespace(self) -> Mapping[str, object]:
        return NAMESPACE

    def from_arrow(self, table: pa.Table) -> object:
        return self.connection.from_arrow(table)

    def read_files(self, paths: Sequence[Path], fmt: FileFormat, schema: pa.Schema) -> object:
        files = [str(path) for path in paths]
        if fmt == "csv":
            # The catalog's types rather than sniffed ones; an empty field reads as null.
            columns = {field.name: _CSV_TYPES[field.type] for field in schema}
            # The stub names one path; DuckDB reads a list of them as one relation.
            return self.connection.read_csv(files, header=True, columns=columns)  # type: ignore[arg-type]
        return self.connection.read_parquet(files)

    def limit(self, frame: object, rows: int) -> object:
        return _relation(frame).limit(rows)

    def is_frame(self, value: object) -> bool:
        return isinstance(value, DuckDBPyRelation)

    def collect(self, frame: object, rows: int) -> pa.Table:
        return _relation(frame).limit(rows).to_arrow_table()


class DuckDBEngine:
    name = "duckdb"
    language = "duckdb-python"
    frame = "a DuckDB relation"
    function_by_name: str | None = "FunctionExpression"

    @property
    def volatile(self) -> frozenset[str]:
        return functions().volatile

    def version(self) -> str:
        return str(duckdb.__version__)

    def vocabulary(self) -> Vocabulary:
        return _vocabulary()

    def open(self, corpus: Path | None = None) -> Session:
        return DuckDBSession(corpus)


def lock(connection: DuckDBPyConnection, corpus: Path | None) -> None:
    """Confine a connection to the corpus root for as long as it runs.

    `allowed_directories` first: DuckDB takes no value for it at `connect`, and refuses to
    change it once external access is off. The root in both spellings, because DuckDB
    resolves a file's path before it compares it — a symlink out of the corpus is refused,
    which AW-3 measured — so a root reached through one, `/tmp` on macOS, is allowed as
    what it resolves to as well as what it was called.
    """
    if corpus is not None:
        roots = sorted({str(corpus.absolute()), str(corpus.resolve())})
        listed = ", ".join(_literal(root.rstrip(os.sep) + os.sep) for root in roots)
        # The corpus root, quoted: configuration, never a query's text.
        connection.execute(f"SET allowed_directories = [{listed}]")
    # An extension is code DuckDB would fetch, or load from this user's home.
    connection.execute("SET autoinstall_known_extensions = false")
    connection.execute("SET autoload_known_extensions = false")
    connection.execute("SET enable_external_access = false")


def sql_rule(call: ast.Call, name: str, tree: ast.Module) -> str | None:
    """DuckDB's rule over a strict query's calls: text reaches no place DuckDB parses as SQL.

    A string handed to an expression is a column's name or a constant's value — AW-3
    measured `ColumnExpression("n") == "1+1"` looking for a column called `1+1` — while a
    relation method parses its text: `project("x + 1")` adds one. So text, or anything
    that may hold it, is refused in a relation method's arguments, except the names in
    `NOT_SQL` and a group key of bare column names. Text reaches a call through a name,
    a property or a method of other text as well as by being written there, which is why
    `_may_be_text` follows all four; and a method called through a name the query gave
    it is refused outright, because then nothing here can see which method it is.
    """
    if name == "FunctionExpression":
        return _function(call)
    if isinstance(call.func, ast.Name) and name in _assigned(tree):
        return (
            f"{name} is called by a name the query gave it, so admission cannot see which "
            "method it is; a strict DuckDB query calls a method by its own name"
        )
    if name not in RELATION_METHODS:
        return None
    text = _text_names(tree)
    for where, argument in _arguments(call):
        if where in NOT_SQL.get(name, frozenset()):
            continue
        if name == "aggregate" and where in GROUP_KEY:
            reason = _group_key(argument, text)
        elif _may_be_text(argument, text):
            reason = _parsed_as_sql(name, argument)
        else:
            reason = None
        if reason is not None:
            return reason
    return None


def _function(call: ast.Call) -> str | None:
    """`FunctionExpression`'s function, against DuckDB's own catalog."""
    named = (
        call.args[0]
        if call.args
        else next((kw.value for kw in call.keywords if kw.arg == "function_name"), None)
    )
    if not (isinstance(named, ast.Constant) and isinstance(named.value, str)):
        return (
            "FunctionExpression names its function as text written in the query, so "
            "admission can read which function it is"
        )
    function = named.value
    if function.lower() in DECLINED_FUNCTIONS:
        return f"{function} is not admitted: {DECLINED_FUNCTIONS[function.lower()]}"
    if function.lower() not in functions().callable:
        return f'"{function}" is not one of DuckDB\'s scalar or aggregate functions'
    return None


def _group_key(argument: ast.expr, text: Set[str]) -> str | None:
    match argument:
        case ast.Constant(value=str(key)) if _COLUMN_NAMES.match(key):
            return None
        case ast.Constant(value=str(key)):
            return (
                f'aggregate\'s group key is "{key}", which DuckDB would parse as SQL; a strict '
                'query groups by column names — "model", or "run_id, model" — and names a '
                "computed key with project() first"
            )
        case _ if _may_be_text(argument, text):
            return (
                "aggregate's group key is written in the query as column names, so admission "
                "can read that it is nothing else"
            )
    return None


_HOW = (
    "a filter is written with ColumnExpression and ConstantExpression, and a computed "
    "column with FunctionExpression"
)


def _parsed_as_sql(name: str, argument: ast.expr) -> str:
    shown = ast.unparse(argument)
    if isinstance(argument, ast.Constant):
        return f"{name} was handed the text {shown}, which DuckDB would parse as SQL; {_HOW}"
    return f"{name} was handed {shown}, and if that is text DuckDB would parse it as SQL; {_HOW}"


def _arguments(call: ast.Call) -> Iterator[tuple[int | str | None, ast.expr]]:
    yield from enumerate(call.args)
    for keyword in call.keywords:
        yield keyword.arg, keyword.value


def _assigned(tree: ast.Module) -> frozenset[str]:
    return frozenset(
        node.id
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store)
    )


def _text_names(tree: ast.Module) -> frozenset[str]:
    """The names a query binds to something that may be text, or may hold it.

    A fixpoint, because one name can be assigned from another; a list comprehension's
    target counts when what it walks may hold text. Conservative by construction: a
    strict query is straight-line, so every binding is visible, and a name bound to text
    anywhere is text everywhere.
    """
    names: set[str] = set()
    while True:
        found = set(names)
        for node in ast.walk(tree):
            match node:
                case ast.Assign(targets=targets, value=value) if _may_be_text(value, names):
                    found |= {n.id for t in targets for n in ast.walk(t) if isinstance(n, ast.Name)}
                case ast.comprehension(target=target, iter=walked) if _may_be_text(walked, names):
                    found |= {n.id for n in ast.walk(target) if isinstance(n, ast.Name)}
        if found == names:
            return frozenset(names)
        names = found


def _may_be_text(node: ast.AST, names: Set[str]) -> bool:
    """Whether a value may be text, or a container holding some, before anything runs."""
    match node:
        case ast.Constant(value=value):
            return isinstance(value, str | bytes)
        case ast.JoinedStr():
            return True
        case ast.Name(id=name):
            return name in names
        case ast.Attribute():
            # A property: a relation's `alias`, its `columns`, its `type`.
            return True
        case ast.Call(func=ast.Attribute(value=receiver)):
            # A method of text is text: `",".join(…)`. A method of a relation or an
            # expression is the engine's object.
            return _may_be_text(receiver, names)
        case ast.Call() | ast.Compare():
            # A constructor, read(), or a comparison: the engine's object or a bool.
            return False
        case ast.Starred(value=inner) | ast.Subscript(value=inner) | ast.UnaryOp(operand=inner):
            return _may_be_text(inner, names)
        case ast.BinOp(left=left, right=right):
            return _may_be_text(left, names) or _may_be_text(right, names)
        case ast.IfExp(body=body, orelse=orelse):
            return _may_be_text(body, names) or _may_be_text(orelse, names)
        case ast.BoolOp(values=values) | ast.List(elts=values) | ast.Tuple(elts=values):
            return any(_may_be_text(value, names) for value in values)
        case ast.Set(elts=values):
            return any(_may_be_text(value, names) for value in values)
        case ast.Dict(keys=keys, values=values):
            return any(_may_be_text(part, names) for part in [*keys, *values] if part)
        case ast.ListComp(elt=element):
            return _may_be_text(element, names)
        case ast.DictComp(key=key, value=value):
            return _may_be_text(key, names) or _may_be_text(value, names)
    # Nothing else is in a strict query's grammar; if it were, it would count as text.
    return True


@functools.cache
def _vocabulary() -> Vocabulary:
    """Once per process: DuckDB's classes do not change while it runs."""
    offered = {name for member in MEMBERS for name in public_members(member)} | set(NAMESPACE)
    module = {name: _MODULE for name in public_members(duckdb) if name not in offered}
    connection = {
        name: _CONNECTION for name in public_members(DuckDBPyConnection) if name not in offered
    }
    return Vocabulary.of(
        "DuckDB",
        names=NAMESPACE,
        members=MEMBERS,
        declined={**module, **connection, **DECLINED},
        call_rule=sql_rule,
    )


def _literal(text: str) -> str:
    return "'" + text.replace("'", "''") + "'"


def _relation(value: object) -> DuckDBPyRelation:
    # `evaluate` asked `is_frame` before handing a value here.
    return cast(DuckDBPyRelation, value)


ENGINE = DuckDBEngine()
REFERENCE = "query_duckdb:ENGINE"
