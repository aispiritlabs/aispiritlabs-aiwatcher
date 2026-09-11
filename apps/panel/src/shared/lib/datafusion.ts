import type { QueryExample } from '@/shared/lib/flow';

/**
 * What the panel ships written in DataFusion's language (AW-3): Python calling
 * DataFusion's DataFrame API with `col`, `lit`, `f` and `read()`. The questions
 * are the ones `flow.ts` asks, so a deployment that runs DataFusion starts from
 * text it runs.
 *
 * A query is a Python module whose last line is the DataFrame it answers with,
 * and a chain spans lines inside parentheses. Everything here is written to be
 * admitted under `strict` — only the names a query is given and DataFusion's own
 * methods — so it runs whichever admission a deployment chose.
 */

/** The query the editor starts with: the runs, one row per agent. */
export const STARTER_QUERY = `(
    read("default")
    .with_column("agent", col("agents"))
    .unnest_columns("agent")
    .aggregate(
        [col("agent")],
        [
            f.count(col("run_id")).alias("runs"),
            f.sum(col("input_tokens")).alias("input_tokens"),
        ],
    )
    .sort(col("runs").sort(ascending=False))
)`;

/** A useful curation: successful production runs, the first of each session. */
export const STARTER_CURATION = `(
    read("default", period="24h")
    .filter(col("status") == lit("succeeded"))
    .distinct_on(
        [col("conversation_id")],
        [
            col("run_id").alias("source_run_id"),
            col("conversation_id").alias("source_session_id"),
            col("trace_id").alias("source_trace_id"),
            col("agents"),
            col("started_at"),
        ],
        [col("conversation_id").sort(), col("started_at").sort()],
    )
)`;

/** What a new transform block starts as. */
export const NEW_TRANSFORM = 'df.limit(100)';

/** How a transform block's text is written, for the block inspector. */
export const TRANSFORM_HELP =
  'Python over df — the rows the blocks before this one produced — ending in a DataFrame, e.g. df.filter(col("status") == lit("error")). col, lit and f are DataFusion\'s, and every DataFusion block before the first notebook runs as one query.';

/** The recipe editor's cheat sheet: one line of DataFusion per thing a curation does. */
export const TRANSFORMATIONS: ReadonlyArray<readonly [string, string, string]> = [
  ['Period', 'read("default", period="24h")', 'Pin a relative period in the saved recipe.'],
  ['Filter', '.filter(col("status") == lit("succeeded"))', 'Keep cases matching a condition.'],
  ['Enrich', '.with_column("label", lit("production"))', 'Add labels or derived fields.'],
  [
    'Recode',
    '.with_column("sex_code", f.when(col("sex") == lit("female"), lit(1)).otherwise(lit(0)))',
    'Map a category onto a number or a shorter label.',
  ],
  [
    'Extract',
    '.with_column("title", f.regexp_replace(col("name"), lit(r"^[^,]*, ([^.]+)\\..*$"), lit(r"\\1")))',
    'Pull a field out of a string: the replacement is the captured group.',
  ],
  [
    'Band',
    '.with_column("band", f.when(col("age").is_null(), lit("unknown")).when(col("age") < lit(13), lit("child")).otherwise(lit("adult")))',
    'Turn a number into buckets. A CASE takes the first branch that holds, so the null check goes first.',
  ],
  [
    'Expand',
    '.with_column("agent", col("agents")).unnest_columns("agent")',
    'Turn a list into one row per value.',
  ],
  [
    'Deduplicate',
    '.distinct_on([col("conversation_id")], [col("run_id"), col("conversation_id")], [col("conversation_id").sort()])',
    'Choose the identity of one case, and which row stands for it.',
  ],
  ['Rename', '.with_column_renamed("run_id", "source_run_id")', 'Shape the dataset contract.'],
  [
    'Combine',
    '.filter((col("agent_id") == lit("a")) | (col("agent_id") == lit("b")))',
    'Match one of several agents or rules.',
  ],
  [
    'Aggregate',
    '.aggregate([col("model")], [f.count(col("span_id")).alias("spans")])',
    'Build summaries or grouped cases.',
  ],
  [
    'Distribution',
    '.aggregate([], [f.median(col("fare")), f.stddev(col("fare")), f.percentile_cont(col("fare").sort(), 0.9)])',
    "DataFusion's own. A group too small for a deviation answers null, not zero.",
  ],
  [
    'Arithmetic',
    '.with_column("family", col("sib_sp") + col("parch") + lit(1))',
    'Operators on columns, and f.round for a result somebody reads.',
  ],
  [
    'Window',
    'typical = df.aggregate([col("status")], [f.median(col("age")).alias("typical")])',
    "A group's answer beside every row: aggregate, then join it back on the key.",
  ],
  [
    'Join',
    '.join(other, left_on="run_id", right_on="run", how="left")',
    'Both sides are DataFrames read through the same catalog. The keys need different names, so rename the right one first.',
  ],
];

/** A Titanic passenger's title, and the status it collapses to. */
const TITLE = `f.regexp_replace(col("name"), lit(r"^[^,]*,\\s*([^.]+)\\..*$"), lit(r"\\1"))`;
const STATUS = `f.when(col("title") == lit("Mr"), lit("mr"))
        .when(col("title") == lit("Master"), lit("master"))
        .when((col("title") == lit("Mrs")) | (col("title") == lit("Mme")), lit("mrs"))
        .when(
            (col("title") == lit("Miss")) | (col("title") == lit("Mlle")) | (col("title") == lit("Ms")),
            lit("miss"),
        )
        .otherwise(lit("rare"))`;
const TITANIC = 'read("hub_rows", dataset="phihung/titanic", split="train", limit=100)';

/** The curations `flow.ts` ships, asked of DataFusion. */
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
        col("row")["Name"].alias("name"),
        col("row")["Sex"].alias("sex"),
        col("row")["Age"].alias("age"),
        col("row")["Pclass"].alias("pclass"),
        col("row")["Fare"].alias("fare"),
        col("row")["Embarked"].alias("embarked"),
        col("row")["Survived"].alias("survived"),
    )
    .with_column("sex_code", f.when(col("sex") == lit("female"), lit(1)).otherwise(lit(0)))
    .with_column("title", ${TITLE})
    .with_column(
        "status",
        ${STATUS},
    )
    .with_column("outcome", f.when(col("survived") == lit(1), lit("survived")).otherwise(lit("died")))
    .with_column(
        "port",
        f.when(col("embarked") == lit("S"), lit("Southampton"))
        .when(col("embarked") == lit("C"), lit("Cherbourg"))
        .when(col("embarked") == lit("Q"), lit("Queenstown"))
        .otherwise(lit("unknown")),
    )
    .with_column(
        "age_band",
        f.when(col("age").is_null(), lit("unknown"))
        .when(col("age") < lit(13), lit("child"))
        .when(col("age") < lit(20), lit("teen"))
        .when(col("age") < lit(60), lit("adult"))
        .otherwise(lit("senior")),
    )
    .select(
        col("name"), col("sex"), col("sex_code"), col("title"), col("status"), col("age"),
        col("age_band"), col("pclass"), col("fare"), col("port"), col("outcome"),
    )
)`,
  },
  {
    title: 'Titanic · features in one query',
    name: 'titanic/features',
    dataset: 'curation/titanic-features-query',
    description:
      "The whole feature engineering as one query: recode, fill a missing age from its status group's median, size the family, and keep the guess distinguishable from the measurement.",
    // The group's median is aggregated and joined back rather than windowed:
    // an aggregate over a partition needs DataFusion's `Window`, which is not a
    // name a query is given. `age` keeps its null and `age_imputed` says what
    // happened, so a measurement and a guess never share a column.
    query: `passengers = (
    ${TITANIC}
    .select(
        col("row")["Name"].alias("name"),
        col("row")["Sex"].alias("sex"),
        col("row")["Age"].alias("age"),
        col("row")["SibSp"].alias("sib_sp"),
        col("row")["Parch"].alias("parch"),
        col("row")["Fare"].alias("fare"),
    )
    .with_column("sex_code", f.when(col("sex") == lit("female"), lit(1)).otherwise(lit(0)))
    .with_column("title", ${TITLE})
    .with_column(
        "status",
        ${STATUS},
    )
)
typical = passengers.aggregate(
    [col("status")], [f.median(col("age")).alias("typical_age")]
).with_column_renamed("status", "status_group")
(
    passengers.join(typical, left_on="status", right_on="status_group", how="left")
    .with_column("age_imputed", col("age").is_null())
    .with_column("age_filled", f.coalesce(col("age"), col("typical_age")))
    .with_column("family_size", col("sib_sp") + col("parch") + lit(1))
    .with_column("is_alone", col("family_size") == lit(1))
    .with_column("fare_per_person", f.round(col("fare") / col("family_size"), lit(2)))
    .select(
        col("name"), col("status"), col("sex_code"), col("age"), col("age_filled"),
        col("age_imputed"), col("family_size"), col("is_alone"), col("fare_per_person"),
    )
)`,
  },
  {
    title: 'Titanic · age and fare spread',
    name: 'titanic/age-and-fare',
    dataset: 'curation/titanic-age-and-fare',
    description:
      'Median, deviation and a 90th percentile per class — the half of describe() a mean leaves out, all of it DataFusion’s own.',
    query: `(
    ${TITANIC}
    .select(
        col("row")["Pclass"].alias("pclass"),
        col("row")["Age"].alias("age"),
        col("row")["Fare"].alias("fare"),
    )
    .aggregate(
        [col("pclass")],
        [
            f.count(col("pclass")).alias("passengers"),
            f.avg(col("age")).alias("mean_age"),
            f.median(col("age")).alias("age_median"),
            f.stddev(col("age")).alias("age_spread"),
            f.median(col("fare")).alias("fare_median"),
            f.percentile_cont(col("fare").sort(), 0.9).alias("fare_p90"),
        ],
    )
    .with_column("age_spread", f.round(col("age_spread"), lit(2)))
    .sort(col("pclass").sort(ascending=True))
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
        col("row")["Name"].alias("name"),
        col("row")["Sex"].alias("sex"),
        col("row")["Pclass"].alias("pclass"),
        col("row")["Survived"].alias("survived"),
    )
    .with_column("title", ${TITLE})
    .with_column(
        "status",
        ${STATUS},
    )
    .aggregate(
        [col("status"), col("sex"), col("pclass")],
        [
            f.count(col("survived")).alias("passengers"),
            f.avg(col("survived")).alias("survival_rate"),
        ],
    )
    .sort(col("survival_rate").sort(ascending=False))
)`,
  },
];
