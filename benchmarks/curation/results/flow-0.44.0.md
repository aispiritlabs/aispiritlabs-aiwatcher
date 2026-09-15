# Flow PHP 0.44.0 — migration and benchmark

Measured on 2026-09-14. Both Composer projects resolve all Flow packages to 0.44.0, with PHP 8.3.0 retained as the dependency resolution floor. Execution and tests ran on PHP 8.5.10.

## 100 MB CSV versus the saved 0.43.0 baseline

The regenerated corpus has the same seed, 551,210 rows, 2 parts and 104,859,210 CSV bytes as the saved baseline. Both runs use tracing JIT and 1,000-row batches. These are individual measurements from different sessions, not repeated trials in an isolated environment.

| Query | 0.43.0 | 0.44.0 | Speedup | 0.44 peak RSS |
|---|---:|---:|---:|---:|
| q1 | 6.885 s | 4.075 s | 1.69× | 42.4 MiB |
| q2 | 10.040 s | 6.236 s | 1.61× | 48.9 MiB |
| q3 | 12.255 s | 8.213 s | 1.49× | 52.5 MiB |
| q4 | 19.301 s | 11.438 s | 1.69× | 60.4 MiB |

The 100 MB CSV and Parquet answers agree with both Polars configurations for all four queries, including every row of q3/q4 CSV output. q3 explicitly converts all-null token sums to zero: Flow 0.44 now returns SQL null, while Polars returns zero. That normalization is included in the measured query time.

[Full 100 MB matrix](100MB.md) · [Historical 0.43.0 raw results](100MB-flow-0.43.0-csv.json)

## 1 GB: 5,643,870 rows

CSV input is 1,073,741,872 bytes; the same rows occupy 147,662,465 bytes of Parquet, in 7 parts. All q1–q4 answers agree with Polars on both formats.

| Query | Flow CSV | Peak RSS | Flow Parquet | Peak RSS | Polars CSV | Polars Parquet |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 46.873 s | 42.4 MiB | 9.636 s | 84.7 MiB | 0.667 s | 0.019 s |
| q2 | 62.652 s | 49.0 MiB | 42.205 s | 128.9 MiB | 0.432 s | 0.023 s |
| q3 | 83.120 s | 65.3 MiB | 59.706 s | 129.7 MiB | 1.571 s | 0.075 s |
| q4 | 113.758 s | 62.6 MiB | 95.990 s | 168.8 MiB | 1.210 s | 0.176 s |

[Full 1 GB report](1GB.md) · [CSV raw results](1GB-csv.json) · [Parquet raw results](1GB-parquet.json)

Flow still uses one PHP process. Its improvement over 0.43 does not close the gap to Polars; the table compares the engines over the same files in this run. Memory is the per-process peak RSS from `/usr/bin/time`, not the PHP allocator counter.

## Measured 1 GB comparison against 0.43.0

[Full version comparison, timings, memory and raw results](flow-0.43-vs-0.44-1GB.md). Both formats and all four queries were measured on 0.43.0 using the identical corpus; every answer agrees across versions and with Polars.

## Migration checks

- `AIWATCHER_QUERY_ENGINE=flow just query-check`: formatter, lint at warning threshold, **180 tests / 442 assertions passed**. Lint reports only help-level suggestions.
- `AIWATCHER_QUERY_ENGINE=flow just query-conformance`: catalog and q1–q4 agree with the checked-in service contract.
- `composer validate --strict`: passed in the service and benchmark projects.
- `php benchmarks/curation/profile_flow.php services/query/contract/conformance/.data/1MB`: all profiling stages completed, including named callable expressions.

The migration adapts array arguments for grouping/sorting/global aggregation, replaces Entry access with typed Rows, declares custom expression types and children, and preserves immutable window binding. HTTP JSON pages get a structure before native projection, without fetching another page or collecting the dataset. Tests cover missing fields, dynamic nested JSON, metadata preservation, serialization, training isolation and lazy inference.

Flow’s fixed `equals`/`notEquals` comparisons are available again. Arbitrary callable execution remains blocked on both standalone and fluent query surfaces despite the new expression-based `call()` signature.

## Reproduce

```bash
just bench-curation-generate 100MB
just bench-curation 100MB --engines polars polars-1t flow --format csv parquet
just bench-curation-generate 1GB
just bench-curation 1GB --engines polars polars-1t flow --format csv parquet
```
