//! What may be done with an image, and therefore with a model trained on it.
//!
//! One module, because this is one question, and it used to be answered in
//! three files: [`UsageRights`] and [`RightsPolicy`] sat with the image record,
//! [`SourceUsage`] with the corpus table, and the rule connecting them with the
//! bulk import. A reader wanting to know "what stops us training a commercial
//! model on CC BY-NC data" had to find all three and notice they were related.
//!
//! The three types answer three different questions and the distinction is
//! load-bearing:
//!
//! * [`UsageRights`] — what somebody **asserted** about one image. A claim,
//!   with a person behind it.
//! * [`RightsPolicy`] — what an export **demands** of the images it includes.
//! * [`SourceUsage`] — what a human **recorded**, at the original, on a date,
//!   about a whole published corpus. The only one of the three that outranks a
//!   caller.
//!
//! Every default here leans the same way: [`RightsPolicy::Commercial`] is the
//! default because the strict answer is the free one, and
//! [`UsageRights::Unknown`] is the default because a claim nobody made should
//! not read as a claim. The failure that follows is a smaller export with a
//! line in its manifest saying what it dropped — not a model trained on
//! somebody else's non-commercial data. See ADR_0017 and ADR_0019.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What may be done with an image, and therefore with a model trained on it.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UsageRights {
    /// Produced here, or supplied with an explicit grant covering training,
    /// derived artifacts and the resulting weights.
    Owned {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        grant: String,
    },
    /// Licensed under terms that permit commercial use.
    Licensed {
        license: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// Licensed for research only — CC BY-NC and everything shaped like it.
    ResearchOnly {
        license: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// Nobody has checked. Usable for an experiment, excluded from anything
    /// that claims a policy.
    Unknown,
}

impl UsageRights {
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Owned { .. } => "owned".to_owned(),
            Self::Licensed { license, .. } => license.clone(),
            Self::ResearchOnly { license, .. } => format!("{license} (research only)"),
            Self::Unknown => "unknown".to_owned(),
        }
    }

    /// Whether an export declaring `policy` may include this image.
    #[must_use]
    pub const fn allows(&self, policy: RightsPolicy) -> bool {
        match (self, policy) {
            (_, RightsPolicy::Any) => true,
            (Self::Owned { .. } | Self::Licensed { .. }, _) => true,
            (Self::ResearchOnly { .. }, RightsPolicy::Research) => true,
            (Self::ResearchOnly { .. } | Self::Unknown, RightsPolicy::Commercial) => false,
            (Self::Unknown, RightsPolicy::Research) => false,
        }
    }
}

/// What an export claims about itself.
///
/// `Commercial` is the default, because the failure this guards against is
/// silent and the correction is one field. An export that wants CubiCasa5K in
/// it has to say `research` and will say so in its manifest forever.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RightsPolicy {
    #[default]
    Commercial,
    Research,
    /// Everything, including images nobody has checked. For an experiment whose
    /// weights are thrown away.
    Any,
}

impl RightsPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commercial => "commercial",
            Self::Research => "research",
            Self::Any => "any",
        }
    }
}

/// What the licence permits, stated conservatively.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceUsage {
    /// The licence permits commercial use of the data and of a model trained
    /// on it.
    Commercial,
    /// Research or non-commercial only.
    NonCommercial,
    /// Mixed, unstated, or stated by the authors as not theirs to give.
    ///
    /// The default, and deliberately: a row that says nothing about its
    /// licence must not read as one that permits anything.
    #[default]
    Unclear,
}

impl SourceUsage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commercial => "commercial",
            Self::NonCommercial => "non_commercial",
            Self::Unclear => "unclear",
        }
    }
}

/// Whether the caller's claim about the licence is one this import may accept.
///
/// # Errors
/// When the batch matched a curated corpus whose licence is research-only and
/// the caller claimed terms better than that. The message names the curated
/// row, because the fix is to read it rather than to try a different wording.
pub fn check_rights(
    rights: &UsageRights,
    curated: Option<SourceUsage>,
    corpus: Option<&str>,
) -> Result<(), String> {
    let claims_commercial = matches!(
        rights,
        UsageRights::Owned { .. } | UsageRights::Licensed { .. }
    );
    if claims_commercial && curated == Some(SourceUsage::NonCommercial) {
        let name = corpus.unwrap_or("a known corpus");
        return Err(format!(
            "this matches {name}, whose licence was read at the original and is research-only; \
             importing it as commercially usable would put it in an export that claims it is. \
             Import it as research_only, or as unknown if the match is wrong."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_research_only_corpus_may_not_be_imported_as_commercially_usable() {
        let refused = check_rights(
            &UsageRights::Licensed {
                license: "MIT".to_owned(),
                url: None,
            },
            Some(SourceUsage::NonCommercial),
            Some("cubicasa5k"),
        );
        let message = refused.expect_err("the curated verdict wins over the caller");
        assert!(message.contains("cubicasa5k"));
        assert!(message.contains("research-only"));
    }

    #[test]
    fn the_same_corpus_may_be_imported_for_what_it_actually_permits() {
        assert!(
            check_rights(
                &UsageRights::ResearchOnly {
                    license: "CC BY-NC 4.0".to_owned(),
                    url: None,
                },
                Some(SourceUsage::NonCommercial),
                Some("cubicasa5k"),
            )
            .is_ok()
        );
        assert!(
            check_rights(
                &UsageRights::Unknown,
                Some(SourceUsage::NonCommercial),
                Some("cubicasa5k"),
            )
            .is_ok()
        );
    }

    #[test]
    fn an_uncurated_row_is_the_callers_call_because_nobody_else_has_read_it() {
        // Not a licence check aiwatcher can perform: there is no curated row
        // to contradict. What it does instead is record who said so and warn
        // that nobody verified it.
        assert!(
            check_rights(
                &UsageRights::Licensed {
                    license: "CC0".to_owned(),
                    url: None
                },
                None,
                None,
            )
            .is_ok()
        );
    }
}
