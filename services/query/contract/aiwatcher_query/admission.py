"""`strict` admission: a query is admitted before it runs, from the engine's own vocabulary.

ADR_0008's shape, in Python. Flow's `Dsl\\Registry` derives what a query may call from
Flow's own signatures and keeps a `DECLINED` table with a reason per name; this derives
what a strict query may name from the engine's own classes and modules, and each engine
keeps its `DECLINED`. There is no hand-written list of calls: the engine's public members
*are* the vocabulary, so a function the engine adds is usable the day it ships, and a
name nobody expected is refused because it is not the engine's.

What is written down is the grammar — the constructs a query is made of: names,
attributes, calls, literals, operators, subscripts and list comprehensions, and nothing
that imports, defines, loops or raises. Then four rules over names:

1. A name is one the query's namespace holds — `col`, `lit`, `f`, `read` — or one the
   query assigned. Builtins are not among them: they stay in the query's frame, because a
   C extension reads them from the frame that called it, and no strict query names one.
2. An attribute is a public member of the engine's classes or modules — a module's being
   its `__all__` when it declares one, which leaves out what the module merely imported:
   `datafusion.functions` imports pyarrow as `pa`, and `f.pa` is not DataFusion's API.
   Nothing that begins with `_` is named, as a name or as an attribute.
3. A name or attribute in `DECLINED` is refused with its reason, whatever else admits it:
   anything that reads or writes a file or a URL, registers, runs SQL or takes a callable.
   Two of those are read off the engine's own signatures rather than written down — a
   member with a parameter annotated as a callable or a path is declined by that alone,
   Flow's `Dsl\\Admission` rule — and an engine's table adds what no signature says.
4. An engine may refuse a call's arguments by a rule of its own — DuckDB's, that a string
   it would parse as SQL is not an argument a strict query passes.

Attributes are admitted by name rather than by the type of what they are read from,
because Python text has no types until it runs. What keeps that sound is that every value
a strict query can hold is the engine's object or a literal: the ways out of the engine —
to pandas, to a list, to Python values — are each engine's `DECLINED`, and `format`, the
one literal method that reaches attributes by name, is everybody's.
"""

from __future__ import annotations

import ast
import inspect
from collections.abc import Callable, Iterable, Iterator, Mapping, Set
from dataclasses import dataclass, field
from types import ModuleType

from aiwatcher_query.errors import QueryRefusedError

#: An engine's rule over one call's arguments: the reason it is refused, or None. It is
#: handed the whole query as well, so it can follow a name to what the query assigned it.
type CallRule = Callable[[ast.Call, str, ast.Module], str | None]

_FORMAT = "a format string reaches attributes by name, which a strict query never does"
_TAKES_CALLABLE = "it takes a callable, and a strict query passes no function"
_TAKES_PATH = "it takes a path, and a strict query reaches data only through read()"

#: Refused in every engine, whatever the engine's classes happen to hold.
EVERYONE_DECLINES: Mapping[str, str] = {"format": _FORMAT, "format_map": _FORMAT}

#: What a strict query is written with. Operators, comparisons and contexts are admitted
#: by their base classes, in `_OPERATORS`.
_GRAMMAR: frozenset[type[ast.AST]] = frozenset(
    {
        ast.Module,
        ast.Expr,
        ast.Assign,
        ast.Name,
        ast.Constant,
        ast.Attribute,
        ast.Subscript,
        ast.Slice,
        ast.Call,
        ast.keyword,
        ast.Starred,
        ast.List,
        ast.Tuple,
        ast.Dict,
        ast.Set,
        ast.ListComp,
        ast.DictComp,
        ast.comprehension,
        ast.BinOp,
        ast.BoolOp,
        ast.UnaryOp,
        ast.Compare,
        ast.IfExp,
        ast.JoinedStr,
        ast.FormattedValue,
    }
)
_OPERATORS = (ast.operator, ast.boolop, ast.unaryop, ast.cmpop, ast.Load, ast.Store)

_NO_FUNCTION = "A strict query passes no function, so it defines none"
_NO_IMPORT = "A strict query imports nothing; a dataset is reached through read()"
_NO_LOOP = "A strict query does not loop; a list comprehension, [ … for … in … ], repeats"
_OUTSIDE = "A strict query reaches nothing outside itself"

#: The commonest refused constructs, each in a sentence.
_CONSTRUCTS: Mapping[type[ast.AST], str] = {
    ast.Import: _NO_IMPORT,
    ast.ImportFrom: _NO_IMPORT,
    ast.Lambda: "A strict query passes no function, so it writes no lambda",
    ast.FunctionDef: _NO_FUNCTION,
    ast.AsyncFunctionDef: _NO_FUNCTION,
    ast.ClassDef: "A strict query defines no class",
    ast.GeneratorExp: "A strict query builds no generator; write a list, [ … ], instead",
    ast.For: _NO_LOOP,
    ast.While: _NO_LOOP,
    ast.Try: "A strict query catches nothing",
    ast.Raise: "A strict query raises nothing",
    ast.With: "A strict query opens no context",
    ast.Delete: "A strict query deletes nothing",
    ast.Global: _OUTSIDE,
    ast.Nonlocal: _OUTSIDE,
}


@dataclass(frozen=True)
class Vocabulary:
    """What a strict query on one engine may name."""

    #: The engine in a sentence: "DataFusion".
    engine: str
    #: The names a query's namespace holds, besides `read`.
    names: frozenset[str]
    #: Every public member of the engine's classes and modules a query reaches.
    attributes: frozenset[str]
    declined: Mapping[str, str] = field(default_factory=lambda: dict(EVERYONE_DECLINES))
    call_rule: CallRule | None = None

    @classmethod
    def of(
        cls,
        engine: str,
        *,
        names: Iterable[str],
        members: Iterable[object],
        declined: Mapping[str, str],
        call_rule: CallRule | None = None,
    ) -> Vocabulary:
        """Derived: the attributes are whatever the engine's classes and modules expose, and
        what their signatures take decides part of `declined` — the engine's own reasons
        win over the derived ones, being about the one name."""
        members = tuple(members)
        return cls(
            engine=engine,
            names=frozenset(names),
            attributes=frozenset(name for member in members for name in public_members(member)),
            declined={**EVERYONE_DECLINES, **declined_by_signature(members), **declined},
            call_rule=call_rule,
        )


def public_members(member: object) -> list[str]:
    """A class's public attributes, or a module's — its `__all__`, when it states one."""
    exported = getattr(member, "__all__", None) if isinstance(member, ModuleType) else None
    names = exported if isinstance(exported, list | tuple) else dir(member)
    return [name for name in names if not name.startswith("_")]


def declined_by_signature(members: Iterable[object]) -> dict[str, str]:
    """Every member whose signature takes a callable or a path, with the reason."""
    found: dict[str, str] = {}
    for member in members:
        for name in public_members(member):
            reason = _signature_refusal(getattr(member, name, None))
            if reason is not None and found.get(name) != _TAKES_CALLABLE:
                found[name] = reason
    return found


def _signature_refusal(value: object) -> str | None:
    if not callable(value):
        return None
    try:
        parameters = inspect.signature(value).parameters.values()
    except TypeError, ValueError:
        # A C function with no signature says nothing either way; its engine's table must.
        return None
    # A string under `from __future__ import annotations`, a type otherwise: both name it.
    annotations = [str(parameter.annotation) for parameter in parameters]
    if any("Callable" in annotation for annotation in annotations):
        return _TAKES_CALLABLE
    if any("Path" in annotation for annotation in annotations):
        return _TAKES_PATH
    return None


def admit(tree: ast.Module, vocabulary: Vocabulary) -> list[QueryRefusedError]:
    """Every reason this query may not run, in the order they appear; empty when it may."""
    assigned = {
        node.id
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store)
    }
    known = vocabulary.names | {"read"} | assigned
    return list(_problems(tree, vocabulary, known, tree))


def _problems(
    node: ast.AST, vocabulary: Vocabulary, known: Set[str], tree: ast.Module
) -> Iterator[QueryRefusedError]:
    """Parent before children, so `duckdb.read_csv(…)` is refused for `read_csv` first."""
    if not _in_grammar(node):
        # Refused as a whole: every name inside it would only be noise.
        yield _at(node, _CONSTRUCTS.get(type(node), _not_made_of(node)))
        return
    reason = _refusal(node, vocabulary, known, tree)
    if reason is not None:
        yield _at(node, reason)
    for child in ast.iter_child_nodes(node):
        yield from _problems(child, vocabulary, known, tree)


def _in_grammar(node: ast.AST) -> bool:
    return type(node) in _GRAMMAR or isinstance(node, _OPERATORS)


def _not_made_of(node: ast.AST) -> str:
    return (
        "A strict query is names, attributes, calls, literals and operators, and "
        f"{type(node).__name__} is none of them"
    )


def _refusal(
    node: ast.AST, vocabulary: Vocabulary, known: Set[str], tree: ast.Module
) -> str | None:
    match node:
        case ast.Name(id=name):
            return _name(name, vocabulary, known)
        case ast.Attribute(attr=attribute):
            return _attribute(attribute, vocabulary)
        case ast.Call(func=ast.Name(id=name) | ast.Attribute(attr=name)):
            return vocabulary.call_rule(node, name, tree) if vocabulary.call_rule else None
        case ast.Call():
            return "A strict query calls a function by its name"
    return None


def _name(name: str, vocabulary: Vocabulary, known: Set[str]) -> str | None:
    if name.startswith("_"):
        return f"{name} begins with an underscore, and a strict query names nothing that does"
    if name in known:
        # A name the query was given or assigned itself holds that value, whatever an
        # engine's table says about a *member* of the same name: a DuckDB relation has a
        # `df()` that leaves the engine, and `df` is the compiler's name for the rows so
        # far. The table still speaks for a name nobody gave the query.
        return None
    if name in vocabulary.declined:
        return f"{name} is not admitted: {vocabulary.declined[name]}"
    offered = ", ".join(sorted(vocabulary.names | {"read"}))
    return (
        f"{name} is not a name a strict query has. It has {offered}, and the names it "
        "assigns itself; a dataset is reached through read()"
    )


def _attribute(attribute: str, vocabulary: Vocabulary) -> str | None:
    if attribute.startswith("_"):
        return f"{attribute} begins with an underscore, and a strict query names nothing that does"
    if attribute in vocabulary.declined:
        return f"{attribute} is not admitted: {vocabulary.declined[attribute]}"
    if attribute not in vocabulary.attributes:
        return f"{attribute} is not part of {vocabulary.engine}'s API"
    return None


def _at(node: ast.AST, reason: str) -> QueryRefusedError:
    line = getattr(node, "lineno", 0)
    column = getattr(node, "col_offset", -1) + 1
    return QueryRefusedError(f"{reason}.", line=line, column=column)
