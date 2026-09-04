//! One immutable pointer to bytes stored outside a message.
//!
//! Two things in this workspace need exactly the same record, for the same
//! reason. A model package names its weights, its tokenizer and its config, and
//! ADR_0023 requires a digest for each because `s3://models/latest.pt` is
//! different bytes tomorrow. An execution step hands its rows to the next step,
//! and ADR_0025 keeps them out of the workflow message for the same reason it
//! keeps them out of the log: a payload that grows with the data is a payload
//! that eventually cannot be stored, replayed or shown.
//!
//! So the type is here, and each caller keeps its own rule about what a valid
//! one is. A model package refuses an artifact with no digest — the whole point
//! of ADR_0023. The workflow fold's own artifact record reads a digest when it
//! is there and never requires one, because a producer pointing at somebody
//! else's bytes may not know it.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What sort of thing an artifact holds.
///
/// An enum rather than free text — unlike `data.step_type`, which is free text
/// so a producer can introduce `rerank` without a backend release. The
/// difference is who chooses: a step type is chosen by whoever instruments an
/// agent, and this is chosen by the code in this workspace that writes the
/// bytes. It decides a key prefix and a filter, and nothing that executes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// A row set: what a Flow query or a notebook step produced.
    Rows,
    /// A snapshot of the code that ran — a notebook source, a compiled script.
    Code,
    /// A bounded sample somebody looks at, never the input to anything.
    Preview,
    /// Captured stdout/stderr, kept as a file when it outgrew a diagnostic.
    Log,
    /// Weights, a tokenizer, a config — what a [`ModelPackage`] is made of.
    ///
    /// [`ModelPackage`]: https://docs.rs/aiwatcher-training
    Model,
    /// A document a suite produced.
    Report,
    /// Bytes nothing here classifies. The honest default, and what a model
    /// package's artifacts serialise as so that promoting this type changed no
    /// stored JSON.
    #[default]
    Blob,
}

impl ArtifactKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::Code => "code",
            Self::Preview => "preview",
            Self::Log => "log",
            Self::Model => "model",
            Self::Report => "report",
            Self::Blob => "blob",
        }
    }
}

/// One file, addressed by what is in it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    /// What the reader calls it: `weights`, `tokenizer`, `rows`.
    pub name: String,
    /// Where the bytes are. A pointer, like every other artifact in this
    /// workspace — nothing here stores a copy.
    ///
    /// A `file://` on a shared volume is accepted and is never the only
    /// reference to something that matters: nothing outside that node can
    /// verify it, so a step handing on a path also reports the object store
    /// copy's digest, or reports no artifact at all.
    pub uri: String,
    /// `sha256` of the bytes, lowercase hex.
    ///
    /// The reason this type exists rather than a `(name, uri)` pair. Whether it
    /// may be empty is the caller's rule, not this type's.
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_type: String,
    /// Added when this moved out of `aiwatcher-training`. Defaulted and skipped
    /// when unset, so every package already written reads and re-writes
    /// unchanged.
    #[serde(default, skip_serializing_if = "is_blob")]
    pub kind: ArtifactKind,
    /// The schema the bytes claim to be in — a dataset version, a `sha256` of a
    /// column list. Read by whoever consumes them; interpreted by nothing here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's `skip_serializing_if` shape
fn is_blob(kind: &ArtifactKind) -> bool {
    matches!(kind, ArtifactKind::Blob)
}

impl ArtifactRef {
    /// A pointer to `uri` holding bytes that hash to `digest`.
    #[must_use]
    pub fn new(name: impl Into<String>, uri: impl Into<String>, digest: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            uri: uri.into(),
            digest: digest.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn of_kind(mut self, kind: ArtifactKind) -> Self {
        self.kind = kind;
        self
    }

    /// Whether `digest` is a lowercase hex `sha256`.
    ///
    /// The check, not the policy: a model package refuses an artifact that
    /// fails it (ADR_0023) and the workflow fold tolerates one, and both ask
    /// the same question.
    #[must_use]
    pub fn has_digest(&self) -> bool {
        self.digest.len() == 64
            && self
                .digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_written_before_this_type_moved_reads_and_rewrites_unchanged() {
        // The whole reason `kind` and `schema_ref` are defaulted and skipped.
        // `deny_unknown_fields` means the other direction is already loud.
        let stored = r#"{"name":"weights","uri":"s3://m/w.pt","digest":"ab","content_type":"x"}"#;
        let artifact: ArtifactRef =
            serde_json::from_str(stored).expect("an artifact written before the move");
        assert_eq!(artifact.kind, ArtifactKind::Blob);
        assert_eq!(
            serde_json::to_string(&artifact).expect("serialising an artifact"),
            stored
        );
    }

    #[test]
    fn an_uppercase_digest_addresses_the_same_bytes_twice_and_is_not_one() {
        assert!(ArtifactRef::new("rows", "s3://a", "a".repeat(64)).has_digest());
        assert!(!ArtifactRef::new("rows", "s3://a", "A".repeat(64)).has_digest());
        assert!(!ArtifactRef::new("rows", "s3://a", "ab").has_digest());
    }
}
