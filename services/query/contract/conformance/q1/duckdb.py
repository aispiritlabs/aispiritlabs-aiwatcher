read("corpus_spans").filter(
    (ColumnExpression("status") == ConstantExpression("error"))
    & (ColumnExpression("duration_ms") > ConstantExpression(1000))
).aggregate([FunctionExpression("count", ColumnExpression("span_id")).alias("spans")])
