read("corpus_spans").filter(col("kind") == lit("llm")).aggregate(
    [col("model")],
    [
        f.count(col("span_id")).alias("spans"),
        f.sum(col("input_tokens")).alias("input_tokens"),
        f.sum(col("output_tokens")).alias("output_tokens"),
        f.avg(col("duration_ms")).alias("avg_duration_ms"),
    ],
).sort(col("model").sort(ascending=True))
