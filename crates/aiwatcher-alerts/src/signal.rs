//! What a watcher saw, in the words a notification is written from.
//!
//! The half that watches knows about executions and evaluations; this crate
//! does not, and the seam between them is one struct. A watcher resolves
//! everything a reader will need — what happened, to what, what is worth
//! saying about it and where to look — and hands it over already decided.
//!
//! Two properties are the whole reason it is a struct rather than a tuple of
//! strings. `within` is what a rule's optional narrowing is compared against,
//! so this crate can match a rule without knowing that a definition name and a
//! context id are different kinds of thing. And `facts` and `links` are the
//! only free-form fields: a watcher may say what it likes in them, which is
//! also why nothing here reads a credential or an address — what goes out of
//! this process is composed by the half that knows which is which.

use serde::{Deserialize, Serialize};

/// Which watcher raised a signal, and which rules can match it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = AlertTriggerKind)]
pub enum TriggerKind {
    /// A managed execution that reached a terminal failure.
    ExecutionFailed,
    /// A completed, comparable evaluation result the gate calls a regression.
    EvaluationRegressed,
}

impl TriggerKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExecutionFailed => "execution_failed",
            Self::EvaluationRegressed => "evaluation_regressed",
        }
    }
}

impl std::fmt::Display for TriggerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One named thing worth saying, in the order it should be read.
///
/// A list rather than a map, because the order is the watcher's argument: the
/// reason a run failed comes before the definition it ran, and a map sorted by
/// key would put `definition` first every time.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema)]
#[schema(as = AlertFact)]
pub struct AlertFact {
    pub label: String,
    pub value: String,
}

impl AlertFact {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// Where to look, as a path on this instance.
///
/// A path and not a URL, because this process does not reliably know what it
/// is reached by — an ingress rewrites, a port-forward is somebody's laptop —
/// and a notification carrying a link to `localhost:8080` is worse than one
/// carrying a path the reader can paste after the host they already used.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema)]
#[schema(as = AlertLink)]
pub struct AlertLink {
    pub label: String,
    /// Begins with one `/`. Anything else is refused where a signal is built.
    pub path: String,
}

impl AlertLink {
    pub fn new(label: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            path: path.into(),
        }
    }
}

/// Something that happened, before any rule has looked at it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertSignal {
    pub trigger: TriggerKind,
    /// What it happened to: an execution id, an evaluation result id. Half of
    /// the dedup key, and the reason a redelivered occurrence is one alert.
    pub subject: String,
    /// What a rule's optional narrowing is held against — a definition name
    /// for an execution, a context id for a result. `None` matches only a rule
    /// that narrows to nothing.
    pub within: Option<String>,
    /// When the thing happened, not when this was noticed.
    pub occurred_at: i64,
    /// One line, for somebody reading a notification and nothing else.
    pub title: String,
    pub facts: Vec<AlertFact>,
    pub links: Vec<AlertLink>,
}

impl AlertSignal {
    /// What makes this occurrence distinct, before a rule version is mixed in.
    ///
    /// The trigger is part of it so that two watchers cannot collide on one
    /// id: an execution and a result are different id spaces, and nothing
    /// guarantees they stay different from each other.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}:{}", self.trigger, self.subject)
    }
}

/// The name a delivery is stored under: `sha256(rule version ‖ occurrence)`.
///
/// Both halves matter and for different reasons. The occurrence is what makes
/// a redelivered execution failure one alert rather than two. The rule version
/// is what makes *editing a rule* mean something: a tolerance loosened after a
/// regression fired is a different question about the same result, and it is
/// worth telling somebody the new answer. A rule switched off and on again
/// publishes no new version, so it does not re-raise what it already sent.
#[must_use]
pub fn dedup_key(rule_version_id: &str, signal: &AlertSignal) -> String {
    crate::digest(format!("{rule_version_id}\0{}", signal.key()).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(subject: &str) -> AlertSignal {
        AlertSignal {
            trigger: TriggerKind::ExecutionFailed,
            subject: subject.to_owned(),
            within: Some("import".to_owned()),
            occurred_at: 0,
            title: "import failed".to_owned(),
            facts: Vec::new(),
            links: Vec::new(),
        }
    }

    #[test]
    fn the_same_occurrence_under_the_same_rule_version_is_one_key() {
        assert_eq!(
            dedup_key("v1", &signal("exec-1")),
            dedup_key("v1", &signal("exec-1"))
        );
    }

    #[test]
    fn another_occurrence_and_another_rule_version_are_each_a_new_key() {
        let one = dedup_key("v1", &signal("exec-1"));
        assert_ne!(one, dedup_key("v1", &signal("exec-2")));
        assert_ne!(one, dedup_key("v2", &signal("exec-1")));
    }

    #[test]
    fn two_watchers_cannot_collide_on_one_id() {
        let mut execution = signal("shared-id");
        execution.trigger = TriggerKind::ExecutionFailed;
        let mut evaluation = signal("shared-id");
        evaluation.trigger = TriggerKind::EvaluationRegressed;
        assert_ne!(dedup_key("v1", &execution), dedup_key("v1", &evaluation));
    }
}
