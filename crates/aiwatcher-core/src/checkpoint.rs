//! Resumable cursors.
//!
//! A checkpoint is an opaque, **lexicographically ordered** string. Emmett
//! makes the same choice (`ProcessorCheckpoint = Brand<string>`), normalising a
//! numeric position by zero-padding it to 19 characters so that string ordering
//! and numeric ordering agree. Keeping it opaque is what lets the same consumer
//! code run against a numeric log offset today and something else later.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Width that keeps `u64::MAX` (20 digits) and Emmett's 19-digit padding
/// mutually ordered — every value is padded to the same length, so a plain
/// string comparison is a numeric comparison.
const PADDED_WIDTH: usize = 20;

/// An opaque resume position in the log.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(transparent)]
pub struct Checkpoint(String);

impl Checkpoint {
    /// Start from the first event ever written.
    ///
    /// Rendered as a padded zero rather than an empty string, so a client can
    /// *say* "from the beginning" over the wire. An empty `Last-Event-ID` is
    /// indistinguishable from an absent one, and those two mean different
    /// things: replay everything, versus live only.
    #[must_use]
    pub fn beginning() -> Self {
        Self::from_global_position(0)
    }

    /// The checkpoint for a global position.
    #[must_use]
    pub fn from_global_position(position: u64) -> Self {
        Self(format!("{position:0PADDED_WIDTH$}"))
    }

    /// Read a checkpoint back from the wire (a `Last-Event-ID` header, a stored
    /// consumer offset).
    pub fn parse(value: &str) -> Result<Self, CoreError> {
        // An empty string still parses, for clients written against an earlier
        // build that rendered `beginning` that way.
        if value.is_empty() {
            return Ok(Self::beginning());
        }
        if !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CoreError::InvalidCheckpoint {
                value: value.to_owned(),
                reason: "expected a zero-padded decimal position",
            });
        }
        // Re-normalise: an unpadded value from an old client still orders
        // correctly once it goes through here.
        let position: u64 = value.parse().map_err(|_| CoreError::InvalidCheckpoint {
            value: value.to_owned(),
            reason: "does not fit in u64",
        })?;
        Ok(Self::from_global_position(position))
    }

    #[must_use]
    pub fn is_beginning(&self) -> bool {
        self.global_position() == Some(0)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The numeric position. `Some(0)` for [`Checkpoint::beginning`].
    #[must_use]
    pub fn global_position(&self) -> Option<u64> {
        self.0.parse().ok()
    }
}

impl fmt::Display for Checkpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Checkpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_beginning() {
            f.write_str("Checkpoint(BEGINNING)")
        } else {
            write!(f, "Checkpoint({})", self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_order_matches_numeric_order() {
        let ten = Checkpoint::from_global_position(10);
        let nine = Checkpoint::from_global_position(9);
        let huge = Checkpoint::from_global_position(u64::MAX);
        assert!(nine < ten, "{nine:?} should sort before {ten:?}");
        assert!(ten < huge);
        assert!(Checkpoint::beginning() < nine);
    }

    #[test]
    fn parsing_normalises_an_unpadded_value() {
        let padded = Checkpoint::from_global_position(7);
        assert_eq!(Checkpoint::parse("7").expect("valid"), padded);
        assert_eq!(Checkpoint::parse(padded.as_str()).expect("valid"), padded);
    }

    #[test]
    fn beginning_round_trips_over_the_wire() {
        let beginning = Checkpoint::beginning();
        assert!(beginning.is_beginning());
        assert_eq!(beginning.global_position(), Some(0));
        // The three spellings a client might send.
        assert_eq!(Checkpoint::parse("").expect("valid"), beginning);
        assert_eq!(Checkpoint::parse("0").expect("valid"), beginning);
        assert_eq!(
            Checkpoint::parse(beginning.as_str()).expect("valid"),
            beginning,
            "and its own rendering, which an empty string could not do"
        );
        assert!(!beginning.as_str().is_empty());
    }

    #[test]
    fn rejects_non_numeric_cursors() {
        assert!(Checkpoint::parse("abc").is_err());
        assert!(Checkpoint::parse("99999999999999999999999").is_err());
    }
}
