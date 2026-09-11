runs = (
    read("corpus_spans")
    .filter(col("kind") == lit("llm"))
    .aggregate([col("run_id")], [f.count(col("span_id")).alias("run_llm_spans")])
    .select(col("run_id").alias("run"), col("run_llm_spans"))
)
(
    read("corpus_spans")
    .filter((col("status") == lit("ok")) & (col("kind") == lit("llm")))
    .join(runs, left_on="run_id", right_on="run", how="inner")
    .select(
        col("span_id"),
        col("run_id"),
        f.split_part(col("start"), lit("T"), lit(1)).alias("day"),
        col("model"),
        (col("input_tokens") + col("output_tokens")).alias("total_tokens"),
        col("run_llm_spans"),
        col("duration_ms"),
    )
    .sort(col("span_id").sort(ascending=True))
    .limit(100)
)
