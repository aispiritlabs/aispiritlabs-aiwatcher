"""`read()`: the one way a query reaches data, in every engine.

A query names a dataset and never a path or a URL — the catalog decides what may be
read, as it does in Flow, where `Loader`, `Extractor` and `Filesystem` stay outside the
language. `read("spans", period="1h")` is Flow's `read(spans, period: '1h')` with
Python's punctuation, and it is checked the way Flow's builder checks it: the dataset
exists, a per-run route has its run, a period goes only to a windowed route, and every
other argument is one the route declares, with a value it takes. The checks are one
function, `plan_read`, which `/query/check` calls on a `read()` whose arguments are
written as literals — so what the check says is what the run would have said.
"""

from __future__ import annotations

import re
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

import httpx
import pyarrow as pa

from aiwatcher_query.api import fetch
from aiwatcher_query.catalog import Catalog, Dataset
from aiwatcher_query.engine import FileFormat, Session
from aiwatcher_query.errors import QueryRefusedError

_PERIOD = re.compile(r"^([1-9][0-9]*)(m|h|d|w)$")
_UNITS = {"m": 60, "h": 3_600, "d": 86_400, "w": 604_800}
MAX_PERIOD_SECONDS = 31_536_000


@dataclass(frozen=True)
class ReadPlan:
    """One `read()`, checked: which dataset, which run, which window, which arguments."""

    dataset: Dataset
    run: str | None = None
    window_seconds: int | None = None
    arguments: dict[str, str] = field(default_factory=dict)


def plan_read(
    catalog: Catalog,
    name: object,
    arguments: Mapping[str, object],
    window_seconds: int | None,
) -> ReadPlan:
    """A `read()`'s arguments, admitted against the catalog, or the refusal that names why.

    `window_seconds` is the request's; a `period=` replaces it for this read, because a
    script that pins its own period is saying how wide.
    """
    if not isinstance(name, str) or not name:
        raise QueryRefusedError('read() takes a dataset name, as text: read("spans").')
    dataset = catalog.resolve(name)
    if dataset is None:
        raise QueryRefusedError(
            f'There is no dataset "{name}". Available: {", ".join(catalog.names())} '
            '(and "default", which is "runs").'
        )

    run: str | None = None
    named: dict[str, str] = {}
    period_set = False
    for key, value in arguments.items():
        if key == "run":
            run = _text(value, "run")
        elif key == "period":
            window_seconds = parse_period(value)
            period_set = True
        else:
            named[key] = _text(value, key)

    if dataset.requires_run and not run:
        raise QueryRefusedError(
            f'The "{dataset.name}" dataset is per run: write read("{dataset.name}", run="run-1"). '
            "Without a run there is no route to call, and walking every run instead would be "
            "a request per run across the whole retention window."
        )
    if period_set and not dataset.windowed:
        raise QueryRefusedError(
            f'Dataset "{dataset.name}" does not take a period: its route has no window.'
        )

    for key, value in named.items():
        parameter = dataset.parameters.get(key)
        if parameter is None:
            raise QueryRefusedError(dataset.explain_unknown_parameter(key))
        if not parameter.accepts(value):
            offered = ", ".join(f"'{option}'" for option in parameter.values)
            raise QueryRefusedError(f'{key}= takes one of {offered}, not "{value}".')
    for key, parameter in dataset.parameters.items():
        if parameter.required and not named.get(key):
            raise QueryRefusedError(
                f'The "{dataset.name}" dataset needs {key}= — {parameter.description} '
                f'Write read("{dataset.name}", {key}="…").'
            )

    return ReadPlan(
        dataset=dataset,
        run=run,
        window_seconds=window_seconds if dataset.windowed else None,
        arguments=named,
    )


def parse_period(value: object) -> int | None:
    """A reproducible relative period: `"15m"`, `"6h"`, `"7d"`, `"2w"`, `"all"`, or seconds."""
    if value == "all":
        return None
    if isinstance(value, int) and not isinstance(value, bool):
        if value > 0:
            return value
        raise QueryRefusedError("period= seconds must be a positive whole number.")
    match = _PERIOD.match(value) if isinstance(value, str) else None
    if match is None:
        raise QueryRefusedError(
            'period= takes a duration such as "15m", "6h", "7d", "2w", "all", or seconds.'
        )
    seconds = int(match[1]) * _UNITS[match[2]]
    if seconds > MAX_PERIOD_SECONDS:
        raise QueryRefusedError(
            'period= is capped at 365d; use "all" for the whole retention window.'
        )
    return seconds


def _text(value: object, key: str) -> str:
    """An argument's value as the query string carries it. A whole number is its digits."""
    if isinstance(value, str):
        return value
    if isinstance(value, int) and not isinstance(value, bool):
        return str(value)
    raise QueryRefusedError(f"{key}= takes text or a whole number, not {type(value).__name__}.")


class Reader:
    """`read` in a query's namespace: a dataset by name, as the engine's frame.

    It remembers what it read, because the answer reports the first dataset's name,
    grain and window, and a corpus read anywhere makes the answer not deterministic.
    """

    def __init__(
        self,
        *,
        session: Session,
        catalog: Catalog,
        client: httpx.Client,
        aiwatcher: str,
        corpus_root: Path | None,
        window_seconds: int | None,
        as_of: int | None,
        input_limit: int | None,
    ) -> None:
        self._session = session
        self._catalog = catalog
        self._client = client
        self._aiwatcher = aiwatcher
        self._corpus_root = corpus_root
        self._window_seconds = window_seconds
        self._as_of = as_of
        self._input_limit = input_limit
        self.reads: list[ReadPlan] = []

    def __call__(self, name: object = None, /, **arguments: object) -> object:
        plan = plan_read(self._catalog, name, arguments, self._window_seconds)
        self.reads.append(plan)
        if plan.dataset.corpus is not None:
            return self._corpus(plan)
        table = fetch(
            self._client,
            self._aiwatcher,
            plan.dataset,
            run=plan.run,
            window_seconds=plan.window_seconds,
            arguments=plan.arguments,
            as_of=self._as_of,
            input_limit=self._input_limit,
        )
        return self._session.from_arrow(table)

    def _corpus(self, plan: ReadPlan) -> object:
        """Part files under the corpus root, scanned by the engine itself.

        Refused rather than read as empty when there are none: a glob matching nothing
        is a table with no rows, and "no spans" would be an answer about the data when
        the truth is that nobody generated any.
        """
        dataset = plan.dataset
        fmt = cast(FileFormat, plan.arguments.get("format") or "csv")
        root = self._corpus_root or Path()
        directory = root / f"{dataset.corpus}.{fmt}"
        parts = sorted(directory.glob(f"part-*.{fmt}"))
        if not parts:
            raise QueryRefusedError(
                f"{dataset.name} has no {fmt} part files in {directory}. Generate them with "
                "`just bench-curation-generate`, or point AIWATCHER_CORPUS_DIR at a directory "
                "that holds them."
            )
        schema = pa.schema(
            [
                pa.field(column, pa.int64() if declared.startswith("int") else pa.string())
                for column, declared in dataset.columns.items()
            ]
        )
        frame = self._session.read_files(parts, fmt, schema)
        # A preview reads a sample, never the corpus, and before any transform, so an
        # aggregation in the preview sees exactly that sample.
        return frame if self._input_limit is None else self._session.limit(frame, self._input_limit)
