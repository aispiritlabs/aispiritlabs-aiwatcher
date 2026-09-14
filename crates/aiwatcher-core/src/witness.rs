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
    /// How the caller takes its answer out of a reply: the rule, canonical.
    Taking,
}

impl Said {
    const fn label(self) -> &'static [u8] {
        match self {
            Self::Asked => b"asked",
            Self::Replied => b"replied",
            Self::Taking => b"taking",
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

/// A text as a question asked in other words still reads: NFKC, lower case,
/// every punctuation character (general category P) gone, and each run of
/// white space (the Unicode `White_Space` property) one space, none at either
/// end.
///
/// What a witness digests a second time beside what a request asked, so a case
/// asked again in another case, another spacing or with its question mark gone
/// is found where the text itself would not be. Byte for byte what the Python
/// gateway and the TypeScript SDK compute, for every character the Unicode
/// version each language ships assigns; a change here is a new digest, not a
/// fix. A paraphrase is not found, by design: that would take the words.
#[must_use]
pub fn normalized(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};
    let folded = text.nfkc().collect::<String>().to_lowercase();
    let mut out = String::with_capacity(folded.len());
    let mut space = false;
    for character in folded.chars() {
        if character.general_category_group() == GeneralCategoryGroup::Punctuation {
            continue;
        }
        if character.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(character);
    }
    out
}

/// The digest of one text on one side of a call, trimmed of surrounding
/// whitespace first.
#[must_use]
pub fn digest(key: &[u8; 32], said: Said, text: &str) -> String {
    let mut hex = hex::encode(hmac(key, &[said.label(), b"\0", text.trim().as_bytes()]));
    hex.truncate(DIGEST_HEX);
    hex
}

/// A number as JavaScript's `String(number)` spells it: the shortest digits
/// that read back as the same double, in positional notation from a millionth
/// up to 10²¹ and in exponent notation outside — and an integer digit for digit.
/// Python's `repr` and Rust's formatting choose the same digits and spell them
/// differently; this is the one spelling both write. A parsed [`Value`] holds
/// an integer wider than 64 bits only as the double it rounds to, so one read
/// from text is spelled with [`canonical_text`].
#[must_use]
pub fn number(value: &serde_json::Number) -> String {
    if value.is_i64() || value.is_u64() {
        return value.to_string();
    }
    let float = value.as_f64().unwrap_or(0.0);
    if float == 0.0 || !float.is_finite() {
        return "0".to_owned();
    }
    let sign = if float < 0.0 { "-" } else { "" };
    let shortest = format!("{:e}", float.abs());
    let (mantissa, exponent) = shortest.split_once('e').unwrap_or((&shortest, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let count = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let point = exponent + 1;
    let spelled = if count <= point && point <= 21 {
        format!(
            "{digits}{}",
            "0".repeat(usize::try_from(point - count).unwrap_or(0))
        )
    } else if 0 < point && point <= 21 {
        let (whole, fraction) = digits.split_at(usize::try_from(point).unwrap_or(0));
        format!("{whole}.{fraction}")
    } else if -6 < point && point <= 0 {
        format!(
            "0.{}{digits}",
            "0".repeat(usize::try_from(-point).unwrap_or(0))
        )
    } else {
        let sign = if point - 1 < 0 { '-' } else { '+' };
        let (first, rest) = digits.split_at(1);
        let rest = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        format!("{first}{rest}e{sign}{}", (point - 1).abs())
    };
    format!("{sign}{spelled}")
}

/// A JSON value as one text: keys sorted by code point, nothing between
/// tokens, strings escaped as JSON escapes them, and every number as
/// [`number`] spells it — so Python and Rust write the same bytes for one
/// value, a float included.
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
        Value::Number(value) => number(value),
        other => other.to_string(),
    }
}

/// A JSON text as [`canonical`] spells the value it holds — except that every
/// integer is spelled digit for digit however wide, where a parsed [`Value`]
/// has already rounded one wider than 64 bits. `None` when it is not JSON.
#[must_use]
pub fn canonical_text(text: &str) -> Option<String> {
    let mut reader = Reader::new(text);
    reader.space();
    let spelled = reader.value(0)?;
    reader.space();
    (reader.at == text.len()).then_some(spelled)
}

/// Whether a JSON text holds a number a parsed [`Value`] cannot hold as it is
/// written: an integer wider than 64 bits, or a decimal with more digits than
/// the double nearest it keeps. `false` for a text that is not JSON.
#[must_use]
pub fn spells_inexact_number(text: &str) -> bool {
    let mut reader = Reader::new(text);
    reader.space();
    reader.value(0).is_some() && reader.inexact
}

/// What an answer read from its JSON text is compared with a reply as — the
/// same as [`answered_as`], with every integer digit for digit.
#[must_use]
pub fn answered_as_text(text: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(text) {
        Ok(value @ (Value::String(_) | Value::Null)) => answered_as(&value),
        Ok(_) => canonical_text(text).into_iter().collect(),
        Err(_) => Vec::new(),
    }
}

/// Reads a JSON text once, spelling each value canonically as it goes.
struct Reader<'a> {
    text: &'a str,
    at: usize,
    /// Whether it has read a number a double or a 64-bit integer does not
    /// hold as written.
    inexact: bool,
}

impl<'a> Reader<'a> {
    /// As deep as `serde_json` reads before it refuses a text.
    const DEEPEST: usize = 128;

    const fn new(text: &'a str) -> Self {
        Self {
            text,
            at: 0,
            inexact: false,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        self.at += usize::from(found);
        found
    }

    fn value(&mut self, depth: usize) -> Option<String> {
        if depth > Self::DEEPEST {
            return None;
        }
        match self.peek()? {
            b'{' => {
                self.at += 1;
                let mut fields = std::collections::BTreeMap::new();
                self.space();
                if !self.eat(b'}') {
                    loop {
                        self.space();
                        let key = self.string()?;
                        self.space();
                        if !self.eat(b':') {
                            return None;
                        }
                        self.space();
                        let value = self.value(depth + 1)?;
                        fields.insert(key, value);
                        self.space();
                        if self.eat(b'}') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                let inner: Vec<String> = fields
                    .into_iter()
                    .map(|(key, value)| format!("{}:{value}", Value::String(key)))
                    .collect();
                Some(format!("{{{}}}", inner.join(",")))
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if !self.eat(b']') {
                    loop {
                        self.space();
                        items.push(self.value(depth + 1)?);
                        self.space();
                        if self.eat(b']') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                Some(format!("[{}]", items.join(",")))
            }
            b'"' => self.string().map(|text| Value::String(text).to_string()),
            b't' | b'f' | b'n' => ["true", "false", "null"].into_iter().find_map(|word| {
                self.text[self.at..].starts_with(word).then(|| {
                    self.at += word.len();
                    word.to_owned()
                })
            }),
            _ => self.number(),
        }
    }

    /// A string token, unescaped by `serde_json` itself.
    fn string(&mut self) -> Option<String> {
        if self.peek() != Some(b'"') {
            return None;
        }
        let bytes = self.text.as_bytes();
        let mut end = self.at + 1;
        loop {
            match bytes.get(end)? {
                b'\\' => end += 2,
                b'"' => break,
                _ => end += 1,
            }
        }
        let token = self.text.get(self.at..=end)?;
        self.at = end + 1;
        serde_json::from_str(token).ok()
    }

    fn number(&mut self) -> Option<String> {
        let start = self.at;
        let bytes = self.text.as_bytes();
        while bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.at += 1;
        }
        let token = self.text.get(start..self.at)?;
        let parsed: serde_json::Number = serde_json::from_str(token).ok()?;
        if token.contains(['.', 'e', 'E']) {
            self.inexact |= crate::exact::Decimal::parse(token)
                != parsed.as_f64().and_then(crate::exact::Decimal::from_f64);
            return Some(number(&parsed));
        }
        if token.trim_start_matches('-') == "0" {
            return Some("0".to_owned());
        }
        if parsed.is_i64() || parsed.is_u64() {
            return Some(parsed.to_string());
        }
        self.inexact = true;
        Some(token.to_owned())
    }
}

/// A number as [`number`] spells it, from its exact value: an integer digit for
/// digit however wide, and anything else as the double nearest it.
#[must_use]
pub fn exact_number(value: &crate::exact::Decimal) -> String {
    value.integer_text().unwrap_or_else(|| {
        serde_json::Number::from_f64(value.to_f64()).map_or_else(|| "0".to_owned(), |n| number(&n))
    })
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

/// The texts a value a template was rendered with may be, when it came from a
/// case's input: the input itself and every part of it — text as itself,
/// anything else as its canonical JSON.
#[must_use]
pub fn carried_as(input: &Value) -> Vec<String> {
    fn parts(value: &Value, into: &mut Vec<String>) {
        into.extend(answered_as(value));
        match value {
            Value::Array(items) => items.iter().for_each(|item| parts(item, into)),
            Value::Object(fields) => fields.values().for_each(|field| parts(field, into)),
            _ => {}
        }
    }
    let mut texts = Vec::new();
    parts(input, &mut texts);
    texts.sort();
    texts.dedup();
    texts
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
        assert_eq!(
            digest(
                &key,
                Said::Taking,
                &canonical(&json!({"map": {"A": "Lima"}}))
            ),
            "3b1d627f22d3a4b86fcd29126042cd6e"
        );
    }

    /// The vectors the Python gateway and the TypeScript SDK normalise alike.
    #[test]
    fn a_question_in_other_case_spacing_and_punctuation_normalises_to_one_text() {
        for (text, normal) in [
            (
                "  What is the CAPITAL of France?  ",
                "what is the capital of france",
            ),
            (
                "\u{ff30}\u{ff41}\u{ff52}\u{ff49}\u{ff53}\u{ff0c}\u{3000}\u{ff26}\u{ff32}\u{ff21}\u{ff2e}\u{ff23}\u{ff25}\u{ff01}",
                "paris france",
            ),
            (
                "Don\u{2019}t\tstop\u{2014}e-mail\u{2026}\u{fb01}ne",
                "dont stopemailfine",
            ),
            (
                "\u{39f}\u{394}\u{39f}\u{3a3} \u{3a3}",
                "\u{3bf}\u{3b4}\u{3bf}\u{3c2} \u{3c3}",
            ),
            ("\u{130}stanbul", "i\u{307}stanbul"),
            (
                "\u{a0}\u{bf}Qu\u{e9}\u{2003}pasa?\u{200b}",
                "qu\u{e9} pasa\u{200b}",
            ),
        ] {
            assert_eq!(normalized(text), normal, "{text:?}");
        }
        let key = key_for("serving-secret");
        assert_eq!(
            digest(
                &key,
                Said::Asked,
                &normalized("  What is the CAPITAL of France?  ")
            ),
            digest(&key, Said::Asked, "what is the capital of france"),
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

    /// The vectors `sdk/python/tests/test_gateway.py` holds its spelling to,
    /// each as JavaScript's `String(number)` writes it.
    #[test]
    fn a_number_is_spelled_the_one_way_both_languages_write_it() {
        for (value, spelled) in [
            (json!(0.1), "0.1"),
            (json!(1e21), "1e+21"),
            (json!(1e-7), "1e-7"),
            (json!(123_456_789.125), "123456789.125"),
            (json!(-0.0), "0"),
            (json!(1.0), "1"),
            (json!(5e-324), "5e-324"),
            (
                json!(1.797_693_134_862_315_7e308),
                "1.7976931348623157e+308",
            ),
            (json!(100.0), "100"),
            (json!(1e20), "100000000000000000000"),
            (json!(0.000_001), "0.000001"),
            (json!(1.23e-18), "1.23e-18"),
            (json!(-3.75e-8), "-3.75e-8"),
            (json!(42), "42"),
            (json!(-7), "-7"),
            (json!(u64::MAX), "18446744073709551615"),
        ] {
            assert_eq!(canonical(&value), spelled, "{value}");
        }
        assert_eq!(
            canonical(&json!({"score": 0.5, "labels": ["a", 2.0]})),
            r#"{"labels":["a",2],"score":0.5}"#
        );
    }

    #[test]
    fn a_text_is_spelled_as_its_value_is_and_an_integer_of_any_width_digit_for_digit() {
        for text in [
            r#"{"z": [1, "two", 0.1, 1e21, -0, 1.0], "a": {"é": "\n", "b": null}, "t": true}"#,
            r#""Lima""#,
            "[]",
            "{}",
            " 18446744073709551615 ",
            r#"{"a": 1, "a": 2}"#,
        ] {
            let parsed: Value = serde_json::from_str(text).expect("JSON");
            assert_eq!(canonical_text(text), Some(canonical(&parsed)), "{text}");
            assert!(!spells_inexact_number(text), "{text}");
        }
        let wide = r#"{"id": 123456789012345678901234567890, "next": -18446744073709551617}"#;
        assert_eq!(
            canonical_text(wide).as_deref(),
            Some(r#"{"id":123456789012345678901234567890,"next":-18446744073709551617}"#)
        );
        assert!(spells_inexact_number(wide));
        assert!(
            spells_inexact_number(r#"{"ratio": 0.12345678901234567890}"#),
            "more digits than a double keeps"
        );
        assert!(!spells_inexact_number(r#"{"ratio": 0.125, "big": 1e300}"#));
        assert_ne!(
            canonical_text("123456789012345678901234567890"),
            canonical_text("123456789012345678901234567891"),
            "two integers a double cannot tell apart"
        );
        assert_eq!(answered_as_text(r#"" Lima ""#), [" Lima "]);
        assert!(answered_as_text("null").is_empty());
        assert_eq!(
            answered_as_text("[123456789012345678901234567890]"),
            ["[123456789012345678901234567890]"]
        );
        for broken in ["", "[1,", r#"{"a" 1}"#, "01", "[1] 2", "truth"] {
            assert_eq!(canonical_text(broken), None, "{broken}");
        }
    }

    #[test]
    fn a_value_is_compared_as_its_text_or_its_canonical_json_and_an_input_by_its_texts_too() {
        let structured = json!({"z": [1, "two"], "a": {"é": "\n"}});
        assert_eq!(canonical(&structured), r#"{"a":{"é":"\n"},"z":[1,"two"]}"#);
        assert_eq!(answered_as(&json!("Lima")), ["Lima"]);
        assert!(answered_as(&json!("  ")).is_empty());
        assert_eq!(
            carried_as(&json!({"question": "Peru?", "options": [1, "Lima"], "note": " "})),
            [
                "1",
                "Lima",
                "Peru?",
                r#"[1,"Lima"]"#,
                r#"{"note":" ","options":[1,"Lima"],"question":"Peru?"}"#,
            ]
        );
        assert_eq!(
            asked_as(&json!({"question": "What is the capital of Peru?"})),
            [
                r#"{"question":"What is the capital of Peru?"}"#,
                "What is the capital of Peru?"
            ]
        );
    }
}
