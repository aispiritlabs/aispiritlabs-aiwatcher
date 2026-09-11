"""The catalog every query engine serves, read from the one file that declares it.

`services/query/contract/catalog.json` holds what a dataset *is* — its route, where its
rows sit in a response, which parameter replays its cursor, its columns, its hints and
its `read()` arguments — and every engine loads it: Flow through `CatalogFile.php`, the
Python engines through this module (AW-3). How a dataset is *read* stays in each
engine, because that half is written in each engine's own language.

Read strictly, and in Flow's sentences. A key this module does not know is refused
naming the dataset and the key, because the likeliest one is a near miss, and
`"window": true` quietly leaving `windowed` false is a query that sends its window
nowhere and reads everything. `$comment` is the one key every level skips.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

COMMENT = "$comment"

DATASET_KEYS = (
    "name",
    "grain",
    "description",
    "path",
    "rows_path",
    "cursor_param",
    "requires_run",
    "windowed",
    "columns",
    "hints",
    "parameters",
    "corpus",
)
PARAMETER_KEYS = ("name", "required", "description", "values")


class CatalogError(Exception):
    """The declared catalog cannot be read: a start-up failure, never a request's."""


@dataclass(frozen=True)
class Parameter:
    """A named `read()` argument a dataset's route accepts, beyond `run` and `period`."""

    name: str
    required: bool
    description: str
    values: tuple[str, ...] = ()

    def accepts(self, value: str) -> bool:
        return not self.values or value in self.values


@dataclass(frozen=True)
class Dataset:
    """One named thing a query can read: a route of the aiwatcher API, or a corpus on disk.

    The columns are the contract rather than documentation. Every engine projects exactly
    these, in this order, so what `/query/datasets` shows is what a query can use.
    """

    name: str
    grain: str
    description: str
    columns: dict[str, str]
    path: str = ""
    rows_path: str = ""
    cursor_param: str = ""
    hints: dict[str, str] = field(default_factory=dict)
    #: The route addresses one run, so a read without `run=` has no route to call.
    requires_run: bool = False
    #: The route accepts `window_seconds` and `as_of`. The API refuses a parameter it
    #: does not know, so sending the window to a route without one is a 400.
    windowed: bool = False
    parameters: dict[str, Parameter] = field(default_factory=dict)
    #: A name under `AIWATCHER_CORPUS_DIR` when the rows are part files rather than a
    #: route; `path`, `rows_path` and `cursor_param` then describe nothing.
    corpus: str | None = None

    def explain_unknown_parameter(self, name: str) -> str:
        """What to tell somebody who wrote an argument this route has no idea about."""
        available = (
            "It takes: {}.".format(", ".join(f"{key}=" for key in self.parameters))
            if self.parameters
            else "It takes a dataset name, and period= when it is windowed."
        )
        return f'Dataset "{self.name}" has no read() argument "{name}". {available}'

    def describe(self) -> dict[str, Any]:
        """This dataset as `/query/datasets` lists it — the shape Flow's `datasets()` has."""
        return {
            "name": self.name,
            "aliases": ["default"] if self.name == "runs" else [],
            "grain": self.grain,
            "description": self.description,
            "requires_run": self.requires_run,
            "parameters": [
                {
                    "name": parameter.name,
                    "required": parameter.required,
                    "description": parameter.description,
                    "values": list(parameter.values),
                }
                for parameter in self.parameters.values()
            ],
            "columns": [{"name": name, "type": kind} for name, kind in self.columns.items()],
        }


class Catalog:
    """The datasets this service offers.

    A corpus dataset is offered only where a corpus root is configured: without one it
    names files that are not there, and "no spans" would be an answer about the data
    when the truth is that nobody pointed at any.
    """

    def __init__(self, datasets: dict[str, Dataset], *, corpus_root: Path | None) -> None:
        self._datasets = {
            name: dataset
            for name, dataset in datasets.items()
            if dataset.corpus is None or corpus_root is not None
        }

    @classmethod
    def load(cls, path: Path, *, corpus_root: Path | None) -> Catalog:
        return cls(read_catalog(path), corpus_root=corpus_root)

    def resolve(self, name: str) -> Dataset | None:
        """`default` is `runs`: the grain the explorer shows, and the cheap one."""
        return self._datasets.get("runs" if name == "default" else name)

    def all(self) -> list[Dataset]:
        return list(self._datasets.values())

    def names(self) -> list[str]:
        return list(self._datasets)


def read_catalog(path: Path) -> dict[str, Dataset]:
    """Every dataset the file declares, by name, in the order it declares them."""
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as error:
        raise CatalogError(
            f"The query catalog is not at {path}. Set AIWATCHER_QUERY_CATALOG to where it is."
        ) from error
    try:
        document = json.loads(raw)
    except json.JSONDecodeError as error:
        raise CatalogError(f"The query catalog at {path} is not JSON: {error}") from error

    version = document.get("version") if isinstance(document, dict) else None
    if (
        not isinstance(document, dict)
        or isinstance(version, bool)
        or version != 1
        or not isinstance(document.get("datasets"), list)
    ):
        raise CatalogError(f"The query catalog at {path} is not version 1 with a list of datasets.")
    _refuse_unknown(document, ("version", "datasets"), f"the catalog at {path}")

    datasets: dict[str, Dataset] = {}
    for entry in document["datasets"]:
        dataset = _dataset(entry, path)
        if dataset.name in datasets:
            raise CatalogError(f'{path} declares "{dataset.name}" twice.')
        datasets[dataset.name] = dataset
    return datasets


def _dataset(entry: object, path: Path) -> Dataset:
    if not isinstance(entry, dict) or not isinstance(entry.get("name"), str):
        raise CatalogError(f"A dataset in {path} has no name.")
    where = f'dataset "{entry["name"]}" in {path}'
    _refuse_unknown(entry, DATASET_KEYS, where)
    return Dataset(
        name=entry["name"],
        grain=_text(entry, "grain", where),
        description=_text(entry, "description", where),
        columns=_table(entry, "columns", where),
        path=_text(entry, "path", where, ""),
        rows_path=_text(entry, "rows_path", where, ""),
        cursor_param=_text(entry, "cursor_param", where, ""),
        hints=_table(entry, "hints", where),
        requires_run=_flag(entry, "requires_run", where),
        windowed=_flag(entry, "windowed", where),
        parameters=_parameters(entry, where),
        corpus=_text(entry, "corpus", where) if "corpus" in entry else None,
    )


def _parameters(entry: dict[str, Any], where: str) -> dict[str, Parameter]:
    declared = entry.get("parameters", [])
    if not isinstance(declared, list):
        raise CatalogError(f'{where}: "parameters" is a list.')
    parameters: dict[str, Parameter] = {}
    for one in declared:
        if not isinstance(one, dict) or not isinstance(one.get("name"), str):
            raise CatalogError(f"{where}: a parameter has no name.")
        at = f'parameter "{one["name"]}" of {where}'
        _refuse_unknown(one, PARAMETER_KEYS, at)
        values = one.get("values", [])
        if not isinstance(values, list) or not all(isinstance(value, str) for value in values):
            raise CatalogError(f'{at}: "values" is a list of strings.')
        parameters[one["name"]] = Parameter(
            name=one["name"],
            required=_flag(one, "required", at),
            description=_text(one, "description", at),
            values=tuple(values),
        )
    return parameters


def _text(entry: dict[str, Any], key: str, where: str, default: str | None = None) -> str:
    value = entry.get(key, default)
    if not isinstance(value, str):
        raise CatalogError(f'{where}: "{key}" is a string, and it is required.')
    return value


def _flag(entry: dict[str, Any], key: str, where: str) -> bool:
    value = entry.get(key, False)
    if not isinstance(value, bool):
        raise CatalogError(f'{where}: "{key}" is true or false.')
    return value


def _table(entry: dict[str, Any], key: str, where: str) -> dict[str, str]:
    """A name-to-text object — columns to their types, or names to hints."""
    value = entry.get(key, {})
    if not isinstance(value, dict) or not all(
        isinstance(text, str) for name, text in value.items() if name != COMMENT
    ):
        raise CatalogError(f'{where}: "{key}" maps names to text.')
    return {name: text for name, text in value.items() if name != COMMENT}


def _refuse_unknown(entry: dict[str, Any], known: tuple[str, ...], where: str) -> None:
    for key in entry:
        if key != COMMENT and key not in known:
            raise CatalogError(
                f'{where} has a key "{key}" this catalog does not know. '
                f"The keys are: {', '.join(known)}."
            )
