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

/// The one role a gate may name, because it is the one the answer route checks.
///
/// The route that carries an answer requires the editor role and reads nothing
/// stricter, so a gate naming any other role would describe a check nobody
/// makes — and somebody offered the buttons under it would press one and be
/// refused. Refused by name rather than silently widened; the day the answer
/// route reads the request's own role, this is the constant that stops being
/// the whole answer.
pub const ANSWERABLE_ROLE: &str = "editor";

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
) -> Vec<String> {
    let mut problems = Vec::new();
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
    if role != ANSWERABLE_ROLE {
        problems.push(format!(
            "{subject} asks for the '{role}' role. The route that carries an answer requires \
             '{ANSWERABLE_ROLE}' and reads no other, so any other role here would describe a \
             check nobody makes"
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

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn a_question_an_editor_can_answer_from_two_buttons_is_accepted() {
        assert!(
            question_problems(
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
        assert!(question_problems("sign-off", "Why?", ANSWERABLE_ROLE, &[]).is_empty());
    }

    #[test]
    fn every_problem_with_one_question_is_reported_together() {
        let problems = question_problems(
            "sign-off",
            "   ",
            "admin",
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
                .any(|problem| problem.contains("asks for the 'admin' role")),
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
    fn the_subject_is_the_word_the_surface_that_asked_uses() {
        // One rule set, two authored surfaces: a canvas block and a workflow
        // step have to be named the way each of them names things.
        assert!(question_problems("a-block", "", ANSWERABLE_ROLE, &[])[0].starts_with("a-block "),);
        assert!(question_problems("a-step", "", ANSWERABLE_ROLE, &[])[0].starts_with("a-step "));
    }
}
