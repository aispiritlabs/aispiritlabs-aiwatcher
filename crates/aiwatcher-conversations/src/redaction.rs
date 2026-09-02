//! What the producer says it removed, and what the server finds anyway.
//!
//! Two halves that must not be confused. [`RedactionRecord`] is a **claim**: a
//! producer names the hook that ran and the rules that fired, and a protected
//! deployment refuses content that carries no such claim. [`scan`] is this
//! server's own look at the bytes, and it exists precisely because the claim
//! cannot be checked — a hook that was misconfigured, or that ran against an
//! older schema, reports exactly the same record as one that worked.
//!
//! **A finding carries no content.** It names a part, a byte range and a rule,
//! and nothing else. A finding that quoted the secret it found would put that
//! secret in the plaintext head, in every list response and in the review
//! queue — which is the one place in this crate where being helpful would undo
//! the encryption.
//!
//! # What this does not do
//!
//! There is no unsafe-output classifier here, and [`FindingKind::Unsafe`]
//! exists only so a **human** reviewer can record one. Shipping a
//! keyword-matching "safety scan" would produce a green tick nobody should
//! trust, and the whole reason for the review gate is that this judgement is
//! not automatable. The same reasoning is why the scanner has no entropy
//! heuristic: at any threshold that catches real keys it also catches base64
//! images and git hashes, and a reviewer who has learned to dismiss findings is
//! a reviewer who dismisses the true one.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::turn::{ContentPart, TurnContent};

/// What a producer says its own redaction hook did.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct RedactionRecord {
    /// The hook, versioned: `acme-scrubber@2.1`. A bare name is accepted and
    /// is worth less — "which version of the scrubber ran" is the question
    /// asked after something gets through.
    pub redactor: String,
    /// The rule ids that fired. Empty is meaningful and common: the hook ran
    /// and found nothing.
    #[serde(default)]
    pub rules: Vec<String>,
    /// How many spans it replaced. Recorded rather than derived, because the
    /// server sees only what is left.
    #[serde(default)]
    pub replaced: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub applied_at: Option<OffsetDateTime>,
}

impl RedactionRecord {
    /// A hook that ran and removed nothing.
    #[must_use]
    pub fn named(redactor: impl Into<String>) -> Self {
        Self {
            redactor: redactor.into(),
            rules: Vec::new(),
            replaced: 0,
            applied_at: None,
        }
    }
}

/// What kind of problem a finding is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Something identifying a person.
    Pii,
    /// A credential. The one kind whose presence in a training corpus is a
    /// live incident rather than a policy problem.
    Secret,
    /// Content a human judged unsafe to train on. Never produced by [`scan`].
    Unsafe,
    /// This exact content is already in the archive under another turn.
    Duplicate,
    /// A policy problem: no basis, no scope, a consent that does not cover
    /// what an export asks for.
    Policy,
}

impl FindingKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pii => "pii",
            Self::Secret => "secret",
            Self::Unsafe => "unsafe",
            Self::Duplicate => "duplicate",
            Self::Policy => "policy",
        }
    }
}

/// One problem, located but not quoted.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct Finding {
    pub kind: FindingKind,
    /// The rule that fired: `email`, `aws-access-key-id`, or a reviewer's own
    /// word when a human recorded it.
    pub rule: String,
    /// Which content part, by index. `None` for a finding about the turn as a
    /// whole — a duplicate, a policy gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<usize>,
    /// Byte offset and length inside that part's text. Enough for the review
    /// UI to highlight it once the content is decrypted, and useless to anyone
    /// who only has the head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
    /// Who found it: `scanner`, or a reviewer's identity.
    pub found_by: String,
}

impl Finding {
    fn scanned(kind: FindingKind, rule: &str, part: usize, start: usize, length: usize) -> Self {
        Self {
            kind,
            rule: rule.to_owned(),
            part: Some(part),
            start: Some(start),
            length: Some(length),
            found_by: "scanner".to_owned(),
        }
    }

    /// A finding about the turn rather than about a span of it.
    #[must_use]
    pub fn about_turn(kind: FindingKind, rule: impl Into<String>, found_by: &str) -> Self {
        Self {
            kind,
            rule: rule.into(),
            part: None,
            start: None,
            length: None,
            found_by: found_by.to_owned(),
        }
    }
}

/// Every credential and identifier this server can recognise in one turn.
///
/// Conservative by construction: each rule matches a shape that is expensive to
/// produce by accident. The cost of a false positive is a reviewer's attention,
/// which is the resource the whole gate depends on.
#[must_use]
pub fn scan(content: &TurnContent) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (index, part) in content.parts.iter().enumerate() {
        // A `Redacted` part is the producer saying it already handled this
        // span; scanning the reason text for secrets would report the word
        // "password" in "password removed".
        if matches!(part, ContentPart::Redacted { .. }) {
            continue;
        }
        if let Some(text) = part.text() {
            scan_text(text, index, &mut findings);
        }
    }
    // Tool output is where a credential most often survives a producer's
    // redaction: the hook rewrites what the model said and passes through what
    // the environment returned.
    for (offset, result) in content.tool_results.iter().enumerate() {
        let index = content.parts.len() + offset;
        scan_text(&result.content, index, &mut findings);
    }
    findings
}

fn scan_text(text: &str, part: usize, findings: &mut Vec<Finding>) {
    let bytes = text.as_bytes();
    find_private_key(bytes, part, findings);
    find_prefixed_tokens(bytes, part, findings);
    find_emails(bytes, part, findings);
    find_card_numbers(bytes, part, findings);
    find_international_phones(bytes, part, findings);
}

/// `-----BEGIN … PRIVATE KEY-----`. The one rule with no false positives worth
/// worrying about: nothing produces that header by accident.
fn find_private_key(bytes: &[u8], part: usize, findings: &mut Vec<Finding>) {
    const OPEN: &[u8] = b"-----BEGIN ";
    const CLOSE: &[u8] = b"PRIVATE KEY-----";
    let mut from = 0;
    while let Some(start) = find(bytes, OPEN, from) {
        let header_end = (start + 64).min(bytes.len());
        if let Some(end) = find(&bytes[start..header_end], CLOSE, 0) {
            findings.push(Finding::scanned(
                FindingKind::Secret,
                "private-key",
                part,
                start,
                end + CLOSE.len(),
            ));
        }
        from = start + OPEN.len();
    }
}

/// Credentials whose issuer put a recognisable prefix on them, which is most of
/// the ones issued this decade.
fn find_prefixed_tokens(bytes: &[u8], part: usize, findings: &mut Vec<Finding>) {
    // (prefix, minimum characters after it, rule)
    const RULES: &[(&[u8], usize, &str)] = &[
        (b"AKIA", 16, "aws-access-key-id"),
        (b"ASIA", 16, "aws-access-key-id"),
        (b"ghp_", 30, "github-token"),
        (b"gho_", 30, "github-token"),
        (b"ghs_", 30, "github-token"),
        (b"ghu_", 30, "github-token"),
        (b"ghr_", 30, "github-token"),
        (b"github_pat_", 40, "github-token"),
        (b"xoxb-", 20, "slack-token"),
        (b"xoxp-", 20, "slack-token"),
        (b"sk-", 32, "api-key"),
        (b"sk_live_", 16, "api-key"),
        (b"rk_live_", 16, "api-key"),
        (b"AIza", 30, "google-api-key"),
        (b"eyJ", 40, "json-web-token"),
    ];
    for (prefix, minimum, rule) in RULES {
        let mut from = 0;
        while let Some(start) = find(bytes, prefix, from) {
            from = start + prefix.len();
            // A prefix in the middle of a longer word is a word, not a token.
            if start > 0 && is_token_byte(bytes[start - 1]) {
                continue;
            }
            let mut end = from;
            while end < bytes.len() && is_token_byte(bytes[end]) {
                end += 1;
            }
            if end - from >= *minimum {
                findings.push(Finding::scanned(
                    FindingKind::Secret,
                    rule,
                    part,
                    start,
                    end - start,
                ));
            }
        }
    }
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

/// `local@domain.tld`, with the conservative reading of every part.
fn find_emails(bytes: &[u8], part: usize, findings: &mut Vec<Finding>) {
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'@' {
            continue;
        }
        let mut start = index;
        while start > 0 && is_local_byte(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = index + 1;
        while end < bytes.len() && is_domain_byte(bytes[end]) {
            end += 1;
        }
        if start == index || end == index + 1 {
            continue;
        }
        let domain = &bytes[index + 1..end];
        // A dot with at least two letters after it. Without this every
        // `user@localhost` and every `@mention` in prose is a finding.
        let has_tld = domain
            .iter()
            .rposition(|byte| *byte == b'.')
            .is_some_and(|dot| {
                domain[dot + 1..].len() >= 2
                    && domain[dot + 1..].iter().all(u8::is_ascii_alphabetic)
            });
        if has_tld {
            findings.push(Finding::scanned(
                FindingKind::Pii,
                "email",
                part,
                start,
                end - start,
            ));
        }
    }
}

fn is_local_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_domain_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')
}

/// A 13-to-19 digit run that passes Luhn.
///
/// Luhn is what keeps this from firing on every order number: roughly nine in
/// ten random digit strings of that length fail it.
fn find_card_numbers(bytes: &[u8], part: usize, findings: &mut Vec<Finding>) {
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        let mut digits: Vec<u8> = Vec::new();
        let mut end = index;
        while end < bytes.len() {
            match bytes[end] {
                byte if byte.is_ascii_digit() => {
                    digits.push(byte - b'0');
                    end += 1;
                }
                // A single separator inside the run, never two.
                b' ' | b'-' if end + 1 < bytes.len() && bytes[end + 1].is_ascii_digit() => end += 1,
                _ => break,
            }
        }
        if (13..=19).contains(&digits.len()) && luhn(&digits) {
            findings.push(Finding::scanned(
                FindingKind::Pii,
                "payment-card",
                part,
                start,
                end - start,
            ));
        }
        index = end.max(start + 1);
    }
}

fn luhn(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    for (position, digit) in digits.iter().rev().enumerate() {
        let mut value = u32::from(*digit);
        if !position.is_multiple_of(2) {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
    }
    sum.is_multiple_of(10)
}

/// `+` then 10 to 15 digits, separators allowed.
///
/// Only the international form. A bare run of digits is an order number far
/// more often than it is a telephone number, and a rule that cannot tell them
/// apart teaches reviewers to skim.
fn find_international_phones(bytes: &[u8], part: usize, findings: &mut Vec<Finding>) {
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'+' {
            index += 1;
            continue;
        }
        let start = index;
        let mut count = 0;
        let mut end = index + 1;
        while end < bytes.len() {
            match bytes[end] {
                byte if byte.is_ascii_digit() => {
                    count += 1;
                    end += 1;
                }
                b' ' | b'-' | b'(' | b')' if count > 0 => end += 1,
                _ => break,
            }
        }
        if (10..=15).contains(&count) {
            findings.push(Finding::scanned(
                FindingKind::Pii,
                "phone-number",
                part,
                start,
                end - start,
            ));
        }
        index = end.max(start + 1);
    }
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::ToolResult;

    fn content(text: &str) -> TurnContent {
        TurnContent {
            parts: vec![ContentPart::Text {
                text: text.to_owned(),
            }],
            tool_results: Vec::new(),
        }
    }

    fn rules(findings: &[Finding]) -> Vec<&str> {
        findings
            .iter()
            .map(|finding| finding.rule.as_str())
            .collect()
    }

    #[test]
    fn a_finding_never_carries_the_thing_it_found() {
        // The one property this module cannot get wrong: a finding is written
        // into the plaintext head, so anything quoted in it defeats the
        // encryption of the content it came from.
        let findings = scan(&content(
            "write to ada@example.com about AKIA1234567890ABCDEF",
        ));
        let json = serde_json::to_string(&findings).expect("serialises");
        assert!(!json.contains("ada@example.com"), "{json}");
        assert!(!json.contains("AKIA"), "{json}");
        assert!(json.contains("email"), "{json}");
    }

    #[test]
    fn a_credential_in_tool_output_is_found_where_a_producers_hook_would_miss_it() {
        // The realistic leak: the hook rewrites what the model said and passes
        // through what the environment returned.
        let content = TurnContent {
            parts: vec![ContentPart::Text {
                text: "reading the config".to_owned(),
            }],
            tool_results: vec![ToolResult {
                call_id: "1".to_owned(),
                name: "read_file".to_owned(),
                ok: true,
                content: "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE".to_owned(),
                error: String::new(),
            }],
        };
        let findings = scan(&content);
        assert_eq!(rules(&findings), vec!["aws-access-key-id"]);
        // Indexed past the parts, so the reviewer can tell which half it was in.
        assert_eq!(findings[0].part, Some(1));
    }

    #[test]
    fn an_email_needs_a_real_top_level_domain() {
        assert_eq!(rules(&scan(&content("ada@example.com"))), vec!["email"]);
        for not_an_email in ["@mention", "user@localhost", "a@b", "@", "x @ y.com"] {
            assert!(
                scan(&content(not_an_email)).is_empty(),
                "{not_an_email} matched"
            );
        }
    }

    #[test]
    fn a_digit_run_is_only_a_card_number_when_it_passes_luhn() {
        // A well-known test number, and the same length with one digit moved.
        assert_eq!(
            rules(&scan(&content("card 4111 1111 1111 1111"))),
            vec!["payment-card"]
        );
        assert!(scan(&content("order 4111111111111112")).is_empty());
        // And an ordinary long number is not a card.
        assert!(scan(&content("build 20260902114500123")).is_empty());
    }

    #[test]
    fn a_token_prefix_inside_a_longer_word_is_a_word() {
        // `sk-` is a prefix people also write in prose and in identifiers.
        assert!(scan(&content("the task-sk-report is ready")).is_empty());
        assert!(scan(&content("risk-assessment-for-the-quarter-ahead")).is_empty());
        assert_eq!(
            rules(&scan(&content(
                "key sk-abcdefghijklmnopqrstuvwxyz0123456789"
            ))),
            vec!["api-key"]
        );
    }

    #[test]
    fn a_redacted_part_is_not_scanned_again() {
        let content = TurnContent {
            parts: vec![ContentPart::Redacted {
                reason: "removed an email like ada@example.com".to_owned(),
                original_bytes: 40,
                original_digest: String::new(),
            }],
            tool_results: Vec::new(),
        };
        assert!(scan(&content).is_empty());
    }

    #[test]
    fn a_private_key_header_is_found_wherever_it_sits() {
        let findings = scan(&content(
            "here it is:\n-----BEGIN RSA PRIVATE KEY-----\nMIIC...\n",
        ));
        assert_eq!(rules(&findings), vec!["private-key"]);
    }

    #[test]
    fn only_the_international_phone_form_matches() {
        assert_eq!(
            rules(&scan(&content("call +44 20 7946 0958 tomorrow"))),
            vec!["phone-number"]
        );
        // Nine digits: a short number, not a phone number.
        assert!(scan(&content("ref +123456789")).is_empty());
    }

    #[test]
    fn the_scanner_never_claims_content_is_safe() {
        // There is no `Unsafe` rule here on purpose — a keyword list would
        // produce a green tick nobody should trust. The kind exists for a
        // human, and this is the test that says so.
        let findings = scan(&content("something a reviewer would refuse"));
        assert!(findings.iter().all(|f| f.kind != FindingKind::Unsafe));
    }
}
