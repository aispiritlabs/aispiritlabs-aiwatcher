//! Which side of the project boundary one read answers on (ADR_0033, amended
//! 2026-09-19, which has the argument).
//!
//! E2 put the project in the row and decided nothing; this is the decision. A
//! read names a side before it starts and gets that side and nothing else, in
//! both directions — a row with one project is invisible to every other read,
//! the global one included.
//!
//! **Not one more axis to narrow by.** `crate::selection` holds the axes a
//! caller chooses and may widen by asking for less. A scope decides which rows
//! are there at all, including for the counts a fold takes *before* it narrows
//! anything (a dimension page's ungrouped total, `runs_retained`). So it is its
//! own argument to every read rather than a field of a filter a query string
//! fills in, and nothing a browser sends can name it.

use aiwatcher_core::ProjectScope;

/// The side of the project boundary a read answers on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReadScope {
    /// Rows no project owns. Every run written before ADR_0033, and every run
    /// a credential with no scope writes.
    #[default]
    Global,
    /// One project's rows.
    Project(ProjectScope),
}

impl ReadScope {
    /// Whether a row that says it belongs to `project` is on this side.
    #[must_use]
    pub fn admits(self, project: Option<ProjectScope>) -> bool {
        match self {
            Self::Global => project.is_none(),
            Self::Project(scope) => project == Some(scope),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn scope(byte: u8) -> ProjectScope {
        ProjectScope::new(
            Uuid::from_bytes([byte; 16]),
            Uuid::from_bytes([byte + 1; 16]),
        )
    }

    #[test]
    fn the_global_side_holds_rows_with_no_project_and_only_those() {
        assert!(ReadScope::Global.admits(None));
        assert!(!ReadScope::Global.admits(Some(scope(0xaa))));
    }

    #[test]
    fn a_project_read_holds_its_own_rows_and_never_the_global_ones() {
        // Both halves matter. The first is what a project member asked for;
        // the second is what keeps a project's list from being every run on
        // the instance with a badge on some of them.
        let mine = ReadScope::Project(scope(0xaa));
        assert!(mine.admits(Some(scope(0xaa))));
        assert!(!mine.admits(Some(scope(0xbb))));
        assert!(!mine.admits(None));
    }

    #[test]
    fn a_read_that_named_no_side_reads_the_side_that_was_always_instance_wide() {
        // Fail closed: the default must never be a project's rows, because the
        // reader that fell back to it proved nothing about who was asking.
        assert_eq!(ReadScope::default(), ReadScope::Global);
        assert!(!ReadScope::default().admits(Some(scope(0xaa))));
    }
}
