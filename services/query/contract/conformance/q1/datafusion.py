read("corpus_spans").filter(
    (col("status") == lit("error")) & (col("duration_ms") > lit(1000))
).aggregate([], [f.count(col("span_id")).alias("spans")])
