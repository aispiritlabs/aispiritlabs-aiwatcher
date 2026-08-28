//! The four ids that tie a run together, plus the message id.
//!
//! Emmett keeps `traceId` / `spanId` / `correlationId` / `causationId` side by
//! side in event metadata and treats them as two pairs: trace and span identify
//! *the operation*, correlation and causation trace *the message flow*. We keep
//! that split — see [`crate::context`] for the resolution rule.
//!
//! On top of it this module adds what an at-least-once pipeline needs and
//! Emmett does not: **deterministic derivation**. Replaying the same event must
//! produce the same `trace_id` and `span_id`, otherwise every redelivery writes
//! a duplicate span. [`TraceId::derive`] and [`SpanId::derive`] are pure
//! functions of their inputs (FNV-1a followed by an avalanche finalizer), so a
//! projector restart, a Laser redelivery and a cold replay all land on the same
//! ids.
//!
//! The finalizer is not decoration. FNV-1a alone leaves a trace id whose bits
//! barely move when two run ids differ only near the end, which is the common
//! case: `run-1`, `run-2`, `run-3`. See `avalanche_128` for the measurement
//! and the amendment to ADR_0001 for why the resulting re-derivation was worth
//! paying once.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

// FNV-1a. Chosen over SipHash because `DefaultHasher` gives no cross-version
// stability guarantee and these ids must stay identical across rebuilds.
const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV64_PRIME: u64 = 0x0000_0100_0000_01b3;
const FNV128_OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const FNV128_PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = FNV64_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV64_PRIME);
    }
    hash
}

fn fnv1a_128(bytes: &[u8]) -> u128 {
    let mut hash = FNV128_OFFSET;
    for byte in bytes {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(FNV128_PRIME);
    }
    hash
}

// The finalizer FNV-1a does not have.
//
// FNV-1a's only diffusion is the multiply, and a multiply propagates carries
// upward only. The 128-bit prime is `2^88 + 0x13b` — two narrow groups of set
// bits — so a difference confined to the *last* input byte lands as
// `d * 2^88 + d * 0x13b`, with no further rounds to spread it. Measured across
// `run-1`..`run-9`, exactly 18 of the 128 output bits can vary, in two runs:
// bits 0..=13 and 88..=91. Sequential run ids therefore derived trace ids
// sharing nine leading hex digits and an identical middle:
//
// ```text
// run-1 -> cf5c62fe3cb22757e060139f368527ff
// run-2 -> cf5c62fe3db22757e060139f3685293a
// ```
//
// Two consequences, only one of them cosmetic:
//
// * A reader cannot tell two runs apart at a glance. The panel pinches ids from
//   both ends, which is a reasonable thing to do to a 32-digit hex string in a
//   table, but it should be a display choice rather than a workaround.
// * The rightmost seven bytes stay nearly constant — 14 of 56 bits varied —
//   and those are exactly what W3C Trace Context's random-trace-id flag and
//   OpenTelemetry's consistent probability sampling read as the random part of
//   a trace id. A 1% ratio sampler over 1000 sequential runs kept 0 of them
//   instead of ~10. Nothing in this stack samples today; the Collector is
//   where that would be added, and the failure would be silent and total.
//
// What this is *not* is a collision problem. Over 5M sequential run ids the
// raw hash collided zero times, and a difference in a single input byte
// provably cannot collide: the delta is `d * PRIME` with `PRIME` odd, hence
// invertible mod 2^128, so it is non-zero for every `d != 0`.
//
// Both finalizers are bijections — `wrapping_add` inverts by `wrapping_sub`,
// and `fmix64`'s xor-shifts and odd multiplies each invert — so they change how
// ids are *distributed* and cannot change which inputs collide. Note that they fix zero: `fmix64(0) == 0` and
// `avalanche_128(0) == 0`, which is why the all-zero guard in each `derive`
// still runs after mixing rather than before.

/// MurmurHash3's 64-bit finalizer: three xor-shifts around two multiplies.
const fn fmix64(mut z: u64) -> u64 {
    z ^= z >> 33;
    z = z.wrapping_mul(0xff51_afd7_ed55_8ccd);
    z ^= z >> 33;
    z = z.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    z ^= z >> 33;
    z
}

/// Spread a 128-bit FNV-1a output across all of its bits.
///
/// The finalization half of MurmurHash3's x64-128: cross-feed the halves so
/// each depends on both, run each through [`fmix64`], cross-feed again. Takes
/// the mean single-bit avalanche from 55.6 to 64.0 of 128 bits — the ideal —
/// and takes the bits that can vary across `run-1`..`run-9` from 18 to 128.
const fn avalanche_128(raw: u128) -> u128 {
    let mut high = (raw >> 64) as u64;
    let mut low = raw as u64;
    high = high.wrapping_add(low);
    low = low.wrapping_add(high);
    high = fmix64(high);
    low = fmix64(low);
    high = high.wrapping_add(low);
    low = low.wrapping_add(high);
    ((high as u128) << 64) | (low as u128)
}

fn decode_hex<const N: usize>(kind: &'static str, value: &str) -> Result<[u8; N], CoreError> {
    if value.len() != N * 2 {
        return Err(CoreError::InvalidId {
            kind,
            value: value.to_owned(),
            reason: "wrong length",
        });
    }
    let mut out = [0u8; N];
    for (index, slot) in out.iter_mut().enumerate() {
        let byte = &value[index * 2..index * 2 + 2];
        *slot = u8::from_str_radix(byte, 16).map_err(|_| CoreError::InvalidId {
            kind,
            value: value.to_owned(),
            reason: "not hexadecimal",
        })?;
    }
    Ok(out)
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Two lowercase hex digits per byte, the W3C Trace Context encoding.
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

/// A W3C Trace Context trace id: 16 bytes, rendered as 32 lowercase hex digits.
///
/// One trace covers one *run* — a single execution of an agent. A conversation
/// spanning several runs is grouped by `conversation_id`, not by trace id;
/// putting a whole conversation in one trace makes the waterfall unreadable and
/// keeps the trace open indefinitely.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraceId([u8; 16]);

impl TraceId {
    /// The trace id every run gets when the producer does not supply one.
    ///
    /// Deterministic in `run_id`, so a redelivered `run.started` maps onto the
    /// trace its earlier delivery created.
    #[must_use]
    pub fn derive(run_id: &str) -> Self {
        let mut raw = avalanche_128(fnv1a_128(format!("aiwatcher/run/{run_id}").as_bytes()));
        // An all-zero trace id is invalid per the spec; collapse it to 1. After
        // the finalizer, which fixes zero, so this still catches the one input
        // that reaches it.
        if raw == 0 {
            raw = 1;
        }
        Self(raw.to_be_bytes())
    }

    pub fn parse(value: &str) -> Result<Self, CoreError> {
        let bytes: [u8; 16] = decode_hex("trace", value)?;
        if bytes == [0u8; 16] {
            return Err(CoreError::InvalidId {
                kind: "trace",
                value: value.to_owned(),
                reason: "all-zero trace ids are invalid",
            });
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }
}

/// A W3C Trace Context span id: 8 bytes, rendered as 16 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpanId([u8; 8]);

impl SpanId {
    /// Derive a span id from its trace and a *stable key*.
    ///
    /// The key is what makes redelivery safe: it must name the operation, not
    /// the delivery. `agent:researcher`, `llm:call-7` and `tool:search:2` are
    /// keys; a message id or a timestamp is not. See
    /// [`crate::catalog::EventType::span_key`] for how the pipeline builds one.
    #[must_use]
    pub fn derive(trace: TraceId, key: &str) -> Self {
        // The key is appended, so sibling keys inside one trace — `tool:call:1`
        // and `tool:call:2` — differ only in the last byte, the case FNV-1a
        // diffuses worst. Unmixed, 64-bit sibling span ids shared five leading
        // hex digits and an identical middle.
        let mut raw = fmix64(fnv1a_64(format!("{}|{key}", trace.to_hex()).as_bytes()));
        if raw == 0 {
            raw = 1;
        }
        Self(raw.to_be_bytes())
    }

    pub fn parse(value: &str) -> Result<Self, CoreError> {
        let bytes: [u8; 8] = decode_hex("span", value)?;
        if bytes == [0u8; 8] {
            return Err(CoreError::InvalidId {
                kind: "span",
                value: value.to_owned(),
                reason: "all-zero span ids are invalid",
            });
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }
}

macro_rules! hex_id {
    ($ty:ident, $kind:literal) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&encode_hex(&self.0))
            }
        }

        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($kind, "({})"), encode_hex(&self.0))
            }
        }

        impl std::str::FromStr for $ty {
            type Err = CoreError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }

        impl Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
                ser.serialize_str(&encode_hex(&self.0))
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(de)?;
                Self::parse(&raw).map_err(serde::de::Error::custom)
            }
        }

        impl utoipa::PartialSchema for $ty {
            fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
                utoipa::openapi::ObjectBuilder::new()
                    .schema_type(utoipa::openapi::schema::SchemaType::Type(
                        utoipa::openapi::schema::Type::String,
                    ))
                    .description(Some(concat!("Lowercase hex ", $kind, " id")))
                    .build()
                    .into()
            }
        }

        impl utoipa::ToSchema for $ty {}
    };
}

hex_id!(TraceId, "trace");
hex_id!(SpanId, "span");

/// A newtype over an opaque, producer-chosen string id.
macro_rules! string_id {
    ($ty:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
        )]
        #[serde(transparent)]
        pub struct $ty(String);

        impl $ty {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($ty), self.0)
            }
        }

        impl From<String> for $ty {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $ty {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

string_id!(
    MessageId,
    "Unique id of a single event, used for deduplication.\n\nGenerated as a UUIDv7 when the producer does not supply one; ULIDs and any\nother sortable opaque string are accepted on the wire."
);
string_id!(
    CorrelationId,
    "Groups an entire flow. Inherited unchanged by everything the flow causes."
);
string_id!(
    CausationId,
    "Names the *direct* cause of this event. Roots itself on the correlation id\nwhen nothing seeded it — the same rule Emmett's scope applies."
);

impl MessageId {
    /// A fresh, time-ordered message id.
    #[must_use]
    pub fn generate() -> Self {
        Self::new(uuid::Uuid::now_v7().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_derivation_is_stable_across_calls() {
        assert_eq!(TraceId::derive("run-1"), TraceId::derive("run-1"));
        assert_ne!(TraceId::derive("run-1"), TraceId::derive("run-2"));
    }

    /// Ids are a contract: a redelivery has to land on the span its first
    /// delivery created, including across a rebuild or a dependency bump. These
    /// are pinned so that changing the derivation is a deliberate act with a
    /// migration attached, never a quiet one.
    #[test]
    fn derived_ids_are_pinned_to_exact_values() {
        assert_eq!(
            TraceId::derive("run-1").to_hex(),
            "7b2e77b06a0816e61d5d3f929ef3f45c"
        );
        assert_eq!(
            TraceId::derive("run-42").to_hex(),
            "4b2aa2cec81978113b3de4ae8cc285c9"
        );
        assert_eq!(
            SpanId::derive(TraceId::derive("run-1"), "agent:researcher").to_hex(),
            "dc84bd03055c2c44"
        );
    }

    fn shared_prefix(left: &str, right: &str) -> usize {
        left.bytes()
            .zip(right.bytes())
            .take_while(|(a, b)| a == b)
            .count()
    }

    /// Sequential run ids are ordinary — `run-<counter>`, `session-<timestamp>`,
    /// a batch job numbering its work. Before the avalanche finalizer these
    /// derived trace ids sharing nine leading hex digits *and* an identical
    /// middle, because raw FNV-1a can only vary 18 of 128 bits when the inputs
    /// differ in their last byte.
    #[test]
    fn sequential_run_ids_do_not_derive_look_alike_trace_ids() {
        let ids: Vec<String> = (0..64)
            .map(|index| TraceId::derive(&format!("run-{index}")).to_hex())
            .collect();

        for (index, left) in ids.iter().enumerate() {
            for right in &ids[index + 1..] {
                assert!(
                    shared_prefix(left, right) < 6,
                    "{left} and {right} share {} leading hex digits",
                    shared_prefix(left, right)
                );
            }
        }
    }

    /// The span key is appended to the trace id, so siblings inside one trace
    /// differ only in their last byte — the case FNV-1a diffuses worst, and the
    /// one a trace waterfall shows side by side.
    #[test]
    fn sibling_span_keys_do_not_derive_look_alike_span_ids() {
        let trace = TraceId::derive("run-1");
        let ids: Vec<String> = (0..64)
            .map(|index| SpanId::derive(trace, &format!("tool:search:{index}")).to_hex())
            .collect();

        for (index, left) in ids.iter().enumerate() {
            for right in &ids[index + 1..] {
                assert!(
                    shared_prefix(left, right) < 5,
                    "{left} and {right} share {} leading hex digits",
                    shared_prefix(left, right)
                );
            }
        }
    }

    /// W3C Trace Context's random-trace-id flag and OpenTelemetry's consistent
    /// probability sampling both read the rightmost seven bytes as the random
    /// part of a trace id. Raw FNV-1a varied 14 of those 56 bits across
    /// sequential runs, which makes a ratio sampler keep all of them or none.
    #[test]
    fn the_rightmost_seven_bytes_vary_across_sequential_runs() {
        let base = TraceId::derive("run-0");
        let mut varying = [0u8; 7];

        for index in 1..256 {
            let other = TraceId::derive(&format!("run-{index}"));
            for (slot, (a, b)) in varying.iter_mut().zip(
                base.as_bytes()[9..]
                    .iter()
                    .zip(other.as_bytes()[9..].iter()),
            ) {
                *slot |= a ^ b;
            }
        }

        let moved: u32 = varying.iter().map(|byte| byte.count_ones()).sum();
        assert_eq!(moved, 56, "only {moved} of the rightmost 56 bits ever vary");
    }

    /// The finalizer's job, stated as a number. An ideal 128-bit hash moves
    /// half its output bits when one input bit flips; raw FNV-1a averaged 55.6
    /// and bottomed out at 7.
    #[test]
    fn flipping_one_bit_of_a_run_id_moves_about_half_the_trace_id_bits() {
        let probe = b"session-1756400000123";
        let base = TraceId::derive(&String::from_utf8_lossy(probe));
        let mut total = 0u32;
        let mut samples = 0u32;
        let mut worst = 128u32;

        for byte in 0..probe.len() {
            for bit in 0..8u8 {
                let mut flipped = probe.to_vec();
                flipped[byte] ^= 1 << bit;
                let other = TraceId::derive(&String::from_utf8_lossy(&flipped));
                let moved: u32 = base
                    .as_bytes()
                    .iter()
                    .zip(other.as_bytes().iter())
                    .map(|(a, b)| (a ^ b).count_ones())
                    .sum();
                total += moved;
                worst = worst.min(moved);
                samples += 1;
            }
        }

        let mean = f64::from(total) / f64::from(samples);
        assert!((60.0..=68.0).contains(&mean), "mean avalanche was {mean}");
        assert!(worst > 40, "worst-case avalanche was only {worst} bits");
    }

    #[test]
    fn span_derivation_is_scoped_to_its_trace() {
        let a = TraceId::derive("run-1");
        let b = TraceId::derive("run-2");
        assert_eq!(SpanId::derive(a, "llm:1"), SpanId::derive(a, "llm:1"));
        assert_ne!(SpanId::derive(a, "llm:1"), SpanId::derive(a, "llm:2"));
        assert_ne!(SpanId::derive(a, "llm:1"), SpanId::derive(b, "llm:1"));
    }

    #[test]
    fn hex_round_trips() {
        let trace = TraceId::derive("run-42");
        let hex = trace.to_hex();
        assert_eq!(hex.len(), 32);
        assert_eq!(TraceId::parse(&hex).expect("valid hex"), trace);

        let span = SpanId::derive(trace, "agent:researcher");
        let hex = span.to_hex();
        assert_eq!(hex.len(), 16);
        assert_eq!(SpanId::parse(&hex).expect("valid hex"), span);
    }

    #[test]
    fn all_zero_ids_are_rejected() {
        assert!(TraceId::parse(&"0".repeat(32)).is_err());
        assert!(SpanId::parse(&"0".repeat(16)).is_err());
    }

    #[test]
    fn malformed_hex_is_rejected() {
        assert!(TraceId::parse("nope").is_err());
        assert!(SpanId::parse("zzzzzzzzzzzzzzzz").is_err());
    }

    #[test]
    fn ids_serialize_as_plain_hex_strings() {
        let trace = TraceId::derive("run-1");
        let json = serde_json::to_string(&trace).expect("serializes");
        assert_eq!(json, format!("\"{}\"", trace.to_hex()));
    }
}
