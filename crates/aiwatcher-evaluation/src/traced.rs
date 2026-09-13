//! What the traces of generated answers show they were made with.
//!
//! A variant pins a prompt, a model and a workflow, which a task resolves or
//! runs rather than holds, so the witness for those is telemetry this deployment
//! folded: each answer may name the run it was made in, whose calls say which
//! prompt and model versions served them and whose declaration says which
//! workflow shape it executed. The traces step reads those runs before the
//! score step reads an answer.
//!
//! It refuses what the traces contradict — another variant or result, another
//! version of the pinned prompt or model, the pinned workflow in another shape
//! or off its nodes — and counts what they merely do not show: telemetry is
//! best effort, and a run the log never received says nothing either way.
//!
//! The application's telemetry comes from the host of its answers, so one that
//! reported the pins while calling something else passes. What it cannot report
//! for itself is another credential's word: a serving host's own run naming the
//! call it served is a second witness, counted apart (ADR_0030, amended).

use std::collections::BTreeMap;

use aiwatcher_core::topology::Topology;
use serde::{Deserialize, Serialize};

use crate::{RecordedAnswer, VariantManifest};

/// What the traces step writes for the score step to read: a row per answer.
pub const GENERATION_TRACES: &str = "traces";

/// One model call, as its span says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TracedCall {
    pub model: Option<String>,
    pub model_version: Option<String>,
    pub prompt_name: Option<String>,
    pub prompt_version: Option<String>,
    /// What the provider said served the call, `gen_ai.response.model`.
    pub served_model: Option<String>,
    /// The credential both ends of the call's span were published under.
    pub published_by: Option<String>,
    /// What the call reported using.
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
}

/// A run an answer names, as the log folded it once it had ended and every
/// call it started had a span.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TracedRun {
    /// The trace the log derived for it, which the answer did not have to know.
    pub trace_id: Option<String>,
    pub variant_id: Option<String>,
    pub evaluation_id: Option<String>,
    pub calls: Vec<TracedCall>,
    /// The credential the run's start was published under.
    pub published_by: Option<String>,
    /// The workflow the run names, and the digest of the shape it declared.
    pub workflow: Option<String>,
    pub workflow_topology: Option<String>,
    /// The workflow nodes it started a step of.
    pub nodes_run: Vec<String>,
    /// Each node step's start and end, in the order its log holds them.
    pub node_steps: Vec<StepSeen>,
    /// Calls other runs say they served for this one: a serving host's run
    /// naming it as the caller, each call with its own publisher.
    pub served_for_it: Vec<TracedCall>,
}

/// One step of a workflow node, as a run's log holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepSeen {
    Started(String),
    Completed(String),
    Failed(String),
}

/// What the traces showed about one answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TracedAnswer {
    pub case_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The run it names was on the log, ended.
    pub seen: bool,
    /// That run's trace, so a case leads to it even when the application could
    /// not say its trace ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// A call in that run rendered the pinned prompt version. Absent when the
    /// variant pins no prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_prompt: Option<bool>,
    /// A call in that run was served by the pinned model version. Absent when
    /// the variant pins no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_model: Option<bool>,
    /// The run declared the pinned workflow's shape and stepped only through
    /// its nodes. Absent when the variant pins no workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_workflow: Option<bool>,
    /// A run published under another credential than this answer's run said it
    /// served one of its calls on the pinned model version. Absent when the
    /// variant pins no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_model: Option<bool>,
    /// What providers said served the run's calls, each once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub served_models: Vec<String>,
    /// The run's calls by the model each named, with what they reported
    /// using: what a price table prices a generated case by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<aiwatcher_core::prices::ModelUsage>,
    /// The variant pins a workflow whose declaration this step could read no
    /// node of, so no run was seen executing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workflow_undeclared: bool,
}

/// One model a provider said served generated answers, and how many.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GenerationServed {
    pub model: String,
    /// Answers whose run had a call this model served.
    pub answers: usize,
}

/// What a generated result says the traces of its answers showed.
///
/// Counts rather than a verdict: how many answers there were, how many named
/// the run they were made in, how many of those runs the log held, and how
/// many ran on each pin. A reader — or a gate — decides whether fewer than all
/// is enough.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GenerationTrace {
    pub answers: usize,
    /// Answers naming the run they were made in.
    pub named: usize,
    /// Of those, runs this deployment's log held, ended, when the step looked.
    pub seen: usize,
    /// Seen runs with a call on the pinned prompt version; absent when the
    /// variant pins no prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_prompt: Option<usize>,
    /// Seen runs with a call served by the pinned model version; absent when
    /// the variant pins no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_model: Option<usize>,
    /// Seen runs that declared the pinned workflow's shape and stepped only
    /// through its nodes; absent when the variant pins no workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_workflow: Option<usize>,
    /// The declaration of the pinned workflow names no node this step could
    /// read, so no run can be seen executing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workflow_undeclared: bool,
    /// Seen runs whose call on the pinned model version a run published under
    /// another credential says it served; absent when the variant pins no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_model: Option<usize>,
    /// What providers said served the calls, compared with nothing: a provider's
    /// name for a model is an alias, a file or a dated snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub served: Vec<GenerationServed>,
}

impl GenerationTrace {
    #[must_use]
    pub fn of(rows: &[TracedAnswer]) -> Self {
        let counted = |side: fn(&TracedAnswer) -> Option<bool>| {
            rows.iter()
                .map(side)
                .collect::<Option<Vec<bool>>>()
                .filter(|_| !rows.is_empty())
                .map(|sides| sides.into_iter().filter(|on| *on).count())
        };
        let mut served: BTreeMap<&str, usize> = BTreeMap::new();
        for row in rows {
            for model in &row.served_models {
                *served.entry(model.as_str()).or_default() += 1;
            }
        }
        Self {
            answers: rows.len(),
            named: rows.iter().filter(|row| row.run_id.is_some()).count(),
            seen: rows.iter().filter(|row| row.seen).count(),
            on_prompt: counted(|row| row.on_prompt),
            on_model: counted(|row| row.on_model),
            on_workflow: counted(|row| row.on_workflow),
            workflow_undeclared: rows.iter().any(|row| row.workflow_undeclared),
            witnessed_model: counted(|row| row.witnessed_model),
            served: served
                .into_iter()
                .map(|(model, answers)| GenerationServed {
                    model: model.to_owned(),
                    answers,
                })
                .collect(),
        }
    }

    /// Whether every answer was seen made on everything the variant pins that
    /// a trace can show.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.seen == self.answers
            && self.on_prompt.is_none_or(|on| on == self.answers)
            && self.on_model.is_none_or(|on| on == self.answers)
            && self.on_workflow.is_none_or(|on| on == self.answers)
    }

    /// Whether every answer on a pinned model had a second credential's word
    /// for the model version that served it.
    #[must_use]
    pub fn witnessed(&self) -> bool {
        self.witnessed_model.is_none_or(|on| on == self.answers)
    }

    /// What is missing, in words; empty when [`Self::complete`].
    #[must_use]
    pub fn shortfall(&self) -> Vec<String> {
        let mut said = Vec::new();
        if self.named < self.answers {
            said.push(format!(
                "{} of {} answers name no run they were made in",
                self.answers - self.named,
                self.answers
            ));
        }
        if self.seen < self.named {
            said.push(format!(
                "{} of the {} runs the answers name were not on the log",
                self.named - self.seen,
                self.named
            ));
        }
        if self.workflow_undeclared {
            said.push(
                "the declaration of the pinned workflow names no node this step could read, so no \
                 run can be seen executing it"
                    .to_owned(),
            );
        }
        for (what, on) in [
            ("call on the pinned prompt", self.on_prompt),
            ("call on the pinned model", self.on_model),
            ("execution of the pinned workflow", self.on_workflow),
        ] {
            if let Some(on) = on
                && on < self.seen
            {
                said.push(format!(
                    "{} of {} seen runs show no {what}",
                    self.seen - on,
                    self.seen
                ));
            }
        }
        said
    }

    /// What a second witness did not show, in words; empty when
    /// [`Self::witnessed`].
    #[must_use]
    pub fn unwitnessed(&self) -> Vec<String> {
        match self.witnessed_model {
            Some(on) if on < self.answers => vec![format!(
                "{} of {} answers have no run published under another credential saying it \
                 served their call on the pinned model",
                self.answers - on,
                self.answers
            )],
            _ => Vec::new(),
        }
    }
}

/// The nodes a run started before any node the declaration leads into them
/// from had completed, each once, with those nodes.
///
/// Any one completed predecessor admits a node: a declared branch runs one
/// side, and a join after it is reached from whichever side ran. A node nothing
/// leads into starts whenever it likes.
fn out_of_order(shape: &Topology, steps: &[StepSeen]) -> Vec<(String, Vec<String>)> {
    let mut completed = std::collections::BTreeSet::new();
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    for step in steps {
        match step {
            StepSeen::Started(node) => {
                let before: Vec<String> = shape
                    .edges
                    .iter()
                    .filter(|(_, to)| to == node)
                    .map(|(from, _)| from.clone())
                    .collect();
                if !before.is_empty()
                    && !before.iter().any(|from| completed.contains(from))
                    && !found.iter().any(|(named, _)| named == node)
                {
                    found.push((node.clone(), before));
                }
            }
            StepSeen::Completed(node) => {
                completed.insert(node.clone());
            }
            StepSeen::Failed(_) => {}
        }
    }
    found
}

/// Hold each generated answer to the run it names.
///
/// `runs` holds the runs the log had, ended and complete; a run an answer names
/// and `runs` lacks is unseen. `workflow` is the shape of the declaration the
/// variant pins, read from its bytes, or `None` where those name no node.
/// Errors carry every contradiction at once, each naming the case, the run and
/// both sides.
///
/// # Errors
///
/// The sentences for every run whose trace contradicts the variant.
pub fn trace_answers(
    variant: &VariantManifest,
    variant_id: &str,
    evaluation_id: &str,
    answers: &[RecordedAnswer],
    runs: &BTreeMap<String, TracedRun>,
    workflow: Option<&Topology>,
) -> std::result::Result<Vec<TracedAnswer>, Vec<String>> {
    let mut rows = Vec::with_capacity(answers.len());
    let mut contradictions = Vec::new();
    let pinned_shape = workflow.map(Topology::digest);
    for answer in answers {
        let traced = answer.run_id.as_ref().and_then(|run_id| runs.get(run_id));
        let mut row = TracedAnswer {
            case_id: answer.case_id.clone(),
            run_id: answer.run_id.clone(),
            seen: traced.is_some(),
            trace_id: traced.and_then(|run| run.trace_id.clone()),
            on_prompt: variant.prompt.as_ref().map(|_| false),
            on_model: variant.model.as_ref().map(|_| false),
            on_workflow: variant.workflow.as_ref().map(|_| false),
            witnessed_model: variant.model.as_ref().map(|_| false),
            served_models: Vec::new(),
            models: Vec::new(),
            workflow_undeclared: variant.workflow.is_some() && workflow.is_none(),
        };
        if let (Some(run_id), Some(run)) = (&answer.run_id, traced) {
            let mut said = |sentence: String| {
                contradictions.push(format!("{} (run {run_id}): {sentence}", answer.case_id));
            };
            if let Some(named) = run
                .variant_id
                .as_deref()
                .filter(|named| *named != variant_id)
            {
                said(format!(
                    "the run names variant {named}, and this result is published as {variant_id}"
                ));
            }
            if let Some(named) = run
                .evaluation_id
                .as_deref()
                .filter(|named| *named != evaluation_id)
            {
                said(format!(
                    "the run answered for {named}, and this result is {evaluation_id}"
                ));
            }
            for call in &run.calls {
                aiwatcher_core::prices::ModelUsage::add_to(
                    &mut row.models,
                    &aiwatcher_core::prices::ModelUsage {
                        model: call.model.clone().unwrap_or_else(|| "unknown".to_owned()),
                        calls: 1,
                        input_tokens: call.input_tokens,
                        output_tokens: call.output_tokens,
                        cached_tokens: call.cached_tokens,
                    },
                );
                if let Some(served) = &call.served_model
                    && !row.served_models.contains(served)
                {
                    row.served_models.push(served.clone());
                }
                if let Some(pinned) = &variant.prompt {
                    let version = call.prompt_version.as_deref();
                    if version == Some(pinned.version.as_str()) {
                        row.on_prompt = Some(true);
                    } else if call.prompt_name.as_deref() == Some(pinned.name.as_str()) {
                        said(format!(
                            "a call rendered {} at {}, and the variant pins {}",
                            pinned.name,
                            version.unwrap_or("no version"),
                            pinned.version
                        ));
                    }
                }
                if let Some(pinned) = &variant.model
                    && call.model.as_deref() == Some(pinned.name.as_str())
                {
                    match call.model_version.as_deref() {
                        Some(version) if version == pinned.version => row.on_model = Some(true),
                        Some(version) => said(format!(
                            "a call was served by {} at {version}, and the variant pins {}",
                            pinned.name, pinned.version
                        )),
                        // A name with no version says which model and not which
                        // of its versions: not a contradiction, and not a sighting.
                        None => {}
                    }
                }
            }
            if let Some(pinned) = &variant.model {
                // Another credential's word, or none: a call the answer's own
                // publisher reported for a serving run is still its word.
                for call in &run.served_for_it {
                    let independent = matches!(
                        (&call.published_by, &run.published_by),
                        (Some(witness), Some(answerer)) if witness != answerer
                    );
                    if !independent || call.model.as_deref() != Some(pinned.name.as_str()) {
                        continue;
                    }
                    match call.model_version.as_deref() {
                        Some(version) if version == pinned.version => {
                            row.witnessed_model = Some(true);
                        }
                        Some(version) => said(format!(
                            "{} says it served this run's call with {} at {version}, and the \
                             variant pins {}",
                            call.published_by.as_deref().unwrap_or("another publisher"),
                            pinned.name,
                            pinned.version
                        )),
                        None => {}
                    }
                }
            }
            if let Some(pinned) = &variant.workflow
                && run.workflow.as_deref() == Some(pinned.name.as_str())
            {
                let mut on = pinned_shape.is_some();
                if let (Some(declared), Some(shape)) = (&run.workflow_topology, &pinned_shape)
                    && declared != shape
                {
                    on = false;
                    said(format!(
                        "the run declared {} in another shape than the declaration the variant \
                         pins",
                        pinned.name
                    ));
                }
                if run.workflow_topology.is_none() {
                    on = false;
                }
                if let Some(shape) = workflow {
                    for node in &run.nodes_run {
                        if !shape.nodes.contains(node) {
                            on = false;
                            said(format!(
                                "the run stepped through {node}, which the declaration of {} the \
                                 variant pins does not name",
                                pinned.name
                            ));
                        }
                    }
                    for (node, before) in out_of_order(shape, &run.node_steps) {
                        on = false;
                        said(format!(
                            "the run started {node} before {} had completed, and the declaration \
                             of {} the variant pins leads into it only from there",
                            before.join(" or "),
                            pinned.name
                        ));
                    }
                }
                row.on_workflow = Some(on);
            }
        }
        rows.push(row);
    }
    if contradictions.is_empty() {
        Ok(rows)
    } else {
        Err(contradictions)
    }
}

#[cfg(test)]
mod tests {
    use aiwatcher_core::{ArtifactKind, ArtifactRef};

    use super::*;
    use crate::{DatasetKind, DatasetReference, VersionReference};

    fn artifact(name: &str) -> ArtifactRef {
        ArtifactRef {
            name: name.to_owned(),
            uri: format!("file://{name}"),
            digest: "a".repeat(64),
            size_bytes: Some(1),
            content_type: String::new(),
            kind: ArtifactKind::Blob,
            schema_ref: None,
        }
    }

    fn variant() -> VariantManifest {
        VariantManifest {
            schema_version: 1,
            experiment_id: "candidate".to_owned(),
            dataset: DatasetReference {
                kind: DatasetKind::Curation,
                name: "capitals".to_owned(),
                version: "d".repeat(64),
            },
            model: Some(VersionReference {
                name: "capitals-model".to_owned(),
                version: "v7".to_owned(),
            }),
            prompt: Some(VersionReference {
                name: "capitals".to_owned(),
                version: "p".repeat(64),
            }),
            code: artifact("code"),
            generation_config: artifact("config"),
            response_schema: None,
            tools: None,
            workflow: None,
        }
    }

    fn answer(case_id: &str, run_id: Option<&str>) -> RecordedAnswer {
        RecordedAnswer {
            case_id: case_id.to_owned(),
            answer: serde_json::json!("Paris"),
            run_id: run_id.map(ToOwned::to_owned),
            trace_id: None,
            span_id: None,
            usage: None,
        }
    }

    fn on_the_pins() -> TracedCall {
        TracedCall {
            model: Some("capitals-model".to_owned()),
            model_version: Some("v7".to_owned()),
            prompt_name: Some("capitals".to_owned()),
            prompt_version: Some("p".repeat(64)),
            served_model: None,
            published_by: Some("worker".to_owned()),
            input_tokens: 12,
            output_tokens: 3,
            cached_tokens: 0,
        }
    }

    fn run(calls: Vec<TracedCall>) -> TracedRun {
        TracedRun {
            trace_id: None,
            variant_id: Some("variant".to_owned()),
            evaluation_id: Some("answers".to_owned()),
            calls,
            published_by: Some("worker".to_owned()),
            ..TracedRun::default()
        }
    }

    #[test]
    fn answers_made_on_the_pins_are_seen_on_them_and_unseen_ones_are_counted_not_refused() {
        let runs = BTreeMap::from([
            ("r1".to_owned(), run(vec![on_the_pins()])),
            (
                "r2".to_owned(),
                run(vec![
                    TracedCall {
                        model: Some("router".to_owned()),
                        ..TracedCall::default()
                    },
                    on_the_pins(),
                ]),
            ),
        ]);
        let answers = [
            answer("c1", Some("r1")),
            answer("c2", Some("r2")),
            answer("c3", Some("r-never-arrived")),
            answer("c4", None),
        ];

        let rows = trace_answers(&variant(), "variant", "answers", &answers, &runs, None)
            .expect("nothing contradicts the pins");
        let trace = GenerationTrace::of(&rows);

        assert_eq!(
            trace,
            GenerationTrace {
                answers: 4,
                named: 3,
                seen: 2,
                on_prompt: Some(2),
                on_model: Some(2),
                on_workflow: None,
                workflow_undeclared: false,
                witnessed_model: Some(0),
                served: Vec::new(),
            }
        );
        assert!(!trace.complete());
        assert_eq!(
            rows[1].models,
            [
                aiwatcher_core::prices::ModelUsage {
                    model: "capitals-model".to_owned(),
                    calls: 1,
                    input_tokens: 12,
                    output_tokens: 3,
                    cached_tokens: 0,
                },
                aiwatcher_core::prices::ModelUsage {
                    model: "router".to_owned(),
                    calls: 1,
                    ..Default::default()
                },
            ],
            "each call by the model it named, what it reported beside it"
        );
        assert_eq!(
            trace.shortfall(),
            vec![
                "1 of 4 answers name no run they were made in".to_owned(),
                "1 of the 3 runs the answers name were not on the log".to_owned(),
            ]
        );
    }

    #[test]
    fn a_call_on_another_version_of_the_pinned_prompt_or_model_refuses_the_answers() {
        let runs = BTreeMap::from([(
            "r1".to_owned(),
            run(vec![
                TracedCall {
                    prompt_version: Some("q".repeat(64)),
                    ..on_the_pins()
                },
                TracedCall {
                    model_version: Some("v6".to_owned()),
                    prompt_name: None,
                    prompt_version: None,
                    ..on_the_pins()
                },
            ]),
        )]);

        let refused = trace_answers(
            &variant(),
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &runs,
            None,
        )
        .expect_err("the trace contradicts the pins");

        assert_eq!(refused.len(), 2, "{refused:?}");
        assert!(refused[0].contains(&"q".repeat(64)) && refused[0].contains(&"p".repeat(64)));
        assert!(refused[1].contains("v6") && refused[1].contains("v7"));
    }

    #[test]
    fn a_run_naming_another_variant_or_result_is_not_this_one_s() {
        let runs = BTreeMap::from([(
            "r1".to_owned(),
            TracedRun {
                trace_id: None,
                variant_id: Some("baseline".to_owned()),
                evaluation_id: Some("answers-baseline".to_owned()),
                calls: vec![on_the_pins()],
                ..TracedRun::default()
            },
        )]);

        let refused = trace_answers(
            &variant(),
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &runs,
            None,
        )
        .expect_err("another variant's run");

        assert!(
            refused
                .iter()
                .any(|said| said.contains("names variant baseline"))
        );
        assert!(
            refused
                .iter()
                .any(|said| said.contains("answered for answers-baseline"))
        );
    }

    #[test]
    fn a_model_named_without_its_version_is_neither_a_sighting_nor_a_contradiction() {
        let mut pins = variant();
        pins.prompt = None;
        let runs = BTreeMap::from([(
            "r1".to_owned(),
            run(vec![TracedCall {
                model_version: None,
                ..on_the_pins()
            }]),
        )]);

        let rows = trace_answers(
            &pins,
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &runs,
            None,
        )
        .expect("no version is no contradiction");

        let trace = GenerationTrace::of(&rows);
        assert_eq!((trace.on_prompt, trace.on_model), (None, Some(0)));
    }

    fn workflow_variant() -> VariantManifest {
        VariantManifest {
            model: None,
            prompt: None,
            workflow: Some(VersionReference {
                name: "capitals-app".to_owned(),
                version: "w".repeat(64),
            }),
            ..variant()
        }
    }

    fn shape(edges: &[(&str, &str)]) -> Topology {
        Topology::read(&serde_json::json!({
            "nodes": ["retrieve", "answer"],
            "edges": edges.iter().map(|(from, to)| [from, to]).collect::<Vec<_>>(),
        }))
        .expect("a shape")
    }

    fn workflow_run(declared: &Topology, nodes_run: &[&str]) -> TracedRun {
        TracedRun {
            workflow: Some("capitals-app".to_owned()),
            workflow_topology: Some(declared.digest()),
            nodes_run: nodes_run.iter().map(|node| (*node).to_owned()).collect(),
            ..run(Vec::new())
        }
    }

    #[test]
    fn a_run_that_declared_the_pinned_shape_and_stayed_on_it_is_seen_executing_it() {
        let pinned = shape(&[("retrieve", "answer")]);
        let runs = BTreeMap::from([
            (
                "r1".to_owned(),
                workflow_run(&pinned, &["retrieve", "answer"]),
            ),
            (
                "r2".to_owned(),
                TracedRun {
                    workflow_topology: None,
                    ..workflow_run(&pinned, &["answer"])
                },
            ),
        ]);

        let rows = trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[answer("c1", Some("r1")), answer("c2", Some("r2"))],
            &runs,
            Some(&pinned),
        )
        .expect("nothing contradicts the pinned shape");
        let trace = GenerationTrace::of(&rows);

        assert_eq!(
            trace.on_workflow,
            Some(1),
            "a run that declared nothing is not seen on it"
        );
        assert_eq!(
            trace.shortfall(),
            ["1 of 2 seen runs show no execution of the pinned workflow".to_owned()]
        );
    }

    #[test]
    fn a_run_declaring_the_pinned_workflow_in_another_shape_or_stepping_off_it_is_refused() {
        let pinned = shape(&[("retrieve", "answer")]);
        let runs = BTreeMap::from([
            (
                "other-shape".to_owned(),
                workflow_run(&shape(&[("answer", "retrieve")]), &[]),
            ),
            (
                "off-the-graph".to_owned(),
                workflow_run(&pinned, &["retrieve", "improvise"]),
            ),
        ]);

        let refused = trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[
                answer("c1", Some("other-shape")),
                answer("c2", Some("off-the-graph")),
            ],
            &runs,
            Some(&pinned),
        )
        .expect_err("both contradict the pinned declaration");

        assert!(refused[0].contains("another shape"), "{refused:?}");
        assert!(refused[1].contains("improvise"), "{refused:?}");
    }

    #[test]
    fn a_run_that_started_a_node_before_what_leads_into_it_completed_is_refused() {
        let pinned = shape(&[("retrieve", "answer")]);
        let steps = |order: &[(&str, bool)]| -> Vec<StepSeen> {
            order
                .iter()
                .map(|(node, started)| {
                    if *started {
                        StepSeen::Started((*node).to_owned())
                    } else {
                        StepSeen::Completed((*node).to_owned())
                    }
                })
                .collect()
        };
        let runs = BTreeMap::from([
            (
                "in-order".to_owned(),
                TracedRun {
                    node_steps: steps(&[
                        ("retrieve", true),
                        ("retrieve", false),
                        ("answer", true),
                        ("answer", false),
                    ]),
                    ..workflow_run(&pinned, &["retrieve", "answer"])
                },
            ),
            (
                "answered-first".to_owned(),
                TracedRun {
                    node_steps: steps(&[
                        ("answer", true),
                        ("retrieve", true),
                        ("retrieve", false),
                        ("answer", false),
                        ("answer", true),
                    ]),
                    ..workflow_run(&pinned, &["answer", "retrieve"])
                },
            ),
        ]);

        let rows = trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[answer("c1", Some("in-order"))],
            &runs,
            Some(&pinned),
        )
        .expect("the order the declaration leads");
        assert_eq!(rows[0].on_workflow, Some(true));

        let refused = trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[answer("c2", Some("answered-first"))],
            &runs,
            Some(&pinned),
        )
        .expect_err("answer started before retrieve completed");
        assert_eq!(refused.len(), 1, "named once: {refused:?}");
        assert!(
            refused[0].contains("started answer before retrieve had completed"),
            "{refused:?}"
        );
    }

    #[test]
    fn a_serving_host_s_word_counts_only_under_another_credential_and_contradicts_under_one() {
        let mut pins = variant();
        pins.prompt = None;
        let served = |publisher: &str, version: &str| TracedCall {
            model: Some("capitals-model".to_owned()),
            model_version: Some(version.to_owned()),
            served_model: Some("capitals-model-q4".to_owned()),
            published_by: Some(publisher.to_owned()),
            ..TracedCall::default()
        };
        let runs = BTreeMap::from([
            (
                "witnessed".to_owned(),
                TracedRun {
                    served_for_it: vec![served("serving", "v7")],
                    ..run(vec![TracedCall {
                        served_model: Some("capitals-model-q4".to_owned()),
                        ..on_the_pins()
                    }])
                },
            ),
            (
                "self-reported".to_owned(),
                TracedRun {
                    served_for_it: vec![served("worker", "v7")],
                    ..run(vec![on_the_pins()])
                },
            ),
        ]);

        let rows = trace_answers(
            &pins,
            "variant",
            "answers",
            &[
                answer("c1", Some("witnessed")),
                answer("c2", Some("self-reported")),
            ],
            &runs,
            None,
        )
        .expect("nothing contradicts the pins");
        let trace = GenerationTrace::of(&rows);
        assert_eq!(
            (trace.on_model, trace.witnessed_model),
            (Some(2), Some(1)),
            "a serving run the worker's own credential published is the worker's word"
        );
        assert!(trace.complete() && !trace.witnessed());
        assert_eq!(
            trace.served,
            [GenerationServed {
                model: "capitals-model-q4".to_owned(),
                answers: 1
            }]
        );

        let contradicted = BTreeMap::from([(
            "r1".to_owned(),
            TracedRun {
                served_for_it: vec![served("serving", "v6")],
                ..run(vec![on_the_pins()])
            },
        )]);
        let refused = trace_answers(
            &pins,
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &contradicted,
            None,
        )
        .expect_err("the serving host served another version");
        assert!(
            refused[0].contains("serving says it served") && refused[0].contains("v6"),
            "{refused:?}"
        );
    }
}
