import type { QueryExample } from '@/shared/lib/flow';

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
 * per key is an aggregate — `arg_min` picks which row stands for it. No text
 * here is SQL: a group key of bare column names is the only string a relation is
 * handed, so everything is admitted under `strict`.
 */

/** The query the editor starts with: the runs, one row per agent. */
export const STARTER_QUERY = `(
    read("default")
    .project(StarExpression(), FunctionExpression("unnest", ColumnExpression("agents")).alias("agent"))
    .aggregate(
        [
            ColumnExpression("agent"),
            FunctionExpression("count", ColumnExpression("run_id")).alias("runs"),
            FunctionExpression("sum", ColumnExpression("input_tokens")).alias("input_tokens"),
        ],
        "agent",
    )
    .sort(ColumnExpression("runs").desc())
)`;

/** The run that stands for a session: the one that started first. */
const FIRST = (column: string, alias: string) =>
  `FunctionExpression("arg_min", ColumnExpression("${column}"), ColumnExpression("started_at")).alias("${alias}")`;

/** A useful curation: successful production runs, the first of each session. */
export const STARTER_CURATION = `(
    read("default", period="24h")
    .filter(ColumnExpression("status") == ConstantExpression("succeeded"))
    .aggregate(
        [
            ${FIRST('run_id', 'source_run_id')},
            ColumnExpression("conversation_id").alias("source_session_id"),
            ${FIRST('trace_id', 'source_trace_id')},
            ${FIRST('agents', 'agents')},
            FunctionExpression("min", ColumnExpression("started_at")).alias("started_at"),
        ],
        "conversation_id",
    )
)`;

/** What a new transform block starts as. */
export const NEW_TRANSFORM = 'df.limit(100)';

/** How a transform block's text is written, for the block inspector. */
export const TRANSFORM_HELP =
  'Python over df — the rows the blocks before this one produced — ending in a relation, e.g. df.filter(ColumnExpression("status") == ConstantExpression("error")). The five Expression classes are DuckDB\'s, and every DuckDB block before the first notebook runs as one query.';

/** The recipe editor's cheat sheet: one line of DuckDB per thing a curation does. */
export const TRANSFORMATIONS: ReadonlyArray<readonly [string, string, string]> = [
  ['Period', 'read("default", period="24h")', 'Pin a relative period in the saved recipe.'],
  [
    'Filter',
    '.filter(ColumnExpression("status") == ConstantExpression("succeeded"))',
    'Keep cases matching a condition.',
  ],
  [
    'Enrich',
    '.project(StarExpression(), ConstantExpression("production").alias("label"))',
    'Add labels or derived fields: every column, and the new one.',
  ],
  [
    'Recode',
    '.project(StarExpression(), CaseExpression(ColumnExpression("sex") == ConstantExpression("female"), ConstantExpression(1)).otherwise(ConstantExpression(0)).alias("sex_code"))',
    'Map a category onto a number or a shorter label.',
  ],
  [
    'Extract',
    '.project(StarExpression(), FunctionExpression("regexp_replace", ColumnExpression("name"), ConstantExpression(r"^[^,]*, ([^.]+)\\..*$"), ConstantExpression(r"\\1")).alias("title"))',
    'Pull a field out of a string: the replacement is the captured group.',
  ],
  [
    'Band',
    '.project(StarExpression(), CaseExpression(ColumnExpression("age").isnull(), ConstantExpression("unknown")).when(ColumnExpression("age") < ConstantExpression(13), ConstantExpression("child")).otherwise(ConstantExpression("adult")).alias("band"))',
    'Turn a number into buckets. A CASE takes the first branch that holds, so the null check goes first.',
  ],
  [
    'Expand',
    '.project(StarExpression(), FunctionExpression("unnest", ColumnExpression("agents")).alias("agent"))',
    'Turn a list into one row per value.',
  ],
  [
    'Deduplicate',
    `.aggregate([ColumnExpression("conversation_id"), ${FIRST('run_id', 'run_id')}], "conversation_id")`,
    'Choose the identity of one case, and which row stands for it.',
  ],
  [
    'Rename',
    '.project(StarExpression(exclude=["run_id"]), ColumnExpression("run_id").alias("source_run_id"))',
    'Shape the dataset contract.',
  ],
  [
    'Combine',
    '.filter((ColumnExpression("agent_id") == ConstantExpression("a")) | (ColumnExpression("agent_id") == ConstantExpression("b")))',
    'Match one of several agents or rules.',
  ],
  [
    'Aggregate',
    '.aggregate([ColumnExpression("model"), FunctionExpression("count", ColumnExpression("span_id")).alias("spans")], "model")',
    'Build summaries or grouped cases: the keys and the numbers, then the keys again as the group.',
  ],
  [
    'Distribution',
    '.aggregate([FunctionExpression("median", ColumnExpression("fare")), FunctionExpression("stddev_samp", ColumnExpression("fare")), FunctionExpression("quantile_cont", ColumnExpression("fare"), ConstantExpression(0.9))])',
    "DuckDB's own. A group too small for a deviation answers null, not zero.",
  ],
  [
    'Arithmetic',
    '.project(StarExpression(), (ColumnExpression("sib_sp") + ColumnExpression("parch") + ConstantExpression(1)).alias("family"))',
    'Operators on columns, and FunctionExpression("round", …) for a result somebody reads.',
  ],
  [
    'Window',
    'typical = df.aggregate([ColumnExpression("status").alias("status_group"), FunctionExpression("median", ColumnExpression("age")).alias("typical")], "status")',
    "A group's answer beside every row: aggregate, then join it back on the key.",
  ],
  [
    'Join',
    '.join(other.set_alias("other"), ColumnExpression("run_id") == ColumnExpression("run"), "left")',
    'Both sides are relations read through the same catalog. Two reads of one dataset need telling apart: an alias on one side, and a key named differently.',
  ],
];

/** A column of a hub row: a field of the `row` struct, by its path. */
const field = (name: string) => `ColumnExpression("row", "${name}")`;
/** One constant, written the way every one here is. */
const is = (column: string, value: string | number) =>
  `ColumnExpression("${column}") == ConstantExpression(${JSON.stringify(value)})`;

/** A Titanic passenger's title, and the status it collapses to. */
const TITLE = `FunctionExpression(
            "regexp_replace",
            ColumnExpression("name"),
            ConstantExpression(r"^[^,]*,\\s*([^.]+)\\..*$"),
            ConstantExpression(r"\\1"),
        )`;
const STATUS = `CaseExpression(${is('title', 'Mr')}, ConstantExpression("mr"))
        .when(${is('title', 'Master')}, ConstantExpression("master"))
        .when(
            ColumnExpression("title").isin(ConstantExpression("Mrs"), ConstantExpression("Mme")),
            ConstantExpression("mrs"),
        )
        .when(
            ColumnExpression("title").isin(
                ConstantExpression("Miss"), ConstantExpression("Mlle"), ConstantExpression("Ms")
            ),
            ConstantExpression("miss"),
        )
        .otherwise(ConstantExpression("rare"))`;
const SEX_CODE = `CaseExpression(${is('sex', 'female')}, ConstantExpression(1)).otherwise(ConstantExpression(0))`;
const TITANIC = 'read("hub_rows", dataset="phihung/titanic", split="train", limit=100)';

/** The curations `flow.ts` ships, asked of DuckDB. */
export const QUERY_EXAMPLES: QueryExample[] = [
  {
    title: 'Successful sessions',
    name: 'production/successful-sessions',
    dataset: 'evaluation/production-sessions',
    description: 'Successful production runs from the last day, the first run of each session.',
    query: STARTER_CURATION,
  },
  {
    title: 'Titanic · sex and status',
    name: 'titanic/passengers',
    dataset: 'curation/titanic-passengers',
    description:
      'Recode sex, pull the title out of the name and collapse it to a status, band the age, and say what Survived and Embarked mean in words.',
    query: `(
    ${TITANIC}
    .select(
        ${field('Name')}.alias("name"),
        ${field('Sex')}.alias("sex"),
        ${field('Age')}.alias("age"),
        ${field('Pclass')}.alias("pclass"),
        ${field('Fare')}.alias("fare"),
        ${field('Embarked')}.alias("embarked"),
        ${field('Survived')}.alias("survived"),
    )
    .project(
        StarExpression(),
        ${SEX_CODE}.alias("sex_code"),
        ${TITLE}.alias("title"),
    )
    .project(
        StarExpression(),
        ${STATUS}.alias("status"),
        CaseExpression(${is('survived', 1)}, ConstantExpression("survived"))
        .otherwise(ConstantExpression("died"))
        .alias("outcome"),
        CaseExpression(${is('embarked', 'S')}, ConstantExpression("Southampton"))
        .when(${is('embarked', 'C')}, ConstantExpression("Cherbourg"))
        .when(${is('embarked', 'Q')}, ConstantExpression("Queenstown"))
        .otherwise(ConstantExpression("unknown"))
        .alias("port"),
        CaseExpression(ColumnExpression("age").isnull(), ConstantExpression("unknown"))
        .when(ColumnExpression("age") < ConstantExpression(13), ConstantExpression("child"))
        .when(ColumnExpression("age") < ConstantExpression(20), ConstantExpression("teen"))
        .when(ColumnExpression("age") < ConstantExpression(60), ConstantExpression("adult"))
        .otherwise(ConstantExpression("senior"))
        .alias("age_band"),
    )
    .select(
        ColumnExpression("name"), ColumnExpression("sex"), ColumnExpression("sex_code"),
        ColumnExpression("title"), ColumnExpression("status"), ColumnExpression("age"),
        ColumnExpression("age_band"), ColumnExpression("pclass"), ColumnExpression("fare"),
        ColumnExpression("port"), ColumnExpression("outcome"),
    )
)`,
  },
  {
    title: 'Titanic · features in one query',
    name: 'titanic/features',
    dataset: 'curation/titanic-features-query',
    description:
      "The whole feature engineering as one query: recode, fill a missing age from its status group's median, size the family, and keep the guess distinguishable from the measurement.",
    // The group's median is aggregated and joined back: a window needs SQL's
    // OVER, which is not a name a query is given. The aggregate is aliased
    // because both sides of the join are one read. `age` keeps its null and
    // `age_imputed` says what happened, so a measurement and a guess never
    // share a column.
    query: `passengers = (
    ${TITANIC}
    .select(
        ${field('Name')}.alias("name"),
        ${field('Sex')}.alias("sex"),
        ${field('Age')}.alias("age"),
        ${field('SibSp')}.alias("sib_sp"),
        ${field('Parch')}.alias("parch"),
        ${field('Fare')}.alias("fare"),
    )
    .project(
        StarExpression(),
        ${SEX_CODE}.alias("sex_code"),
        ${TITLE}.alias("title"),
    )
    .project(StarExpression(), ${STATUS}.alias("status"))
)
typical = passengers.aggregate(
    [
        ColumnExpression("status").alias("status_group"),
        FunctionExpression("median", ColumnExpression("age")).alias("typical_age"),
    ],
    "status",
).set_alias("typical")
(
    passengers.join(typical, ColumnExpression("status") == ColumnExpression("status_group"), "left")
    .project(
        StarExpression(),
        ColumnExpression("age").isnull().alias("age_imputed"),
        CaseExpression(ColumnExpression("age").isnull(), ColumnExpression("typical_age"))
        .otherwise(ColumnExpression("age"))
        .alias("age_filled"),
        (ColumnExpression("sib_sp") + ColumnExpression("parch") + ConstantExpression(1))
        .alias("family_size"),
    )
    .project(
        StarExpression(),
        (${is('family_size', 1)}).alias("is_alone"),
        FunctionExpression(
            "round", ColumnExpression("fare") / ColumnExpression("family_size"), ConstantExpression(2)
        ).alias("fare_per_person"),
    )
    .select(
        ColumnExpression("name"), ColumnExpression("status"), ColumnExpression("sex_code"),
        ColumnExpression("age"), ColumnExpression("age_filled"), ColumnExpression("age_imputed"),
        ColumnExpression("family_size"), ColumnExpression("is_alone"),
        ColumnExpression("fare_per_person"),
    )
)`,
  },
  {
    title: 'Titanic · age and fare spread',
    name: 'titanic/age-and-fare',
    dataset: 'curation/titanic-age-and-fare',
    description:
      'Median, deviation and a 90th percentile per class — the half of describe() a mean leaves out, all of it DuckDB’s own.',
    query: `(
    ${TITANIC}
    .select(
        ${field('Pclass')}.alias("pclass"),
        ${field('Age')}.alias("age"),
        ${field('Fare')}.alias("fare"),
    )
    .aggregate(
        [
            ColumnExpression("pclass"),
            FunctionExpression("count", ColumnExpression("pclass")).alias("passengers"),
            FunctionExpression("avg", ColumnExpression("age")).alias("mean_age"),
            FunctionExpression("median", ColumnExpression("age")).alias("age_median"),
            FunctionExpression(
                "round", FunctionExpression("stddev_samp", ColumnExpression("age")), ConstantExpression(2)
            ).alias("age_spread"),
            FunctionExpression("median", ColumnExpression("fare")).alias("fare_median"),
            FunctionExpression(
                "quantile_cont", ColumnExpression("fare"), ConstantExpression(0.9)
            ).alias("fare_p90"),
        ],
        "pclass",
    )
    .sort(ColumnExpression("pclass").asc())
)`,
  },
  {
    title: 'Titanic · survival by status',
    name: 'titanic/survival-by-status',
    dataset: 'curation/titanic-survival-by-status',
    description:
      'Survival rate and headcount by status and class, sorted by rate. The first table every Titanic notebook draws.',
    query: `(
    ${TITANIC}
    .select(
        ${field('Name')}.alias("name"),
        ${field('Sex')}.alias("sex"),
        ${field('Pclass')}.alias("pclass"),
        ${field('Survived')}.alias("survived"),
    )
    .project(StarExpression(), ${TITLE}.alias("title"))
    .project(StarExpression(), ${STATUS}.alias("status"))
    .aggregate(
        [
            ColumnExpression("status"),
            ColumnExpression("sex"),
            ColumnExpression("pclass"),
            FunctionExpression("count", ColumnExpression("survived")).alias("passengers"),
            FunctionExpression("avg", ColumnExpression("survived")).alias("survival_rate"),
        ],
        "status, sex, pclass",
    )
    .sort(ColumnExpression("survival_rate").desc())
)`,
  },
];
