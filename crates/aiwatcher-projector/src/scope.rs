//! Which side of the project boundary one read answers on.
//!
//! E2 put the project **in the row**: a run, a dimension row, a span, a period
//! and a journal entry each say whose they are, and absence is the global side
//! (ADR_0033, `crate::dimensions`). That was a fact and no decision — the rows
//! knew whose they were and every read still answered with all of them.
//!
//! This is the decision. A read names a side before it starts, and gets that
//! side and nothing else:
//!
//! * [`ReadScope::Global`] is every row with no project — every run this build
//!   has ever written, and every run a credential with no scope writes next. It
//!   is what the routes without a project in their path answer, under instance
//!   authorization, exactly as they always have.
//! * [`ReadScope::Project`] is one project's rows and nothing else. It is what
//!   `/api/v1/orgs/{organization}/projects/{project}/…` answers, after a grant
//!   check asked of IAM on that request.
//!
//! **A scope is not one more axis to narrow by.** `crate::selection` holds the
//! axes a caller chooses — a session, an agent, a model — and a caller may
//! widen any of them by asking for less. A scope is the opposite: it decides
//! which rows are there at all, including for the counts a fold takes *before*
//! it narrows anything, like a dimension page's ungrouped total or the metrics
//! summary's `runs_retained`. So it arrives as its own argument to every read
//! rather than as a field of a filter struct that a query string fills in, and
//! nothing a browser sends can name it.
//!
//! **Absence is the global side, in both directions.** A row with no project is
//! invisible to a project read, and a row with one is invisible to a global
//! read. The second half is what makes this a boundary rather than a label: an
//! instance viewer who kept seeing a project's runs on `/api/v1/runs` would be
//! reading a project they hold no grant on, and the additive route family would
//! have added nothing.
//!
//! [`Self::default`] is [`Self::Global`], which is the side that fails closed:
//! a reader that forgot to name one sees the rows that were always instance-wide
//! and never a project's.

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
