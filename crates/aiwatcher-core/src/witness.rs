//! What a witness says about the words of a call without saying them.
//!
//! A gateway relaying a call sees what it asked and what came back, and must
//! put neither on the log. What it publishes instead is a keyed digest of each:
//! an HMAC under a key derived from the credential it publishes with, so a
//! reader of the log — who holds no such secret — cannot test guesses against
//! a one-word answer, while this deployment, which issued the credential, can
//! ask whether an answer is one of the replies and whether a case's input was
//! in the request. The application cannot publish one: it holds neither the
//! gateway's credential nor the key.
//!
//! The Python gateway computes the same bytes (`aiwatcher_sdk.gateway`), so
//! both sides are written against one set of vectors.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// What a key is derived for, so a credential's secret is never itself the key.
const KEY_LABEL: &[u8] = b"aiwatcher.witness.v1";
/// How many hex characters of a digest are kept: 128 bits.
const DIGEST_HEX: usize = 32;
/// How many digests of one side a call keeps.
pub const MOST_DIGESTS: usize = 64;

/// Which side of a call a text was on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Said {
    /// In the request: a message, or a value a template was rendered with.
    Asked,
    /// In a reply.
    Replied,
}

impl Said {
    const fn label(self) -> &'static [u8] {
        match self {
            Self::Asked => b"asked",
            Self::Replied => b"replied",
        }
    }
}

/// HMAC-SHA256, RFC 2104.
fn hmac(key: &[u8], message: &[&[u8]]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(padded.map(|byte| byte ^ 0x36));
    for part in message {
        inner.update(part);
    }
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(padded.map(|byte| byte ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

/// The witness key of a credential, from its secret.
#[must_use]
pub fn key_for(secret: &str) -> [u8; 32] {
    hmac(secret.as_bytes(), &[KEY_LABEL])
}

/// The digest of one text on one side of a call, trimmed of surrounding
/// whitespace first.
#[must_use]
pub fn digest(key: &[u8; 32], said: Said, text: &str) -> String {
    let mut hex = hex::encode(hmac(key, &[said.label(), b"\0", text.trim().as_bytes()]));
    hex.truncate(DIGEST_HEX);
    hex
}

/// A JSON value as one text: keys sorted, nothing between tokens — Python's
/// `json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`
/// for everything but a float the two languages spell differently.
#[must_use]
pub fn canonical(value: &Value) -> String {
    match value {
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|key| format!("{}:{}", Value::String(key.clone()), canonical(&fields[key])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

/// The texts an answer is compared with a reply as: itself, where it is text;
/// its canonical JSON, where it is not.
#[must_use]
pub fn answered_as(answer: &Value) -> Vec<String> {
    match answer {
        Value::String(text) if !text.trim().is_empty() => vec![text.clone()],
        Value::String(_) | Value::Null => Vec::new(),
        other => vec![canonical(other)],
    }
}

/// The texts a case's input is looked for in a request as: itself, where it is
/// text; otherwise its canonical JSON and every text inside it.
#[must_use]
pub fn asked_as(input: &Value) -> Vec<String> {
    fn leaves(value: &Value, into: &mut Vec<String>) {
        match value {
            Value::String(text) if !text.trim().is_empty() => into.push(text.clone()),
            Value::Array(items) => items.iter().for_each(|item| leaves(item, into)),
            Value::Object(fields) => fields.values().for_each(|field| leaves(field, into)),
            _ => {}
        }
    }
    match input {
        Value::String(_) | Value::Null => answered_as(input),
        other => {
            let mut texts = vec![canonical(other)];
            leaves(other, &mut texts);
            texts
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The vectors `sdk/python/tests/test_gateway.py` holds the gateway to.
    #[test]
    fn a_digest_is_the_bytes_the_gateway_computes() {
        let key = key_for("serving-secret");
        assert_eq!(
            hex::encode(key),
            "b27f733074077db3bb749314c6dbc46fc967028defacac53f83907b0ecf83c15"
        );
        assert_eq!(
            digest(&key, Said::Replied, " Lima\n"),
            "424880e43451b760e40c782d90743997"
        );
        assert_eq!(
            digest(&key, Said::Asked, "What is the capital of Peru?"),
            "8f3cf1564557884b7b44e9bf0077f300"
        );
        assert_ne!(
            digest(&key, Said::Asked, "Lima"),
            digest(&key, Said::Replied, "Lima"),
            "a side is part of what is digested"
        );
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hex::encode(hmac(b"Jefe", &[b"what do ya want ", b"for nothing?"])),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex::encode(hmac(
                &[0xaa; 131],
                &[b"Test Using Larger Than Block-Size Key - Hash Key First"]
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn a_value_is_compared_as_its_text_or_its_canonical_json_and_an_input_by_its_texts_too() {
        let structured = json!({"z": [1, "two"], "a": {"é": "\n"}});
        assert_eq!(canonical(&structured), r#"{"a":{"é":"\n"},"z":[1,"two"]}"#);
        assert_eq!(answered_as(&json!("Lima")), ["Lima"]);
        assert!(answered_as(&json!("  ")).is_empty());
        assert_eq!(
            asked_as(&json!({"question": "What is the capital of Peru?"})),
            [
                r#"{"question":"What is the capital of Peru?"}"#,
                "What is the capital of Peru?"
            ]
        );
    }
}
