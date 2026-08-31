//! The level above a run.
//!
//! `conversation_id` groups the runs a person would call one session. The runs
//! list and the trace view both work a level below that, which is why moving
//! between "this session" and "this LLM call" meant going back to a list and
//! re-filtering. This is the missing top of the hierarchy:
//!
//! ```text
//! conversation  ── this module
//! └── run                     = one trace
//!     └── agent               = a span
//!         ├── LLM call        = a span
//!         └── tool call       = a span
//!             └── events      = the raw log
//! ```
//!
//! Folded from the read model, like the metrics view: no extra store, and the
//! same retention window.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::ports::CompletedSpan;

use crate::dimensions::{self, DimensionFilter, DimensionKind, DimensionSummary};
use crate::readmodel::RunSummary;

#[derive(Clone, Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ConversationFilter {
    /// Only sessions with activity in the last this-many seconds. See
    /// [`crate::window`].
    pub window_seconds: Option<i64>,
    pub agent_id: Option<String>,
    /// Substring match on the conversation id. The one control that turns a
    /// long list into the session someone is actually looking for.
    pub search: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub runs: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub running: u64,
    /// Every agent seen across the conversation's runs, in first-seen order.
    pub agents: Vec<String>,
    pub llm_calls: u64,
    pub tool_calls: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// The newest event on any of the session's runs. What the list sorts by,
    /// so an active session stays at the top.
    #[serde(with = "time::serde::rfc3339")]
    pub last_activity_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ConversationPage {
    pub conversations: Vec<ConversationSummary>,
    pub total: usize,
    /// Runs with no `conversation_id`. They are reachable from the runs list
    /// but have no session to sit under, and silently dropping them would make
    /// the two views disagree about how much exists.
    pub ungrouped_runs: u64,
}

pub fn compute(
    runs: &[RunSummary],
    spans: &HashMap<String, Vec<CompletedSpan>>,
    filter: &ConversationFilter,
    now: OffsetDateTime,
) -> ConversationPage {
    // One fold, seven dimensions: see [`crate::dimensions`]. A session is the
    // `conversation_id` dimension with a name people already use, so this is a
    // rename of that page rather than a second implementation of the same
    // grouping.
    let page = dimensions::compute(
        runs,
        spans,
        DimensionKind::Session,
        &DimensionFilter {
            window_seconds: filter.window_seconds,
            agent_id: filter.agent_id.clone(),
            search: filter.search.clone(),
            after: None,
            limit: filter.limit,
        },
        now,
    );

    ConversationPage {
        conversations: page
            .rows
            .into_iter()
            .map(ConversationSummary::from)
            .collect(),
        total: page.total,
        ungrouped_runs: page.ungrouped_runs,
    }
}

impl From<DimensionSummary> for ConversationSummary {
    fn from(row: DimensionSummary) -> Self {
        Self {
            conversation_id: row.key,
            runs: row.runs,
            succeeded: row.succeeded,
            failed: row.failed,
            running: row.running,
            agents: row.agents,
            llm_calls: row.llm_calls,
            tool_calls: row.tool_calls,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cached_tokens: row.cached_tokens,
            started_at: row.started_at,
            last_activity_at: row.last_activity_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::{Checkpoint, TraceId};

    use super::*;
    use crate::readmodel::RunStatus;

    fn now() -> OffsetDateTime {
        datetime!(2026-08-27 12:00:00 UTC)
    }

    fn run(
        run_id: &str,
        conversation: Option<&str>,
        status: RunStatus,
        started: OffsetDateTime,
    ) -> RunSummary {
        RunSummary {
            run_id: run_id.to_owned(),
            conversation_id: conversation.map(ToOwned::to_owned),
            trace_id: TraceId::derive(run_id),
            status,
            agents: vec!["researcher".to_owned()],
            runtimes: vec!["agent-service".to_owned()],
            workflow: None,
            started_at: started,
            last_event_at: started,
            ended_at: None,
            duration_ms: Some(1000),
            event_count: 3,
            llm_calls: 2,
            tool_calls: 1,
            input_tokens: 100,
            output_tokens: 20,
            cached_tokens: 10,
            error: None,
            last_checkpoint: Checkpoint::from_global_position(1),
        }
    }

    #[test]
    fn runs_fold_into_the_conversation_they_belong_to() {
        let runs = vec![
            run(
                "a",
                Some("conv-1"),
                RunStatus::Succeeded,
                datetime!(2026-08-27 10:00:00 UTC),
            ),
            run(
                "b",
                Some("conv-1"),
                RunStatus::Failed,
                datetime!(2026-08-27 10:05:00 UTC),
            ),
            run(
                "c",
                Some("conv-2"),
                RunStatus::Running,
                datetime!(2026-08-27 10:01:00 UTC),
            ),
        ];
        let page = compute(
            &runs,
            &HashMap::new(),
            &ConversationFilter::default(),
            now(),
        );

        assert_eq!(page.total, 2);
        let first = &page.conversations[0];
        assert_eq!(first.conversation_id, "conv-1", "newest activity leads");
        assert_eq!(first.runs, 2);
        assert_eq!(first.succeeded, 1);
        assert_eq!(first.failed, 1);
        assert_eq!(first.input_tokens, 200);
        assert_eq!(first.started_at, datetime!(2026-08-27 10:00:00 UTC));
        assert_eq!(first.last_activity_at, datetime!(2026-08-27 10:05:00 UTC));
    }

    #[test]
    fn runs_without_a_conversation_are_counted_rather_than_dropped() {
        let runs = vec![
            run(
                "a",
                Some("conv-1"),
                RunStatus::Succeeded,
                datetime!(2026-08-27 10:00:00 UTC),
            ),
            run(
                "b",
                None,
                RunStatus::Succeeded,
                datetime!(2026-08-27 10:01:00 UTC),
            ),
        ];
        let page = compute(
            &runs,
            &HashMap::new(),
            &ConversationFilter::default(),
            now(),
        );
        assert_eq!(page.total, 1);
        assert_eq!(
            page.ungrouped_runs, 1,
            "otherwise this view and the runs list disagree about how much exists"
        );
    }

    #[test]
    fn search_narrows_by_conversation_id() {
        let runs = vec![
            run(
                "a",
                Some("chat-alpha"),
                RunStatus::Succeeded,
                datetime!(2026-08-27 10:00:00 UTC),
            ),
            run(
                "b",
                Some("chat-beta"),
                RunStatus::Succeeded,
                datetime!(2026-08-27 10:01:00 UTC),
            ),
        ];
        let page = compute(
            &runs,
            &HashMap::new(),
            &ConversationFilter {
                search: Some("alpha".to_owned()),
                ..ConversationFilter::default()
            },
            now(),
        );
        assert_eq!(page.conversations.len(), 1);
        assert_eq!(page.conversations[0].conversation_id, "chat-alpha");
    }

    #[test]
    fn agents_are_unioned_across_a_conversations_runs() {
        let mut second = run(
            "b",
            Some("conv"),
            RunStatus::Succeeded,
            datetime!(2026-08-27 10:01:00 UTC),
        );
        second.agents = vec!["planner".to_owned(), "researcher".to_owned()];
        let runs = vec![
            run(
                "a",
                Some("conv"),
                RunStatus::Succeeded,
                datetime!(2026-08-27 10:00:00 UTC),
            ),
            second,
        ];
        let page = compute(
            &runs,
            &HashMap::new(),
            &ConversationFilter::default(),
            now(),
        );
        assert_eq!(page.conversations[0].agents, vec!["researcher", "planner"]);
    }
}
