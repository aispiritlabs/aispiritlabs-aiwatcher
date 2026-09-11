---
id: AW-3
step: tests
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-3, step/tests, branch/main, status/doing]
---

`#spec/AW-3` · `#step/tests` · `#branch/main` · repo `aiwatcher`

# ④ Tests — AW-3

Verified on `main`, where AW-3's work sits uncommitted, on 2026-09-11. Python tests are
under `services/query/`; Rust tests are `crate::module::tests::name`; panel tests are
`apps/panel/src/…`. A scenario proven by a measurement or an end-to-end run rather than
an automated test says so, and names the job-note log line that holds the numbers.

## Verification matrix

### ADDED — A deployment chooses one query engine
| Spec scenario | Test | Result |
|---|---|---|
| nothing set is today's deployment | `aiwatcher-server config::tests::the_defaults_are_runnable_without_any_environment`; `execution::query::tests::a_deployment_holds_the_executor_for_its_engine_and_no_other` (an address and no engine: the Flow executor, `flow_php` only) | ✅ |
| a DataFusion deployment claims only DataFusion work | `execution::query::tests::a_deployment_holds_the_executor_for_its_engine_and_no_other` (DataFusion → `[datafusion]`, DuckDB → `[duckdb]`) | ✅ |
| an engine that is not offered | `config::tests::an_engine_that_is_not_offered_is_refused_naming_the_variable_and_the_engines`; `aiwatcher-datasets engine::tests::an_engine_that_is_not_offered_is_refused_naming_the_ones_that_are` | ✅ |

### ADDED — One query contract, proved against every engine
| Spec scenario | Test | Result |
|---|---|---|
| the panel learns which engine it is talking to | `datafusion/tests/test_datafusion.py::test_healthz_names_datafusion_python`; `duckdb/tests/test_duckdb.py::test_healthz_names_duckdb_python`; `contract/tests/test_service.py::test_healthz_names_the_engine_and_its_language`; `shared/lib/query.test.ts › reads which engine a deployment runs, and a Flow build older than the field as Flow` | ✅ |
| the same question, the same rows | `just query-conformance` against each served engine: Flow, DataFusion and DuckDB each answered `datasets same` and q1–q4 `same`. q3 and q4 are bounded to 100 rows — the narrowing the job note recorded as a decision | ✅ |
| an answer over the cap is reported, not cut silently | `contract/tests/test_evaluate.py::test_an_answer_over_the_cap_is_reported_not_cut_silently`; `test_an_answer_stops_at_the_row_ceiling_and_says_so` in both engines' tests | ✅ |

### ADDED — A DataFusion query is ordinary DataFusion Python
| Spec scenario | Test | Result |
|---|---|---|
| the Query tab's first question, in DataFusion | `test_datafusion.py::test_the_query_tabs_first_question_answers_one_row_per_model` | ✅ |
| a transform reads what came before it | `test_datafusion.py::test_a_transform_reads_the_rows_the_chain_has_so_far` | ✅ |
| a result that is not a frame | `test_datafusion.py::test_a_result_that_is_not_a_frame_is_refused_naming_what_it_was` | ✅ |
| a syntax error is located | `test_datafusion.py::test_a_syntax_error_is_located_where_pythons_parser_put_it`; `contract/tests/test_evaluate.py::test_a_syntax_error_carries_pythons_line_and_column` | ✅ |
| a corpus larger than the process stays small (SHOULD, < 512 MiB) | Measured, not automated (log 3.7, `benchmarks/curation/README.md`): q2 over the 5 GB corpus through the service — the query child at 129 MiB over CSV and 228–234 MiB over Parquet, the service at 74 MiB | ✅ |

### ADDED — A DuckDB query is ordinary DuckDB Python
| Spec scenario | Test | Result |
|---|---|---|
| the first question, in DuckDB Python | `test_duckdb.py::test_the_first_question_answers_one_row_per_model` (the spec's text verbatim); conformance q2 on DuckDB `same` | ✅ |
| a transform reads what came before it, in DuckDB | `test_duckdb.py::test_a_transform_reads_the_rows_the_chain_has_so_far` (run under strict) | ✅ |
| an expression is not a relation | `test_duckdb.py::test_an_expression_is_not_a_relation_and_is_refused_naming_it` | ✅ |
| a sum wider than 64 bits comes back as an integer | `test_duckdb.py::test_a_sum_of_a_bigint_comes_back_as_an_integer`; `test_a_sum_wider_than_64_bits_is_refused_naming_its_column` | ✅ |
| DuckDB over Parquet stays the leanest (SHOULD, < 512 MiB) | Measured, not automated (log 4.5, `benchmarks/curation/README.md`): the query child at 123–126 MiB over Parquet, and 259–472 MiB over CSV, across two runs of each | ✅ |

### ADDED — Running typed Python has a stated boundary
| Spec scenario | Test | Result |
|---|---|---|
| strict mode refuses a file read by name | `test_duckdb.py::test_strict_refuses_a_file_read_naming_read_csv_and_read` | ✅ |
| strict mode refuses a function | `test_datafusion.py::test_strict_refuses_a_function_by_naming_udf` | ✅ |
| strict mode refuses SQL in DataFusion | `test_datafusion.py::test_strict_refuses_sql_saying_a_dataset_is_reached_through_read`; `test_strict_refuses_everything_a_session_offers` (`register_csv`) | ✅ |
| strict mode refuses a SQL string in DuckDB | `test_duckdb.py::test_strict_refuses_a_sql_string_in_a_filter_naming_the_string`; `test_strict_refuses_sql_by_call_saying_how_a_filter_is_written` (`SQLExpression`, `duckdb.sql`); `test_strict_refuses_text_however_it_reaches_a_relation_method` (9 routes) | ✅ |
| strict mode admits the benchmark | `test_strict_admits_and_runs_every_conformance_question[q1–q4]`, in both engines' tests: all eight admitted and run | ✅ |
| open mode stops a query at its ceiling | `contract/tests/test_service.py::test_open_mode_stops_a_query_at_its_ceiling_while_answering_others`; in each image under the chart's constraints, a spin stopped at its 10 s ceiling with healthz answering (log 3.6, 4.4) | ✅ |
| open mode holds no credentials | `test_service.py::test_open_mode_holds_no_credentials`; `test_the_service_process_keeps_no_credential_after_starting`; in the DuckDB image, a child saw no `AIWATCHER_*` with one set at start (log 4.4) | ✅ |

### ADDED — The Python engines' catalog is Flow's catalog
| Spec scenario | Test | Result |
|---|---|---|
| a window reaches the API | `contract/tests/test_api.py::test_a_window_reaches_every_page_it_requests`; `test_reading.py::test_a_period_replaces_the_requests_window` | ✅ |
| a corpus read is never remembered | `contract/tests/test_evaluate.py::test_a_corpus_read_is_never_remembered`; conformance `datasets same` on every engine | ✅ |

### ADDED — A managed step on the deployed engine
| Spec scenario | Test | Result |
|---|---|---|
| the benchmark pipeline, on DataFusion | `aiwatcher-execution compile::tests::a_datafusion_chain_compiles_to_one_datafusion_step_whose_script_binds_df`; end to end on the server (log 3.8): the DataFusion step 0.85 s, all eight models agreeing, the waterfall's fold drawing it. DuckDB the same way (log 4.5): `a_duckdb_chain_compiles_to_one_duckdb_step_whose_script_binds_df`, the step 0.89 s | ✅ |
| a Flow pipeline on a DataFusion deployment | `compile::tests::a_chain_written_for_another_engine_is_refused_naming_the_block_and_both_engines`; `a_datafusion_chain_on_a_duckdb_deployment_is_refused_naming_both_engines`; end to end (log 3.8 and 4.5): 422 `plan_refused` naming the block and both engines, and no run of it in the fold | ✅ |

### ADDED — Content names the engine it was written for
| Spec scenario | Test | Result |
|---|---|---|
| a pipeline saved before the field | `aiwatcher-datasets pipeline::tests::a_transform_saved_before_the_engine_field_is_flow_and_serialises_as_it_did`; `lib::tests::a_recipe_saved_before_the_engine_field_keeps_its_revision`; `a_version_published_before_the_engine_field_keeps_its_id`. The `plan_id` half holds by construction rather than by a pinned digest: the Flow binding's spec before AW-3 (the parent of `43453ed`) had the same three fields, serde attributes and `flow_php` tag as today's `QueryStepSpec` | ✅ |
| the seed on any deployment | `data-curation/lib/examples.test.ts › ships a DataFusion variant…` and `› ships a DuckDB variant of the seed pipelines, each naming its engine`; `python3 examples/build_seed.py --check`; the server's seed import took all 16 pipelines (log 4.5) | ✅ |

### ADDED — The panel follows the deployed engine
| Spec scenario | Test | Result |
|---|---|---|
| Build mode on DataFusion | `observability/lib/query-builder.test.ts › compile for DataFusion › writes model and the count as DataFusion Python`; `datafusion/tests/test_panel_shapes.py::test_build_mode_model_and_count_answers_one_row_per_model`; in the browser (log 3.4). DuckDB the same: `› compile for DuckDB`, `duckdb/tests/test_duckdb_panel_shapes.py`, and in the browser (log 4.5) | ✅ |
| a Flow recipe opened on DataFusion | `data-curation/lib/pipeline.test.ts › says which engine it needs and which one this deployment runs`; in the browser, read-only with the sentence (log 3.4) | ✅ |

### MODIFIED — The query engine's address
| Spec scenario | Test | Result |
|---|---|---|
| an existing deployment upgrades | `config::tests::an_installation_that_set_only_the_flow_address_upgrades_unchanged` | ✅ |
| two addresses for one engine | `config::tests::two_addresses_for_one_engine_are_refused_naming_both` | ✅ |

### MODIFIED — The query limits are the engine's, not Flow's
| Spec scenario | Test | Result |
|---|---|---|
| a long corpus query on any engine | `compile::tests::a_configured_query_timeout_reaches_the_step_whichever_engine_runs_it` (added here); `a_configured_flow_timeout_reaches_the_flow_step_and_an_unset_one_stays_five_minutes`; the chart's pairing, rendered (step 7200 → engine ceiling 7260); on the server, Flow's 95.2 s step completed in one attempt under a 7200 s step and a 7500 s ceiling (log 3.8). Red as written until this phase — see *Issues* | ✅ after the fix |

### MODIFIED — The engines live side by side
| Spec scenario | Test | Result |
|---|---|---|
| the check an engine change has to pass | `.github/workflows/ci.yml`: the `query` job's matrix is `[flow, datafusion, duckdb]`, running each engine's checks and then its conformance, beside `query-contract`. CI was not run from here; the same commands were run locally and are under *Commands run* | ✅ |

### MODIFIED — Deployment values name the engine
| Spec scenario | Test | Result |
|---|---|---|
| switching an installed release | `helm template` with `query.engine` set: the engine's image is `aiwatcher-query-datafusion` or `aiwatcher-query-duckdb`, and with managed execution on the server's `AIWATCHER_QUERY_ENGINE` is `"datafusion"` or `"duckdb"`. With it off the chart renders no engine variable, because nothing in the server reads it then; `just chart-check` validates both environments | ✅ |

### MODIFIED — A query surface is admitted, or runs where code runs
| Spec scenario | Test | Result |
|---|---|---|
| the decision is where the next reader looks | `docs/ADR/ADR_0008_FLOW_QUERY_SURFACE.md`: its status line and a dated amendment point at ADR_0028 (3 references). `ADR_0028_QUERY_ENGINES.md` states what `open` costs and has a *What would make this wrong* section | ✅ |

### The proposal's success criteria
| Criterion | Evidence | Result |
|---|---|---|
| with no new setting, everything behaves as before | the whole existing suite: `cargo test` 1141 passed, `just flow-check` 169, panel 176; stored digests unchanged (the three tests above) | ✅ |
| one conformance suite passes against all three engines | `just query-conformance` × 3, all `same` | ✅ |
| DataFusion and DuckDB each run their benchmark variant as a managed pipeline | end to end, log 3.8 and 4.5 | ✅ |
| neither Python engine passes 1 GiB over 5 GB, CSV or Parquet | measured, log 3.7 and 4.5: largest child 472 MiB | ✅ |
| switching a Helm release between engines is one value | the `helm template` renders above | ✅ |
| strict refuses a file read, an import, a function, SQL, naming what it tried | the strict rows above; `contract/tests/test_admission.py` for the import | ✅ |

## Commands run
```
just check                        (last run, after trimming payload.rs's module doc)
  PASS  all 19 — curation seed · portable examples · cargo fmt · cargo clippy · cargo
        test (41 suites, 1102 passed, 0 failed) · dockerfile lists every crate · openapi
        contract is current · panel build + typecheck · panel tests · typescript sdk
        typecheck · python sdk · agentic engine · k8s manifests · helm chart · Tiltfile
        · comments · typos · taplo fmt · cargo deny
  exit 0
                                  41 suites, not 44: a concurrent AW-4 session deleted
                                  crates/aiwatcher-pipeline and
                                  server/tests/engine_end_to_end.rs meanwhile
just check                        (the run before it) — cargo fmt, helm chart and typos
                                  red: the first two on AW-4 files edited during the
                                  run (server/src/config.rs, templates/_helpers.tpl),
                                  clean on a re-run; typos on a plural of SHOULD in this note
just check                        (second run, after the fixes below)
  PASS  curation seed · portable examples · cargo fmt · cargo clippy · cargo test
        (44 suites, 1141 passed, 0 failed) · dockerfile lists every crate · openapi
        contract is current · panel build + typecheck · panel tests · typescript sdk
        typecheck · python sdk · agentic engine · k8s manifests · helm chart · Tiltfile
        · typos · taplo fmt · cargo deny
  FAIL  comments — crates/aiwatcher-conversations/src/payload.rs:1, a 45-line //!
        block, unchanged since HEAD and not AW-3's
  exit 1
just check                        (first run) — the same, plus FAIL curation seed
                                  (stale, see Issues), FAIL taplo fmt
                                  (sdk/agentic/pyproject.toml) and a second comment
                                  finding (server/conversations.rs:188, a §43.11
                                  reference) — both outside AW-3, and both gone by
                                  the second run, the files matching HEAD
just flow-check                   — OK (169 tests, 398 assertions)
just query-contract-check         — ruff format: 38 files already formatted; ruff:
                                    all checks passed; mypy --strict: no issues in
                                    36 files; pytest: 186 passed (red on the first
                                    run, see Issues)
just ml-pipeline-check            — ruff and mypy clean; 76 passed
AIWATCHER_QUERY_ENGINE=flow just query-conformance
                                  — datasets same; q1 same (1 row); q2 same (8);
                                    q3 same (100); q4 same (100)
AIWATCHER_QUERY_ENGINE=datafusion just query-conformance — the same, all same
AIWATCHER_QUERY_ENGINE=duckdb just query-conformance     — the same, all same
cargo test -p aiwatcher-execution --lib a_configured_
                                  — 2 passed (the new any-engine timeout test and
                                    Flow's)
cargo clippy -p aiwatcher-execution --all-targets -- -Dwarnings — clean
just chart-check                  — helm lint 0 failed; default 17 and planner 25
                                    resources, all valid under kubeconform
helm template … (8 cases)         — execution off: no engine ceiling; on: step 300 →
                                    ceiling 360; step 7200 → 7260;
                                    query.timeoutSeconds=900 → 900; Flow and DuckDB
                                    alike; a step of 0 refused by the schema
docker compose -f deploy/docker-compose.yml config -q — valid; the pair 300 / 360
npx vitest run (apps/panel)       — 176 passed
typos (every file this phase touched) — clean
```

## Issues found & resolutions
- **The spec's timeout MUST was not kept by anything.** *The service's ceiling MUST stay
  above the step's*: the two live in two processes, the chart set neither, and the
  defaults (engine 30 s, step 300 s) are the other way round. So with managed execution
  on, any query step over 30 s failed once, with a 422 naming
  `AIWATCHER_QUERY_TIMEOUT_SECONDS`, and *a long corpus query on any engine* failed as
  written unless both values were raised by hand, as `bench-curation-serve` does.
  → **Decided by the user, 2026-09-11: the chart pairs them.** `execution.queryStepTimeoutSeconds`
  (default 300) is rendered as the server's `AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS`, and
  the engine's `AIWATCHER_QUERY_TIMEOUT_SECONDS` is derived a minute above it whenever
  managed execution is on, unless `query.timeoutSeconds` is set. With execution off the
  engine keeps its own 30 s. `just query-serve` exports the same derivation,
  `run-execution` passes the step's value, and Compose pairs 300 with 360; the schema
  takes both keys; `INSTALL.md` and the query README say so. Verified by the renders
  above and `just chart-check`.
- **The step-timeout scenario was covered for Flow only.** → Added
  `compile::tests::a_configured_query_timeout_reaches_the_step_whichever_engine_runs_it`.
  The code was already engine-independent (the timeout is set on the query step after the
  engine is chosen), so the test passed first time; it now fails if an engine's step ever
  stops taking the value.
- **`just check`'s curation seed was stale.** A seed notebook block pins the digest of its
  notebook, and phase 4 edited `flow_vs_polars.py`'s docstring after the seed was built.
  → Rebuilt with `python3 examples/build_seed.py`; the diff moved only that notebook's
  pinned revision. `--check` passes.
- **`just query-contract-check` was red on `services/query/README.md`.** This ruff formats
  Python code blocks inside Markdown too, and one DuckDB example line was too long.
  → Reformatted as ruff asked.
- **`just check` stays red on the comment lint, outside AW-3.** One file:
  `crates/aiwatcher-conversations/src/payload.rs`, a 45-line module doc, unchanged since
  HEAD and flagged as other work on this branch since the 1.9 gate. The first run also
  flagged `crates/aiwatcher-server/src/conversations.rs:188`, and taplo flagged
  `sdk/agentic/pyproject.toml`; both files match HEAD now and passed the second run.
  → **Resolved 2026-09-11, the user's call:** the module doc trimmed to 25 lines, the
  same rules kept (the head written before the content, the content erased before the
  head, erasure by subject not reaching a payload) and the argument around them
  dropped. The last `just check` run is green.
- **Spec narrowing, carried from the job note.** Conformance q3 and q4 end in a limit of
  100 so every engine stays inside the 1 000-row answer. It is narrower than *the same
  question, the same rows*, and was recorded as a decision before it was built.
- **Evidence that is not an automated test.** The two memory SHOULD scenarios are measurements,
  and the managed-pipeline and panel scenarios were also proven end to end and in the
  browser; their numbers are in the job note's log (3.4, 3.6–3.8, 4.4, 4.5) and the
  benchmark README. Each has automated tests for its logic, listed above, and none of
  the measurements is re-run by CI.

## Quality checks
- [x] The project's verification command passes — `just check` 19 of 19 on the last
      run, and the Flow, query-contract and ml-pipeline gates above
- [ ] Every spec scenario has a test that fails without the change — every scenario has
      automated tests for its logic; the two memory SHOULD scenarios rest on measurement alone,
      and *the check an engine change has to pass* on CI's configuration, which was not
      run from here
- [x] No backward-incompatible migration in a single release — no PostgreSQL migration;
      every stored revision, version and Flow `plan_id` keeps its digest; the Flow-era
      names are read as aliases for one release, and the job note's last 5.2 log line
      lists them for `/spec-deploy`

## Log
- 2026-09-11 11:20 — verification run on `main`: every AW-3 scenario green (the timeout scenario after the chart pairing the user chose); `just check` red on the comment lint, in two files outside AW-3 — blocked on that call
- 2026-09-11 11:21 — correction: the second just check run flags one file outside AW-3, not two — crates/aiwatcher-conversations/src/payload.rs; server/conversations.rs and sdk/agentic/pyproject.toml match HEAD and passed it
- 2026-09-11 11:40 — payload.rs's module doc trimmed to 25 lines, the user's call; just check green, 19 of 19, cargo test 41 suites 1102 passed — unblocked
