import file from '@/shared/lib/content/duckdb.json';
import { readContent } from '@/shared/lib/content/read';

/**
 * What the panel ships written in DuckDB's language (AW-3): Python calling
 * DuckDB's relational API with `ColumnExpression`, `ConstantExpression`,
 * `FunctionExpression`, `CaseExpression`, `StarExpression` and `read()`. The
 * questions are the ones `flow.ts` asks, so a deployment that runs DuckDB starts
 * from text it runs.
 *
 * A relation has no `with_column`: a new column is a projection of every column
 * and the new one, `.project(StarExpression(), … .alias("name"))`, and a list
 * becomes one row per value the same way, through `unnest`. A struct's field is
 * a column named by its path, `ColumnExpression("row", "Name")`, and one row
 * per key is an aggregate — `arg_min` picks which row stands for it. None of the
 * text is SQL: a group key of bare column names is the only string a relation is
 * handed, so all of it is admitted under `strict`.
 *
 * The text itself is `content/duckdb.json`, the one copy, and
 * `services/query/duckdb/tests/test_duckdb_shipped_content.py` admits every piece
 * of it under `strict`. That check found `FunctionExpression("coalesce")` —
 * `coalesce` is SQL's operator, not a function in DuckDB's catalog — so the
 * features example fills a missing age with a CASE, from the status group's
 * median aggregated and joined back, because a window needs SQL's OVER; the
 * aggregate is aliased because both sides of the join are one read. `age` keeps
 * its null and `age_imputed` marks the guess, so the two never share a column.
 */
const content = readContent(file, 'content/duckdb.json');

/** The query the editor starts with: the runs, one row per agent. */
export const STARTER_QUERY = content.starterQuery;

/** A useful curation: successful production runs, the first of each session. */
export const STARTER_CURATION = content.starterCuration;

/** What a new transform block starts as. */
export const NEW_TRANSFORM = content.newTransform;

/** How a transform block's text is written, for the block inspector. */
export const TRANSFORM_HELP = content.transformHelp;

/** The recipe editor's cheat sheet: one line of DuckDB per thing a curation does. */
export const TRANSFORMATIONS = content.transformations;

/** The curations `flow.ts` ships, asked of DuckDB. */
export const QUERY_EXAMPLES = content.examples;
