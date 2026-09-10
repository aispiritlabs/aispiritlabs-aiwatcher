//! The curation benchmark's q2 in a Rust process, on the `datafusion` crate.
//!
//! The same question `polars_bench.py`, the `flow_vs_polars` notebook and
//! `../rust` ask — the LLM spans per model, their token sums and mean latency —
//! as the SQL `engines_5gb.py` hands DataFusion's Python binding, over a CSV
//! read with the declared schema or over the Parquet copy.
//!
//! ```text
//! datafusion-bench csv <file.csv>
//! datafusion-bench parquet <file.parquet>
//! ```
//!
//! The answer goes to stdout as CSV, for a harness to check against the other
//! engines; the time the query took goes to stderr as one JSON line.

use std::io::stdout;
use std::time::Instant;

use datafusion::arrow::csv::WriterBuilder;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::error::Result;
use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, SessionContext};

// The allocator DataFusion's own benchmarks use, and the one ../rust links, so
// the two Rust engines are compared on the same footing.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const SQL: &str = "SELECT model, count(*) AS spans, sum(input_tokens) AS input_tokens, \
                   sum(output_tokens) AS output_tokens, avg(duration_ms) AS avg_duration_ms \
                   FROM spans WHERE kind = 'llm' GROUP BY model ORDER BY model";

fn schema() -> Schema {
    let text = |name: &str| Field::new(name, DataType::Utf8, true);
    let number = |name: &str| Field::new(name, DataType::Int64, true);
    Schema::new(vec![
        text("run_id"),
        text("trace_id"),
        text("span_id"),
        text("parent_span_id"),
        text("name"),
        text("kind"),
        text("start"),
        text("end"),
        number("duration_ms"),
        text("operation"),
        text("agent_id"),
        text("model"),
        text("tool"),
        text("step_type"),
        text("status"),
        number("input_tokens"),
        number("output_tokens"),
        text("error"),
    ])
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(format), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: datafusion-bench csv|parquet <path>");
        std::process::exit(2);
    };

    let started = Instant::now();
    let ctx = SessionContext::new();
    let schema = schema();
    match format.as_str() {
        "csv" => {
            ctx.register_csv("spans", path, CsvReadOptions::new().has_header(true).schema(&schema))
                .await?
        }
        _ => ctx.register_parquet("spans", path, ParquetReadOptions::default()).await?,
    }
    let batches = ctx.sql(SQL).await?.collect().await?;
    let seconds = started.elapsed().as_secs_f64();

    let mut writer = WriterBuilder::new().with_header(true).build(stdout());
    for batch in &batches {
        writer.write(batch)?;
    }
    eprintln!(
        "{{\"engine\":\"datafusion-rust\",\"seconds\":{seconds:.3},\"partitions\":{}}}",
        ctx.state().config().target_partitions()
    );
    Ok(())
}
