//! The context every submission to one lab publishes under, and the id it is
//! found by.
//!
//! A `context_id` is the content address of the cohort, the split, the suite,
//! the scorer and the metric definitions **together**, so every result measured
//! on one cohort by one card shares it whoever produced it. That is what makes
//! a lab's marks a list somebody can already ask for — `GET
//! /api/v1/evaluation-results?context_id=…` — rather than a second index this
//! registry would have to keep.
//!
//! It is answerable before any result exists, which is the whole point: an
//! instructor pins a card and a cohort, and the lab can say the key its
//! submissions will land under before anybody has submitted one.
//!
//! Nothing here reads a clock or opens a socket. The caller resolves the card
//! and the cohort and hands both in.

use aiwatcher_evaluation::{Cohort, DatasetReference, EvaluationContext, Rubrics, Scorecard};
use serde::Serialize;

use crate::{LabError, LabTests, Result};

/// What a lab measures, and the key its results share.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = LabMeasurement)]
pub struct LabMeasurement {
    /// The context a submission publishes in: the cases, the card and what
    /// each metric means.
    pub context: EvaluationContext,
    /// Its content address — what `GET /api/v1/evaluation-results` is
    /// narrowed by to list this lab's marks.
    pub context_id: String,
}

impl LabTests {
    /// The measurement this lab pins, from the card and the cohort it names.
    ///
    /// The metrics come from the card rather than from anybody's hand: which
    /// way a scorer's number is better is a fact about what it counts, and a
    /// lab restating it would be free to disagree with the card it points at.
    ///
    /// # Errors
    ///
    /// [`LabError::Unmeasurable`] when the card asks for something a lab does
    /// not pin — a judge, or a calibrated framework metric — naming the metric
    /// that asks. Both are declared per *run* today, so two participants could
    /// be graded by two different judges and their results would not share a
    /// context; saying so is better than answering an id that quietly means
    /// less than it looks like it means.
    ///
    /// [`LabError::Invalid`] when the card and the cohort do not fit together,
    /// carrying the evaluation registry's own sentence.
    pub fn measurement(
        &self,
        dataset: &DatasetReference,
        cohort: &Cohort,
        card: &Scorecard,
        rubrics: &Rubrics,
    ) -> Result<LabMeasurement> {
        for spec in &card.scorers {
            if spec.scorer.rubric().is_some() {
                return Err(LabError::Unmeasurable {
                    metric: spec.metric.clone(),
                    reason: "is judged by a model, and a judge is declared per scoring run rather \
                             than by a lab — so two submissions need not have been graded by the \
                             same one and their results share no context"
                        .into(),
                });
            }
            if let Some(external) = spec.scorer.external()
                && external.calibration.is_some()
            {
                return Err(LabError::Unmeasurable {
                    metric: spec.metric.clone(),
                    reason: "is a framework metric held to a calibration set, which is named per \
                             scoring run rather than by a lab"
                        .into(),
                });
            }
        }
        let context = EvaluationContext {
            dataset: dataset.clone(),
            case_manifest: cohort.case_manifest.clone(),
            case_count: cohort.case_count,
            split: cohort.split.clone(),
            suite: self.scorecard.clone(),
            scorer: aiwatcher_evaluation::scoring_engine(),
            input_schema: cohort.input_schema.clone(),
            expectations_schema: cohort.expectations_schema.clone(),
            judge: None,
            external_calibration: None,
            metrics: card.metrics(rubrics)?,
        };
        let context_id = context.id()?;
        Ok(LabMeasurement {
            context,
            context_id,
        })
    }
}
