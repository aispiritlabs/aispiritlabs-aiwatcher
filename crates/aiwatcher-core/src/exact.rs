//! Numbers as they were written.
//!
//! JSON has one kind of number and no bound on its digits. A parsed
//! [`serde_json::Value`] holds an integer wider than 64 bits, and a decimal
//! with more digits than a double keeps, only as the double nearest it — so two
//! numbers somebody asked to be told apart can read as one, and a distance
//! between them as nought. [`ExactValue`] is a JSON value whose numbers are
//! [`Decimal`]s: compared and subtracted without a double in between, and made
//! into one only for a figure that is published as one.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::Value;

/// How far a number's point may sit from its digits, and how many digits it
/// may have: past a double's range many times over, and bounded, so a number
/// written to exhaust memory is not a number here.
const MOST_EXPONENT: i64 = 4_096;
const MOST_DIGITS: usize = 4_096;
/// As deep as `serde_json` reads before it refuses a text.
const DEEPEST: usize = 128;

/// A number exactly: `digits × 10^exponent`, negative or not.
///
/// Kept with no leading and no trailing zeros, so one value has one spelling
/// and equality is equality of the parts; nought has no digits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decimal {
    negative: bool,
    digits: Vec<u8>,
    exponent: i64,
}

impl Decimal {
    /// A JSON number token, as JSON's grammar writes one — nothing around it.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        let bytes = token.as_bytes();
        let mut at = 0;
        let negative = bytes.first() == Some(&b'-');
        at += usize::from(negative);
        let whole_from = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let whole = &bytes[whole_from..at];
        if whole.is_empty() || (whole.len() > 1 && whole[0] == b'0') {
            return None;
        }
        let mut fraction: &[u8] = &[];
        if bytes.get(at) == Some(&b'.') {
            let from = at + 1;
            at = from;
            while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                at += 1;
            }
            fraction = &bytes[from..at];
            if fraction.is_empty() {
                return None;
            }
        }
        let mut exponent: i64 = 0;
        if matches!(bytes.get(at), Some(b'e' | b'E')) {
            at += 1;
            let below = bytes.get(at) == Some(&b'-');
            at += usize::from(matches!(bytes.get(at), Some(b'-' | b'+')));
            let from = at;
            while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                exponent = exponent
                    .checked_mul(10)?
                    .checked_add(i64::from(bytes[at] - b'0'))?;
                at += 1;
            }
            if at == from {
                return None;
            }
            if below {
                exponent = -exponent;
            }
        }
        if at != bytes.len() {
            return None;
        }
        let mut digits: Vec<u8> = whole.iter().chain(fraction).map(|d| d - b'0').collect();
        exponent = exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?;
        Self::normal(negative, &mut digits, exponent)
    }

    /// The double's own digits: the shortest that read back as it.
    #[must_use]
    pub fn from_f64(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let spelled = format!("{value:e}");
        let (mantissa, exponent) = spelled.split_once('e')?;
        let negative = mantissa.starts_with('-');
        let mantissa = mantissa.trim_start_matches('-');
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let mut digits: Vec<u8> = whole
            .bytes()
            .chain(fraction.bytes())
            .map(|d| d - b'0')
            .collect();
        let exponent: i64 = exponent.parse().ok()?;
        Self::normal(
            negative,
            &mut digits,
            exponent - i64::try_from(fraction.len()).ok()?,
        )
    }

    /// A JSON number's own value: an integer digit for digit, a double by its
    /// shortest digits — what a parsed value still knows.
    #[must_use]
    pub fn from_number(number: &serde_json::Number) -> Option<Self> {
        if number.is_i64() || number.is_u64() {
            return Self::parse(&number.to_string());
        }
        number.as_f64().and_then(Self::from_f64)
    }

    fn normal(negative: bool, digits: &mut Vec<u8>, exponent: i64) -> Option<Self> {
        let normal = Self::stripped(negative, digits, exponent)?;
        (normal.digits.len() <= MOST_DIGITS && normal.exponent.abs() <= MOST_EXPONENT)
            .then_some(normal)
    }

    fn stripped(negative: bool, digits: &mut Vec<u8>, mut exponent: i64) -> Option<Self> {
        let leading = digits.iter().take_while(|d| **d == 0).count();
        digits.drain(..leading);
        while digits.last() == Some(&0) {
            digits.pop();
            exponent = exponent.checked_add(1)?;
        }
        if digits.is_empty() {
            return Some(Self::zero());
        }
        Some(Self {
            negative,
            digits: std::mem::take(digits),
            exponent,
        })
    }

    const fn zero() -> Self {
        Self {
            negative: false,
            digits: Vec::new(),
            exponent: 0,
        }
    }

    /// How far from nought, whichever side.
    #[must_use]
    pub fn abs(&self) -> Self {
        Self {
            negative: false,
            ..self.clone()
        }
    }

    fn negated(&self) -> Self {
        Self {
            negative: !self.negative && !self.digits.is_empty(),
            ..self.clone()
        }
    }

    /// `self − other`, exactly.
    #[must_use]
    pub fn minus(&self, other: &Self) -> Self {
        self.plus(&other.negated())
    }

    fn plus(&self, other: &Self) -> Self {
        if self.digits.is_empty() {
            return other.clone();
        }
        if other.digits.is_empty() {
            return self.clone();
        }
        let exponent = self.exponent.min(other.exponent);
        let aligned = |number: &Self| {
            let mut digits = number.digits.clone();
            digits.resize(
                digits.len() + usize::try_from(number.exponent - exponent).unwrap_or(0),
                0,
            );
            digits
        };
        let (a, b) = (aligned(self), aligned(other));
        let mut digits;
        let negative;
        if self.negative == other.negative {
            digits = add(&a, &b);
            negative = self.negative;
        } else {
            match compare_magnitude(&a, &b) {
                Ordering::Equal => return Self::zero(),
                Ordering::Greater => {
                    digits = subtract(&a, &b);
                    negative = self.negative;
                }
                Ordering::Less => {
                    digits = subtract(&b, &a);
                    negative = other.negative;
                }
            }
        }
        // The bounds are on what is read; a sum of two numbers inside them is
        // kept however far it reaches.
        Self::stripped(negative, &mut digits, exponent).unwrap_or_else(Self::zero)
    }

    /// The nearest double, which is what a published metric is.
    #[must_use]
    pub fn to_f64(&self) -> f64 {
        if self.digits.is_empty() {
            return 0.0;
        }
        let digits: String = self.digits.iter().map(|d| char::from(b'0' + d)).collect();
        format!(
            "{}{digits}e{}",
            if self.negative { "-" } else { "" },
            self.exponent
        )
        .parse()
        .unwrap_or(f64::NAN)
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        let difference = self.minus(other);
        if difference.digits.is_empty() {
            Ordering::Equal
        } else if difference.negative {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }
}

/// Digits most significant first, both as long as each other.
fn compare_magnitude(a: &[u8], b: &[u8]) -> Ordering {
    let a = &a[a.iter().take_while(|d| **d == 0).count()..];
    let b = &b[b.iter().take_while(|d| **d == 0).count()..];
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn add(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut sum = Vec::with_capacity(a.len().max(b.len()) + 1);
    let mut carry = 0;
    let (mut i, mut j) = (a.len(), b.len());
    while i > 0 || j > 0 || carry > 0 {
        let mut digit = carry;
        if i > 0 {
            i -= 1;
            digit += a[i];
        }
        if j > 0 {
            j -= 1;
            digit += b[j];
        }
        sum.push(digit % 10);
        carry = digit / 10;
    }
    sum.reverse();
    sum
}

/// `a − b`, where `a` is the larger.
fn subtract(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut difference = Vec::with_capacity(a.len());
    let mut borrow = 0i8;
    let mut j = b.len();
    for i in (0..a.len()).rev() {
        let mut digit = i8::try_from(a[i]).unwrap_or(0) - borrow;
        if j > 0 {
            j -= 1;
            digit -= i8::try_from(b[j]).unwrap_or(0);
        }
        borrow = i8::from(digit < 0);
        difference.push(u8::try_from(digit + 10 * borrow).unwrap_or(0));
    }
    difference.reverse();
    difference
}

/// A JSON value whose numbers are exact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExactValue {
    Null,
    Bool(bool),
    Number(Decimal),
    Text(String),
    Array(Vec<ExactValue>),
    Object(BTreeMap<String, ExactValue>),
}

impl ExactValue {
    /// A JSON text, every number as it is written. `None` when it is not JSON,
    /// or holds a number past the bounds.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut reader = Reader { text, at: 0 };
        reader.space();
        let value = reader.value(0)?;
        reader.space();
        (reader.at == text.len()).then_some(value)
    }

    /// A parsed value: what it still knows of each number.
    #[must_use]
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(flag) => Self::Bool(*flag),
            Value::Number(number) => Decimal::from_number(number).map_or(Self::Null, Self::Number),
            Value::String(text) => Self::Text(text.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from_value).collect()),
            Value::Object(fields) => Self::Object(
                fields
                    .iter()
                    .map(|(key, field)| (key.clone(), Self::from_value(field)))
                    .collect(),
            ),
        }
    }

    /// The value at a JSON Pointer (RFC 6901); the whole value for `""`.
    #[must_use]
    pub fn pointer(&self, pointer: &str) -> Option<&Self> {
        if pointer.is_empty() {
            return Some(self);
        }
        let rest = pointer.strip_prefix('/')?;
        rest.split('/').try_fold(self, |found, token| {
            let token = token.replace("~1", "/").replace("~0", "~");
            match found {
                Self::Object(fields) => fields.get(&token),
                Self::Array(items) => {
                    if token.len() > 1 && token.starts_with('0') {
                        return None;
                    }
                    token.parse::<usize>().ok().and_then(|at| items.get(at))
                }
                _ => None,
            }
        })
    }

    /// The text, where this is text.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The number this is — or the number a text is, where the text, trimmed,
    /// is nothing but one, as JSON writes it.
    #[must_use]
    pub fn number(&self) -> Option<Decimal> {
        match self {
            Self::Number(number) => Some(number.clone()),
            Self::Text(text) => Decimal::parse(text.trim()),
            _ => None,
        }
    }
}

struct Reader<'a> {
    text: &'a str,
    at: usize,
}

impl Reader<'_> {
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

    fn value(&mut self, depth: usize) -> Option<ExactValue> {
        if depth > DEEPEST {
            return None;
        }
        match self.peek()? {
            b'{' => {
                self.at += 1;
                let mut fields = BTreeMap::new();
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
                Some(ExactValue::Object(fields))
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
                Some(ExactValue::Array(items))
            }
            b'"' => self.string().map(ExactValue::Text),
            b't' | b'f' | b'n' => [
                ("true", ExactValue::Bool(true)),
                ("false", ExactValue::Bool(false)),
                ("null", ExactValue::Null),
            ]
            .into_iter()
            .find_map(|(word, value)| {
                self.text[self.at..].starts_with(word).then(|| {
                    self.at += word.len();
                    value
                })
            }),
            _ => {
                let start = self.at;
                while self
                    .peek()
                    .is_some_and(|b| matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
                {
                    self.at += 1;
                }
                Decimal::parse(self.text.get(start..self.at)?).map(ExactValue::Number)
            }
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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn number(token: &str) -> Decimal {
        Decimal::parse(token).expect(token)
    }

    #[test]
    fn one_value_is_one_decimal_however_it_is_written() {
        assert_eq!(number("1.10"), number("1.1"));
        assert_eq!(number("110e-2"), number("1.1"));
        assert_eq!(number("-0"), number("0.000"));
        assert_eq!(number("1e3"), number("1000"));
        assert_eq!(Decimal::from_f64(0.1), Some(number("0.1")));
        assert_eq!(Decimal::from_f64(-3.75e-8), Some(number("-0.0000000375")));
        assert_eq!(
            Decimal::from_number(&serde_json::Number::from(u64::MAX)),
            Some(number("18446744073709551615"))
        );
        for broken in [
            "", "-", "01", "1.", ".5", "1e", "+1", "1 ", "0x10", "1e99999",
        ] {
            assert_eq!(Decimal::parse(broken), None, "{broken}");
        }
    }

    #[test]
    fn two_numbers_a_double_cannot_tell_apart_are_two_numbers_here() {
        let wide = number("123456789012345678901234567890");
        let next = number("123456789012345678901234567891");
        assert_ne!(wide, next);
        assert_eq!(wide.to_f64(), next.to_f64(), "as doubles they are one");
        assert_eq!(next.minus(&wide), number("1"));
        assert_eq!(wide.minus(&next).abs().to_f64(), 1.0);
        assert!(wide < next);
        assert!(number("-2") < number("-1.5"));
        assert!(number("0.30000000000000000001") > number("0.3"));
        assert_eq!(number("0.1").minus(&number("0.3")), number("-0.2"));
        assert_eq!(number("5").minus(&number("5.0")), number("0"));
        assert_eq!(number("-1e-3").minus(&number("2e2")), number("-200.001"));
    }

    #[test]
    fn a_text_keeps_every_number_as_written_and_a_parsed_value_what_it_still_knows() {
        let text = r#"{"id": 123456789012345678901234567890, "score": 0.5, "tags": ["a", 2.0], "n": null}"#;
        let exact = ExactValue::parse(text).expect("JSON");
        assert_eq!(
            exact.pointer("/id").and_then(ExactValue::number),
            Some(number("123456789012345678901234567890"))
        );
        assert_eq!(
            exact.pointer("/tags/1"),
            Some(&ExactValue::Number(number("2")))
        );
        assert_eq!(exact.pointer("/tags/01"), None);
        let parsed: Value = serde_json::from_str(text).expect("JSON");
        assert_ne!(
            ExactValue::from_value(&parsed),
            exact,
            "the parsed value rounded the identifier"
        );
        assert_eq!(
            ExactValue::from_value(&json!({"score": 0.5, "tags": ["a", 2]})),
            ExactValue::parse(r#"{"tags":["a",2.00],"score":5e-1}"#).expect("JSON")
        );
        assert_eq!(
            ExactValue::Text(" 42.0 ".into()).number(),
            Some(number("42"))
        );
        assert_eq!(ExactValue::Text("42 minutes".into()).number(), None);
        for broken in ["", "[1,", r#"{"a" 1}"#, "01", "[1] 2", "truth"] {
            assert_eq!(ExactValue::parse(broken), None, "{broken}");
        }
    }
}
