"""One notebook, run as a step, through marimo's own API.

`python -m ml_pipeline.step <notebook> <staged input> <output>`, which is what
[`ml_pipeline.runner`][] starts. It is a module with a `main` rather than code
inside the runner because the process boundary is the point — see the runner
for why — and this is the far side of it.

What happens here is three lines of marimo:

* import the notebook file, which is a Python module whose `app` is a
  `marimo.App`;
* `app.run(defs={"rows": …, "params": …})`, which uses those values *instead of*
  running the cell that would define them and re-runs everything downstream;
* read `output` out of the definitions it hands back.

No harness in the notebook, no output file for it to write, and the cell that
would have read the staged rows is simply not executed. What the notebook does
when somebody opens it in the panel is the same code taking its other branch.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from typing import Any

from ml_pipeline.block import DETERMINISTIC, OUTPUT
from ml_pipeline.staging import Row, read_input, write_json


class StepError(RuntimeError):
    """Something the notebook did, or did not do, that the chain cannot use."""


def load_app(path: Path) -> Any:
    """Import a notebook file and hand back its `marimo.App`."""
    spec = importlib.util.spec_from_file_location(f"ml_pipeline_notebook_{path.stem}", path)
    if spec is None or spec.loader is None:
        raise StepError(f"{path} cannot be imported as a module")
    module = importlib.util.module_from_spec(spec)
    # Registered before it is executed: a notebook whose cells reference the
    # module (marimo's generated `__main__` guard does) needs to find it.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    app = getattr(module, "app", None)
    if app is None:
        raise StepError(f"{path.name} defines no marimo app: a notebook needs `app = marimo.App()`")
    return app


def inject(app: Any, rows: list[Row], params: dict[str, Any], notebook: str) -> dict[str, Any]:
    """`App.run` with the block's rows in place of the cell that would read them.

    marimo replaces a **whole cell**, not one name, so the cell defining `rows`
    and `params` must define those two and nothing else. Getting that wrong is
    the one mistake this contract invites, and marimo's own message for it
    names the missing definitions but not what to do about them — so it is
    caught here and answered.
    """
    try:
        _outputs, definitions = app.run(defs={"rows": rows, "params": params})
    except Exception as error:
        # Matched by name rather than imported: `IncompleteRefsError` lives
        # under marimo's private `_ast.errors`, and an import of that is a
        # dependency on a path with no promise attached to it.
        if type(error).__name__ != "IncompleteRefsError":
            raise
        raise StepError(
            f"{notebook}'s `rows` and `params` cannot be injected, because the cell that "
            f"defines them defines other things too. marimo replaces the whole cell, so "
            f"move everything else into a cell of its own — and prefix what only that cell "
            f"uses with an underscore. marimo said: {error}"
        ) from error
    return dict(definitions)


def deterministic_of(definitions: dict[str, Any]) -> bool:
    """Whether this notebook says running it again would answer the same thing.

    Absent means yes. A value that is not a bool is also yes rather than an
    error: a notebook binding this name to something else has not said "no",
    and refusing the whole run over it would fail a chain for a spelling.
    """
    declared = definitions.get(DETERMINISTIC, True)
    return declared if isinstance(declared, bool) else True


def rows_of(definitions: dict[str, Any], notebook: str) -> list[Row]:
    """The `output` definition, checked for being a list of rows."""
    if OUTPUT not in definitions:
        raise StepError(
            f"{notebook} defines no `{OUTPUT}`: a block's last cell names the rows it "
            f"hands on, as `{OUTPUT} = …`"
        )
    produced = definitions[OUTPUT]
    if not isinstance(produced, list) or any(not isinstance(row, dict) for row in produced):
        raise StepError(
            f"{notebook}'s `{OUTPUT}` is {type(produced).__name__}; a block hands on a list of rows"
        )
    return list(produced)


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print("usage: python -m ml_pipeline.step <notebook> <input> <output>", file=sys.stderr)
        return 2

    notebook_path, input_path, output_path = (Path(value) for value in argv)
    try:
        staged = read_input(input_path, notebook_path.stem)
        app = load_app(notebook_path)
        definitions = inject(app, staged.rows, staged.params, notebook_path.stem)
        rows = rows_of(definitions, notebook_path.stem)
    except StepError as error:
        print(error, file=sys.stderr)
        return 1

    write_json(
        output_path,
        {
            "notebook": notebook_path.stem,
            "rows": rows,
            "deterministic": deterministic_of(definitions),
        },
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
