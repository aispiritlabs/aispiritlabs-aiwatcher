model = ColumnExpression("model")
read("corpus_spans").filter(ColumnExpression("kind") == ConstantExpression("llm")).aggregate(
    [
        model,
        FunctionExpression("count", ColumnExpression("span_id")).alias("spans"),
        FunctionExpression("sum", ColumnExpression("input_tokens")).alias("input_tokens"),
        FunctionExpression("sum", ColumnExpression("output_tokens")).alias("output_tokens"),
        FunctionExpression("avg", ColumnExpression("duration_ms")).alias("avg_duration_ms"),
    ],
    "model",
).sort(model)
