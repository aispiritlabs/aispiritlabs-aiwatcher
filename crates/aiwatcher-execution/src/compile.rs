//! From an authored curation chain to a runnable plan.
//!
//! ADR_0024's four block kinds compile to three runtimes, and the interesting
//! part is that they do not compile one-to-one. **Flow executes one pipeline**,
//! so a source and every transform after it fold into a single
//! [`RuntimeBinding::FlowPhp`] step whose `blocks` lists the authored ids the
//! panel lights up together. That fold is also what makes "no transform after a
//! notebook" a compiler refusal rather than a runtime surprise: there is
//! nowhere for a second Flow step to read from.
//!
//! The shape rules are **not** re-implemented here.
//! [`aiwatcher_datasets::order_of`] owns them and reports every problem at
//! once; this compiler calls it and adds only what compilation itself can see —
//! a notebook with no pinned revision, a view with no dataset. Two validators
//! would drift, and the day they do is the day somebody trusts the wrong one.

use std::collections::BTreeMap;

use aiwatcher_core::ArtifactKind;
use aiwatcher_datasets::{BlockSpec, CurationPipeline, PipelineBlock, order_of};

use crate::error::CompileError;
use crate::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, ExecutionPlan, FlowSourceRef, FlowStepSpec,
    InputBinding, MarimoStepSpec, OutputDeclaration, PlanEdge, PlanStep, PublishDatasetSpec,
    ResolvedWindow, RetryPolicy, RuntimeBinding,
};

/// A Flow query is a request/response to a service that holds nothing.
const FLOW_TIMEOUT_SECONDS: u64 = 300;
/// A notebook is a subprocess somebody is halfway through writing.
const NOTEBOOK_TIMEOUT_SECONDS: u64 = 900;
/// Publishing writes one content-addressed version in this process.
const PUBLISH_TIMEOUT_SECONDS: u64 = 120;

/// What a compilation may resolve that the definition left open.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompileOptions {
    /// A relative time window resolved to exact bounds, once, here.
    ///
    /// Resolving it at compile time rather than at dispatch is what makes a
    /// retry three hours later read the same rows — and what makes the step
    /// cacheable at all.
    pub window: Option<ResolvedWindow>,
}

/// Compile a saved curation pipeline into the plan that runs it.
///
/// # Errors
///
/// [`CompileError::Refused`] carrying every problem: the chain's, from
/// [`order_of`], plus the ones only compilation sees.
pub fn compile_curation(
    pipeline: &CurationPipeline,
    options: CompileOptions,
) -> Result<ExecutionPlan, CompileError> {
    let chain = order_of(&pipeline.blocks, &pipeline.edges).map_err(CompileError::Refused)?;

    let mut problems = Vec::new();
    let mut steps: Vec<PlanStep> = Vec::new();
    let mut edges: Vec<PlanEdge> = Vec::new();
    let mut previous: Option<String> = None;

    // The source and every transform before the first notebook are one Flow
    // query. `order_of` has already refused a transform after a notebook, so
    // "the transforms" is a prefix rather than a search.
    let flow_blocks: Vec<&PipelineBlock> = chain
        .iter()
        .copied()
        .take_while(|block| {
            matches!(
                block.spec,
                BlockSpec::Source { .. } | BlockSpec::Transform { .. }
            )
        })
        .collect();

    if let Some(source) = flow_blocks.first() {
        let BlockSpec::Source { dataset, arguments } = &source.spec else {
            problems.push(format!(
                "the chain starts with {}, which is not a source",
                source.id
            ));
            return Err(CompileError::Refused(problems));
        };
        let step_id = step_id_of(source);
        let script = flow_script(&source.spec, &flow_blocks);
        steps.push(PlanStep {
            id: step_id.clone(),
            runtime: RuntimeBinding::FlowPhp(FlowStepSpec {
                script,
                source: FlowSourceRef {
                    dataset: dataset.clone(),
                    arguments: arguments.clone(),
                    resolved_revision: None,
                    window: options.window,
                    cursor: None,
                },
                blocks: flow_blocks.iter().map(|block| block.id.clone()).collect(),
            }),
            inputs: Vec::new(),
            outputs: vec![OutputDeclaration {
                name: "rows".to_owned(),
                kind: ArtifactKind::Rows,
                schema_ref: None,
            }],
            retry: RetryPolicy::default(),
            timeout_seconds: FLOW_TIMEOUT_SECONDS,
            // A query over a resolved window is a pure function of things that
            // are addressed. Over a moving one it is not, and `cache_key`
            // refuses to produce a key rather than trusting this flag.
            cache: CachePolicy::ByContent,
        });
        previous = Some(step_id);
    }

    for block in chain.iter().skip(flow_blocks.len()) {
        let step_id = step_id_of(block);
        match &block.spec {
            BlockSpec::Source { .. } | BlockSpec::Transform { .. } => {
                // `order_of` refused this shape; reaching here means the two
                // disagree, which is worth saying rather than compiling past.
                problems.push(format!(
                    "{} is a Flow PHP block after a notebook, which has no rows to read",
                    block.id
                ));
            }
            BlockSpec::Notebook {
                notebook,
                revision,
                params,
            } => {
                let Some(revision) = revision.as_ref().filter(|r| !r.is_empty()) else {
                    problems.push(format!(
                        "{} names the notebook '{notebook}' without pinning a revision. \
                         A managed run pins the code it ran; save the pipeline with the \
                         notebook runtime reachable",
                        block.id
                    ));
                    continue;
                };
                steps.push(PlanStep {
                    id: step_id.clone(),
                    runtime: RuntimeBinding::Marimo(MarimoStepSpec {
                        notebook: notebook.clone(),
                        code_revision: revision.clone(),
                        params: params.clone(),
                        block: Some(block.id.clone()),
                    }),
                    inputs: previous
                        .iter()
                        .map(|step| InputBinding::Step {
                            step: step.clone(),
                            output: "rows".to_owned(),
                        })
                        .collect(),
                    outputs: vec![OutputDeclaration {
                        name: "rows".to_owned(),
                        kind: ArtifactKind::Rows,
                        schema_ref: None,
                    }],
                    retry: RetryPolicy::default(),
                    timeout_seconds: NOTEBOOK_TIMEOUT_SECONDS,
                    cache: CachePolicy::ByContent,
                });
            }
            BlockSpec::View { dataset } => {
                let Some(dataset) = dataset.as_ref().filter(|name| !name.is_empty()) else {
                    problems.push(format!(
                        "{} does not say which dataset it publishes to",
                        block.id
                    ));
                    continue;
                };
                steps.push(PlanStep {
                    id: step_id.clone(),
                    runtime: RuntimeBinding::PublishDataset(PublishDatasetSpec {
                        dataset: dataset.clone(),
                        // Provenance, and the authored revision rather than the
                        // plan id: it names the picture somebody saved.
                        produced_by: format!("{}@{}", pipeline.name, pipeline.revision),
                        block: Some(block.id.clone()),
                    }),
                    inputs: previous
                        .iter()
                        .map(|step| InputBinding::Step {
                            step: step.clone(),
                            output: "rows".to_owned(),
                        })
                        .collect(),
                    outputs: Vec::new(),
                    retry: RetryPolicy::default(),
                    timeout_seconds: PUBLISH_TIMEOUT_SECONDS,
                    // Publishing is idempotent by content, and a cache hit
                    // would skip writing a version that a second dataset name
                    // needs. Cheap, and never worth reusing.
                    cache: CachePolicy::Never,
                });
            }
        }
        if let Some(from) = previous.replace(step_id.clone()) {
            edges.push(PlanEdge { from, to: step_id });
        }
    }

    if steps.is_empty() && problems.is_empty() {
        problems.push("this pipeline compiles to no steps".to_owned());
    }
    if !problems.is_empty() {
        return Err(CompileError::Refused(problems));
    }

    Ok(ExecutionPlan::seal(
        DefinitionKind::CurationPipeline,
        pipeline.name.clone(),
        DefinitionRevision(pipeline.revision.clone()),
        steps,
        edges,
    ))
}

/// A plan step is named after the authored block it came from.
///
/// One name, so that an editor link, a `step.started` on the log and a canvas
/// box are the same thing to a reader. The Flow step takes the *source*
/// block's id and lists the rest, because that is the box people point at.
fn step_id_of(block: &PipelineBlock) -> String {
    block.id.clone()
}

/// The Flow PHP script a source and its transforms add up to.
///
/// This is the authoritative generator (ADR_0025): the panel may show the same
/// text, and what runs is what this produced. Ported line for line from the
/// browser's `compileFlow` so that moving it here changed no query.
#[must_use]
pub fn flow_script(source: &BlockSpec, chain: &[&PipelineBlock]) -> String {
    let steps: Vec<String> = chain
        .iter()
        .filter_map(|block| match &block.spec {
            BlockSpec::Transform { steps } => Some(steps.clone()),
            _ => None,
        })
        .collect();

    let mut lines = vec![
        "data_frame()".to_owned(),
        format!("    {}", read_call(source)),
    ];
    for line in steps.join("\n").lines() {
        let line = line.trim();
        if !line.is_empty() {
            lines.push(format!("    {line}"));
        }
    }
    lines.push("    ->write(to_output(truncate: false))".to_owned());
    lines.push("    ->run();".to_owned());
    lines.join("\n")
}

/// One `read()`, with the arguments the catalog declares for that dataset.
#[must_use]
pub fn read_call(spec: &BlockSpec) -> String {
    let BlockSpec::Source { dataset, arguments } = spec else {
        return "->read(default)".to_owned();
    };
    let mut parts = vec![dataset.clone()];
    for (name, value) in arguments {
        if value.trim().is_empty() {
            continue;
        }
        parts.push(format!("{name}: {}", literal(value)));
    }
    format!("->read({})", parts.join(", "))
}

/// A value as the query language writes it.
///
/// Numbers unquoted, because `limit: 25` is how somebody would type it;
/// anything else single-quoted, because the lexer refuses double quotes
/// outright. A value carrying a quote is escaped rather than dropped: the
/// service refuses what it cannot lex, and it says where.
fn literal(value: &str) -> String {
    let numeric = {
        let body = value.strip_prefix('-').unwrap_or(value);
        !body.is_empty()
            && body
                .split_once('.')
                .map_or(!body.is_empty(), |(whole, fraction)| {
                    !whole.is_empty() && !fraction.is_empty()
                })
            && body
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
            && body.matches('.').count() <= 1
    };
    if numeric {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The named-parameter form the panel and the query service both read.
#[must_use]
pub fn arguments_of(spec: &BlockSpec) -> BTreeMap<String, String> {
    match spec {
        BlockSpec::Source { arguments, .. } => arguments.clone(),
        _ => BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use aiwatcher_datasets::{BlockPosition, PipelineEdge};
    use time::OffsetDateTime;

    use super::*;
    use crate::plan::RuntimeKind;

    fn block(id: &str, spec: BlockSpec) -> PipelineBlock {
        PipelineBlock {
            id: id.to_owned(),
            title: String::new(),
            position: BlockPosition::default(),
            spec,
        }
    }

    fn pipeline(blocks: Vec<PipelineBlock>) -> CurationPipeline {
        let edges = blocks
            .windows(2)
            .map(|pair| PipelineEdge {
                from: pair[0].id.clone(),
                to: pair[1].id.clone(),
            })
            .collect();
        CurationPipeline {
            name: "pii".to_owned(),
            description: String::new(),
            blocks,
            edges,
            revision: "ab".repeat(32),
            saved_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn source() -> BlockSpec {
        BlockSpec::Source {
            dataset: "hub_rows".to_owned(),
            arguments: BTreeMap::from([
                ("dataset".to_owned(), "ai4privacy/pii".to_owned()),
                ("limit".to_owned(), "500".to_owned()),
            ]),
        }
    }

    #[test]
    fn a_source_and_its_transforms_compile_to_one_flow_step_that_names_all_three() {
        // Flow executes one pipeline, which is why three canvas boxes light up
        // together — and why "no transform after a notebook" is a refusal here
        // rather than a surprise at run time.
        let plan = compile_curation(
            &pipeline(vec![
                block("read", source()),
                block(
                    "clean",
                    BlockSpec::Transform {
                        steps: "->filter(ref('text')->isNotNull())".to_owned(),
                    },
                ),
                block(
                    "trim",
                    BlockSpec::Transform {
                        steps: "->limit(100)".to_owned(),
                    },
                ),
            ]),
            CompileOptions::default(),
        )
        .expect("a source and two transforms");

        assert_eq!(plan.steps.len(), 1);
        let RuntimeBinding::FlowPhp(spec) = &plan.steps[0].runtime else {
            panic!("the one step is a Flow query");
        };
        assert_eq!(spec.blocks, vec!["read", "clean", "trim"]);
        assert!(spec.script.contains("->filter(ref('text')->isNotNull())"));
        assert!(spec.script.contains("->limit(100)"));
    }

    #[test]
    fn the_generated_script_is_the_one_the_browser_used_to_write() {
        // Ported line for line so that moving compilation into Rust changed no
        // query. A difference here is a difference in what runs.
        let script = flow_script(&source(), &[]);
        assert_eq!(
            script,
            "data_frame()\n\
             \x20   ->read(hub_rows, dataset: 'ai4privacy/pii', limit: 500)\n\
             \x20   ->write(to_output(truncate: false))\n\
             \x20   ->run();"
        );
    }

    #[test]
    fn a_value_carrying_a_quote_is_escaped_rather_than_dropped() {
        assert_eq!(literal("500"), "500");
        assert_eq!(literal("-1.5"), "-1.5");
        assert_eq!(literal("1.2.3"), "'1.2.3'");
        assert_eq!(literal("o'brien"), r"'o\'brien'");
        assert_eq!(literal(""), "''");
    }

    #[test]
    fn a_notebook_nobody_pinned_is_refused_by_name_rather_than_run_unpinned() {
        // A managed run pins the code it ran. Unpinned, the pipeline cannot say
        // what produced its dataset version, which is the whole provenance
        // claim.
        let error = compile_curation(
            &pipeline(vec![
                block("read", source()),
                block(
                    "detect",
                    BlockSpec::Notebook {
                        notebook: "pii_scan".to_owned(),
                        revision: None,
                        params: BTreeMap::new(),
                    },
                ),
            ]),
            CompileOptions::default(),
        )
        .expect_err("an unpinned notebook");
        assert!(
            error.problems()[0].contains("without pinning a revision"),
            "{error}"
        );
    }

    #[test]
    fn the_whole_pii_chain_compiles_to_flow_then_notebook_then_publish() {
        let plan = compile_curation(
            &pipeline(vec![
                block("read", source()),
                block(
                    "clean",
                    BlockSpec::Transform {
                        steps: "->limit(500)".to_owned(),
                    },
                ),
                block(
                    "detect",
                    BlockSpec::Notebook {
                        notebook: "pii_scan".to_owned(),
                        revision: Some("cd".repeat(32)),
                        params: BTreeMap::from([("threshold".to_owned(), serde_json::json!(0.8))]),
                    },
                ),
                block(
                    "publish",
                    BlockSpec::View {
                        dataset: Some("pii-clean".to_owned()),
                    },
                ),
            ]),
            CompileOptions::default(),
        )
        .expect("the PII chain");

        let kinds: Vec<RuntimeKind> = plan.steps.iter().map(|step| step.runtime.kind()).collect();
        assert_eq!(
            kinds,
            vec![
                RuntimeKind::FlowPhp,
                RuntimeKind::Marimo,
                RuntimeKind::PublishDataset
            ]
        );
        assert_eq!(plan.edges.len(), 2);
        assert!(plan.is_acyclic());

        let RuntimeBinding::PublishDataset(publish) = &plan.steps[2].runtime else {
            panic!("the last step publishes");
        };
        // Provenance names the *authored* revision, not the plan id: it is what
        // somebody saved, and it is what `produced_by` has always meant.
        assert_eq!(publish.produced_by, format!("pii@{}", "ab".repeat(32)));
    }

    #[test]
    fn moving_a_block_on_the_canvas_compiles_to_the_same_plan() {
        // The authored revision digests the positions and this does not, which
        // is the whole reason `plan_id` is a second digest: a canvas tidy-up
        // must invalidate no cache and start no different run.
        let mut moved = pipeline(vec![
            block("read", source()),
            block(
                "publish",
                BlockSpec::View {
                    dataset: Some("clean".to_owned()),
                },
            ),
        ]);
        let first = compile_curation(&moved, CompileOptions::default()).expect("a plan");
        moved.blocks[0].position = BlockPosition { x: 400.0, y: 12.5 };
        moved.blocks[0].title = "read the corpus".to_owned();
        let second = compile_curation(&moved, CompileOptions::default()).expect("a plan");
        assert_eq!(first.plan_id, second.plan_id);
    }

    #[test]
    fn a_chain_that_is_not_a_chain_is_refused_with_every_reason_at_once() {
        // Not re-implemented here: `order_of` owns the shape rules and reports
        // every problem at once. Two validators would drift.
        let broken = CurationPipeline {
            edges: Vec::new(),
            ..pipeline(vec![
                block("read", source()),
                block(
                    "other",
                    BlockSpec::Source {
                        dataset: "runs".to_owned(),
                        arguments: BTreeMap::new(),
                    },
                ),
            ])
        };
        let error = compile_curation(&broken, CompileOptions::default())
            .expect_err("two disconnected heads");
        assert!(
            error
                .problems()
                .iter()
                .any(|p| p.contains("chain of their own")),
            "{error}"
        );
    }
}
