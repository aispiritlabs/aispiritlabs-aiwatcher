"""Everything the panel ships written in DataFusion's language, admitted by it under strict.

`test_panel_shapes.py` pins what the panel *generates*; this holds what it *ships* — both
starters, the examples, the cheat sheet and a new transform block's text and help — read
from `apps/panel/src/shared/lib/content/datafusion.json`, the file the panel is built from
(`conftest.pieces`). A piece refused here is text somebody would be offered and then
refused, in words they would read as their own mistake. `test_duckdb_shipped_content.py`
does this for DuckDB.
"""

from __future__ import annotations

import pytest

from aiwatcher_query.catalog import Catalog
from aiwatcher_query.config import DEFAULT_CATALOG
from conftest import Piece, pieces, refusals
from query_datafusion import ENGINE

PIECES = pieces("datafusion")


@pytest.mark.parametrize("piece", PIECES, ids=[piece.name for piece in PIECES])
def test_every_piece_the_panel_ships_is_admitted_under_strict(piece: Piece) -> None:
    catalog = Catalog.load(DEFAULT_CATALOG, corpus_root=None)
    assert refusals(piece, catalog, ENGINE.vocabulary()) == [], piece.text
