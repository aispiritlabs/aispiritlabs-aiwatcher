//! What makes two steps the same step.
//!
//! A cache key is content-derived and covers exactly the things that could
//! change the answer: the runtime and its implementation version, the resolved
//! code, the input artifacts in order, the canonical parameters and the output
//! schema. Nothing else — and in particular not where the block sat on a canvas,
//! which is why the plan has a `plan_id` separate from the authored revision.
//!
//! The rules that are not in the formula are the ones worth stating:
//!
//! * **Caching is opt-in.** [`CachePolicy::Never`] is the default everywhere,
//!   because claiming a step is a pure function of digest-addressed things is
//!   wrong often enough to be worth making out loud.
//! * **A moving window is not cacheable.** "The last hour" resolved at compile
//!   time is; unresolved, two runs an hour apart would share a key and not a
//!   question. [`cache_key`] refuses to produce one.
//! * **An agent turn and a human answer are never cacheable**, by the runtime's
//!   own `is_cacheable`. A cache hit on a decision somebody made about another
//!   run is not a saving.
//! * **Deleting the index loses nothing authoritative.** A hit records the key
//!   and the reused artifact ids in the workflow history, so the run stays
//!   explainable after the index is dropped.

use std::collections::BTreeMap;

use aiwatcher_core::ArtifactRef;
use serde::Serialize;
use serde_json::Value;

use crate::digest;
use crate::plan::{CachePolicy, PlanStep, RuntimeBinding, RuntimeKind};

/// The version of *this* implementation of a runtime.
///
/// Bumped when the same inputs would produce different bytes — a Flow script
/// generator change, a notebook harness change. Without it, fixing a compiler
/// bug would silently serve the old wrong answer out of the cache.
pub const RUNTIME_IMPLEMENTATION_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
struct Material<'a> {
    runtime: &'static str,
    implementation_version: u32,
    /// The digest of the code that runs: a notebook revision, the Flow script,
    /// a task's pinned version.
    code: String,
    /// The exact bounds a windowed source resolved to.
    ///
    /// In the key rather than only gating it: one script over two spans is two
    /// questions, and a key that covered only the script's text would answer
    /// yesterday's with today's rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<(i64, i64)>,
    /// Input artifact digests, in the order the step reads them. Order is part
    /// of the key because two inputs swapped is a different question.
    inputs: Vec<&'a str>,
    parameters: Value,
    outputs: Vec<String>,
}

/// The key for one attempt, or `None` when this step may not be cached.
///
/// `None` rather than a key nobody should use: a function that returned a key
/// for an uncacheable step would put the decision in every caller.
#[must_use]
pub fn cache_key(step: &PlanStep, inputs: &[ArtifactRef]) -> Option<String> {
    if step.cache == CachePolicy::Never || !step.runtime.kind().is_cacheable() {
        return None;
    }
    if inputs.iter().any(|artifact| !artifact.has_digest()) {
        // An input nobody can address is an input that may have changed. This
        // is the `file://` case: a path on a shared volume is accepted as a
        // pointer and never as an identity.
        return None;
    }
    let code = code_digest(&step.runtime)?;
    let material = Material {
        runtime: step.runtime.kind().as_str(),
        implementation_version: RUNTIME_IMPLEMENTATION_VERSION,
        code,
        window: resolved_window(&step.runtime),
        inputs: inputs.iter().map(|a| a.digest.as_str()).collect(),
        parameters: parameters(&step.runtime),
        outputs: step
            .outputs
            .iter()
            .map(|output| {
                format!(
                    "{}:{}:{}",
                    output.name,
                    output.kind.as_str(),
                    output.schema_ref.as_deref().unwrap_or("")
                )
            })
            .collect(),
    };
    Some(digest(crate::plan::canonical(&material).as_bytes()))
}

/// What the step runs, addressed. `None` when it is not pinned, which is what
/// makes an unresolved source or an unsaved notebook uncacheable rather than
/// cacheable-by-accident.
fn code_digest(runtime: &RuntimeBinding) -> Option<String> {
    match runtime {
        RuntimeBinding::FlowPhp(spec) => {
            // A pinned source *or* a pinned window, and the second only counts
            // because the query service can now read one: `POST /flow/query`
            // takes `window_from`/`window_to` and the API's windowed routes take
            // `as_of`, so a plan that pinned 09:00–10:00 and a retry five
            // minutes later read the same rows.
            //
            // The key being *well defined* is this function's question. Whether
            // the run that produced a result actually happened under those
            // conditions is the executor's, answered on `ActivityResult
            // ::cacheable` — an older query service that never learnt `as_of`
            // still reads a drifting window, and says so rather than being
            // assumed about.
            if spec.source.window.is_none() && spec.source.resolved_revision.is_none() {
                return None;
            }
            Some(digest(spec.script.as_bytes()))
        }
        RuntimeBinding::Marimo(spec) => {
            (!spec.code_revision.is_empty()).then(|| digest(spec.code_revision.as_bytes()))
        }
        RuntimeBinding::PythonTask(spec) => spec
            .task_ref
            .split_once('@')
            .map(|_| digest(spec.task_ref.as_bytes())),
        RuntimeBinding::PublishDataset(_)
        | RuntimeBinding::HumanInput(_)
        | RuntimeBinding::ExternalWorkflow(_) => None,
    }
}

/// The bounds a windowed source resolved to, when it has any.
fn resolved_window(runtime: &RuntimeBinding) -> Option<(i64, i64)> {
    match runtime {
        RuntimeBinding::FlowPhp(spec) => spec.source.window.map(|w| (w.from, w.to)),
        _ => None,
    }
}

fn parameters(runtime: &RuntimeBinding) -> Value {
    let map: BTreeMap<String, Value> = match runtime {
        RuntimeBinding::Marimo(spec) => spec.params.clone(),
        RuntimeBinding::PythonTask(spec) => spec.params.clone(),
        RuntimeBinding::FlowPhp(spec) => spec
            .source
            .arguments
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
        _ => BTreeMap::new(),
    };
    serde_json::to_value(map).unwrap_or(Value::Null)
}

/// Whether a runtime kind may ever be cached, for a caller deciding what to
/// offer rather than what to do.
#[must_use]
pub const fn is_cacheable(kind: RuntimeKind) -> bool {
    kind.is_cacheable()
}

#[cfg(test)]
mod tests {
    use aiwatcher_core::ArtifactKind;

    use super::*;
    use crate::plan::{
        FlowSourceRef, FlowStepSpec, HumanInputSpec, MarimoStepSpec, OutputDeclaration,
        ResolvedWindow, RetryPolicy,
    };

    /// A Flow step whose source is pinned, by a resolved window. See
    /// `code_digest` for why that is enough and what it depends on.
    fn flow_step(resolved: bool) -> PlanStep {
        PlanStep {
            id: "query".to_owned(),
            runtime: RuntimeBinding::FlowPhp(FlowStepSpec {
                script: "data_frame()->read(runs())".to_owned(),
                source: FlowSourceRef {
                    dataset: "runs".to_owned(),
                    window: resolved.then_some(ResolvedWindow { from: 0, to: 3600 }),
                    ..FlowSourceRef::default()
                },
                blocks: vec!["a".to_owned()],
            }),
            inputs: Vec::new(),
            outputs: vec![OutputDeclaration {
                name: "rows".to_owned(),
                kind: ArtifactKind::Rows,
                schema_ref: None,
            }],
            retry: RetryPolicy::default(),
            timeout_seconds: 60,
            cache: CachePolicy::ByContent,
        }
    }

    #[test]
    fn caching_is_opt_in_and_the_default_produces_no_key() {
        let mut step = flow_step(true);
        step.cache = CachePolicy::Never;
        assert!(cache_key(&step, &[]).is_none());
        assert!(cache_key(&flow_step(true), &[]).is_some());
    }

    #[test]
    fn a_window_nobody_resolved_is_not_a_question_two_runs_share() {
        // "The last hour" asked twice an hour apart is two questions. Resolved
        // to exact bounds at compile time it is one, and — since the query
        // service reads those bounds rather than a width from its own now — it
        // is one the rows actually answer.
        assert!(cache_key(&flow_step(false), &[]).is_none());
        assert!(cache_key(&flow_step(true), &[]).is_some());
    }

    #[test]
    fn one_script_over_two_spans_is_two_keys() {
        // The window is in the material, not merely a gate on it. A key that
        // covered only the script's text would answer yesterday's question
        // with today's rows.
        let morning = flow_step(true);
        let mut afternoon = flow_step(true);
        if let RuntimeBinding::FlowPhp(spec) = &mut afternoon.runtime {
            spec.source.window = Some(ResolvedWindow {
                from: 14_400,
                to: 18_000,
            });
        }
        assert_ne!(cache_key(&morning, &[]), cache_key(&afternoon, &[]));
    }

    #[test]
    fn an_input_nobody_can_address_makes_the_step_uncacheable() {
        // The `file://` case: a path on a shared volume is accepted as a
        // pointer and never as an identity.
        let unaddressed = ArtifactRef::new("rows", "file:///data/rows.parquet", "");
        assert!(cache_key(&flow_step(true), &[unaddressed]).is_none());
        let addressed = ArtifactRef::new("rows", "s3://a/rows", "ab".repeat(32));
        assert!(cache_key(&flow_step(true), &[addressed]).is_some());
    }

    #[test]
    fn swapping_two_inputs_is_a_different_question() {
        let one = ArtifactRef::new("a", "s3://a", "aa".repeat(32));
        let two = ArtifactRef::new("b", "s3://b", "bb".repeat(32));
        let forward = cache_key(&flow_step(true), &[one.clone(), two.clone()]);
        let backward = cache_key(&flow_step(true), &[two, one]);
        assert_ne!(forward, backward);
        assert!(forward.is_some());
    }

    #[test]
    fn a_notebook_nobody_saved_is_not_cacheable_and_a_pinned_one_is() {
        let mut step = flow_step(true);
        step.runtime = RuntimeBinding::Marimo(MarimoStepSpec {
            notebook: "pii".to_owned(),
            code_revision: String::new(),
            params: BTreeMap::new(),
            block: None,
        });
        assert!(cache_key(&step, &[]).is_none());
        step.runtime = RuntimeBinding::Marimo(MarimoStepSpec {
            notebook: "pii".to_owned(),
            code_revision: "cd".repeat(32),
            params: BTreeMap::new(),
            block: None,
        });
        assert!(cache_key(&step, &[]).is_some());
    }

    #[test]
    fn a_decision_somebody_made_about_another_run_is_never_reused() {
        let mut step = flow_step(true);
        step.runtime = RuntimeBinding::HumanInput(HumanInputSpec {
            prompt: "promote?".to_owned(),
            role: "admin".to_owned(),
            choices: vec!["yes".to_owned(), "no".to_owned()],
            block: None,
            timeout_seconds: None,
            on_timeout: Default::default(),
        });
        assert!(cache_key(&step, &[]).is_none());
    }

    #[test]
    fn a_fix_to_the_script_generator_does_not_serve_the_old_answer() {
        // What `RUNTIME_IMPLEMENTATION_VERSION` is for: it is in the material,
        // so bumping it invalidates every key of that runtime at once.
        let key = cache_key(&flow_step(true), &[]).expect("a cacheable step");
        assert!(
            crate::plan::canonical(&Material {
                runtime: "flow_php",
                implementation_version: RUNTIME_IMPLEMENTATION_VERSION,
                code: String::new(),
                window: None,
                inputs: Vec::new(),
                parameters: Value::Null,
                outputs: Vec::new(),
            })
            .contains("implementation_version"),
            "the version has to be in the material to invalidate anything"
        );
        assert_eq!(key.len(), 64);
    }
}
