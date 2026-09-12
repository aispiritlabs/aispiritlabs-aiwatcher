//! Whether two measurements may be subtracted, wherever they were measured.
//!
//! Two surfaces answer it. A folded report compares five optional strings a
//! producer may or may not have sent, so most of that rule is about absence;
//! published evidence compares one content address that pins the cohort, the
//! split, the suite, the scorer and the metric definitions together. The rules
//! are different because the evidence is, and collapsing them would cost the
//! second one everything that makes it stronger — but what a reader *does*
//! with the answer is the same on both, and two enums with these three names
//! would be two vocabularies one release apart. `human_input`'s reason, for a
//! verdict rather than for a question.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Comparability {
    /// The deltas are the evidence's own.
    Comparable,
    /// Something differs that makes a delta a claim nobody measured.
    Incompatible,
    /// Nothing differs; there is not enough readable evidence to say it does.
    Unverified,
}
