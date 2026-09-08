"""Fill a missing age, size a family, and band what the columns say.

The second demo block, and the one that shows where the seam between the two
engines actually is. It runs after a Flow PHP transform that has already
flattened the Titanic corpus and recoded `Sex` and the title in `Name` — see
the `Hugging Face → Flow PHP → marimo → View` example named
`curation/titanic-features` in the panel.

It does two things: fills a missing age with the median age of the passengers
carrying the same status, and adds `family_size` — siblings plus parents plus
one — with the bands and the per-person fare that follow from those.

**None of that needs a notebook any more, and it is worth saying so here.**
When this was written, the query surface was a hand-written list of names that
had no arithmetic in it and no way to put a group's answer back beside a row,
so this block was described as the far side of a line ADR_0024 drew. That was
a claim about the list rather than about Flow. The list is gone (ADR_0008,
amended), and the same work is now one query — the Recipe view ships it as
`titanic/features`, with `->over(window()->partitionBy(ref('status')))` doing
the fill and `->plus(...)` doing the arithmetic.

What is left here is a worked example of a *notebook block*: the contract, the
widgets, and a computation somebody can read. The honest rule for reaching for
one is narrower than this file's first version claimed — a notebook is for what
the query language has no vocabulary for at all, which is a model, a classifier
or a library, and `pii_detection` is that example. Arithmetic and group
statistics are not, and assuming they were cost this repository a second engine
in a chain that did not need one.

One rule it keeps, and it is the interesting part rather than the medians. **An
imputed age never overwrites a reported one.** `age_filled` is the column a
model would read, `age_imputed` says whether it was invented, and the original
`age` is still there holding `None`. A pipeline that filled the column in place
would publish a dataset in which a measurement and a guess are the same thing,
and nothing downstream — a split, a metric, a model card — could tell them
apart afterwards.

It is deterministic: the same rows and the same settings give the same answer,
so a managed run may cache it. That is a claim about this notebook and not
about notebooks, which is why `deterministic` is something a block says rather
than something the runtime works out.
"""

import marimo

__generated_with = "0.24.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import statistics

    import marimo as mo

    from ml_pipeline import Block

    return Block, mo, statistics


@app.cell
def _(Block):
    # The one injected cell. Running as a step, `App.run(defs=…)` provides both
    # names and this cell is not executed at all; open in the panel, it reads
    # the rows the last run staged. `_block` is underscored so the cell defines
    # nothing else — an injected name replaces its whole cell.
    _block = Block.for_notebook(__file__)
    rows = _block.get_rows()
    params = _block.get_params()
    return params, rows


@app.cell
def _(mo, rows):
    _columns = sorted({name for row in rows for name in row})
    mo.md(
        f"""
        ## Titanic features

        **{len(rows)}** rows in
        {"" if rows else "— nothing yet: press **Preview** in the panel"}
        · columns: `{"`, `".join(_columns) or "none"}`
        """
    )
    return


@app.cell
def _(mo, params, rows):
    # Which column is which is a question about the query that ran before this,
    # so it is a setting rather than a constant. The defaults are the names the
    # shipped Flow transform produces.
    _columns = sorted({name for row in rows for name in row})

    def _column(setting: str, fallback: str, label: str):
        # The configured name is always an option, so a block whose upstream
        # query has not run yet still opens on what it was configured with
        # rather than on somebody else's first column.
        wanted = str(params.get(setting) or fallback)
        return mo.ui.dropdown(options=sorted({*_columns, wanted}), value=wanted, label=label)

    age = _column("age_column", "age", "Age")
    status = _column("status_column", "status", "Status")
    siblings = _column("siblings_column", "sib_sp", "Siblings and spouses")
    parents = _column("parents_column", "parch", "Parents and children")
    fare = _column("fare_column", "fare", "Fare")

    mo.hstack([age, status, siblings, parents, fare], justify="start", gap=1)
    return age, fare, parents, siblings, status


@app.cell
def _(mo, params):
    _strategies = ["status_median", "overall_median", "keep"]
    _wanted = str(params.get("age_strategy") or _strategies[0])
    strategy = mo.ui.dropdown(
        options=_strategies,
        value=_wanted if _wanted in _strategies else _strategies[0],
        label="Fill a missing age with",
    )

    def _within(setting: str, fallback: int, start: int, stop: int) -> int:
        # A slider's ends are an affordance rather than a policy, and a setting
        # outside them is a `ValueError` that takes the whole run down before
        # anything has read a row. Clamped, and the two ends are wide enough
        # that clamping is an accident rather than a design.
        return max(start, min(stop, int(params.get(setting) or fallback)))

    child_max = mo.ui.slider(
        start=1,
        stop=25,
        value=_within("child_max", 12, 1, 25),
        label="A child is under",
    )
    senior_min = mo.ui.slider(
        start=30,
        stop=90,
        value=_within("senior_min", 60, 30, 90),
        label="A senior is from",
    )
    mo.hstack([strategy, child_max, senior_min], justify="start", gap=2)
    return child_max, senior_min, strategy


@app.cell
def _(age, rows, statistics, status):
    # The statistic this block exists for: one median per status, over the rows
    # that reported an age. A group nobody reported an age for falls back to the
    # overall median rather than to nothing, which is what stops one rare title
    # deciding a row keeps no age at all.
    def number(value):
        """A float, or None. A bool is not a number here — `True` is not an age."""
        if value is None or isinstance(value, bool):
            return None
        if isinstance(value, int | float):
            return float(value)
        try:
            return float(str(value).strip())
        except ValueError:
            return None

    _reported = {}
    for _row in rows:
        _age = number(_row.get(age.value))
        if _age is None:
            continue
        _reported.setdefault(str(_row.get(status.value) or "unknown"), []).append(_age)

    medians = {key: statistics.median(values) for key, values in sorted(_reported.items())}
    overall = (
        statistics.median([value for values in _reported.values() for value in values])
        if _reported
        else None
    )
    return medians, number, overall


@app.cell
def _(
    age,
    child_max,
    fare,
    medians,
    number,
    overall,
    parents,
    rows,
    senior_min,
    siblings,
    status,
    strategy,
):
    # `output` is what the next block reads: the step takes it out of the
    # definitions `App.run` returns. Every other name this cell binds is its own
    # business and is underscored — marimo refuses a notebook where two cells
    # define the same name, which is the rule that makes the graph a graph.
    def _band(value):
        if value is None:
            return "unknown"
        if value < child_max.value:
            return "child"
        # Twenty is where a teenager stops being one, which is a fact about the
        # word rather than a choice this block offers.
        if value < 20:
            return "teen"
        if value >= senior_min.value:
            return "senior"
        return "adult"

    def _filled(reported, group):
        """The age to model on, and whether it was invented."""
        if reported is not None or strategy.value == "keep":
            return reported, False
        guess = medians.get(group) if strategy.value == "status_median" else overall
        guess = overall if guess is None else guess
        return (None, False) if guess is None else (round(guess, 1), True)

    output = []
    for _row in rows:
        _status = str(_row.get(status.value) or "unknown")
        _age, _imputed = _filled(number(_row.get(age.value)), _status)
        # Plus one for the passenger, which is the whole of the arithmetic the
        # query could not do.
        _siblings = int(number(_row.get(siblings.value)) or 0)
        _parents = int(number(_row.get(parents.value)) or 0)
        _family = 1 + _siblings + _parents
        _fare = number(_row.get(fare.value))
        output.append(
            {
                **_row,
                "age_filled": _age,
                "age_imputed": _imputed,
                "age_band": _band(_age),
                "family_size": _family,
                "is_alone": _family == 1,
                "fare_per_person": None if _fare is None else round(_fare / _family, 2),
            }
        )
    return (output,)


@app.cell
def _(mo, output):
    _bands: dict[str, int] = {}
    for _row in output:
        _bands[_row["age_band"]] = _bands.get(_row["age_band"], 0) + 1

    mo.hstack(
        [
            mo.stat(label="Rows", value=len(output)),
            mo.stat(
                label="Ages imputed",
                value=sum(1 for row in output if row["age_imputed"]),
            ),
            mo.stat(label="Travelling alone", value=sum(1 for row in output if row["is_alone"])),
            mo.stat(
                label="Bands",
                value=", ".join(f"{band} {count}" for band, count in sorted(_bands.items()))
                or "none",
            ),
        ],
        gap=1,
    )
    return


@app.cell
def _(medians, mo):
    # The medians are shown because they are the part of this block a reader has
    # to agree with: they are what every imputed age in the table below is.
    mo.md(
        "**Median reported age by status** — "
        + (
            ", ".join(f"`{status}` {median:g}" for status, median in medians.items())
            or "nothing reported an age"
        )
    )
    return


@app.cell
def _(mo, output):
    mo.ui.table(output, page_size=10)
    return


if __name__ == "__main__":
    app.run()
