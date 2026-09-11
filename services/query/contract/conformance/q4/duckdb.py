runs = (
    read("corpus_spans")
    .filter(ColumnExpression("kind") == ConstantExpression("llm"))
    .aggregate(
        [
            ColumnExpression("run_id").alias("run"),
            FunctionExpression("count", ColumnExpression("span_id")).alias("run_llm_spans"),
        ],
        "run_id",
    )
    # Both sides read the same files, and a join needs them told apart.
    .set_alias("runs")
)
(
    read("corpus_spans")
    .filter(
        (ColumnExpression("status") == ConstantExpression("ok"))
        & (ColumnExpression("kind") == ConstantExpression("llm"))
    )
    .join(runs, ColumnExpression("run_id") == ColumnExpression("run"))
    .select(
        ColumnExpression("span_id"),
        ColumnExpression("run_id"),
        FunctionExpression(
            "split_part", ColumnExpression("start"), ConstantExpression("T"), ConstantExpression(1)
        ).alias("day"),
        ColumnExpression("model"),
        (ColumnExpression("input_tokens") + ColumnExpression("output_tokens")).alias(
            "total_tokens"
        ),
        ColumnExpression("run_llm_spans"),
        ColumnExpression("duration_ms"),
    )
    .sort(ColumnExpression("span_id"))
    .limit(100)
)
