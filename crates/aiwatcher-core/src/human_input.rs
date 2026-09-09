//! The rules a question put to a person has to satisfy, wherever it was authored.
//!
//! Two surfaces author one: a curation block on a canvas, and a step of a
//! registered workflow. They compile to the same runtime binding and are
//! answered through the same route, so a question one accepts and the other
//! refuses would be two ideas of what a gate is — and the day they disagree is
//! the day somebody trusts the wrong one. The rules live here, above both, and
//! each surface supplies only the word it calls the thing being refused.
//!
//! What is *not* here is the runtime shape. A binding, its attempt and its
//! answer belong to the execution engine; this is the authored content and the
//! one role that content may name.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// What a gate is answered by when nobody said otherwise.
///
/// Every write inside aiwatcher needs an editor, so that is the floor the
/// answer route holds whatever a question says.
pub const ANSWERABLE_ROLE: &str = "editor";

/// The roles a gate may name, in the order they narrow.
///
/// A gate only ever *raises* the floor. `admin` is honoured — the answer route
/// reads the question's own role and requires it — and a weaker name is
/// refused rather than accepted, because a gate promising that a viewer may
/// answer would put buttons in front of somebody the route is about to refuse.
/// The same shape as a worker queue: it narrows what may be done and never
/// widens it.
pub const ANSWERABLE_ROLES: [&str; 2] = [ANSWERABLE_ROLE, "admin"];

/// What happens to a step whose question nobody answered in time.
///
/// A deadline without one of these would be a clock with nothing behind it, so
/// the two are authored together and refused apart. Which of the three is right
/// is the author's judgement and not this crate's: a gate on a publication
/// fails closed, a gate on an optional enrichment is skipped, and a gate whose
/// answer has an obvious default says so.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum OnTimeout {
    /// The step failed. Everything after it is skipped, as for any failure —
    /// the safe reading of "nobody said yes".
    #[default]
    Fail,
    /// The step is passed over and the chain goes on without it.
    Skip,
    /// Answer it with this, and record that nobody did.
    Answer {
        #[schema(value_type = Value)]
        response: Value,
    },
}

impl OnTimeout {
    /// The word this policy is called by, for a message a person reads.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Skip => "skip",
            Self::Answer { .. } => "answer",
        }
    }
}

/// The longest a gate may be left waiting: a fortnight.
///
/// Not a guess at how long a person takes — that is the author's — but a bound
/// on a row that sits in the timer table until it fires. A gate meant to wait
/// indefinitely says so by naming no deadline at all, which is the default.
pub const MAX_TIMEOUT_SECONDS: u64 = 14 * 24 * 60 * 60;

/// A question somebody reads on a card, not a briefing document.
const MAX_PROMPT_BYTES: usize = 4 * 1024;
/// Answers are buttons. Past a handful they stop being a decision and start
/// being a form.
const MAX_CHOICES: usize = 8;
const MAX_CHOICE_BYTES: usize = 80;

/// Everything wrong with one authored question, all of it at once.
///
/// `subject` is what the caller's own refusals call the thing — a block id on a
/// canvas, a step id in a workflow — so one rule set produces messages that
/// read as if they had been written for each surface.
#[must_use]
pub fn question_problems(
    subject: &str,
    prompt: &str,
    role: &str,
    choices: &[String],
    timeout_seconds: Option<u64>,
    on_timeout: &OnTimeout,
) -> Vec<String> {
    let mut problems = Vec::new();
    problems.extend(deadline_problems(
        subject,
        choices,
        timeout_seconds,
        on_timeout,
    ));
    if prompt.trim().is_empty() {
        problems.push(format!(
            "{subject} does not say what it is asking. A gate whose question is blank is a run \
             stopped for a reason nobody can read"
        ));
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        problems.push(format!(
            "{subject}'s question is {} bytes; the limit is {MAX_PROMPT_BYTES}",
            prompt.len()
        ));
    }
    if !ANSWERABLE_ROLES.contains(&role) {
        problems.push(format!(
            "{subject} asks for the '{role}' role. A gate is answered by an \
             '{ANSWERABLE_ROLE}' or, where a decision needs one, an 'admin' — it raises that \
             floor and never lowers it, so anything else here would promise a check the answer \
             route is not going to make"
        ));
    }
    if choices.len() > MAX_CHOICES {
        problems.push(format!(
            "{subject} offers {} answers; the limit is {MAX_CHOICES}",
            choices.len()
        ));
    }
    if choices
        .iter()
        .any(|choice| choice.trim().is_empty() || choice.len() > MAX_CHOICE_BYTES)
    {
        problems.push(format!(
            "{subject}'s answers are each a word or two, at most {MAX_CHOICE_BYTES} bytes, and \
             none of them blank"
        ));
    }
    // An answer is matched against this list by equality, so two identical
    // entries are two buttons that mean the same thing and one of them can
    // never be the one that was pressed.
    if choices
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != choices.len()
    {
        problems.push(format!("{subject} offers the same answer twice"));
    }
    problems
}

/// What is wrong with a gate's clock and what it does when the clock runs out.
fn deadline_problems(
    subject: &str,
    choices: &[String],
    timeout_seconds: Option<u64>,
    on_timeout: &OnTimeout,
) -> Vec<String> {
    let mut problems = Vec::new();
    match timeout_seconds {
        // The pair is authored together. A policy with no deadline is a rule
        // nothing can reach, which is the shape this repository refuses by name
        // everywhere else rather than storing and ignoring.
        None => {
            if on_timeout != &OnTimeout::default() {
                problems.push(format!(
                    "{subject} says what to do on a timeout and names no deadline, so nothing \
                     would ever do it — give it a timeout, or leave it waiting"
                ));
            }
        }
        Some(0) => problems.push(format!(
            "{subject}'s deadline is zero seconds, which is a question that times out before \
             anybody is shown it. A gate that should not wait is a gate nobody needs"
        )),
        Some(seconds) if seconds > MAX_TIMEOUT_SECONDS => problems.push(format!(
            "{subject} waits {seconds} seconds; the limit is {MAX_TIMEOUT_SECONDS}. A gate \
             meant to wait indefinitely names no deadline at all"
        )),
        Some(_) => {}
    }
    // The answer a timeout writes is an answer, so it is the same answer a
    // person could have given. Anything else would be a run recording a choice
    // the step declares it does not offer.
    if let OnTimeout::Answer { response } = on_timeout
        && !choices.is_empty()
        && !response
            .as_str()
            .is_some_and(|value| choices.iter().any(|choice| choice == value))
    {
        problems.push(format!(
            "{subject} answers its own timeout with something it does not offer. The answer a \
             timeout writes is one of the choices, because that is what an answer to this \
             question is"
        ));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// A question with no clock, which is what most of them are.
    fn waiting(subject: &str, prompt: &str, role: &str, choices: &[String]) -> Vec<String> {
        question_problems(subject, prompt, role, choices, None, &OnTimeout::Fail)
    }

    #[test]
    fn a_question_an_editor_can_answer_from_two_buttons_is_accepted() {
        assert!(
            waiting(
                "sign-off",
                "Publish these rows?",
                ANSWERABLE_ROLE,
                &answers(&["approve", "reject"])
            )
            .is_empty()
        );
    }

    #[test]
    fn a_free_text_question_offers_no_answers_and_is_still_a_question() {
        assert!(waiting("sign-off", "Why?", ANSWERABLE_ROLE, &[]).is_empty());
    }

    #[test]
    fn a_decision_that_needs_an_admin_may_say_so() {
        // Honoured rather than refused, because the answer route reads the
        // question's own role and requires it.
        assert!(waiting("promote", "Promote this model?", "admin", &[]).is_empty());
    }

    #[test]
    fn a_gate_may_not_promise_that_a_viewer_answers_it() {
        // Every write here needs an editor, so a gate saying `viewer` would
        // offer buttons to somebody the route refuses. A gate raises the floor
        // and never lowers it.
        let problems = waiting("sign-off", "Go on?", "viewer", &[]);

        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("asks for the 'viewer' role")),
            "{problems:?}"
        );
    }

    #[test]
    fn every_problem_with_one_question_is_reported_together() {
        let problems = waiting(
            "sign-off",
            "   ",
            "supervisor",
            &answers(&["yes", "yes", "  ", "a", "b", "c", "d", "e", "f"]),
        );

        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("does not say what it is asking")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("asks for the 'supervisor' role")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|problem| problem.contains("9 answers")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("none of them blank")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("the same answer twice")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_deadline_and_what_happens_at_it_are_authored_together() {
        // Either half alone is the shape this repository refuses everywhere
        // else: a clock with nothing behind it, or a rule nothing can reach.
        let orphan = question_problems(
            "sign-off",
            "Go on?",
            ANSWERABLE_ROLE,
            &[],
            None,
            &OnTimeout::Skip,
        );
        assert!(
            orphan
                .iter()
                .any(|problem| problem.contains("names no deadline")),
            "{orphan:?}"
        );
        assert!(
            question_problems(
                "sign-off",
                "Go on?",
                ANSWERABLE_ROLE,
                &[],
                Some(3600),
                &OnTimeout::Skip
            )
            .is_empty()
        );
    }

    #[test]
    fn a_timeout_may_only_write_an_answer_the_question_offers() {
        // What a timeout writes is an answer, so it is one a person could have
        // given. Otherwise a run records a choice the step says it does not
        // offer, and nothing downstream can tell the two apart.
        let problems = question_problems(
            "sign-off",
            "Publish these rows?",
            ANSWERABLE_ROLE,
            &answers(&["approve", "reject"]),
            Some(3600),
            &OnTimeout::Answer {
                response: serde_json::json!("approve with edits"),
            },
        );

        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("does not offer")),
            "{problems:?}"
        );
        assert!(
            question_problems(
                "sign-off",
                "Publish these rows?",
                ANSWERABLE_ROLE,
                &answers(&["approve", "reject"]),
                Some(3600),
                &OnTimeout::Answer {
                    response: serde_json::json!("reject")
                },
            )
            .is_empty()
        );
    }

    #[test]
    fn a_deadline_nobody_could_meet_is_refused() {
        let instant = question_problems(
            "sign-off",
            "Go on?",
            ANSWERABLE_ROLE,
            &[],
            Some(0),
            &OnTimeout::Fail,
        );
        assert!(
            instant
                .iter()
                .any(|problem| problem.contains("before anybody is shown it")),
            "{instant:?}"
        );
        let forever = question_problems(
            "sign-off",
            "Go on?",
            ANSWERABLE_ROLE,
            &[],
            Some(MAX_TIMEOUT_SECONDS + 1),
            &OnTimeout::Fail,
        );
        assert!(
            forever
                .iter()
                .any(|problem| problem.contains("names no deadline at all")),
            "{forever:?}"
        );
    }

    #[test]
    fn the_subject_is_the_word_the_surface_that_asked_uses() {
        // One rule set, two authored surfaces: a canvas block and a workflow
        // step have to be named the way each of them names things.
        assert!(waiting("a-block", "", ANSWERABLE_ROLE, &[])[0].starts_with("a-block "),);
        assert!(waiting("a-step", "", ANSWERABLE_ROLE, &[])[0].starts_with("a-step "));
    }
}
