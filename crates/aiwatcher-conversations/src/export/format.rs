//! The training shapes, derived from the conversation rather than stored as
//! one.
//!
//! **No training library is named here and none is imported.** Every format is
//! JSONL over plain fields, because the alternative — emitting whatever the
//! current favourite tokeniser wants — is a decision that expires. The shapes
//! themselves are the stable part: a chat transcript, a prompt and a response,
//! a completion with its context, a preference pair. Four renderings of the
//! same turns, and none of them recoverable from any other, which is why the
//! archive keeps the conversation and the export chooses the shape.
//!
//! Every row carries the fields that prove it was eligible — the turn's id and
//! digest, the consent basis and reference, who approved it and when. An
//! auditor holding one row and no other access can answer "why was this in the
//! corpus", which is the acceptance criterion this whole workstream exists for.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use utoipa::ToSchema;

use crate::archive::ArchivedTurn;
use crate::turn::{ContentPart, Role, TurnContent};

/// Which of two answers a reviewer preferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreferenceLabel {
    Chosen,
    Rejected,
}

impl PreferenceLabel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chosen => "chosen",
            Self::Rejected => "rejected",
        }
    }
}

/// The shape a row is written in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// One row per conversation: `{ messages: [{role, content}, …] }`.
    ///
    /// The default, and the only lossless one — every other shape here throws
    /// something away, and this is the one an unforeseen task can be rebuilt
    /// from.
    #[default]
    Chat,
    /// One row per assistant turn: `{ prompt, response }`, where the prompt is
    /// the user turn that preceded it. The shape a small model is fine-tuned
    /// on, and the one that loses tool use entirely.
    PromptResponse,
    /// One row per assistant turn: the full preceding context as `messages`,
    /// plus `completion`. Supervised fine-tuning, with the history kept.
    Sft,
    /// One row per preference pair: shared context, `chosen` and `rejected`.
    ///
    /// Built only from sibling assistant turns a reviewer explicitly labelled —
    /// see [`crate::review::TurnReview::preference`]. Nothing is inferred from
    /// a rejection, because a rejection has several reasons and only one of
    /// them is "the other answer was better".
    Dpo,
}

impl ExportFormat {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::PromptResponse => "prompt_response",
            Self::Sft => "sft",
            Self::Dpo => "dpo",
        }
    }
}

/// One turn, with the content the export decrypted for it.
#[derive(Debug)]
pub struct Selected<'a> {
    pub turn: &'a ArchivedTurn,
    pub content: &'a TurnContent,
}

impl Selected<'_> {
    /// The plain text of the message, tool calls rendered as JSON.
    #[must_use]
    pub fn text(&self) -> String {
        let mut rendered = String::new();
        for part in &self.content.parts {
            match part {
                ContentPart::Text { text } => push(&mut rendered, text),
                // Reasoning is withheld from every shape. A provider's chain of
                // thought is not the answer, and a corpus that mixes the two
                // teaches a model to emit its scratchpad.
                ContentPart::Reasoning { .. } => {}
                ContentPart::ToolCall {
                    name, arguments, ..
                } => push(&mut rendered, &format!("{name}({arguments})")),
                ContentPart::Redacted { reason, .. } => {
                    push(&mut rendered, &format!("[redacted: {reason}]"));
                }
                ContentPart::Reference { uri, .. } => push(&mut rendered, &format!("[{uri}]")),
            }
        }
        rendered
    }

    fn message(&self) -> Value {
        let mut message = Map::new();
        message.insert("role".to_owned(), json!(self.turn.role.as_str()));
        message.insert("content".to_owned(), json!(self.text()));
        if !self.content.tool_results.is_empty() {
            message.insert(
                "tool_results".to_owned(),
                json!(
                    self.content
                        .tool_results
                        .iter()
                        .map(|result| json!({
                            "call_id": result.call_id,
                            "name": result.name,
                            "ok": result.ok,
                            "content": result.content,
                        }))
                        .collect::<Vec<_>>()
                ),
            );
        }
        Value::Object(message)
    }

    /// Why this row was allowed into the corpus.
    fn eligibility(&self) -> Value {
        let turn = self.turn;
        json!({
            "turn_id": turn.turn_id,
            "content_digest": turn.content_digest,
            "conversation_id": turn.conversation_id,
            "message_id": turn.message_id,
            "consent_basis": turn.policy.consent.basis.as_str(),
            "consent_subject": turn.policy.consent.subject,
            "consent_reference": turn.policy.consent.reference,
            "consent_scope": turn.policy.consent.scope
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>(),
            "retention_days": turn.policy.retention.ttl_days,
            "redactor": turn.policy.redaction.as_ref().map(|record| record.redactor.clone()),
            "reviewer": turn.review.reviewer,
            "reviewed_at": turn.review.decided_at.map(|at| at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()),
            "run_id": turn.provenance.run_id,
            "model": turn.provenance.model,
            "prompt": turn.provenance.prompt,
        })
    }
}

fn push(rendered: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    if !rendered.is_empty() {
        rendered.push_str("\n\n");
    }
    rendered.push_str(text);
}

/// Turn one conversation's selected turns into the rows this format asks for.
///
/// Returns whole rows: a format that produces none for a given conversation —
/// a preference export over a conversation nobody labelled — contributes
/// nothing rather than an empty row.
#[must_use]
pub fn rows(format: ExportFormat, conversation_id: &str, selected: &[Selected<'_>]) -> Vec<Value> {
    match format {
        ExportFormat::Chat => chat(conversation_id, selected),
        ExportFormat::PromptResponse => prompt_response(selected),
        ExportFormat::Sft => sft(selected),
        ExportFormat::Dpo => preference(selected),
    }
}

fn chat(conversation_id: &str, selected: &[Selected<'_>]) -> Vec<Value> {
    if selected.is_empty() {
        return Vec::new();
    }
    vec![json!({
        "conversation_id": conversation_id,
        "messages": selected.iter().map(Selected::message).collect::<Vec<_>>(),
        "eligibility": selected.iter().map(Selected::eligibility).collect::<Vec<_>>(),
    })]
}

fn prompt_response(selected: &[Selected<'_>]) -> Vec<Value> {
    let mut rows = Vec::new();
    for (index, turn) in selected.iter().enumerate() {
        if turn.turn.role != Role::Assistant {
            continue;
        }
        // The nearest preceding user turn, rather than the immediately
        // preceding one: a tool result between the question and the answer is
        // common, and taking it as the prompt produces a corpus of a model
        // answering its own tool output.
        let Some(prompt) = selected[..index]
            .iter()
            .rev()
            .find(|candidate| candidate.turn.role == Role::User)
        else {
            continue;
        };
        rows.push(json!({
            "prompt": prompt.text(),
            "response": turn.text(),
            "eligibility": [prompt.eligibility(), turn.eligibility()],
        }));
    }
    rows
}

fn sft(selected: &[Selected<'_>]) -> Vec<Value> {
    let mut rows = Vec::new();
    for (index, turn) in selected.iter().enumerate() {
        if turn.turn.role != Role::Assistant {
            continue;
        }
        let context: Vec<Value> = selected[..index].iter().map(Selected::message).collect();
        if context.is_empty() {
            continue;
        }
        rows.push(json!({
            "messages": context,
            "completion": turn.text(),
            "eligibility": turn.eligibility(),
        }));
    }
    rows
}

/// Sibling assistant turns a reviewer labelled, paired.
///
/// Siblings means "the same `parent_message_id`", which is what makes two
/// answers comparable: they were produced in response to the same thing. Two
/// turns from different points in a conversation are two answers to two
/// questions, and a preference between them is not a preference.
fn preference(selected: &[Selected<'_>]) -> Vec<Value> {
    use std::collections::BTreeMap;

    let mut families: BTreeMap<&str, Vec<&Selected<'_>>> = BTreeMap::new();
    for turn in selected {
        if turn.turn.role != Role::Assistant {
            continue;
        }
        if turn.turn.review.preference.is_none() {
            continue;
        }
        let parent = turn.turn.parent_message_id.as_deref().unwrap_or("");
        families.entry(parent).or_default().push(turn);
    }

    let mut rows = Vec::new();
    for (parent, siblings) in families {
        let chosen: Vec<_> = siblings
            .iter()
            .filter(|turn| turn.turn.review.preference == Some(PreferenceLabel::Chosen))
            .collect();
        let rejected: Vec<_> = siblings
            .iter()
            .filter(|turn| turn.turn.review.preference == Some(PreferenceLabel::Rejected))
            .collect();
        // Every chosen against every rejected. Two of each is four pairs and
        // that is correct: each is an independent comparison somebody made.
        for chosen in &chosen {
            for rejected in &rejected {
                let context: Vec<Value> = selected
                    .iter()
                    .take_while(|candidate| candidate.turn.message_id != chosen.turn.message_id)
                    .filter(|candidate| candidate.turn.role != Role::Assistant)
                    .map(Selected::message)
                    .collect();
                rows.push(json!({
                    "parent_message_id": parent,
                    "messages": context,
                    "chosen": chosen.text(),
                    "rejected": rejected.text(),
                    "eligibility": [chosen.eligibility(), rejected.eligibility()],
                }));
            }
        }
    }
    rows
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::policy::ContentPolicy;
    use crate::review::{ReviewState, TurnReview};
    use crate::turn::{Provenance, TurnState};
    use time::OffsetDateTime;

    fn turn(message_id: &str, parent: Option<&str>, role: Role) -> ArchivedTurn {
        ArchivedTurn {
            turn_id: crate::turn::turn_id("c", message_id),
            conversation_id: "c".to_owned(),
            message_id: message_id.to_owned(),
            parent_message_id: parent.map(str::to_owned),
            ordinal: 0,
            role,
            content_digest: crate::digest(message_id.as_bytes()),
            content_bytes: 0,
            parts: Vec::new(),
            tool_results: 0,
            provenance: Provenance::default(),
            policy: ContentPolicy::default(),
            retention_clamped: false,
            findings: Vec::new(),
            review: TurnReview {
                state: ReviewState::Approved,
                ..TurnReview::default()
            },
            state: TurnState::Held,
            erasure: None,
            received_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid"),
            occurred_at: None,
            expires_at: None,
            index_shard: Some(0),
        }
    }

    fn content(text: &str) -> TurnContent {
        TurnContent {
            parts: vec![ContentPart::Text {
                text: text.to_owned(),
            }],
            tool_results: Vec::new(),
        }
    }

    #[test]
    fn a_chat_row_is_the_whole_conversation_and_loses_nothing() {
        let user = turn("m1", None, Role::User);
        let assistant = turn("m2", Some("m1"), Role::Assistant);
        let (question, answer) = (content("what is it?"), content("it is this"));
        let selected = vec![
            Selected {
                turn: &user,
                content: &question,
            },
            Selected {
                turn: &assistant,
                content: &answer,
            },
        ];
        let rows = rows(ExportFormat::Chat, "c", &selected);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["messages"].as_array().expect("array").len(), 2);
        assert_eq!(rows[0]["messages"][0]["role"], "user");
        // And the audit trail rides along, one entry per turn.
        assert_eq!(rows[0]["eligibility"].as_array().expect("array").len(), 2);
    }

    #[test]
    fn a_prompt_response_row_reaches_past_a_tool_turn_for_its_question() {
        // The failure this avoids: taking the immediately preceding turn makes
        // a corpus of a model answering its own tool output.
        let user = turn("m1", None, Role::User);
        let tool = turn("m2", Some("m1"), Role::Tool);
        let assistant = turn("m3", Some("m2"), Role::Assistant);
        let (question, output, answer) = (
            content("what is the weather?"),
            content("{\"temp\": 9}"),
            content("nine degrees"),
        );
        let selected = vec![
            Selected {
                turn: &user,
                content: &question,
            },
            Selected {
                turn: &tool,
                content: &output,
            },
            Selected {
                turn: &assistant,
                content: &answer,
            },
        ];
        let rows = rows(ExportFormat::PromptResponse, "c", &selected);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["prompt"], "what is the weather?");
        assert_eq!(rows[0]["response"], "nine degrees");
    }

    #[test]
    fn reasoning_never_reaches_a_training_row() {
        let assistant = turn("m2", Some("m1"), Role::Assistant);
        let content = TurnContent {
            parts: vec![
                ContentPart::Reasoning {
                    text: "the user probably means Berlin".to_owned(),
                },
                ContentPart::Text {
                    text: "nine degrees".to_owned(),
                },
            ],
            tool_results: Vec::new(),
        };
        let selected = Selected {
            turn: &assistant,
            content: &content,
        };
        assert_eq!(selected.text(), "nine degrees");
    }

    #[test]
    fn a_preference_pair_needs_two_siblings_a_reviewer_actually_labelled() {
        let user = turn("m1", None, Role::User);
        let mut good = turn("m2", Some("m1"), Role::Assistant);
        let mut bad = turn("m3", Some("m1"), Role::Assistant);
        good.review.preference = Some(PreferenceLabel::Chosen);
        bad.review.preference = Some(PreferenceLabel::Rejected);
        let (question, better, worse) = (content("q"), content("better"), content("worse"));
        let selected = vec![
            Selected {
                turn: &user,
                content: &question,
            },
            Selected {
                turn: &good,
                content: &better,
            },
            Selected {
                turn: &bad,
                content: &worse,
            },
        ];
        let rows = rows(ExportFormat::Dpo, "c", &selected);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["chosen"], "better");
        assert_eq!(rows[0]["rejected"], "worse");
        // The context is the question, not the sibling answers.
        assert_eq!(rows[0]["messages"].as_array().expect("array").len(), 1);
    }

    #[test]
    fn nothing_is_inferred_into_a_preference_pair() {
        // Two sibling answers nobody labelled produce no row at all. The
        // alternative — reading a review rejection as "worse answer" — is how a
        // turn rejected for holding an address becomes the rejected half of a
        // preference pair and puts that address in the corpus.
        let user = turn("m1", None, Role::User);
        let first = turn("m2", Some("m1"), Role::Assistant);
        let mut second = turn("m3", Some("m1"), Role::Assistant);
        second.review.state = ReviewState::Rejected;
        let (question, a, b) = (content("q"), content("a"), content("b"));
        let selected = vec![
            Selected {
                turn: &user,
                content: &question,
            },
            Selected {
                turn: &first,
                content: &a,
            },
            Selected {
                turn: &second,
                content: &b,
            },
        ];
        assert!(rows(ExportFormat::Dpo, "c", &selected).is_empty());
    }

    #[test]
    fn two_answers_to_two_different_questions_are_not_a_preference() {
        let mut first = turn("m2", Some("m1"), Role::Assistant);
        let mut second = turn("m4", Some("m3"), Role::Assistant);
        first.review.preference = Some(PreferenceLabel::Chosen);
        second.review.preference = Some(PreferenceLabel::Rejected);
        let (a, b) = (content("a"), content("b"));
        let selected = vec![
            Selected {
                turn: &first,
                content: &a,
            },
            Selected {
                turn: &second,
                content: &b,
            },
        ];
        assert!(rows(ExportFormat::Dpo, "c", &selected).is_empty());
    }

    #[test]
    fn every_row_carries_the_reason_it_was_eligible() {
        // The acceptance criterion, as a test: an auditor with one row and no
        // other access can answer "why was this in the corpus".
        let mut user = turn("m1", None, Role::User);
        user.policy.consent.subject = "tenant-17".to_owned();
        user.policy.consent.reference = "ticket-4102".to_owned();
        user.review.reviewer = "ada@example.com".to_owned();
        let assistant = turn("m2", Some("m1"), Role::Assistant);
        let (question, answer) = (content("q"), content("a"));
        let selected = vec![
            Selected {
                turn: &user,
                content: &question,
            },
            Selected {
                turn: &assistant,
                content: &answer,
            },
        ];
        let rows = rows(ExportFormat::Sft, "c", &selected);
        assert_eq!(rows.len(), 1);
        let eligibility = &rows[0]["eligibility"];
        assert_eq!(eligibility["turn_id"], assistant.turn_id);
        assert!(eligibility["content_digest"].is_string());
        assert!(eligibility["consent_basis"].is_string());
    }
}
