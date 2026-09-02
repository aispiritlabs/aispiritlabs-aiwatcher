//! The contract: what one message in a conversation is, on the wire.
//!
//! Deliberately not span attributes. The thing this replaced put `input` and
//! `output` on `llm.completed`, which cannot say who spoke, which message
//! answered which, what the tool actually returned, or on whose consent any of
//! it was kept — and it put all of it in the durable log. Two flat strings are
//! not a conversation; they are the shape a training script wants after
//! somebody has already decided what to include.
//!
//! So the wire form is the *conversation*, and the training shape is derived
//! from it by an export ([`crate::export::format`]). That direction is the one
//! that survives a second task: a preference pair and a chat transcript are two
//! renderings of the same turns, and neither can be recovered from the other.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::policy::ContentPolicy;
use crate::redaction::Finding;
use crate::review::TurnReview;
use crate::{Error, MAX_CONTENT_BYTES, MAX_PARTS, MAX_TOOL_RESULTS, Result, digest, validate_name};

/// Who spoke.
///
/// `Developer` is separate from `System` because the two have different
/// retention answers: a developer instruction is the product's own text and a
/// system prompt may be a customer's. An export can drop one and keep the
/// other, which it could not do if they were one role.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
// An OpenAPI components block is one global namespace and `Role` is already
// the caller's role. Renamed there rather than here, because inside this
// crate `Role` is exactly what it is.
#[schema(as = ConversationRole)]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

impl Role {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Developer => "developer",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One piece of a message.
///
/// A list rather than a string, because a modern turn is not one: an assistant
/// message can be reasoning, then a tool call, then prose, and a training shape
/// that has to guess which part was which guesses wrong on the interesting
/// examples.
///
/// [`Self::Reference`] is the guardrail the annotation registry already keeps
/// for artifacts: a pointer to bytes somebody else stored, never the bytes. An
/// image or a PDF inlined here would put a file in an archive that is meant to
/// hold messages, and would put it under an encryption key sized for text.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ContentPart {
    Text {
        text: String,
    },
    /// The model's own reasoning, where a provider exposes it. Its own variant
    /// because most training shapes must *not* include it, and a `Text` part
    /// carrying it would be indistinguishable from the answer.
    Reasoning {
        text: String,
    },
    /// A call the assistant asked for. `arguments` is the JSON the model
    /// produced, kept verbatim: a re-serialised copy is a different string and
    /// would break the content digest for no reason.
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// What a producer removed before sending. The digest is of the original,
    /// so a reviewer can tell two different redactions apart, and an export can
    /// exclude a turn whose redaction ate the answer.
    Redacted {
        reason: String,
        original_bytes: usize,
        original_digest: String,
    },
    /// Bytes somebody else stored. Never the bytes themselves.
    Reference {
        uri: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        media_type: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        digest: String,
    },
}

impl ContentPart {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Reasoning { .. } => "reasoning",
            Self::ToolCall { .. } => "tool_call",
            Self::Redacted { .. } => "redacted",
            Self::Reference { .. } => "reference",
        }
    }

    /// The characters a scanner reads and an export renders. `None` for a part
    /// that carries no prose.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text } | Self::Reasoning { text } => Some(text),
            Self::ToolCall { arguments, .. } => Some(arguments),
            Self::Redacted { .. } | Self::Reference { .. } => None,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Self::Text { text } | Self::Reasoning { text } => text.len(),
            Self::ToolCall {
                call_id,
                name,
                arguments,
            } => call_id.len() + name.len() + arguments.len(),
            Self::Redacted { reason, .. } => reason.len(),
            Self::Reference {
                uri,
                media_type,
                digest,
            } => uri.len() + media_type.len() + digest.len(),
        }
    }
}

/// What a tool handed back.
///
/// Attached to the turn rather than folded into a `Text` part, because a
/// failure is a training signal and a stringified error is not: an SFT shape
/// wants the successful ones, a preference shape often wants exactly the pairs
/// where one branch failed.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub ok: bool,
    pub content: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// Where this turn came from, in the telemetry that is still on the log.
///
/// Every field is a join back. The archive expires on its own clock, and when
/// it does these ids are what is left — which is the point of keeping them out
/// of the encrypted half. "This run used a model that answered badly" stays
/// answerable after the words are gone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct Provenance {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub trace_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub span_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub call_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// The prompt version this turn ran on, where the producer knows it —
    /// `planner.floor-plan.system@sha256`. The join ADR_0011 exists for.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
}

/// The half that is encrypted.
///
/// A type of its own rather than two fields on the head, because the split is
/// the security boundary: everything in here is sealed, everything outside it
/// is readable by anyone who may list the archive, and a field that moved
/// across the line by accident would be a leak nobody could see in a diff.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct TurnContent {
    #[serde(default)]
    pub parts: Vec<ContentPart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolResult>,
}

impl TurnContent {
    /// The canonical bytes a content digest is taken over.
    ///
    /// `serde_json` with `BTreeMap`-free structs is already field-ordered, so
    /// the serialisation is stable across builds — which is what makes the same
    /// turn sent twice one row rather than two, and what makes an export
    /// version reproducible.
    fn canonical(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    #[must_use]
    pub fn digest(&self) -> String {
        digest(&self.canonical())
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.parts.iter().map(ContentPart::bytes).sum::<usize>()
            + self
                .tool_results
                .iter()
                .map(|result| result.content.len() + result.error.len())
                .sum::<usize>()
    }

    /// What the head keeps: shape without words.
    #[must_use]
    pub fn summarise(&self) -> Vec<PartSummary> {
        self.parts
            .iter()
            .map(|part| PartSummary {
                kind: part.kind().to_owned(),
                bytes: part.bytes(),
            })
            .collect()
    }
}

/// One part's shape, with none of its content.
///
/// What makes a tombstoned turn still legible: "an assistant message of three
/// parts, 4 kB of text and a tool call" is an auditable fact that survives the
/// erasure of the words.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct PartSummary {
    pub kind: String,
    pub bytes: usize,
}

/// Whether the content is still there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    /// Content is in the archive, sealed.
    #[default]
    Held,
    /// Content is gone: the retention clock ran out, or somebody asked. The
    /// head remains, which is what lets an export explain a row it can no
    /// longer produce.
    Erased,
}

impl TurnState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Erased => "erased",
        }
    }
}

/// What a producer sends.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordTurnRequest {
    /// The same `conversation_id` the telemetry carries. This is the join, and
    /// a turn that invents one is a turn nothing can be traced back from.
    pub conversation_id: String,
    /// The producer's stable id for this message. Sending it twice replaces the
    /// turn rather than adding one — a retried flush must not double a corpus.
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_message_id: Option<String>,
    /// Position among siblings. Ties are broken by `message_id`, so a producer
    /// that sends every turn at ordinal 0 still gets a deterministic order —
    /// just not the one it meant.
    #[serde(default)]
    pub ordinal: u32,
    pub role: Role,
    pub content: TurnContent,
    #[serde(default)]
    pub provenance: Provenance,
    /// Consent, retention and what the producer says it redacted. Required in
    /// a protected deployment; recorded as `unknown` in an open one, which an
    /// export then excludes by name.
    #[serde(default)]
    pub policy: ContentPolicy,
    /// When the message was said, rather than when it arrived. Ordering within
    /// a conversation uses arrival, because a producer's clock is not something
    /// an export's determinism can depend on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    #[serde(with = "time::serde::rfc3339::option")]
    pub occurred_at: Option<OffsetDateTime>,
}

impl RecordTurnRequest {
    /// # Errors
    ///
    /// [`Error::Invalid`] for a malformed id, [`Error::TooLarge`] for content
    /// past the archive's caps.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.conversation_id, "conversation_id")?;
        validate_name(&self.message_id, "message_id")?;
        if let Some(parent) = &self.parent_message_id {
            validate_name(parent, "parent_message_id")?;
            if parent == &self.message_id {
                return Err(Error::Invalid("a turn cannot be its own parent".to_owned()));
            }
        }
        if self.content.parts.len() > MAX_PARTS {
            return Err(Error::TooLarge {
                what: "the turn's content parts",
                size: self.content.parts.len(),
                limit: MAX_PARTS,
            });
        }
        if self.content.tool_results.len() > MAX_TOOL_RESULTS {
            return Err(Error::TooLarge {
                what: "the turn's tool results",
                size: self.content.tool_results.len(),
                limit: MAX_TOOL_RESULTS,
            });
        }
        let bytes = self.content.bytes();
        if bytes > MAX_CONTENT_BYTES {
            return Err(Error::TooLarge {
                what: "the turn's content",
                size: bytes,
                limit: MAX_CONTENT_BYTES,
            });
        }
        for part in &self.content.parts {
            if let ContentPart::Redacted {
                original_digest, ..
            } = part
                && !original_digest.is_empty()
            {
                crate::validate_digest(original_digest, "a redacted part's original_digest")?;
            }
        }
        Ok(())
    }

    /// The archive's identity for this message.
    ///
    /// A function of the conversation and the message id and nothing else, so
    /// a producer that retries lands on the turn it already wrote. Hashed
    /// rather than concatenated because both halves come from a producer and
    /// either may hold a slash.
    #[must_use]
    pub fn turn_id(&self) -> String {
        turn_id(&self.conversation_id, &self.message_id)
    }
}

/// `sha256(conversation_id \0 message_id)`.
#[must_use]
pub fn turn_id(conversation_id: &str, message_id: &str) -> String {
    let mut bytes = Vec::with_capacity(conversation_id.len() + message_id.len() + 1);
    bytes.extend_from_slice(conversation_id.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(message_id.as_bytes());
    digest(&bytes)
}

/// What a write answers with.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RecordedTurn {
    pub turn_id: String,
    pub content_digest: String,
    /// False when this exact content was already stored under this turn id.
    pub created: bool,
    /// What the server's own scan found, whatever the producer claimed. Empty
    /// is the common case and the one worth not celebrating: a clean scan is
    /// the absence of a match, not evidence there is nothing to find.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    pub review: TurnReview,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub expires_at: Option<OffsetDateTime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> ContentPart {
        ContentPart::Text {
            text: value.to_owned(),
        }
    }

    #[test]
    fn a_turn_id_is_a_function_of_the_conversation_and_the_message() {
        assert_eq!(turn_id("c", "m"), turn_id("c", "m"));
        assert_ne!(turn_id("c", "m"), turn_id("c", "n"));
        // And the separator is what stops two different pairs colliding: a
        // plain concatenation would give ("ab", "c") and ("a", "bc") one id,
        // which is one message overwriting another.
        assert_ne!(turn_id("ab", "c"), turn_id("a", "bc"));
    }

    #[test]
    fn the_same_content_digests_the_same_however_it_was_built() {
        let first = TurnContent {
            parts: vec![text("hello"), text("world")],
            tool_results: Vec::new(),
        };
        let second = TurnContent {
            parts: vec![text("hello"), text("world")],
            tool_results: Vec::new(),
        };
        assert_eq!(first.digest(), second.digest());

        // Order is content: two parts swapped is a different message.
        let swapped = TurnContent {
            parts: vec![text("world"), text("hello")],
            tool_results: Vec::new(),
        };
        assert_ne!(first.digest(), swapped.digest());
    }

    #[test]
    fn a_summary_keeps_the_shape_and_none_of_the_words() {
        let content = TurnContent {
            parts: vec![
                text("a secret"),
                ContentPart::ToolCall {
                    call_id: "1".to_owned(),
                    name: "search".to_owned(),
                    arguments: "{}".to_owned(),
                },
            ],
            tool_results: Vec::new(),
        };
        let summary = content.summarise();
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0].kind, "text");
        assert_eq!(summary[0].bytes, "a secret".len());
        assert_eq!(summary[1].kind, "tool_call");
        // The whole point: nothing in the summary can be read back as content.
        let json = serde_json::to_string(&summary).expect("serialises");
        assert!(!json.contains("secret"), "{json}");
    }

    #[test]
    fn a_turn_cannot_be_its_own_parent() {
        let mut request = RecordTurnRequest {
            conversation_id: "c".to_owned(),
            message_id: "m".to_owned(),
            parent_message_id: Some("m".to_owned()),
            ordinal: 0,
            role: Role::User,
            content: TurnContent::default(),
            provenance: Provenance::default(),
            policy: ContentPolicy::default(),
            occurred_at: None,
        };
        assert!(request.validate().is_err());
        request.parent_message_id = None;
        assert!(request.validate().is_ok());
    }

    #[test]
    fn content_past_the_cap_is_refused_before_it_is_encrypted() {
        let request = RecordTurnRequest {
            conversation_id: "c".to_owned(),
            message_id: "m".to_owned(),
            parent_message_id: None,
            ordinal: 0,
            role: Role::User,
            content: TurnContent {
                parts: vec![text(&"x".repeat(MAX_CONTENT_BYTES + 1))],
                tool_results: Vec::new(),
            },
            provenance: Provenance::default(),
            policy: ContentPolicy::default(),
            occurred_at: None,
        };
        assert!(matches!(request.validate(), Err(Error::TooLarge { .. })));
    }

    #[test]
    fn a_reference_part_carries_a_pointer_rather_than_bytes() {
        // The annotation registry's rule, restated where it would be easiest to
        // break: an image pasted into a chat is a URI here, never a data URL
        // the archive would then be storing under a text-sized key.
        let part = ContentPart::Reference {
            uri: "s3://bucket/scan.pdf".to_owned(),
            media_type: "application/pdf".to_owned(),
            digest: String::new(),
        };
        assert_eq!(part.text(), None);
        assert_eq!(part.kind(), "reference");
    }
}
