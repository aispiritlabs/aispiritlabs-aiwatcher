//! What an assessment is allowed to say.
//!
//! A number with no scale behind it is a number nobody can read back: `3` is
//! excellent on one team's form and a failure on another's. A rubric is that
//! form — the question, the words a person and a judge are both given, the set
//! of answers it admits, and which end of it is better — and it is versioned by
//! its content, so an assessment made under one set of levels keeps meaning
//! what it meant after somebody rewrites them.

use serde::{Deserialize, Serialize};

use crate::{MetricDirection, Result, SCHEMA_VERSION, digest, require, text};

/// The answers one rubric admits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scale {
    /// A bounded number. The bounds are part of the version, so widening them
    /// is a new rubric rather than a quiet re-reading of every score.
    Numeric { min: f64, max: f64 },
    /// Named levels, in the order they were declared. Order is meaning here —
    /// the direction says which end is better — so reordering is a new version
    /// for the same reason renaming an annotation class is.
    Ordinal {
        #[schema(min_items = 2, max_items = 32)]
        levels: Vec<String>,
    },
    /// Yes or no, where a scale would be a scale of two.
    Flag,
}

/// One answer, in the shape its scale declared.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssessmentValue {
    Number { value: f64 },
    Level { value: String },
    Flag { value: bool },
}

impl Scale {
    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::Numeric { min, max } => require(
                min.is_finite() && max.is_finite() && min < max,
                "rubric.scale",
                "a numeric scale needs finite bounds with min below max",
            ),
            Self::Ordinal { levels } => {
                require(
                    (2..=32).contains(&levels.len()),
                    "rubric.scale",
                    "an ordinal scale needs 2–32 levels",
                )?;
                for level in levels {
                    text(level, "rubric.scale.levels")?;
                }
                let mut seen = std::collections::BTreeSet::new();
                require(
                    levels.iter().all(|level| seen.insert(level)),
                    "rubric.scale.levels",
                    "level names must be unique",
                )
            }
            Self::Flag => Ok(()),
        }
    }

    /// Whether this scale admits that answer, and why not when it does not.
    ///
    /// The refusal names both sides. "Invalid value" sends somebody looking at
    /// their own typing; "this rubric is ordinal and you sent a number" sends
    /// them to the rubric, which is where the disagreement actually is.
    pub(crate) fn admits(&self, value: &AssessmentValue) -> Result<()> {
        match (self, value) {
            (Self::Numeric { min, max }, AssessmentValue::Number { value }) => require(
                value.is_finite() && value >= min && value <= max,
                "value",
                &format!("must be a number within {min}..={max}"),
            ),
            (Self::Ordinal { levels }, AssessmentValue::Level { value }) => require(
                levels.contains(value),
                "value",
                &format!("must be one of the rubric's levels: {}", levels.join(", ")),
            ),
            (Self::Flag, AssessmentValue::Flag { .. }) => Ok(()),
            (scale, value) => Err(crate::EvaluationError::Invalid {
                field: "value".into(),
                reason: format!(
                    "this rubric is {} and the answer is {}",
                    scale.word(),
                    match value {
                        AssessmentValue::Number { .. } => "a number",
                        AssessmentValue::Level { .. } => "a level",
                        AssessmentValue::Flag { .. } => "a flag",
                    }
                ),
            }),
        }
    }

    const fn word(&self) -> &'static str {
        match self {
            Self::Numeric { .. } => "numeric",
            Self::Ordinal { .. } => "ordinal",
            Self::Flag => "a flag",
        }
    }
}

/// The form itself. Everything here is part of the version: two rubrics that
/// ask different questions under one name are two rubrics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Rubric {
    pub name: String,
    /// What the assessor is being asked. The one sentence that has to survive
    /// being read a year later beside a score somebody wrote today.
    pub question: String,
    /// The same words a person and a judge are given. A judge admitted against
    /// a calibration set was calibrated against *these* words, so they are
    /// pinned with everything else rather than kept in a prompt beside them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub guidance: String,
    pub scale: Scale,
    /// Which way is better. The same vocabulary a metric definition uses,
    /// because it is the same question — and it is declared here so nothing
    /// downstream has to guess whether a rise is good news.
    pub direction: MetricDirection,
}

impl Rubric {
    pub(crate) fn validate(&self) -> Result<()> {
        text(&self.name, "rubric.name")?;
        require(
            !self.name.contains('/'),
            "rubric.name",
            "must not contain a path separator: the name is how a reader asks for the form",
        )?;
        text(&self.question, "rubric.question")?;
        require(
            self.guidance.len() <= 8 * 1024 && !self.guidance.contains('\0'),
            "rubric.guidance",
            "must be at most 8 KiB of text",
        )?;
        self.scale.validate()
    }

    /// The content address of this form. Publishing the same words twice lands
    /// on the version that is already there, the way a prompt's does.
    pub(crate) fn version(&self) -> Result<String> {
        digest(&(SCHEMA_VERSION, "evaluation.rubric", self))
    }
}

/// One immutable version, and who put it there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RubricVersion {
    pub version: String,
    /// Nested rather than flattened: the form denies unknown fields, and a
    /// flattened struct that does reports every field beside it as one.
    pub rubric: Rubric,
    pub published_by: String,
    pub published_at: i64,
}

/// Which version a caller that named no version gets. Derived from the
/// versions, which are the truth — and moved by publishing, never edited.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RubricHead {
    pub name: String,
    pub version: String,
    pub question: String,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RubricPage {
    pub rubrics: Vec<RubricHead>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordinal() -> Scale {
        Scale::Ordinal {
            levels: vec!["bad".into(), "fine".into(), "good".into()],
        }
    }

    #[test]
    fn a_scale_refuses_an_answer_of_the_wrong_shape_by_naming_both() {
        let refusal = ordinal()
            .admits(&AssessmentValue::Number { value: 2.0 })
            .expect_err("an ordinal scale takes no numbers");
        let said = refusal.to_string();
        assert!(said.contains("ordinal"), "{said}");
        assert!(said.contains("a number"), "{said}");
    }

    #[test]
    fn an_ordinal_scale_names_the_levels_it_does_admit() {
        let refusal = ordinal()
            .admits(&AssessmentValue::Level {
                value: "excellent".into(),
            })
            .expect_err("that level was never declared");
        assert!(refusal.to_string().contains("bad, fine, good"));
        assert!(
            ordinal()
                .admits(&AssessmentValue::Level {
                    value: "fine".into()
                })
                .is_ok()
        );
    }

    #[test]
    fn a_numeric_scale_bounds_both_ends_and_refuses_what_is_not_a_number() {
        let scale = Scale::Numeric { min: 1.0, max: 5.0 };
        assert!(
            scale
                .admits(&AssessmentValue::Number { value: 5.0 })
                .is_ok()
        );
        assert!(
            scale
                .admits(&AssessmentValue::Number { value: 5.5 })
                .is_err()
        );
        assert!(
            scale
                .admits(&AssessmentValue::Number { value: 0.5 })
                .is_err()
        );
        assert!(
            scale
                .admits(&AssessmentValue::Number { value: f64::NAN })
                .is_err()
        );
    }

    fn form(name: &str) -> Rubric {
        Rubric {
            name: name.into(),
            question: "did the answer help?".into(),
            guidance: String::new(),
            scale: ordinal(),
            direction: MetricDirection::Higher,
        }
    }

    #[test]
    fn a_name_no_reader_could_ask_for_is_refused_when_it_is_published() {
        assert!(form("helpfulness").validate().is_ok());
        let refused = form("team/helpfulness")
            .validate()
            .expect_err("a name is a path segment of the route that serves it");
        assert!(refused.to_string().contains("rubric.name"));
    }

    #[test]
    fn rewriting_the_levels_is_a_different_version_rather_than_the_same_one() {
        let mut rubric = Rubric {
            name: "helpfulness".into(),
            question: "did the answer help?".into(),
            guidance: String::new(),
            scale: ordinal(),
            direction: MetricDirection::Higher,
        };
        let first = rubric.version().expect("a rubric addresses");
        rubric.scale = Scale::Ordinal {
            levels: vec!["good".into(), "fine".into(), "bad".into()],
        };
        let reordered = rubric.version().expect("a rubric addresses");
        assert_ne!(
            first, reordered,
            "order is meaning on an ordinal scale, so reordering is a new rubric"
        );
    }
}
