import file from '@/shared/lib/content/datafusion.json';
import { readContent } from '@/shared/lib/content/read';

/**
 * What the panel ships written in DataFusion's language (AW-3): Python calling
 * DataFusion's DataFrame API with `col`, `lit`, `f` and `read()`. The questions
 * are the ones `flow.ts` asks, so a deployment that runs DataFusion starts from
 * text it runs.
 *
 * A query is a Python module whose last line is the DataFrame it answers with,
 * and a chain spans lines inside parentheses. Everything is written to be
 * admitted under `strict` — only the names a query is given and DataFusion's own
 * methods — so it runs whichever admission a deployment chose.
 *
 * The text itself is `content/datafusion.json`, the one copy, and
 * `services/query/datafusion/tests/test_datafusion_shipped_content.py` holds that
 * claim: it admits every piece of the file under `strict`. The features example
 * aggregates the status group's median and joins it back rather than windowing
 * it: an aggregate over a partition needs DataFusion's `Window`, which is not a
 * name a query is given. `age` keeps its null and `age_imputed` says what
 * happened, so a measurement and a guess never share a column.
 */
const content = readContent(file, 'content/datafusion.json');

/** The query the editor starts with: the runs, one row per agent. */
export const STARTER_QUERY = content.starterQuery;

/** A useful curation: successful production runs, the first of each session. */
export const STARTER_CURATION = content.starterCuration;

/** What a new transform block starts as. */
export const NEW_TRANSFORM = content.newTransform;

/** How a transform block's text is written, for the block inspector. */
export const TRANSFORM_HELP = content.transformHelp;

/** The recipe editor's cheat sheet: one line of DataFusion per thing a curation does. */
export const TRANSFORMATIONS = content.transformations;

/** The curations `flow.ts` ships, asked of DataFusion. */
export const QUERY_EXAMPLES = content.examples;
