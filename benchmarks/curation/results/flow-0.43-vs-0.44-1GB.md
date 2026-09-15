# Flow 0.43.0 vs 0.44.0 — measured at 1 GB

Measured on 2026-09-14 over the identical 5,643,870-row corpus (7 parts, seed 20260910): 1,073,741,872 bytes CSV and 147,662,465 bytes Parquet. PHP 8.5.10, tracing JIT, 128 MiB JIT buffer, 1,000-row batches, warm page cache; one PHP process per query. The 0.43.0 dependencies and original benchmark script were installed separately under `.data/flow-0.43.0`; the project remains on 0.44.0.

These are single measured runs from sequential benchmark sessions, not estimates scaled from 100 MB. Flow 0.43.0 uses its original q3, where all-null sums are zero. The migrated q3 explicitly coalesces SQL null sums to zero; its normalization cost is included. All eight Flow answers agree both across versions and with Polars, including every q3/q4 output row.

| Query | CSV 0.43 | CSV 0.44 | Speedup | Parquet 0.43 | Parquet 0.44 | Speedup |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 81.376 s | 46.873 s | 1.74× | 17.245 s | 9.636 s | 1.79× |
| q2 | 106.042 s | 62.652 s | 1.69× | 64.433 s | 42.205 s | 1.53× |
| q3 | 134.817 s | 83.120 s | 1.62× | 78.066 s | 59.706 s | 1.31× |
| q4 | 207.293 s | 113.758 s | 1.82× | 156.067 s | 95.990 s | 1.63× |

## Peak process RSS

| Query | CSV 0.43 | CSV 0.44 | Parquet 0.43 | Parquet 0.44 |
|---|---:|---:|---:|---:|
| q1 | 52.5 MiB | 42.4 MiB | 87.5 MiB | 84.7 MiB |
| q2 | 59.2 MiB | 49.0 MiB | 131.8 MiB | 128.9 MiB |
| q3 | 74.7 MiB | 65.3 MiB | 133.2 MiB | 129.7 MiB |
| q4 | 68.8 MiB | 62.6 MiB | 174.5 MiB | 168.8 MiB |

## Raw data and reproduction

[0.43 CSV](1GB-flow-0.43.0-csv.json) · [0.43 Parquet](1GB-flow-0.43.0-parquet.json) · [0.44 CSV](1GB-csv.json) · [0.44 Parquet](1GB-parquet.json)

Baseline source commit: `595844a55c1e8fc45b7c1de08b40581ab993784e` (the original benchmark script and Composer lock). The baseline harness and Polars reference were copied from the current benchmark directory.

From `benchmarks/curation`, after installing the baseline Composer project:

```bash
.venv/bin/python .data/flow-0.43.0/bench.py run --size 1GB --data-dir .data --engines polars flow --format csv parquet
```

q1’s CSV Polars reference was rerun after adding its missing `corpus.py` import to the isolated directory; all final stored runs are successful. Flow timings were not replaced by estimates.
