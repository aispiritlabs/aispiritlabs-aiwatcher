read("corpus_spans").aggregate(
    [
        ColumnExpression("run_id"),
        FunctionExpression("count", ColumnExpression("span_id")).alias("spans"),
        FunctionExpression("sum", ColumnExpression("input_tokens")).alias("input_tokens"),
        FunctionExpression("sum", ColumnExpression("output_tokens")).alias("output_tokens"),
        FunctionExpression("max", ColumnExpression("duration_ms")).alias("max_duration_ms"),
    ],
    "run_id",
).sort(ColumnExpression("run_id")).limit(100)
