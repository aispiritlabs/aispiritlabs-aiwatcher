//! What the traces of generated answers show they were made with.
//!
//! `generated_with` is the task's word about the code and configuration it
//! holds. A variant also pins a prompt and a model, which a task resolves
//! through a registry rather than holds — so the witness for those is the
//! application's own telemetry, folded by this deployment: each answer may name
//! the run it was made in, and that run's model calls say which prompt version
//! they rendered and which model version served them. The traces step reads
//! those runs off the log before the score step reads an answer.
//!
//! It refuses what the traces contradict — a run naming another variant or
//! another result, a call on another version of the pinned prompt, a call to
//! the pinned model at another version — because those answers are not the
//! variant's. What the traces merely do not show is reported and not refused:
//! telemetry is best effort by design, and a run the log never received says
//! nothing either way. The result carries how many answers were seen on the
//! pinned prompt and model, and a gate may require all of them.
//!
//! Still not a proof. The telemetry comes from the same host as the answers,
//! and an application that reported the pins while calling something else would
//! pass; what it can no longer do is report something else and pass.

use std::collections::BTreeMap;

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
}

/// What a generated result says the traces of its answers showed.
///
/// Counts rather than a verdict: how many answers there were, how many named
/// the run they were made in, how many of those runs the log held, and how
/// many ran on the pinned prompt and model. A reader — or a gate — decides
/// whether fewer than all is enough.
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
        Self {
            answers: rows.len(),
            named: rows.iter().filter(|row| row.run_id.is_some()).count(),
            seen: rows.iter().filter(|row| row.seen).count(),
            on_prompt: counted(|row| row.on_prompt),
            on_model: counted(|row| row.on_model),
        }
    }

    /// Whether every answer was seen made on everything the variant pins that
    /// a trace can show.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.seen == self.answers
            && self.on_prompt.is_none_or(|on| on == self.answers)
            && self.on_model.is_none_or(|on| on == self.answers)
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
        for (what, on) in [("prompt", self.on_prompt), ("model", self.on_model)] {
            if let Some(on) = on
                && on < self.seen
            {
                said.push(format!(
                    "{} of {} seen runs show no call on the pinned {what}",
                    self.seen - on,
                    self.seen
                ));
            }
        }
        said
    }
}

/// Hold each generated answer to the run it names.
///
/// `runs` holds the runs the log had, ended and complete; a run an answer names
/// and `runs` lacks is unseen. Errors carry every contradiction at once, each
/// naming the case, the run and both sides.
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
) -> std::result::Result<Vec<TracedAnswer>, Vec<String>> {
    let mut rows = Vec::with_capacity(answers.len());
    let mut contradictions = Vec::new();
    for answer in answers {
        let traced = answer.run_id.as_ref().and_then(|run_id| runs.get(run_id));
        let mut row = TracedAnswer {
            case_id: answer.case_id.clone(),
            run_id: answer.run_id.clone(),
            seen: traced.is_some(),
            trace_id: traced.and_then(|run| run.trace_id.clone()),
            on_prompt: variant.prompt.as_ref().map(|_| false),
            on_model: variant.model.as_ref().map(|_| false),
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
        }
    }

    fn run(calls: Vec<TracedCall>) -> TracedRun {
        TracedRun {
            trace_id: None,
            variant_id: Some("variant".to_owned()),
            evaluation_id: Some("answers".to_owned()),
            calls,
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

        let rows = trace_answers(&variant(), "variant", "answers", &answers, &runs)
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
            }
        );
        assert!(!trace.complete());
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
            },
        )]);

        let refused = trace_answers(
            &variant(),
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &runs,
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
        )
        .expect("no version is no contradiction");

        let trace = GenerationTrace::of(&rows);
        assert_eq!((trace.on_prompt, trace.on_model), (None, Some(0)));
    }
}
