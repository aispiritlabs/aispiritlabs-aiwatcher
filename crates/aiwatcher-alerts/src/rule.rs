//! What is worth telling somebody about, as a version of a document.
//!
//! ADR_0011's shape, once more: a rule is content, its id is the digest of
//! that content, and re-sending an unchanged rule is not a new version. What
//! it is *not* is a destination — see the crate's docstring for why a webhook
//! URL may not arrive in a request body.
//!
//! One field sits on the head rather than in the version, and the split is the
//! design: `enabled`. Switching a rule off while an incident is being worked
//! on is an operational act, not an edit, and a version per silence would make
//! the dedup key move — so the same occurrence would alert again the moment
//! somebody switched it back on.

use std::collections::BTreeMap;

use aiwatcher_evaluation::GatePolicy;
use serde::{Deserialize, Serialize};

use crate::signal::{AlertSignal, TriggerKind};
use crate::{Result, require, text};

/// The name a rule is addressed by.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct RuleName(String);

impl RuleName {
    /// # Errors
    ///
    /// [`AlertError::Invalid`](crate::AlertError::Invalid) for anything that is
    /// not 1–64 bytes of lowercase letters, digits, `.`, `_` and `-`, beginning
    /// with a letter or a digit.
    pub fn parse(value: &str) -> Result<Self> {
        let ok = (1..=64).contains(&value.len())
            && value.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && value.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            });
        require(
            ok,
            "name",
            "is 1–64 characters of a-z, 0-9, dot, underscore and hyphen, starting with a letter \
             or a digit",
        )?;
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RuleName {
    type Error = crate::AlertError;
    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<RuleName> for String {
    fn from(value: RuleName) -> Self {
        value.0
    }
}

impl std::fmt::Display for RuleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl utoipa::PartialSchema for RuleName {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        String::schema()
    }
}

impl utoipa::ToSchema for RuleName {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("AlertRuleName")
    }
}

/// What a rule fires on.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[schema(as = AlertTrigger)]
pub enum AlertTrigger {
    /// A managed execution that reached a terminal failure.
    ExecutionFailed {
        /// Only executions of this definition. Absent is every definition,
        /// which is the useful default for a deployment running three.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        definition: Option<String>,
    },
    /// A completed, comparable evaluation result the gate calls a regression
    /// against the baseline an operator admitted beside it.
    EvaluationRegressed {
        /// Only results measured in this evaluation context — the content
        /// address of the cohort, the split, the suite and the scorers
        /// together, so narrowing by it is narrowing to one comparable
        /// measurement rather than to a name somebody typed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_id: Option<String>,
        /// How far worse each metric may be, and which cases must hold. The
        /// default holds every metric to no worse at all, which is what a
        /// deployment that has not thought about tolerances means.
        #[serde(default)]
        policy: GatePolicy,
    },
}

impl AlertTrigger {
    #[must_use]
    pub const fn kind(&self) -> TriggerKind {
        match self {
            Self::ExecutionFailed { .. } => TriggerKind::ExecutionFailed,
            Self::EvaluationRegressed { .. } => TriggerKind::EvaluationRegressed,
        }
    }

    /// What this trigger narrows to, if anything.
    #[must_use]
    pub fn narrowed_to(&self) -> Option<&str> {
        match self {
            Self::ExecutionFailed { definition } => definition.as_deref(),
            Self::EvaluationRegressed { context_id, .. } => context_id.as_deref(),
        }
    }

    /// Whether this trigger is about the signal at all.
    ///
    /// Kind first, then the narrowing. A trigger that names something the
    /// signal does not carry does not match: a rule for one definition must
    /// not fire on an execution whose definition nobody could read, because
    /// the narrowing is the operator's way of saying *not the others*.
    #[must_use]
    pub fn matches(&self, signal: &AlertSignal) -> bool {
        self.kind() == signal.trigger
            && match self.narrowed_to() {
                None => true,
                Some(wanted) => signal.within.as_deref() == Some(wanted),
            }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::ExecutionFailed { definition } => {
                if let Some(definition) = definition {
                    text(definition, "trigger.definition")?;
                }
                Ok(())
            }
            Self::EvaluationRegressed { context_id, policy } => {
                if let Some(context_id) = context_id {
                    text(context_id, "trigger.context_id")?;
                }
                policy.validate()?;
                Ok(())
            }
        }
    }
}

/// The document a version is a version of.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AlertRule)]
pub struct AlertRule {
    pub name: RuleName,
    /// Why somebody wants to hear about this. Carried into every delivery,
    /// because the person woken by one did not write the rule.
    pub description: String,
    pub trigger: AlertTrigger,
}

impl AlertRule {
    pub(crate) fn validate(&self) -> Result<()> {
        text(&self.description, "description")?;
        self.trigger.validate()
    }

    /// `sha256` over the whole document, canonically ordered.
    ///
    /// # Errors
    ///
    /// [`AlertError::Encoding`](crate::AlertError::Encoding) when the rule
    /// cannot be written as JSON, which a validated rule cannot be.
    pub fn version_id(&self) -> Result<String> {
        Ok(crate::digest(&crate::canonical(self)?))
    }
}

/// One immutable version of a rule.
#[derive(Clone, Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[schema(as = AlertRuleVersion)]
pub struct RuleVersion {
    pub version_id: String,
    pub rule: AlertRule,
    /// Why this version exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub published_at: i64,
}

/// One version as an index lists it.
#[derive(Clone, Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[schema(as = AlertRuleVersionSummary)]
pub struct RuleVersionSummary {
    pub version_id: String,
    pub trigger: TriggerKind,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub published_at: i64,
}

/// What a name points at now.
#[derive(Clone, Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[schema(as = AlertRuleHead)]
pub struct RuleHead {
    pub name: RuleName,
    /// The version every new occurrence is matched against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// Off raises nothing. On the head and not in a version, so silencing a
    /// rule does not move the dedup key and un-silencing it does not re-send
    /// what it already sent.
    pub enabled: bool,
    /// Newest first, bounded — a rule edited daily for a year is still one
    /// object somebody can read.
    #[serde(default)]
    pub versions: Vec<RuleVersionSummary>,
    /// Labels a deployment may move by hand. Unused by the matcher, kept so a
    /// name like `reviewed` means the same here as in the other registries.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub updated_at: i64,
}

/// How many versions of one rule the head indexes.
pub const MAX_VERSIONS_INDEXED: usize = 100;

impl RuleHead {
    #[must_use]
    pub fn new(name: RuleName, now: i64) -> Self {
        Self {
            name,
            current: None,
            enabled: true,
            versions: Vec::new(),
            labels: BTreeMap::new(),
            updated_at: now,
        }
    }

    pub(crate) fn index(&mut self, version: &RuleVersion, now: i64) {
        self.versions
            .retain(|summary| summary.version_id != version.version_id);
        self.versions.insert(
            0,
            RuleVersionSummary {
                version_id: version.version_id.clone(),
                trigger: version.rule.trigger.kind(),
                description: version.rule.description.clone(),
                author: version.author.clone(),
                published_at: version.published_at,
            },
        );
        self.versions.truncate(MAX_VERSIONS_INDEXED);
        self.current = Some(version.version_id.clone());
        self.updated_at = now;
    }

    /// The summary of the version a rule is at, if the head indexes it.
    #[must_use]
    pub fn current_summary(&self) -> Option<&RuleVersionSummary> {
        let current = self.current.as_deref()?;
        self.versions
            .iter()
            .find(|summary| summary.version_id == current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::AlertSignal;

    fn rule(trigger: AlertTrigger) -> AlertRule {
        AlertRule {
            name: RuleName::parse("nightly").expect("a name"),
            description: "the nightly import must not die quietly".to_owned(),
            trigger,
        }
    }

    fn signal(trigger: TriggerKind, within: Option<&str>) -> AlertSignal {
        AlertSignal {
            trigger,
            subject: "exec-1".to_owned(),
            within: within.map(ToOwned::to_owned),
            occurred_at: 0,
            title: "it failed".to_owned(),
            facts: Vec::new(),
            links: Vec::new(),
        }
    }

    #[test]
    fn a_rule_that_narrows_to_nothing_matches_every_occurrence_of_its_kind() {
        let trigger = AlertTrigger::ExecutionFailed { definition: None };
        assert!(trigger.matches(&signal(TriggerKind::ExecutionFailed, Some("import"))));
        assert!(trigger.matches(&signal(TriggerKind::ExecutionFailed, None)));
        assert!(!trigger.matches(&signal(TriggerKind::EvaluationRegressed, None)));
    }

    #[test]
    fn a_rule_that_narrows_does_not_fire_on_an_occurrence_that_cannot_say() {
        let trigger = AlertTrigger::ExecutionFailed {
            definition: Some("import".to_owned()),
        };
        assert!(trigger.matches(&signal(TriggerKind::ExecutionFailed, Some("import"))));
        assert!(!trigger.matches(&signal(TriggerKind::ExecutionFailed, Some("other"))));
        // The narrowing is the operator saying *not the others*, so an
        // occurrence carrying no definition is one of the others.
        assert!(!trigger.matches(&signal(TriggerKind::ExecutionFailed, None)));
    }

    #[test]
    fn the_same_document_reaches_the_same_version_id() {
        let one = rule(AlertTrigger::ExecutionFailed { definition: None });
        let two = rule(AlertTrigger::ExecutionFailed { definition: None });
        assert_eq!(
            one.version_id().expect("an id"),
            two.version_id().expect("an id")
        );
    }

    #[test]
    fn a_narrowed_rule_is_a_different_version_from_a_broad_one() {
        let broad = rule(AlertTrigger::ExecutionFailed { definition: None });
        let narrow = rule(AlertTrigger::ExecutionFailed {
            definition: Some("import".to_owned()),
        });
        assert_ne!(
            broad.version_id().expect("an id"),
            narrow.version_id().expect("an id")
        );
    }

    #[test]
    fn a_head_indexes_its_newest_version_first_and_points_at_it() {
        let mut head = RuleHead::new(RuleName::parse("nightly").expect("a name"), 0);
        for (id, at) in [("aa", 1), ("bb", 2)] {
            head.index(
                &RuleVersion {
                    version_id: id.to_owned(),
                    rule: rule(AlertTrigger::ExecutionFailed { definition: None }),
                    notes: None,
                    author: None,
                    published_at: at,
                },
                at,
            );
        }
        assert_eq!(head.current.as_deref(), Some("bb"));
        assert_eq!(head.versions[0].version_id, "bb");
        assert_eq!(head.versions.len(), 2);
    }

    #[test]
    fn republishing_one_version_does_not_index_it_twice() {
        let mut head = RuleHead::new(RuleName::parse("nightly").expect("a name"), 0);
        let version = RuleVersion {
            version_id: "aa".to_owned(),
            rule: rule(AlertTrigger::ExecutionFailed { definition: None }),
            notes: None,
            author: None,
            published_at: 1,
        };
        head.index(&version, 1);
        head.index(&version, 2);
        assert_eq!(head.versions.len(), 1);
        assert_eq!(head.updated_at, 2);
    }
}
