"""Find personal data in a text column, count it, and hand on the masked text.

The demo block for the curation pipeline, and a working one: it is what the
`Hugging Face → Flow PHP → marimo → View` example in the panel runs.

Two rules it keeps, and they are the interesting part rather than the regular
expressions.

**A finding never carries the text it matched.** It carries a kind and a byte
range. A finding that quoted the address it found would put that address in
every list, every preview and every saved dataset version — the same rule the
conversation archive keeps for the same reason (see ADR_0021).

**What it hands on is masked.** The rows leaving this block are the ones a
dataset version is published from, so the text in them is the text with the
matches replaced by their kind. The original stays in the corpus it came from.

It is a *shape* matcher, not a classifier. It finds things built to a format —
an address, a card number, an IBAN — and it will not find a name, a diagnosis
or an address written out in prose. Treat a green row as "nothing of these
shapes", never as "no personal data".

And a shape is only a shape. The first real batch this ran over reported the
IMEI `06-184755-866851-3` as a card number, correctly by its own rules: an IMEI
carries a Luhn check digit, so it is fifteen digits that pass the test a card
number passes. What a masked span says is "something of this shape was here",
not what it was.
"""

import marimo

__generated_with = "0.24.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import re

    import marimo as mo

    from ml_pipeline import Block

    return Block, mo, re


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
        ## PII detection

        **{len(rows)}** rows in
        {"" if rows else "— nothing yet: press **Preview** in the panel"}
        · columns: `{"`, `".join(_columns) or "none"}`
        """
    )
    return


@app.cell
def _(mo, params, rows):
    # Every control's default comes from the block's settings, so a headless run
    # honours what the block was configured with and a live one starts there.
    # `.value` is the default until somebody moves it, which is what lets one
    # file be both the step and the thing you build the step in.
    _columns = sorted({name for row in rows for name in row}) or ["text"]
    column = mo.ui.dropdown(
        options=_columns,
        value=str(params.get("text_column") or _columns[0]),
        label="Text column",
    )
    kinds = mo.ui.multiselect(
        options=["email", "phone", "credit_card", "iban", "ip_address", "national_id"],
        value=list(params.get("kinds") or ["email", "phone", "credit_card", "iban", "ip_address"]),
        label="Look for",
    )
    only_matches = mo.ui.checkbox(
        value=bool(params.get("only_matches", False)),
        label="Keep only rows that matched",
    )
    mo.hstack([column, kinds, only_matches], justify="start", gap=2)
    return column, kinds, only_matches


@app.cell
def _(re):
    # Shapes, in the order a longer one must win: a card number would otherwise
    # be found by the phone pattern first and masked as the wrong thing.
    DETECTORS = {
        "iban": re.compile(r"\b[A-Z]{2}\d{2}[ ]?(?:[A-Z0-9]{4}[ ]?){2,7}[A-Z0-9]{1,4}\b"),
        # Ends on a digit rather than on an optional separator: `(?:\d[ -]?){13,19}`
        # swallows the space after the last one, and the mask then runs into the
        # next word.
        "credit_card": re.compile(r"\b\d(?:[ -]?\d){12,18}\b"),
        "email": re.compile(r"\b[\w.%+-]+@[\w-]+\.[A-Za-z]{2,}\b"),
        "ip_address": re.compile(r"\b(?:\d{1,3}\.){3}\d{1,3}\b"),
        "phone": re.compile(r"(?<!\w)(?:\+\d{1,3}[ -]?)?(?:\(?\d{2,4}\)?[ -]?){2,4}\d{2,4}(?!\w)"),
        "national_id": re.compile(r"\b\d{3}-\d{2}-\d{4}\b"),
    }

    def passes_luhn(digits: str) -> bool:
        """Whether a run of digits could be a card number.

        Without it every long number — an order id, a timestamp in milliseconds
        repeated twice — is a credit card, and a detector that flags everything
        is one people turn off.
        """
        numbers = [int(digit) for digit in digits if digit.isdigit()]
        if not 13 <= len(numbers) <= 19:
            return False
        total = 0
        for index, digit in enumerate(reversed(numbers)):
            if index % 2:
                digit *= 2
                if digit > 9:
                    digit -= 9
            total += digit
        return total % 10 == 0

    def scan(text: str, wanted: list[str]) -> list[dict[str, object]]:
        """Every match, as a kind and a range. Never as the text it matched."""
        found: list[dict[str, object]] = []
        taken: list[tuple[int, int]] = []
        for kind in DETECTORS:
            if kind not in wanted:
                continue
            for match in DETECTORS[kind].finditer(text):
                start, end = match.span()
                if kind == "credit_card" and not passes_luhn(match.group()):
                    continue
                if any(start < seen_end and seen_start < end for seen_start, seen_end in taken):
                    continue
                taken.append((start, end))
                found.append({"kind": kind, "start": start, "end": end})
        return sorted(found, key=lambda finding: int(finding["start"]))

    def mask(text: str, findings: list[dict[str, object]]) -> str:
        """The text with every match replaced by its kind, right to left."""
        masked = text
        for finding in reversed(findings):
            start, end = int(finding["start"]), int(finding["end"])
            masked = f"{masked[:start]}[{str(finding['kind']).upper()}]{masked[end:]}"
        return masked

    return mask, scan


@app.cell
def _(column, kinds, mask, only_matches, rows, scan):
    # `output` is what the next block reads: the step takes it out of the
    # definitions `App.run` returns. Every other name this cell binds is its own
    # business and is underscored — marimo refuses a notebook where two cells
    # define the same name, which is the rule that makes the graph a graph.
    output = []
    for _row in rows:
        _value = _row.get(column.value)
        _text = "" if _value is None else str(_value)
        _findings = scan(_text, list(kinds.value))
        if only_matches.value and not _findings:
            continue
        output.append(
            {
                **{name: value for name, value in _row.items() if name != column.value},
                column.value: mask(_text, _findings),
                "pii_count": len(_findings),
                "pii_kinds": sorted({str(finding["kind"]) for finding in _findings}),
                "pii_findings": _findings,
            }
        )
    return (output,)


@app.cell
def _(mo, output, rows):
    _matched = [row for row in output if row["pii_count"]]
    _by_kind: dict[str, int] = {}
    for _row in output:
        for _kind in _row["pii_kinds"]:
            _by_kind[_kind] = _by_kind.get(_kind, 0) + 1

    mo.hstack(
        [
            mo.stat(label="Rows scanned", value=len(rows)),
            mo.stat(label="Rows with a match", value=len(_matched)),
            mo.stat(label="Findings", value=sum(row["pii_count"] for row in output)),
            mo.stat(
                label="Kinds",
                value=", ".join(f"{kind} {count}" for kind, count in sorted(_by_kind.items()))
                or "none",
            ),
        ],
        gap=1,
    )
    return


@app.cell
def _(mo, output):
    mo.ui.table(output, page_size=10)
    return


if __name__ == "__main__":
    app.run()
