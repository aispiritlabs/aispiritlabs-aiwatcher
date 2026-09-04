//! What a committed decision publishes onto the event log.
//!
//! ADR_0026's table, as code. A managed execution is a producer like any other:
//! its facts ride the log under `source.service = "aiwatcher-execution"`, and
//! the folds that already exist — the workflow graph, the waterfall, `Pending`,
//! the live SSE, VictoriaTraces — draw them with no second read path.
//!
//! | The engine does | It publishes |
//! |---|---|
//! | starts a run | `workflow.declared` (the plan as nodes and edges) and `execution.requested` |
//! | dispatches an attempt that a reactor ran | `step.started` / `step.completed` / `step.failed` |
//! | stores a result | `artifact.produced`, with a digest |
//! | changes execution state | `execution.*` |
//!
//! **What is not here is the point.** `StepScheduled`, `StepRetryScheduled`,
//! `ExecutionCancelling`, `InputProvided` and every effect command produce no
//! envelope: they are decisions, leases and scheduling, and they stay in the
//! store. An engine that published one message per decision would flood the log
//! it observes. One `workflow.declared` per execution, one `step.*` pair per
//! attempt, one `artifact.produced` per artifact, and nothing else.
//!
//! **Exactly one party per attempt.** A reactor-run step is published here with
//! `data.published_by = "engine"`; a worker publishes its own attempts through
//! its own client, because it is the process that ran them and its agent spans
//! nest under them. The workflow fold flags a node that received both.

use aiwatcher_core::{EventEnvelope, EventType, MessageId, Sdk, Source};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::message::{OutboxMessage, WorkflowEvent};
use crate::plan::{ExecutionPlan, RuntimeBinding};
use crate::state::{ExecutionId, ExecutionOwner};

/// What every fact this engine publishes says it is.
///
/// Named for the role rather than the binary, because `aiwatcher serve` and
/// `aiwatcher work` are one binary and only one of them publishes these.
pub const PUBLISHER_SERVICE: &str = "aiwatcher-execution";

/// Which party ran an attempt, on `data.published_by`.
///
/// Not there to resolve a disagreement — to make one visible. Two publishers on
/// one node means a managed step's producer code opened its own `node()` scope,
/// and the fix is in the producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishedBy {
    /// A reactor in the work role.
    Engine,
    /// A worker process that claimed the attempt.
    Worker,
}

impl PublishedBy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Worker => "worker",
        }
    }
}

/// Everything a fact needs that the event itself does not carry.
#[derive(Clone, Copy, Debug)]
pub struct FactContext<'a> {
    pub execution: &'a ExecutionId,
    pub plan: &'a ExecutionPlan,
    pub owner: &'a ExecutionOwner,
    pub occurred_at: OffsetDateTime,
    /// The message this fact came from. Every envelope's `event_id` derives
    /// from it, so a redelivered publish lands on the record its first
    /// delivery created rather than beside it.
    pub cause: &'a MessageId,
}

/// The envelopes one committed fact publishes. Empty for a fact that stays in
/// the store.
#[must_use]
pub fn envelopes_for(event: &WorkflowEvent, context: &FactContext<'_>) -> Vec<EventEnvelope> {
    let mut out = Vec::new();
    match event {
        // Two facts, and they answer different questions: the shape, so the
        // panel can draw every step `Pending` before any of them runs, and the
        // request, so "who started this, when" has an answer.
        WorkflowEvent::ExecutionRequested { requested_by, .. } => {
            out.push(envelope(
                EventType::WorkflowDeclared,
                context,
                declaration(context.plan),
            ));
            out.push(envelope(
                EventType::ExecutionRequested,
                context,
                json!({
                    "plan_id": context.plan.plan_id.to_string(),
                    "owner": context.owner.as_string(),
                    "requested_by": requested_by,
                    "steps": context.plan.steps.len(),
                }),
            ));
        }
        WorkflowEvent::ExecutionStarted => {
            out.push(envelope(
                EventType::ExecutionStarted,
                context,
                base(context),
            ));
        }
        WorkflowEvent::StepStarted { step_id, attempt } => {
            out.push(envelope(
                EventType::StepStarted,
                context,
                step_payload(context, step_id, *attempt),
            ));
        }
        WorkflowEvent::StepCompleted {
            step_id,
            attempt,
            outputs,
            ..
        } => {
            out.push(envelope(
                EventType::StepCompleted,
                context,
                step_payload(context, step_id, *attempt),
            ));
            for artifact in outputs {
                out.push(envelope(
                    EventType::ArtifactProduced,
                    context,
                    json!({
                        "node": step_id,
                        "name": artifact.name,
                        "uri": artifact.uri,
                        // An engine-published artifact always carries one. The
                        // fold reads a digest when present and requires none,
                        // because a producer pointing at somebody else's bytes
                        // may not know it.
                        "digest": artifact.digest,
                        "size_bytes": artifact.size_bytes,
                        "media_type": artifact.content_type,
                        "kind": artifact.kind.as_str(),
                    }),
                ));
            }
        }
        WorkflowEvent::StepFailed {
            step_id,
            attempt,
            error,
        } => {
            let mut payload = step_payload(context, step_id, *attempt);
            if let Some(object) = payload.as_object_mut() {
                object.insert("error".to_owned(), Value::String(error.message.clone()));
                object.insert(
                    "failure_class".to_owned(),
                    Value::String(error.class.as_str().to_owned()),
                );
            }
            out.push(envelope(EventType::StepFailed, context, payload));
        }
        // A hit is a completed step, named. No `step.started` beside it: there
        // was no attempt, and a zero-duration bar in the waterfall would be
        // claiming there was.
        WorkflowEvent::StepCacheHit {
            step_id, cache_key, ..
        } => {
            let mut payload = step_payload(context, step_id, 0);
            if let Some(object) = payload.as_object_mut() {
                object.insert("state_name".to_owned(), Value::String("Cached".to_owned()));
                object.insert("cache_key".to_owned(), Value::String(cache_key.clone()));
            }
            out.push(envelope(EventType::StepCompleted, context, payload));
        }
        WorkflowEvent::InputRequested {
            step_id, request, ..
        } => {
            let mut payload = base(context);
            if let Some(object) = payload.as_object_mut() {
                object.insert("node".to_owned(), Value::String(step_id.clone()));
                object.insert("role".to_owned(), Value::String(request.role.clone()));
            }
            out.push(envelope(
                EventType::ExecutionAwaitingInput,
                context,
                payload,
            ));
        }
        WorkflowEvent::ExecutionPaused => {
            out.push(envelope(EventType::ExecutionPaused, context, base(context)));
        }
        WorkflowEvent::ExecutionResumed => {
            out.push(envelope(
                EventType::ExecutionResumed,
                context,
                base(context),
            ));
        }
        WorkflowEvent::ExecutionCompleted => {
            out.push(envelope(
                EventType::ExecutionCompleted,
                context,
                base(context),
            ));
        }
        WorkflowEvent::ExecutionFailed { reason } => {
            let mut payload = base(context);
            if let Some(object) = payload.as_object_mut() {
                object.insert("reason".to_owned(), Value::String(reason.clone()));
            }
            out.push(envelope(EventType::ExecutionFailed, context, payload));
        }
        WorkflowEvent::ExecutionCancelled => {
            out.push(envelope(
                EventType::ExecutionCancelled,
                context,
                base(context),
            ));
        }

        // Decisions, leases and scheduling. They stay in the store; the log
        // gets facts about work. See the module docs.
        WorkflowEvent::StepScheduled { .. }
        | WorkflowEvent::StepRetryScheduled { .. }
        | WorkflowEvent::StepSkipped { .. }
        | WorkflowEvent::InputProvided { .. }
        | WorkflowEvent::ExecutionCancelling { .. } => {}
    }

    // Derived from the causing message and the position in this list, so a
    // republished batch carries the ids its first publish did.
    for (ordinal, envelope) in out.iter_mut().enumerate() {
        envelope.event_id = Some(MessageId::new(crate::derive_uuid(&format!(
            "aiwatcher/execution/fact/{}/{}/{ordinal}",
            context.execution, context.cause
        ))));
    }
    out
}

/// Wrap envelopes as outbox rows, ready to be appended in the same transaction
/// as the decision that produced them.
#[must_use]
pub fn outbox_rows(envelopes: Vec<EventEnvelope>, execution: &ExecutionId) -> Vec<OutboxMessage> {
    envelopes
        .into_iter()
        .filter_map(|envelope| {
            let message_id = envelope.event_id.clone()?;
            let event_type = envelope.event_type.as_str().to_owned();
            let available_at = envelope.occurred_at;
            Some(OutboxMessage {
                message_id,
                event_type,
                // Section 11.2: one partition per decision scope, so the
                // ordering that matters is preserved without global ordering.
                partition_key: format!("workflow:{execution}"),
                payload: serde_json::to_value(envelope).unwrap_or(Value::Null),
                available_at,
                attempts: 0,
                published_at: None,
                last_error: None,
            })
        })
        .collect()
}

/// The plan as the workflow fold reads a declaration.
///
/// `version` is the `plan_id` rather than the authored revision: a declaration
/// describes what will run, and two pipelines differing only in layout compile
/// to one plan. The authored revision is provenance and travels on the dataset
/// version instead.
fn declaration(plan: &ExecutionPlan) -> Value {
    json!({
        "workflow_id": plan.definition_name,
        "name": plan.definition_name,
        "version": plan.plan_id.to_string(),
        "nodes": plan
            .steps
            .iter()
            .map(|step| json!({
                "id": step.id,
                "name": step.id,
                "kind": node_kind(&step.runtime),
            }))
            .collect::<Vec<_>>(),
        "edges": plan
            .edges
            .iter()
            .map(|edge| json!({ "from": edge.from, "to": edge.to }))
            .collect::<Vec<_>>(),
    })
}

/// What a node is called in the graph, in the vocabulary `step.*` already uses.
///
/// `chain` for work that stays in a process, and the remote kinds for what
/// leaves it — the split `catalog::step_type::is_remote` makes, so the
/// waterfall separates "we waited on somebody else" from "we were busy".
fn node_kind(runtime: &RuntimeBinding) -> &'static str {
    match runtime {
        RuntimeBinding::FlowPhp(_) => "retriever",
        RuntimeBinding::Marimo(_) | RuntimeBinding::PythonTask(_) => "chain",
        RuntimeBinding::PublishDataset(_) => "chain",
        RuntimeBinding::HumanInput(_) => "guardrail",
        RuntimeBinding::ExternalWorkflow(_) => "chain",
    }
}

fn base(context: &FactContext<'_>) -> Value {
    json!({
        "plan_id": context.plan.plan_id.to_string(),
        "owner": context.owner.as_string(),
    })
}

/// A step fact, with the two fields ADR_0012 says a node execution carries and
/// the one ADR_0026 adds.
fn step_payload(context: &FactContext<'_>, step_id: &str, attempt: u32) -> Value {
    let kind = context
        .plan
        .step(step_id)
        .map_or("chain", |step| node_kind(&step.runtime));
    json!({
        "node": step_id,
        "name": step_id,
        "step_type": kind,
        // Distinct per attempt, so the fold counts retries rather than
        // collapsing them — ADR_0012's rule for `call_id`.
        "call_id": format!("{step_id}/{attempt}"),
        "attempt": attempt,
        "published_by": PublishedBy::Engine.as_str(),
        "plan_id": context.plan.plan_id.to_string(),
    })
}

fn envelope(event_type: EventType, context: &FactContext<'_>, data: Value) -> EventEnvelope {
    let mut envelope = EventEnvelope::new(
        event_type,
        // An engine-run step happens in no agent's process, so the execution
        // *is* the run. A worker-run attempt is published by the worker, under
        // the run it opened.
        context.execution.as_str(),
        context.occurred_at,
        Source::new(PUBLISHER_SERVICE, Sdk::Rust),
    );
    envelope.workflow_id = Some(context.plan.definition_name.clone());
    envelope.workflow_run_id = Some(context.execution.as_str().to_owned());
    envelope.correlation_id = Some(aiwatcher_core::CorrelationId::new(
        context.execution.as_str(),
    ));
    envelope.causation_id = Some(aiwatcher_core::CausationId::new(context.cause.as_str()));
    envelope.data = data;
    envelope
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aiwatcher_core::{ArtifactKind, ArtifactRef};

    use super::*;
    use crate::plan::{
        CachePolicy, DefinitionKind, DefinitionRevision, PlanEdge, PlanStep, PythonTaskSpec,
        RetryPolicy,
    };
    use crate::state::{FailureClass, StepError};

    fn plan() -> ExecutionPlan {
        let step = |id: &str| PlanStep {
            id: id.to_owned(),
            runtime: RuntimeBinding::PythonTask(PythonTaskSpec {
                task_ref: "stage@1".to_owned(),
                queue: "default".to_owned(),
                params: BTreeMap::new(),
            }),
            inputs: Vec::new(),
            outputs: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_seconds: 60,
            cache: CachePolicy::Never,
        };
        ExecutionPlan::seal(
            DefinitionKind::Workflow,
            "house-import".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            vec![step("acquire"), step("persist")],
            vec![PlanEdge {
                from: "acquire".to_owned(),
                to: "persist".to_owned(),
            }],
        )
    }

    fn context<'a>(
        execution: &'a ExecutionId,
        plan: &'a ExecutionPlan,
        owner: &'a ExecutionOwner,
        cause: &'a MessageId,
    ) -> FactContext<'a> {
        FactContext {
            execution,
            plan,
            owner,
            occurred_at: OffsetDateTime::UNIX_EPOCH,
            cause,
        }
    }

    fn publish(event: &WorkflowEvent) -> Vec<EventEnvelope> {
        let execution = ExecutionId::new("exec-1");
        let plan = plan();
        let owner = ExecutionOwner::Local;
        let cause = MessageId::new("cause");
        envelopes_for(event, &context(&execution, &plan, &owner, &cause))
    }

    #[test]
    fn starting_a_run_declares_the_graph_so_every_step_draws_as_pending() {
        // The Phase 3 exit: a managed execution appears in the workflow tab
        // before any panel work exists for it.
        let envelopes = publish(&WorkflowEvent::ExecutionRequested {
            execution_id: ExecutionId::new("exec-1"),
            plan: Box::new(plan()),
            owner: ExecutionOwner::Local,
            mode: crate::state::ExecutionMode::Compiled,
            requested_by: "mk".to_owned(),
            input: BTreeMap::new(),
        });
        assert_eq!(
            envelopes
                .iter()
                .map(|e| e.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["workflow.declared", "execution.requested"]
        );

        let declaration = &envelopes[0].data;
        assert_eq!(declaration["workflow_id"], "house-import");
        assert_eq!(declaration["nodes"].as_array().expect("nodes").len(), 2);
        assert_eq!(declaration["edges"].as_array().expect("edges").len(), 1);
        // The declaration's version is the plan, not the authored revision: two
        // pipelines differing only in layout compile to one plan.
        assert_eq!(declaration["version"], plan().plan_id.to_string());
    }

    #[test]
    fn a_scheduling_decision_publishes_nothing() {
        // The line ADR_0026 draws. An engine that published one message per
        // decision would flood the log it observes.
        for event in [
            WorkflowEvent::StepScheduled {
                step_id: "acquire".to_owned(),
                attempt: 1,
                runtime: crate::RuntimeKind::PythonTask,
                cache_key: None,
            },
            WorkflowEvent::StepRetryScheduled {
                step_id: "acquire".to_owned(),
                attempt: 2,
                not_before: OffsetDateTime::UNIX_EPOCH,
            },
            WorkflowEvent::ExecutionCancelling {
                reason: String::new(),
            },
        ] {
            assert!(
                publish(&event).is_empty(),
                "{} published a decision",
                event.name()
            );
        }
    }

    #[test]
    fn an_attempt_says_which_node_which_try_and_who_ran_it() {
        let envelopes = publish(&WorkflowEvent::StepStarted {
            step_id: "acquire".to_owned(),
            attempt: 2,
        });
        let data = &envelopes[0].data;
        assert_eq!(envelopes[0].event_type.as_str(), "step.started");
        assert_eq!(data["node"], "acquire");
        // Distinct per attempt, so the fold counts a retry rather than
        // collapsing two attempts into one node execution.
        assert_eq!(data["call_id"], "acquire/2");
        assert_eq!(data["published_by"], "engine");
        assert_eq!(envelopes[0].workflow_run_id.as_deref(), Some("exec-1"));
    }

    #[test]
    fn a_result_is_published_as_a_pointer_with_a_digest_and_never_as_rows() {
        let envelopes = publish(&WorkflowEvent::StepCompleted {
            step_id: "acquire".to_owned(),
            attempt: 1,
            outputs: vec![
                ArtifactRef::new("rows", "s3://bucket/rows.jsonl", "ab".repeat(32))
                    .of_kind(ArtifactKind::Rows),
            ],
            result: None,
        });
        assert_eq!(
            envelopes
                .iter()
                .map(|e| e.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["step.completed", "artifact.produced"]
        );
        assert_eq!(envelopes[1].data["digest"], "ab".repeat(32));
        assert_eq!(envelopes[1].data["node"], "acquire");
        assert!(
            envelopes[1].data.get("rows").is_none(),
            "the registry stores no rows, and neither does the log"
        );
    }

    #[test]
    fn a_cache_hit_completes_a_node_without_claiming_it_ran() {
        // No `step.started` beside it: there was no attempt, and a
        // zero-duration bar in the waterfall would be claiming there was.
        let envelopes = publish(&WorkflowEvent::StepCacheHit {
            step_id: "acquire".to_owned(),
            cache_key: "cd".repeat(32),
            outputs: Vec::new(),
        });
        assert_eq!(envelopes.len(), 1);
        assert_eq!(envelopes[0].event_type.as_str(), "step.completed");
        assert_eq!(envelopes[0].data["state_name"], "Cached");
    }

    #[test]
    fn a_failure_carries_the_class_that_decided_whether_to_retry() {
        let envelopes = publish(&WorkflowEvent::StepFailed {
            step_id: "acquire".to_owned(),
            attempt: 1,
            error: StepError::new(FailureClass::Infrastructure, "the lease expired"),
        });
        assert_eq!(envelopes[0].event_type.as_str(), "step.failed");
        assert_eq!(envelopes[0].data["failure_class"], "infrastructure");
        assert_eq!(envelopes[0].data["error"], "the lease expired");
    }

    #[test]
    fn every_execution_fact_forms_no_span_and_names_the_engine_as_its_source() {
        for event in [
            WorkflowEvent::ExecutionStarted,
            WorkflowEvent::ExecutionPaused,
            WorkflowEvent::ExecutionResumed,
            WorkflowEvent::ExecutionCompleted,
            WorkflowEvent::ExecutionCancelled,
            WorkflowEvent::ExecutionFailed {
                reason: "acquire failed".to_owned(),
            },
        ] {
            let envelopes = publish(&event);
            assert_eq!(envelopes.len(), 1, "{}", event.name());
            assert!(
                !envelopes[0].event_type.forms_span(),
                "{} would put an execution in a waterfall",
                envelopes[0].event_type
            );
            assert_eq!(envelopes[0].source.service, PUBLISHER_SERVICE);
        }
    }

    #[test]
    fn republishing_one_fact_carries_the_ids_its_first_publish_did() {
        // What makes the outbox safe to repeat: the projector deduplicates by
        // `event_id`, so a publisher that crashed between the send and the mark
        // re-sends the same records rather than duplicates.
        let event = WorkflowEvent::StepStarted {
            step_id: "acquire".to_owned(),
            attempt: 1,
        };
        assert_eq!(publish(&event)[0].event_id, publish(&event)[0].event_id);
    }

    #[test]
    fn two_facts_of_one_decision_do_not_share_an_id() {
        let envelopes = publish(&WorkflowEvent::StepCompleted {
            step_id: "acquire".to_owned(),
            attempt: 1,
            outputs: vec![ArtifactRef::new("rows", "s3://a", "ab".repeat(32))],
            result: None,
        });
        assert_ne!(envelopes[0].event_id, envelopes[1].event_id);
    }

    #[test]
    fn an_outbox_row_partitions_by_the_execution_it_belongs_to() {
        let execution = ExecutionId::new("exec-1");
        let rows = outbox_rows(publish(&WorkflowEvent::ExecutionStarted), &execution);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].partition_key, "workflow:exec-1");
        assert_eq!(rows[0].event_type, "execution.started");
        assert!(rows[0].published_at.is_none());
    }
}
