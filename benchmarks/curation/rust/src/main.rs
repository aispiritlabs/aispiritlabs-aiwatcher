//! The curation benchmark's q2 in a Rust process, on the `polars` crate.
//!
//! The same question `polars_bench.py` and the `flow_vs_polars` notebook ask —
//! the LLM spans per model, with their token sums and mean latency — typed with
//! the same schema and collected on the same streaming engine, so what differs
//! between the three is the process around the query and nothing in it.
//!
//! ```text
//! polars-bench <csv file or glob>
//! ```
//!
//! The answer goes to stdout as CSV, so a harness can check it against the
//! other engines; the time the query took goes to stderr as one JSON line.

use std::io::stdout;
use std::time::Instant;

use polars::prelude::*;

// The allocator the Python wheel uses on macOS. With the system allocator the
// same query ran ten times slower, which would measure malloc rather than Polars.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn schema() -> SchemaRef {
    let strings = |name: &str| Field::new(name.into(), DataType::String);
    let integers = |name: &str| Field::new(name.into(), DataType::Int64);
    Arc::new(Schema::from_iter([
        strings("run_id"),
        strings("trace_id"),
        strings("span_id"),
        strings("parent_span_id"),
        strings("name"),
        strings("kind"),
        strings("start"),
        strings("end"),
        integers("duration_ms"),
        strings("operation"),
        strings("agent_id"),
        strings("model"),
        strings("tool"),
        strings("step_type"),
        strings("status"),
        integers("input_tokens"),
        integers("output_tokens"),
        strings("error"),
    ]))
}

fn main() -> PolarsResult<()> {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: polars-bench <csv file or glob>");
        std::process::exit(2);
    };

    let started = Instant::now();
    let mut per_model = LazyCsvReader::new(PlRefPath::from(path.as_str()))
        .with_has_header(true)
        .with_schema(Some(schema()))
        .finish()?
        .filter(col("kind").eq(lit("llm")))
        .group_by([col("model")])
        .agg([
            len().alias("spans"),
            col("input_tokens").sum(),
            col("output_tokens").sum(),
            col("duration_ms").mean().alias("avg_duration_ms"),
        ])
        .sort(["model"], SortMultipleOptions::default())
        .collect_with_engine(Engine::Streaming)?
        .unwrap_single();
    let seconds = started.elapsed().as_secs_f64();

    CsvWriter::new(stdout()).finish(&mut per_model)?;
    eprintln!("{{\"engine\":\"rust\",\"seconds\":{seconds:.3}}}");
    Ok(())
}
