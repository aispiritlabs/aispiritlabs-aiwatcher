//! The event taxonomy, and the rule that turns events into spans.
//!
//! **An event is not a span.** A run emits ten to thousands of events; the
//! trace it should produce has a handful of spans. The mapping is:
//!
//! ```text
//! run                             trace
//! └── agent execution             span    agent.started  → agent.completed
//!     ├── LLM call                span    llm.started    → llm.completed
//!     │   ├── first token         span event
//!     │   └── chunks              live only, never a span
//!     └── tool call               span    tool.started   → tool.completed
//! ```
//!
//! [`Phase`] is what encodes it: a `Start` opens a span, an `End` closes it, a
//! `Point` becomes a span event or is dropped after the live fan-out.

use std::fmt;

use serde::{Deserialize, Serialize};

/// What an event is about. Decides the span's name and kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    Run,
    Agent,
    Llm,
    Tool,
    /// Anything else with a start and an end: a retrieval, an embedding, a
    /// rerank, a parse, a guardrail, a plain chain node.
    ///
    /// One subject rather than one per kind. The specific kind travels in
    /// `data.step_type`, so a producer can introduce `rerank` or `guardrail`
    /// without a backend release — the same reason [`EventType::Unknown`]
    /// exists. A frozen enum here would make every new step type a deploy.
    Step,
    /// One execution of an evaluation suite: parameters in, metrics and a
    /// report out.
    ///
    /// Forms no span — see [`EventType::forms_span`].
    Eval,
    /// The shape of an orchestration and what it produced: a declared topology,
    /// and the artifacts its nodes handed on.
    ///
    /// Forms no span either, and for the same reason as [`Self::Eval`]: a
    /// topology is a document and an artifact is a pointer. Neither is
    /// something that happened to a request. The *execution* of a node is a
    /// [`Self::Step`], which does form a span.
    Workflow,
    Unknown,
}

impl Subject {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Agent => "agent",
            Self::Llm => "llm",
            Self::Tool => "tool",
            Self::Step => "step",
            Self::Eval => "eval",
            Self::Workflow => "workflow",
            Self::Unknown => "unknown",
        }
    }
}

/// Where in a span's life this event sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub enum Phase {
    /// Opens a span.
    Start,
    /// Neither opens nor closes one: a span event at most.
    Point,
    /// Closes a span. `ok` becomes the span status.
    End { ok: bool },
}

impl Phase {
    #[must_use]
    pub const fn is_start(self) -> bool {
        matches!(self, Self::Start)
    }

    #[must_use]
    pub const fn is_end(self) -> bool {
        matches!(self, Self::End { .. })
    }
}

macro_rules! event_catalog {
    ($( $variant:ident => $wire:literal, $subject:expr, $phase:expr );+ $(;)?) => {
        /// Every event type aiwatcher understands, plus a passthrough for the
        /// ones it does not.
        ///
        /// [`EventType::Unknown`] is deliberate: a producer running a newer SDK
        /// must not have its events rejected. They are stored and streamed
        /// live, they simply take part in no span.
        #[derive(Clone, Debug, PartialEq, Eq, Hash, utoipa::ToSchema)]
        pub enum EventType {
            $($variant,)+
            Unknown(String),
        }

        impl EventType {
            /// Every known type, in catalog order. Used by docs and tests.
            pub const KNOWN: &'static [EventType] = &[$(EventType::$variant),+];

            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $wire,)+
                    Self::Unknown(raw) => raw.as_str(),
                }
            }

            /// Never fails: an unrecognised type is kept verbatim.
            #[must_use]
            pub fn parse(value: &str) -> Self {
                match value {
                    $($wire => Self::$variant,)+
                    other => Self::Unknown(other.to_owned()),
                }
            }

            #[must_use]
            pub fn subject(&self) -> Subject {
                match self {
                    $(Self::$variant => $subject,)+
                    Self::Unknown(_) => Subject::Unknown,
                }
            }

            /// `None` for an unknown type — it takes part in no span.
            #[must_use]
            pub fn phase(&self) -> Option<Phase> {
                match self {
                    $(Self::$variant => Some($phase),)+
                    Self::Unknown(_) => None,
                }
            }
        }
    };
}

event_catalog! {
    RunStarted      => "run.started",      Subject::Run,   Phase::Start;
    RunCompleted    => "run.completed",    Subject::Run,   Phase::End { ok: true };
    RunFailed       => "run.failed",       Subject::Run,   Phase::End { ok: false };

    AgentStarted    => "agent.started",    Subject::Agent, Phase::Start;
    AgentCompleted  => "agent.completed",  Subject::Agent, Phase::End { ok: true };
    AgentFailed     => "agent.failed",     Subject::Agent, Phase::End { ok: false };
    // One agent addressing another. A span *event* on the sending agent's span,
    // never a span of its own: a handoff has a moment, not a duration.
    AgentMessage    => "agent.message",    Subject::Agent, Phase::Point;

    LlmStarted      => "llm.started",      Subject::Llm,   Phase::Start;
    LlmFirstToken   => "llm.first_token",  Subject::Llm,   Phase::Point;
    LlmChunk        => "llm.chunk",        Subject::Llm,   Phase::Point;
    LlmCompleted    => "llm.completed",    Subject::Llm,   Phase::End { ok: true };
    LlmFailed       => "llm.failed",       Subject::Llm,   Phase::End { ok: false };

    ToolStarted     => "tool.started",     Subject::Tool,  Phase::Start;
    ToolCompleted   => "tool.completed",   Subject::Tool,  Phase::End { ok: true };
    ToolFailed      => "tool.failed",      Subject::Tool,  Phase::End { ok: false };

    StepStarted     => "step.started",     Subject::Step,  Phase::Start;
    StepCompleted   => "step.completed",   Subject::Step,  Phase::End { ok: true };
    StepFailed      => "step.failed",      Subject::Step,  Phase::End { ok: false };

    // Phases, but no spans. See `EventType::forms_span`.
    EvalStarted     => "eval.started",     Subject::Eval,  Phase::Start;
    EvalCase        => "eval.case",        Subject::Eval,  Phase::Point;
    EvalCompleted   => "eval.completed",   Subject::Eval,  Phase::End { ok: true };
    EvalFailed      => "eval.failed",      Subject::Eval,  Phase::End { ok: false };

    // No phases and no spans: neither is an execution. The topology is what a
    // node's execution is drawn against, and an artifact is what one produced.
    WorkflowDeclared => "workflow.declared", Subject::Workflow, Phase::Point;
    ArtifactProduced => "artifact.produced", Subject::Workflow, Phase::Point;
}

/// The step kinds this build knows how to name and classify.
///
/// Not exhaustive and not enforced: an unrecognised `step_type` still produces
/// a span, named after itself. These are the ones that get a specific span kind
/// and a specific name shape.
pub mod step_type {
    /// Leaves the process — a vector store, an embedding endpoint.
    pub const RETRIEVER: &str = "retriever";
    pub const EMBEDDING: &str = "embedding";
    pub const RERANKER: &str = "reranker";
    /// Stays in the process.
    pub const CHAIN: &str = "chain";
    pub const PARSER: &str = "parser";
    pub const GUARDRAIL: &str = "guardrail";
    pub const MEMORY: &str = "memory";

    /// Whether a step of this kind makes a call out of the process.
    ///
    /// Decides the OpenTelemetry span kind, which is what a trace UI uses to
    /// tell "we waited on someone else" from "we were busy".
    #[must_use]
    pub fn is_remote(step_type: &str) -> bool {
        matches!(
            step_type.to_ascii_lowercase().as_str(),
            RETRIEVER | EMBEDDING | RERANKER
        )
    }
}

impl EventType {
    /// A step's kind, from its payload. `None` for everything else.
    #[must_use]
    pub fn step_type<'a>(&self, data: &'a serde_json::Value) -> Option<&'a str> {
        if self.subject() != Subject::Step {
            return None;
        }
        data.get("step_type")
            .or_else(|| data.get("span_type"))
            .and_then(serde_json::Value::as_str)
    }

    /// Whether this event should ever be written to trace storage.
    ///
    /// `llm.chunk` is the reason this exists. Streaming a 2000-token response
    /// emits 2000 chunk events for one LLM call; they belong on the live
    /// channel and in the log, never as 2000 trace records.
    #[must_use]
    pub fn is_high_cardinality(&self) -> bool {
        matches!(self, Self::LlmChunk)
    }

    /// Whether this event takes part in a span at all.
    ///
    /// Two subjects are withheld, for one reason between them: what they carry
    /// is a document, not something that happened to a request.
    ///
    /// An evaluation report has a start, an end and a duration — the
    /// evaluation projection reads all three — and still belongs in no trace.
    /// Scoring a suite is a batch job whose payload is a document, and writing
    /// it to the trace store would put a report where a span goes. The phase is
    /// kept because the projection needs it; the span is what is withheld.
    ///
    /// A workflow declaration is the *shape* of an orchestration and an
    /// artifact is a *pointer* to a byte range somebody else stored. A shape
    /// has no duration at all, and a waterfall that showed one would be showing
    /// the moment a producer got round to describing itself. The executions
    /// drawn against that shape are `step.*`, and those do form spans.
    ///
    /// Distinct from [`Self::is_high_cardinality`], which suppresses a *record*
    /// for an event that still belongs to a span.
    #[must_use]
    pub fn forms_span(&self) -> bool {
        !matches!(self.subject(), Subject::Eval | Subject::Workflow)
    }

    /// The stable key a span id derives from when the producer sent none.
    ///
    /// `call_id` is what distinguishes two LLM calls inside one agent; without
    /// it, parallel calls would collapse into one span. Producers that cannot
    /// supply a `call_id` must send an explicit `span_id` instead.
    #[must_use]
    pub fn span_key(&self, call_id: Option<&str>, agent_id: Option<&str>) -> String {
        match self.subject() {
            Subject::Run => "run".to_owned(),
            Subject::Agent => format!("agent:{}", agent_id.unwrap_or("default")),
            subject @ (Subject::Llm | Subject::Tool) => format!(
                "{}:{}:{}",
                subject.as_str(),
                agent_id.unwrap_or("default"),
                call_id.unwrap_or("default"),
            ),
            Subject::Step => format!(
                "step:{}:{}",
                agent_id.unwrap_or("default"),
                call_id.unwrap_or("default")
            ),
            // One key per evaluation, so a redelivered report lands on the
            // row it already wrote instead of a second one.
            Subject::Eval => "eval".to_owned(),
            // Never used for a span — see `forms_span`. Derived anyway, and
            // derived per event type rather than per subject, because
            // `record` computes it unconditionally and a key that collided
            // across the two types would be a lie waiting for the day one of
            // them starts forming a span.
            Subject::Workflow => format!("workflow:{}", self.as_str()),
            Subject::Unknown => format!("event:{}", self.as_str()),
        }
    }

    /// The span name shown in the waterfall.
    ///
    /// Follows the OpenTelemetry GenAI convention of `<operation> <target>`
    /// (e.g. `chat gpt-5`) where a target is known, falling back to the
    /// subject.
    #[must_use]
    pub fn span_name(&self, target: Option<&str>) -> String {
        match (self.subject(), target) {
            (Subject::Llm, Some(model)) => format!("chat {model}"),
            (Subject::Llm, None) => "chat".to_owned(),
            (Subject::Tool, Some(tool)) => format!("execute_tool {tool}"),
            (Subject::Tool, None) => "execute_tool".to_owned(),
            (Subject::Agent, Some(agent)) => format!("invoke_agent {agent}"),
            (Subject::Agent, None) => "invoke_agent".to_owned(),
            (Subject::Run, _) => "run".to_owned(),
            // A step names itself after its kind, so `retriever knowledge_base`
            // reads the same way `chat gpt-5` does.
            (Subject::Step, Some(target)) => target.to_owned(),
            (Subject::Step, None) => "step".to_owned(),
            (Subject::Eval | Subject::Workflow | Subject::Unknown, _) => self.as_str().to_owned(),
        }
    }
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EventType {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(value))
    }
}

impl Serialize for EventType {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EventType {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(de)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_type_round_trips_through_its_wire_name() {
        for event_type in EventType::KNOWN {
            assert_eq!(&EventType::parse(event_type.as_str()), event_type);
        }
    }

    #[test]
    fn an_unrecognised_type_is_kept_verbatim_rather_than_rejected() {
        let unknown = EventType::parse("guardrail.tripped");
        assert_eq!(unknown, EventType::Unknown("guardrail.tripped".to_owned()));
        assert_eq!(unknown.as_str(), "guardrail.tripped");
        assert_eq!(unknown.subject(), Subject::Unknown);
        assert_eq!(unknown.phase(), None);
    }

    #[test]
    fn every_subject_has_a_matched_start_and_end() {
        // `Subject::Workflow` is deliberately absent: a declaration and an
        // artifact are points, and a topology has no end to wait for. The
        // execution that does have one is a `Subject::Step`.
        for subject in [
            Subject::Run,
            Subject::Agent,
            Subject::Llm,
            Subject::Tool,
            Subject::Step,
            Subject::Eval,
        ] {
            let of_subject: Vec<_> = EventType::KNOWN
                .iter()
                .filter(|event_type| event_type.subject() == subject)
                .collect();
            assert!(
                of_subject.iter().any(|e| e.phase() == Some(Phase::Start)),
                "{subject:?} has no start event"
            );
            assert!(
                of_subject
                    .iter()
                    .any(|e| e.phase() == Some(Phase::End { ok: true })),
                "{subject:?} has no success end event"
            );
            assert!(
                of_subject
                    .iter()
                    .any(|e| e.phase() == Some(Phase::End { ok: false })),
                "{subject:?} has no failure end event"
            );
        }
    }

    #[test]
    fn an_evaluation_has_phases_but_takes_part_in_no_span() {
        // The phase is what the evaluation projection folds on — a report is
        // running until its end arrives, and failed if that end is a failure.
        assert_eq!(EventType::EvalStarted.phase(), Some(Phase::Start));
        assert_eq!(
            EventType::EvalCompleted.phase(),
            Some(Phase::End { ok: true })
        );
        assert_eq!(
            EventType::EvalFailed.phase(),
            Some(Phase::End { ok: false })
        );
        // And none of it reaches the trace store: a report is not a trace.
        for event_type in EventType::KNOWN {
            assert_eq!(
                event_type.forms_span(),
                !matches!(event_type.subject(), Subject::Eval | Subject::Workflow),
                "{event_type} disagrees with its subject about being traced"
            );
        }
    }

    #[test]
    fn a_declared_topology_and_an_artifact_form_no_span() {
        // The reason is not the same as the evaluation's, though the rule is:
        // a shape has no duration, and an artifact is a pointer to bytes
        // somebody else stored. Putting either in a waterfall would be showing
        // the moment a producer described itself.
        for event_type in [EventType::WorkflowDeclared, EventType::ArtifactProduced] {
            assert_eq!(event_type.subject(), Subject::Workflow);
            assert_eq!(event_type.phase(), Some(Phase::Point));
            assert!(!event_type.forms_span(), "{event_type} must not be traced");
        }
    }

    #[test]
    fn a_declaration_and_an_artifact_do_not_share_a_span_key() {
        // Neither key reaches a span today. They are still distinct, because
        // `record` derives one unconditionally and a shared key would be a lie
        // waiting for the day one of them starts forming a span.
        assert_ne!(
            EventType::WorkflowDeclared.span_key(None, None),
            EventType::ArtifactProduced.span_key(None, None),
        );
    }

    #[test]
    fn an_agent_message_belongs_to_the_sending_agents_span() {
        // A handoff has a moment, not a duration, so it is a span event rather
        // than a span. Sharing the agent's key is what puts it there.
        let message = EventType::AgentMessage.span_key(None, Some("planner"));
        let agent = EventType::AgentStarted.span_key(None, Some("planner"));
        assert_eq!(message, agent);
        assert_eq!(EventType::AgentMessage.phase(), Some(Phase::Point));
        assert!(EventType::AgentMessage.forms_span());
    }

    #[test]
    fn every_event_of_one_evaluation_shares_a_span_key() {
        // Which is what makes a redelivered report land on the row it already
        // wrote: the evaluation is identified by its run id, and there is only
        // ever one scope inside it.
        let started = EventType::EvalStarted.span_key(Some("case-1"), None);
        let case = EventType::EvalCase.span_key(Some("case-2"), None);
        let completed = EventType::EvalCompleted.span_key(None, Some("judge"));
        assert_eq!(started, "eval");
        assert_eq!(started, case);
        assert_eq!(started, completed);
    }

    #[test]
    fn only_chunks_are_treated_as_high_cardinality() {
        let high: Vec<_> = EventType::KNOWN
            .iter()
            .filter(|e| e.is_high_cardinality())
            .collect();
        assert_eq!(high, vec![&EventType::LlmChunk]);
    }

    #[test]
    fn parallel_calls_in_one_agent_get_distinct_span_keys() {
        let first = EventType::LlmStarted.span_key(Some("call-1"), Some("researcher"));
        let second = EventType::LlmStarted.span_key(Some("call-2"), Some("researcher"));
        assert_ne!(first, second);
    }

    #[test]
    fn a_start_and_its_end_share_one_span_key() {
        let started = EventType::LlmStarted.span_key(Some("call-1"), Some("a"));
        let completed = EventType::LlmCompleted.span_key(Some("call-1"), Some("a"));
        let failed = EventType::LlmFailed.span_key(Some("call-1"), Some("a"));
        assert_eq!(started, completed);
        assert_eq!(started, failed);
    }

    #[test]
    fn a_step_carries_its_kind_in_the_payload_rather_than_the_event_type() {
        let retrieval = EventType::StepStarted;
        let data = serde_json::json!({ "step_type": "retriever", "name": "knowledge_base" });
        assert_eq!(retrieval.step_type(&data), Some("retriever"));
        assert_eq!(retrieval.subject(), Subject::Step);

        // `span_type` is accepted too: it is what the agentic tracer already
        // calls the field.
        let alias = serde_json::json!({ "span_type": "RETRIEVER" });
        assert_eq!(retrieval.step_type(&alias), Some("RETRIEVER"));

        // A kind this build has never heard of still produces a step.
        let novel = serde_json::json!({ "step_type": "policy_check" });
        assert_eq!(retrieval.step_type(&novel), Some("policy_check"));

        // And nothing else reads the field.
        assert_eq!(
            EventType::LlmStarted.step_type(&data),
            None,
            "only a step has a step type"
        );
    }

    #[test]
    fn remote_step_kinds_are_the_ones_that_leave_the_process() {
        for remote in ["retriever", "EMBEDDING", "Reranker"] {
            assert!(step_type::is_remote(remote), "{remote}");
        }
        for local in ["chain", "parser", "guardrail", "memory", "whatever"] {
            assert!(!step_type::is_remote(local), "{local}");
        }
    }

    #[test]
    fn two_steps_in_one_agent_get_distinct_span_keys() {
        let first = EventType::StepStarted.span_key(Some("s1"), Some("a"));
        let second = EventType::StepStarted.span_key(Some("s2"), Some("a"));
        assert_ne!(first, second);
        assert_eq!(
            EventType::StepCompleted.span_key(Some("s1"), Some("a")),
            first,
            "a start and its end share one key"
        );
    }

    #[test]
    fn span_names_follow_the_genai_operation_target_convention() {
        assert_eq!(EventType::LlmStarted.span_name(Some("gpt-5")), "chat gpt-5");
        assert_eq!(
            EventType::ToolStarted.span_name(Some("web_search")),
            "execute_tool web_search"
        );
        assert_eq!(EventType::RunStarted.span_name(None), "run");
    }
}
