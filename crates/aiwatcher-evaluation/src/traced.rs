//! What the traces of generated answers show they were made with.
//!
//! A variant pins a prompt, a model and a workflow, which a task resolves or
//! runs rather than holds, so the witness for those is telemetry this deployment
//! folded: each answer may name the run it was made in, whose calls say which
//! prompt and model versions served them and whose declaration says which
//! workflow shape it executed. The traces step reads those runs before the
//! score step reads an answer.
//!
//! It refuses what the traces contradict — another variant or result, another
//! version of the pinned prompt or model, the pinned workflow in another shape
//! or off its nodes — and counts what they merely do not show: telemetry is
//! best effort, and a run the log never received says nothing either way.
//!
//! The application's telemetry comes from the host of its answers, so one that
//! reported the pins while calling something else passes. What it cannot report
//! for itself is another credential's word: a serving host's own run naming the
//! call it served is a second witness, counted apart (ADR_0030, amended).

use std::collections::BTreeMap;

use aiwatcher_core::topology::Topology;
use serde::{Deserialize, Serialize};

use crate::{RecordedAnswer, VariantManifest};

/// What the traces step writes for the score step to read: a row per answer.
pub const GENERATION_TRACES: &str = "traces";

/// One model call, as its span says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TracedCall {
    pub model: Option<String>,
    pub model_version: Option<String>,
    pub prompt_name: Option<String>,
    pub prompt_version: Option<String>,
    /// What the provider said served the call, `gen_ai.response.model`.
    pub served_model: Option<String>,
    /// Whether a host that saw the request's text found the named prompt
    /// version's template in it: a gateway's word, absent from the
    /// application's own calls.
    pub prompt_verified: Option<bool>,
    /// Whether that host found the request's text to be nothing but the
    /// template rendered and the values it was rendered with.
    pub prompt_exact: Option<bool>,
    /// The credential both ends of the call's span were published under.
    pub published_by: Option<String>,
    /// What the call reported using.
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    /// A witness's keyed digests of what the request held and what came back
    /// ([`aiwatcher_core::witness`]); empty on the application's own calls.
    pub asked: Vec<String>,
    pub replied: Vec<String>,
    /// The same witness's digests of each value the named template was found
    /// rendered with, made as a reply's are.
    pub rendered: Vec<String>,
}

/// Who may witness a generated answer, the key each one's digests of a call's
/// words are made under, and the inputs of the cases those digests are tested
/// for.
#[derive(Clone, Default)]
pub struct Witnesses {
    named: Vec<String>,
    keys: BTreeMap<String, [u8; 32]>,
    inputs: BTreeMap<String, serde_json::Value>,
    spelled: BTreeMap<String, String>,
}

impl std::fmt::Debug for Witnesses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the keys: each is derived from a credential's secret.
        f.debug_struct("Witnesses")
            .field("named", &self.named)
            .field("keyed", &self.keys.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Witnesses {
    /// Only these credentials witness; naming none, any credential other than
    /// the answer's own does.
    #[must_use]
    pub fn named(named: Vec<String>) -> Self {
        Self {
            named,
            ..Self::default()
        }
    }

    /// Each credential's witness key ([`aiwatcher_core::witness::key_for`]):
    /// a witness whose key is not here says nothing about a call's words.
    #[must_use]
    pub fn keyed(mut self, keys: impl IntoIterator<Item = (String, [u8; 32])>) -> Self {
        self.keys.extend(keys);
        self
    }

    /// Each case's input, which a witness's digests of a request are tested
    /// for.
    #[must_use]
    pub fn asked(mut self, inputs: BTreeMap<String, serde_json::Value>) -> Self {
        self.inputs = inputs;
        self
    }

    /// Each case's answer as the generation step spelled it, in JSON: what a
    /// witness's digests of a reply are tested for where the answer holds an
    /// integer a parsed answer would round.
    #[must_use]
    pub fn spelled(mut self, answers: BTreeMap<String, String>) -> Self {
        self.spelled = answers;
        self
    }

    /// The texts an answer is compared with a witness's digests as: from its
    /// JSON as the generation step spelled it, where that was kept, so every
    /// integer is compared digit for digit.
    fn answered_as(&self, answer: &RecordedAnswer) -> Vec<String> {
        self.spelled.get(&answer.case_id).map_or_else(
            || aiwatcher_core::witness::answered_as(&answer.answer),
            |spelled| aiwatcher_core::witness::answered_as_text(spelled),
        )
    }

    /// The credentials named.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.named
    }

    fn admits(&self, witness: &str) -> bool {
        self.named.is_empty() || self.named.iter().any(|named| named == witness)
    }
}

/// A run an answer names, as the log folded it once it had ended and every
/// call it started had a span.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TracedRun {
    /// The trace the log derived for it, which the answer did not have to know.
    pub trace_id: Option<String>,
    pub variant_id: Option<String>,
    pub evaluation_id: Option<String>,
    pub calls: Vec<TracedCall>,
    /// The credential the run's start was published under.
    pub published_by: Option<String>,
    /// The workflow the run names, and the digest of the shape it declared.
    pub workflow: Option<String>,
    pub workflow_topology: Option<String>,
    /// The workflow nodes it started a step of.
    pub nodes_run: Vec<String>,
    /// Each node step's start and end, in the order its log holds them.
    pub node_steps: Vec<StepSeen>,
    /// It took more node steps than its log's fold keeps, so the order of the
    /// rest was not read.
    pub node_steps_dropped: bool,
    /// Calls other runs say they served for this one: a serving host's run
    /// naming it as the caller, each call with its own publisher.
    pub served_for_it: Vec<TracedCall>,
}

/// One step of a workflow node, as a run's log holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepSeen {
    Started(String),
    Completed(String),
    Failed(String),
}

/// What the traces showed about one answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TracedAnswer {
    pub case_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The run it names was on the log, ended.
    pub seen: bool,
    /// That run's trace, so a case leads to it even when the application could
    /// not say its trace ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// A call in that run rendered the pinned prompt version. Absent when the
    /// variant pins no prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_prompt: Option<bool>,
    /// A call in that run was served by the pinned model version. Absent when
    /// the variant pins no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_model: Option<bool>,
    /// The run declared the pinned workflow's shape and stepped only through
    /// its nodes. Absent when the variant pins no workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_workflow: Option<bool>,
    /// A run published under another credential than this answer's run said it
    /// served one of its calls on the pinned model version. Absent when the
    /// variant pins no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_model: Option<bool>,
    /// Such a run matched the pinned prompt version's template against the
    /// text of the request it relayed. Absent when the variant pins no prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_prompt: Option<bool>,
    /// Such a run relayed a reply that is this answer, word for word — so the
    /// answer came back through the witness rather than around it. Absent
    /// when the variant pins neither a model nor a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_answer: Option<bool>,
    /// Such a run's request held the case's input — a message, or a value it
    /// found the pinned template rendered with. Absent when the variant pins
    /// neither a model nor a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_input: Option<bool>,
    /// One such call did all of it at once: it relayed this answer as its
    /// reply, to a request that was nothing but the pinned prompt rendered —
    /// the answer not in it — with values that are each the case's input or a
    /// part of it, or the reply of another call so made. Absent when the
    /// variant pins no prompt, which is what says what a request should hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_exchange: Option<bool>,
    /// The credentials whose runs witnessed it, each once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub witnessed_by: Vec<String>,
    /// A serving run named the pinned model or prompt under the answer's own
    /// credential — the application's word again, and no witness.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub self_witnessed: bool,
    /// What providers said served the run's calls, each once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub served_models: Vec<String>,
    /// The run's calls by the model each named, with what they reported
    /// using: what a price table prices a generated case by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<aiwatcher_core::prices::ModelUsage>,
    /// The variant pins a workflow whose declaration this step could read no
    /// node of, so no run was seen executing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workflow_undeclared: bool,
    /// The run took more node steps than the fold keeps, so the order of the
    /// rest was not read and it is not seen executing the pinned workflow.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workflow_steps_unread: bool,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// One model a provider said served generated answers, and how many.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GenerationServed {
    pub model: String,
    /// Answers whose run had a call this model served.
    pub answers: usize,
}

/// What a generated result says the traces of its answers showed.
///
/// Counts rather than a verdict: how many answers there were, how many named
/// the run they were made in, how many of those runs the log held, and how
/// many ran on each pin. A reader — or a gate — decides whether fewer than all
/// is enough.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GenerationTrace {
    pub answers: usize,
    /// Answers naming the run they were made in.
    pub named: usize,
    /// Of those, runs this deployment's log held, ended, when the step looked.
    pub seen: usize,
    /// Seen runs with a call on the pinned prompt version; absent when the
    /// variant pins no prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_prompt: Option<usize>,
    /// Seen runs with a call served by the pinned model version; absent when
    /// the variant pins no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_model: Option<usize>,
    /// Seen runs that declared the pinned workflow's shape and stepped only
    /// through its nodes; absent when the variant pins no workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_workflow: Option<usize>,
    /// The declaration of the pinned workflow names no node this step could
    /// read, so no run can be seen executing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workflow_undeclared: bool,
    /// Seen runs that took more node steps than the fold keeps, whose order
    /// past them was therefore not read.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub steps_unread: usize,
    /// Seen runs whose call on the pinned model version a run published under
    /// another credential says it served; absent when the variant pins no model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_model: Option<usize>,
    /// Seen runs whose request such a run matched against the pinned prompt
    /// version's template; absent when the variant pins no prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_prompt: Option<usize>,
    /// Answers that are, word for word, a reply such a run relayed for their
    /// run; absent when the variant pins neither a model nor a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_answer: Option<usize>,
    /// Answers whose case's input such a run found in the request it relayed;
    /// absent when the variant pins neither a model nor a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_input: Option<usize>,
    /// Answers one witnessed call relayed as its reply to a request that was
    /// nothing but the pinned prompt, rendered with their case's input or with
    /// what a call so made had replied; absent when the variant pins no prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_exchange: Option<usize>,
    /// Answers whose serving runs were published under their own run's
    /// credential, which witnesses nothing: one token on two hosts.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub self_witnessed: usize,
    /// The credentials that witnessed any answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub witnesses: Vec<String>,
    /// What providers said served the calls, compared with nothing: a provider's
    /// name for a model is an alias, a file or a dated snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub served: Vec<GenerationServed>,
}

impl GenerationTrace {
    #[must_use]
    pub fn of(rows: &[TracedAnswer]) -> Self {
        let counted = |side: fn(&TracedAnswer) -> Option<bool>| {
            rows.iter()
                .map(side)
                .collect::<Option<Vec<bool>>>()
                .filter(|_| !rows.is_empty())
                .map(|sides| sides.into_iter().filter(|on| *on).count())
        };
        let mut served: BTreeMap<&str, usize> = BTreeMap::new();
        for row in rows {
            for model in &row.served_models {
                *served.entry(model.as_str()).or_default() += 1;
            }
        }
        Self {
            answers: rows.len(),
            named: rows.iter().filter(|row| row.run_id.is_some()).count(),
            seen: rows.iter().filter(|row| row.seen).count(),
            on_prompt: counted(|row| row.on_prompt),
            on_model: counted(|row| row.on_model),
            on_workflow: counted(|row| row.on_workflow),
            workflow_undeclared: rows.iter().any(|row| row.workflow_undeclared),
            steps_unread: rows.iter().filter(|row| row.workflow_steps_unread).count(),
            witnessed_model: counted(|row| row.witnessed_model),
            witnessed_prompt: counted(|row| row.witnessed_prompt),
            witnessed_answer: counted(|row| row.witnessed_answer),
            witnessed_input: counted(|row| row.witnessed_input),
            witnessed_exchange: counted(|row| row.witnessed_exchange),
            self_witnessed: rows.iter().filter(|row| row.self_witnessed).count(),
            witnesses: {
                let mut witnesses: Vec<String> = rows
                    .iter()
                    .flat_map(|row| row.witnessed_by.iter().cloned())
                    .collect();
                witnesses.sort();
                witnesses.dedup();
                witnesses
            },
            served: served
                .into_iter()
                .map(|(model, answers)| GenerationServed {
                    model: model.to_owned(),
                    answers,
                })
                .collect(),
        }
    }

    /// Whether every answer was seen made on everything the variant pins that
    /// a trace can show.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.seen == self.answers
            && self.on_prompt.is_none_or(|on| on == self.answers)
            && self.on_model.is_none_or(|on| on == self.answers)
            && self.on_workflow.is_none_or(|on| on == self.answers)
    }

    /// Whether every answer on a pinned model had a second credential's word
    /// for the model version that served it.
    #[must_use]
    pub fn witnessed(&self) -> bool {
        self.witnessed_model.is_none_or(|on| on == self.answers)
            && self.witnessed_prompt.is_none_or(|on| on == self.answers)
    }

    /// What is missing, in words; empty when [`Self::complete`].
    #[must_use]
    pub fn shortfall(&self) -> Vec<String> {
        let mut said = Vec::new();
        if self.named < self.answers {
            said.push(format!(
                "{} of {} answers name no run they were made in",
                self.answers - self.named,
                self.answers
            ));
        }
        if self.seen < self.named {
            said.push(format!(
                "{} of the {} runs the answers name were not on the log",
                self.named - self.seen,
                self.named
            ));
        }
        if self.workflow_undeclared {
            said.push(
                "the declaration of the pinned workflow names no node this step could read, so no \
                 run can be seen executing it"
                    .to_owned(),
            );
        }
        if self.steps_unread > 0 {
            said.push(format!(
                "{} seen runs took more node steps than a run's fold keeps, so the order of the \
                 rest was not read",
                self.steps_unread
            ));
        }
        for (what, on) in [
            ("call on the pinned prompt", self.on_prompt),
            ("call on the pinned model", self.on_model),
            ("execution of the pinned workflow", self.on_workflow),
        ] {
            if let Some(on) = on
                && on < self.seen
            {
                said.push(format!(
                    "{} of {} seen runs show no {what}",
                    self.seen - on,
                    self.seen
                ));
            }
        }
        said
    }

    /// Whether every answer is a reply a witness relayed, to a request that
    /// held its case's input: an application that called around the gateway,
    /// or answered other than the model did, has none.
    #[must_use]
    pub fn answers_witnessed(&self) -> bool {
        self.witnessed_answer.is_none_or(|on| on == self.answers)
            && self.witnessed_input.is_none_or(|on| on == self.answers)
            && self.witnessed_exchange.is_none_or(|on| on == self.answers)
    }

    /// What a witness did not show of the answers and the questions, in words;
    /// empty when [`Self::answers_witnessed`].
    #[must_use]
    pub fn unwitnessed_answers(&self) -> Vec<String> {
        let mut said = Vec::new();
        if let Some(on) = self.witnessed_answer
            && on < self.answers
        {
            said.push(format!(
                "{} of {} answers are no reply a witness relayed for their run — made around the \
                 gateway, or not what the model said",
                self.answers - on,
                self.answers
            ));
        }
        if let Some(on) = self.witnessed_input
            && on < self.answers
        {
            said.push(format!(
                "{} of {} answers had no request a witness relayed holding their case's input",
                self.answers - on,
                self.answers
            ));
        }
        if let Some(on) = self.witnessed_exchange
            && on < self.answers
        {
            said.push(format!(
                "{} of {} answers had no one witnessed call relaying them as its reply to a \
                 request that was nothing but the pinned prompt, rendered with their case's input \
                 or with what a call so made had replied — words beside those, or a value the \
                 application made, such as an answer to repeat, witness nothing",
                self.answers - on,
                self.answers
            ));
        }
        said
    }

    /// What a second witness did not show, in words; empty when
    /// [`Self::witnessed`].
    #[must_use]
    pub fn unwitnessed(&self) -> Vec<String> {
        let mut said = Vec::new();
        for (what, on) in [
            (
                "served their call on the pinned model",
                self.witnessed_model,
            ),
            (
                "found the pinned prompt in the request it relayed",
                self.witnessed_prompt,
            ),
        ] {
            if let Some(on) = on
                && on < self.answers
            {
                said.push(format!(
                    "{} of {} answers have no run published under a witness's credential saying \
                     it {what}",
                    self.answers - on,
                    self.answers
                ));
            }
        }
        if self.self_witnessed > 0 {
            said.push(format!(
                "{} answers' serving runs were published under the application's own \
                 credential, which witnesses nothing — give the serving host a token of its own",
                self.self_witnessed
            ));
        }
        said
    }
}

/// A node start the pinned declaration does not lead to.
#[derive(Debug, PartialEq, Eq)]
enum Misstep {
    /// Its first start, before anything leading into it had completed.
    Before { node: String, from: Vec<String> },
    /// A later start, with no completion leading into it since the last one
    /// it had used — a second pass nothing sent it on, or a loop the
    /// declaration does not have.
    Again { node: String, from: Vec<String> },
    /// A start past the number the declaration allows the node.
    Beyond { node: String, at_most: u64 },
    /// A start that followed an edge more often than the declaration allows
    /// the edge: a cycle gone round more times than its way back permits.
    Followed {
        from: String,
        to: String,
        at_most: u64,
    },
    /// Starts that followed several edges sharing one bound more often, all
    /// told, than the bound allows: a cycle with more than one way back gone
    /// round more times than its rounds permit.
    Shared {
        edges: Vec<(String, String)>,
        at_most: u64,
    },
}

/// Each node's starts the declaration does not lead to, each node once.
///
/// A completed node sends the run on along each edge out of it, once; a start
/// uses one of those, and a failed start gives its back, so a retry needs no
/// second completion. Where the run enters the shape — a node no edge leads
/// into, or a cycle entered from nowhere else — admits one start. A node
/// declared `repeats` needs only its first. So a branch runs one side, a join
/// is reached from whichever side ran, a declared loop goes round as often as
/// its nodes complete, and a node run twice for one completion, or again when
/// nothing leads back into it, is named — as is a start past the node's
/// declared `at_most`, counting every start, retries included, and a start
/// that follows an edge past that edge's `at_most`, counting the turns it
/// used and did not give back, and a start past a bound several edges share,
/// counting every edge in it together. A start uses an unbounded turn before a
/// bounded one, and of bounded ones the edge with the most left under each of
/// its bounds, so an edge is counted only for a start nothing else led to.
fn out_of_order(shape: &Topology, steps: &[StepSeen]) -> Vec<Misstep> {
    type Edge<'a> = (&'a str, &'a str);
    let entries = shape.entries();
    let mut entered = vec![false; entries.len()];
    // Each bound an edge is under: its own `at_most`, and each one it shares.
    let shared: Vec<(Vec<Edge<'_>>, u64)> = shape
        .bounds
        .iter()
        .map(|(edges, at_most)| {
            (
                edges
                    .iter()
                    .map(|(from, to)| (from.as_str(), to.as_str()))
                    .collect(),
                *at_most,
            )
        })
        .collect();
    let own = |edge: Edge<'_>| {
        shape
            .edges_at_most
            .get(&(edge.0.to_owned(), edge.1.to_owned()))
            .copied()
    };
    let sharing = |edge: Edge<'_>| -> Vec<usize> {
        shared
            .iter()
            .enumerate()
            .filter(|(_, (edges, _))| edges.contains(&edge))
            .map(|(at, _)| at)
            .collect()
    };
    let bounded = |edge: Edge<'_>| own(edge).is_some() || !sharing(edge).is_empty();
    // Turns waiting at a node: along unbounded edges, and given back by a
    // failed start nothing bounded led to.
    let mut sent: BTreeMap<&str, u64> = BTreeMap::new();
    // Turns waiting along each bounded edge, how often each was followed, and
    // how often the edges of each shared bound were, all told.
    let mut waiting: BTreeMap<Edge<'_>, u64> = BTreeMap::new();
    let mut followed: BTreeMap<Edge<'_>, u64> = BTreeMap::new();
    let mut followed_shared = vec![0u64; shared.len()];
    // What each start still running used, so a failure gives back that turn.
    let mut using: BTreeMap<&str, Vec<Option<Edge<'_>>>> = BTreeMap::new();
    let mut started = std::collections::BTreeSet::new();
    let mut starts: BTreeMap<&str, u64> = BTreeMap::new();
    let mut found: Vec<Misstep> = Vec::new();
    let mut named_shared = vec![false; shared.len()];
    let named = |found: &[Misstep], node: &str| {
        found.iter().any(|misstep| match misstep {
            Misstep::Before { node: named, .. }
            | Misstep::Again { node: named, .. }
            | Misstep::Beyond { node: named, .. } => named == node,
            Misstep::Followed { .. } | Misstep::Shared { .. } => false,
        })
    };
    for step in steps {
        match step {
            StepSeen::Started(node) => {
                let node = node.as_str();
                let first = started.insert(node);
                let count = starts.entry(node).or_default();
                *count += 1;
                if let Some(bound) = shape.at_most.get(node).filter(|bound| *count > **bound)
                    && !named(&found, node)
                {
                    found.push(Misstep::Beyond {
                        node: node.to_owned(),
                        at_most: *bound,
                    });
                }
                if shape.repeats.contains(node) && !first {
                    continue;
                }
                let left = |edge: Edge<'_>| {
                    let mut left = own(edge).map_or(u64::MAX, |at_most| {
                        at_most.saturating_sub(followed.get(&edge).copied().unwrap_or(0))
                    });
                    for at in sharing(edge) {
                        left = left.min(shared[at].1.saturating_sub(followed_shared[at]));
                    }
                    left
                };
                let admitted = if let Some(turns) = sent.get_mut(node).filter(|n| **n > 0) {
                    *turns -= 1;
                    Some(None)
                } else if let Some(at) = entries
                    .iter()
                    .position(|part| part.contains(node))
                    .filter(|at| !entered[*at])
                {
                    entered[at] = true;
                    Some(None)
                } else if let Some(edge) = waiting
                    .iter()
                    .filter(|((_, to), turns)| *to == node && **turns > 0)
                    .max_by_key(|(edge, _)| left(**edge))
                    .map(|(edge, _)| *edge)
                {
                    *waiting.entry(edge).or_default() -= 1;
                    let times = followed.entry(edge).or_default();
                    *times += 1;
                    if let Some(at_most) = own(edge).filter(|at_most| *times > *at_most)
                        && !found.iter().any(|misstep| {
                            matches!(misstep, Misstep::Followed { from, to, .. }
                                if from == edge.0 && to == edge.1)
                        })
                    {
                        found.push(Misstep::Followed {
                            from: edge.0.to_owned(),
                            to: edge.1.to_owned(),
                            at_most,
                        });
                    }
                    for at in sharing(edge) {
                        followed_shared[at] += 1;
                        if followed_shared[at] > shared[at].1 && !named_shared[at] {
                            named_shared[at] = true;
                            found.push(Misstep::Shared {
                                edges: shared[at]
                                    .0
                                    .iter()
                                    .map(|(from, to)| ((*from).to_owned(), (*to).to_owned()))
                                    .collect(),
                                at_most: shared[at].1,
                            });
                        }
                    }
                    Some(Some(edge))
                } else {
                    None
                };
                if let Some(turn) = admitted {
                    using.entry(node).or_default().push(turn);
                    continue;
                }
                if named(&found, node) {
                    continue;
                }
                let from: Vec<String> = shape
                    .edges
                    .iter()
                    .filter(|(_, to)| to == node)
                    .map(|(from, _)| from.clone())
                    .collect();
                let node = node.to_owned();
                found.push(if first {
                    Misstep::Before { node, from }
                } else {
                    Misstep::Again { node, from }
                });
            }
            StepSeen::Completed(node) => {
                using.get_mut(node.as_str()).and_then(Vec::pop);
                for (from, to) in &shape.edges {
                    if from == node {
                        if bounded((from.as_str(), to.as_str())) {
                            *waiting.entry((from.as_str(), to.as_str())).or_default() += 1;
                        } else {
                            *sent.entry(to.as_str()).or_default() += 1;
                        }
                    }
                }
            }
            StepSeen::Failed(node) => match using.get_mut(node.as_str()).and_then(Vec::pop) {
                Some(Some(edge)) => {
                    *waiting.entry(edge).or_default() += 1;
                    if let Some(times) = followed.get_mut(&edge) {
                        *times = times.saturating_sub(1);
                    }
                    for at in sharing(edge) {
                        followed_shared[at] = followed_shared[at].saturating_sub(1);
                    }
                }
                Some(None) => *sent.entry(node.as_str()).or_default() += 1,
                None => {}
            },
        }
    }
    found
}

/// Which of the calls a run's witnesses served for it were made of nothing the
/// application added: on the pinned prompt, found to be nothing but its
/// template rendered, with every value it was rendered with a part of the
/// case's input or a reply of another such call. Found by adding a call once
/// all its values are accounted for, until none is left to add — so a chain of
/// calls feeding each other's replies on is counted, and two calls whose
/// values are only each other's replies are not.
fn grounded_calls(
    run: &TracedRun,
    variant: &VariantManifest,
    witnesses: &Witnesses,
    input: Option<&serde_json::Value>,
) -> Vec<bool> {
    use aiwatcher_core::witness::{Said, carried_as, digest};
    let calls = &run.served_for_it;
    let mut grounded = vec![false; calls.len()];
    let (Some(pin), Some(input)) = (&variant.prompt, input) else {
        return grounded;
    };
    let carried = carried_as(input);
    let from_input: Vec<Option<std::collections::BTreeSet<String>>> = calls
        .iter()
        .map(|call| {
            let witness = call.published_by.as_deref()?;
            let admitted = run
                .published_by
                .as_deref()
                .is_some_and(|publisher| publisher != witness)
                && witnesses.admits(witness);
            let made_of_the_prompt = call.prompt_name.as_deref() == Some(pin.name.as_str())
                && call.prompt_version.as_deref() == Some(pin.version.as_str())
                && call.prompt_verified == Some(true)
                && call.prompt_exact == Some(true)
                && !call.rendered.is_empty();
            let key = witnesses
                .keys
                .get(witness)
                .filter(|_| admitted && made_of_the_prompt)?;
            Some(
                carried
                    .iter()
                    .map(|text| digest(key, Said::Replied, text))
                    .collect(),
            )
        })
        .collect();
    loop {
        let mut added = false;
        for (at, call) in calls.iter().enumerate() {
            let Some(from_input) = from_input[at].as_ref().filter(|_| !grounded[at]) else {
                continue;
            };
            let accounted = call.rendered.iter().all(|value| {
                from_input.contains(value)
                    || calls.iter().enumerate().any(|(other, earlier)| {
                        other != at && grounded[other] && earlier.replied.contains(value)
                    })
            });
            if accounted {
                grounded[at] = true;
                added = true;
            }
        }
        if !added {
            return grounded;
        }
    }
}

/// Hold each generated answer to the run it names.
///
/// `runs` holds the runs the log had, ended and complete; a run an answer names
/// and `runs` lacks is unseen. `workflow` is the shape of the declaration the
/// variant pins, read from its bytes, or `None` where those name no node.
/// Errors carry every contradiction at once, each naming the case, the run and
/// both sides.
///
/// # Errors
///
/// The sentences for every run whose trace contradicts the variant.
pub fn trace_answers(
    variant: &VariantManifest,
    variant_id: &str,
    evaluation_id: &str,
    answers: &[RecordedAnswer],
    runs: &BTreeMap<String, TracedRun>,
    workflow: Option<&Topology>,
    witnesses: &Witnesses,
) -> std::result::Result<Vec<TracedAnswer>, Vec<String>> {
    let mut rows = Vec::with_capacity(answers.len());
    let mut contradictions = Vec::new();
    let pinned_shape = workflow.map(Topology::digest);
    for answer in answers {
        let traced = answer.run_id.as_ref().and_then(|run_id| runs.get(run_id));
        let mut row = TracedAnswer {
            case_id: answer.case_id.clone(),
            run_id: answer.run_id.clone(),
            seen: traced.is_some(),
            trace_id: traced.and_then(|run| run.trace_id.clone()),
            on_prompt: variant.prompt.as_ref().map(|_| false),
            on_model: variant.model.as_ref().map(|_| false),
            on_workflow: variant.workflow.as_ref().map(|_| false),
            witnessed_model: variant.model.as_ref().map(|_| false),
            witnessed_prompt: variant.prompt.as_ref().map(|_| false),
            witnessed_answer: (variant.model.is_some() || variant.prompt.is_some())
                .then_some(false),
            witnessed_input: (variant.model.is_some() || variant.prompt.is_some()).then_some(false),
            witnessed_exchange: variant.prompt.as_ref().map(|_| false),
            witnessed_by: Vec::new(),
            self_witnessed: false,
            served_models: Vec::new(),
            models: Vec::new(),
            workflow_undeclared: variant.workflow.is_some() && workflow.is_none(),
            workflow_steps_unread: false,
        };
        if let (Some(run_id), Some(run)) = (&answer.run_id, traced) {
            let mut said = |sentence: String| {
                contradictions.push(format!("{} (run {run_id}): {sentence}", answer.case_id));
            };
            if let Some(named) = run
                .variant_id
                .as_deref()
                .filter(|named| *named != variant_id)
            {
                said(format!(
                    "the run names variant {named}, and this result is published as {variant_id}"
                ));
            }
            if let Some(named) = run
                .evaluation_id
                .as_deref()
                .filter(|named| *named != evaluation_id)
            {
                said(format!(
                    "the run answered for {named}, and this result is {evaluation_id}"
                ));
            }
            for call in &run.calls {
                aiwatcher_core::prices::ModelUsage::add_to(
                    &mut row.models,
                    &aiwatcher_core::prices::ModelUsage {
                        model: call.model.clone().unwrap_or_else(|| "unknown".to_owned()),
                        calls: 1,
                        input_tokens: call.input_tokens,
                        output_tokens: call.output_tokens,
                        cached_tokens: call.cached_tokens,
                    },
                );
                if let Some(served) = &call.served_model
                    && !row.served_models.contains(served)
                {
                    row.served_models.push(served.clone());
                }
                if let Some(pinned) = &variant.prompt {
                    let version = call.prompt_version.as_deref();
                    if version == Some(pinned.version.as_str()) {
                        row.on_prompt = Some(true);
                    } else if call.prompt_name.as_deref() == Some(pinned.name.as_str()) {
                        said(format!(
                            "a call rendered {} at {}, and the variant pins {}",
                            pinned.name,
                            version.unwrap_or("no version"),
                            pinned.version
                        ));
                    }
                }
                if let Some(pinned) = &variant.model
                    && call.model.as_deref() == Some(pinned.name.as_str())
                {
                    match call.model_version.as_deref() {
                        Some(version) if version == pinned.version => row.on_model = Some(true),
                        Some(version) => said(format!(
                            "a call was served by {} at {version}, and the variant pins {}",
                            pinned.name, pinned.version
                        )),
                        // A name with no version says which model and not which
                        // of its versions: not a contradiction, and not a sighting.
                        None => {}
                    }
                }
            }
            let grounded = grounded_calls(
                run,
                variant,
                witnesses,
                witnesses.inputs.get(&answer.case_id),
            );
            // Another credential's word, or none: a call the answer's own
            // publisher reported for a serving run is still its word, and a
            // credential the deployment did not name a witness is nobody's.
            for (at, call) in run.served_for_it.iter().enumerate() {
                let names_a_pin = variant
                    .model
                    .as_ref()
                    .is_some_and(|pin| call.model.as_deref() == Some(pin.name.as_str()))
                    || variant
                        .prompt
                        .as_ref()
                        .is_some_and(|pin| call.prompt_name.as_deref() == Some(pin.name.as_str()));
                let Some(witness) = call.published_by.as_deref() else {
                    continue;
                };
                if run.published_by.as_deref() == Some(witness) {
                    row.self_witnessed |= names_a_pin;
                    continue;
                }
                if run.published_by.is_none() || !witnesses.admits(witness) {
                    continue;
                }
                let mut vouched = false;
                // Its digests of the call's words, under its own key: the
                // answer among the replies, the case's input in the request.
                if names_a_pin && let Some(key) = witnesses.keys.get(witness) {
                    use aiwatcher_core::witness::{Said, asked_as, digest};
                    let holds = |digests: &[String], said: Said, texts: Vec<String>| {
                        texts
                            .iter()
                            .any(|text| digests.contains(&digest(key, said, text)))
                    };
                    let answered = witnesses.answered_as(answer);
                    let replied = holds(&call.replied, Said::Replied, answered.clone());
                    let input = witnesses
                        .inputs
                        .get(&answer.case_id)
                        .is_some_and(|input| holds(&call.asked, Said::Asked, asked_as(input)));
                    if replied {
                        row.witnessed_answer = Some(true);
                        vouched = true;
                    }
                    if input {
                        row.witnessed_input = Some(true);
                        vouched = true;
                    }
                    if replied && grounded[at] && !holds(&call.asked, Said::Asked, answered) {
                        row.witnessed_exchange = Some(true);
                    }
                }
                if let Some(pinned) = &variant.model
                    && call.model.as_deref() == Some(pinned.name.as_str())
                {
                    match call.model_version.as_deref() {
                        Some(version) if version == pinned.version => {
                            row.witnessed_model = Some(true);
                            vouched = true;
                        }
                        Some(version) => said(format!(
                            "{witness} says it served this run's call with {} at {version}, and \
                             the variant pins {}",
                            pinned.name, pinned.version
                        )),
                        None => {}
                    }
                }
                if let Some(pinned) = &variant.prompt
                    && call.prompt_name.as_deref() == Some(pinned.name.as_str())
                {
                    let version = call.prompt_version.as_deref().unwrap_or("no version");
                    match call.prompt_verified {
                        Some(true) if version == pinned.version => {
                            row.witnessed_prompt = Some(true);
                            vouched = true;
                        }
                        Some(true) => said(format!(
                            "{witness} found {} at {version} in the request it relayed, and the \
                             variant pins {}",
                            pinned.name, pinned.version
                        )),
                        Some(false) => said(format!(
                            "{witness} relayed a request naming {} at {version} whose text does \
                             not hold that version's template",
                            pinned.name
                        )),
                        None => {}
                    }
                }
                if vouched && !row.witnessed_by.iter().any(|named| named == witness) {
                    row.witnessed_by.push(witness.to_owned());
                }
            }
            if let Some(pinned) = &variant.workflow
                && run.workflow.as_deref() == Some(pinned.name.as_str())
            {
                let mut on = pinned_shape.is_some();
                if let (Some(declared), Some(shape)) = (&run.workflow_topology, &pinned_shape)
                    && declared != shape
                {
                    on = false;
                    said(format!(
                        "the run declared {} in another shape than the declaration the variant \
                         pins",
                        pinned.name
                    ));
                }
                if run.workflow_topology.is_none() {
                    on = false;
                }
                if let Some(shape) = workflow {
                    for node in &run.nodes_run {
                        if !shape.nodes.contains(node) {
                            on = false;
                            said(format!(
                                "the run stepped through {node}, which the declaration of {} the \
                                 variant pins does not name",
                                pinned.name
                            ));
                        }
                    }
                    for misstep in out_of_order(shape, &run.node_steps) {
                        on = false;
                        said(match misstep {
                            Misstep::Before { node, from } => format!(
                                "the run started {node} before {} had completed, and the \
                                 declaration of {} the variant pins leads into it only from there",
                                from.join(" or "),
                                pinned.name
                            ),
                            Misstep::Again { node, from } if from.is_empty() => format!(
                                "the run started {node} again, and nothing in the declaration of \
                                 {} the variant pins leads back into it",
                                pinned.name
                            ),
                            Misstep::Beyond { node, at_most } => format!(
                                "the run started {node} more than {at_most} times, and the \
                                 declaration of {} the variant pins allows it at most that many",
                                pinned.name
                            ),
                            Misstep::Followed { from, to, at_most } => format!(
                                "the run went from {from} to {to} more than {at_most} times, and \
                                 the declaration of {} the variant pins allows that edge at most \
                                 that many",
                                pinned.name
                            ),
                            Misstep::Shared { edges, at_most } => format!(
                                "the run went along {} more than {at_most} times between them, \
                                 and the declaration of {} the variant pins allows those edges at \
                                 most that many together",
                                edges
                                    .iter()
                                    .map(|(from, to)| format!("{from} to {to}"))
                                    .collect::<Vec<_>>()
                                    .join(" and "),
                                pinned.name
                            ),
                            Misstep::Again { node, from } => format!(
                                "the run started {node} again with no completion of {} since, and \
                                 the declaration of {} the variant pins leads into it only from \
                                 there",
                                from.join(" or "),
                                pinned.name
                            ),
                        });
                    }
                    if run.node_steps_dropped {
                        on = false;
                        row.workflow_steps_unread = true;
                    }
                }
                row.on_workflow = Some(on);
            }
        }
        rows.push(row);
    }
    if contradictions.is_empty() {
        Ok(rows)
    } else {
        Err(contradictions)
    }
}

#[cfg(test)]
mod tests {
    use aiwatcher_core::{ArtifactKind, ArtifactRef};

    use super::*;
    use crate::{DatasetKind, DatasetReference, VersionReference};

    fn artifact(name: &str) -> ArtifactRef {
        ArtifactRef {
            name: name.to_owned(),
            uri: format!("file://{name}"),
            digest: "a".repeat(64),
            size_bytes: Some(1),
            content_type: String::new(),
            kind: ArtifactKind::Blob,
            schema_ref: None,
        }
    }

    fn variant() -> VariantManifest {
        VariantManifest {
            schema_version: 1,
            experiment_id: "candidate".to_owned(),
            dataset: DatasetReference {
                kind: DatasetKind::Curation,
                name: "capitals".to_owned(),
                version: "d".repeat(64),
            },
            model: Some(VersionReference {
                name: "capitals-model".to_owned(),
                version: "v7".to_owned(),
            }),
            prompt: Some(VersionReference {
                name: "capitals".to_owned(),
                version: "p".repeat(64),
            }),
            code: artifact("code"),
            generation_config: artifact("config"),
            response_schema: None,
            tools: None,
            workflow: None,
        }
    }

    fn answer(case_id: &str, run_id: Option<&str>) -> RecordedAnswer {
        RecordedAnswer {
            case_id: case_id.to_owned(),
            answer: serde_json::json!("Paris"),
            run_id: run_id.map(ToOwned::to_owned),
            trace_id: None,
            span_id: None,
            usage: None,
        }
    }

    fn on_the_pins() -> TracedCall {
        TracedCall {
            model: Some("capitals-model".to_owned()),
            model_version: Some("v7".to_owned()),
            prompt_name: Some("capitals".to_owned()),
            prompt_version: Some("p".repeat(64)),
            served_model: None,
            prompt_verified: None,
            prompt_exact: None,
            published_by: Some("worker".to_owned()),
            input_tokens: 12,
            output_tokens: 3,
            cached_tokens: 0,
            asked: Vec::new(),
            replied: Vec::new(),
            rendered: Vec::new(),
        }
    }

    fn run(calls: Vec<TracedCall>) -> TracedRun {
        TracedRun {
            trace_id: None,
            variant_id: Some("variant".to_owned()),
            evaluation_id: Some("answers".to_owned()),
            calls,
            published_by: Some("worker".to_owned()),
            ..TracedRun::default()
        }
    }

    #[test]
    fn answers_made_on_the_pins_are_seen_on_them_and_unseen_ones_are_counted_not_refused() {
        let runs = BTreeMap::from([
            ("r1".to_owned(), run(vec![on_the_pins()])),
            (
                "r2".to_owned(),
                run(vec![
                    TracedCall {
                        model: Some("router".to_owned()),
                        ..TracedCall::default()
                    },
                    on_the_pins(),
                ]),
            ),
        ]);
        let answers = [
            answer("c1", Some("r1")),
            answer("c2", Some("r2")),
            answer("c3", Some("r-never-arrived")),
            answer("c4", None),
        ];

        let rows = trace_answers(
            &variant(),
            "variant",
            "answers",
            &answers,
            &runs,
            None,
            &Witnesses::default(),
        )
        .expect("nothing contradicts the pins");
        let trace = GenerationTrace::of(&rows);

        assert_eq!(
            trace,
            GenerationTrace {
                answers: 4,
                named: 3,
                seen: 2,
                on_prompt: Some(2),
                on_model: Some(2),
                on_workflow: None,
                workflow_undeclared: false,
                steps_unread: 0,
                witnessed_model: Some(0),
                witnessed_prompt: Some(0),
                witnessed_answer: Some(0),
                witnessed_input: Some(0),
                witnessed_exchange: Some(0),
                self_witnessed: 0,
                witnesses: Vec::new(),
                served: Vec::new(),
            }
        );
        assert!(!trace.complete());
        assert_eq!(
            rows[1].models,
            [
                aiwatcher_core::prices::ModelUsage {
                    model: "capitals-model".to_owned(),
                    calls: 1,
                    input_tokens: 12,
                    output_tokens: 3,
                    cached_tokens: 0,
                },
                aiwatcher_core::prices::ModelUsage {
                    model: "router".to_owned(),
                    calls: 1,
                    ..Default::default()
                },
            ],
            "each call by the model it named, what it reported beside it"
        );
        assert_eq!(
            trace.shortfall(),
            vec![
                "1 of 4 answers name no run they were made in".to_owned(),
                "1 of the 3 runs the answers name were not on the log".to_owned(),
            ]
        );
    }

    #[test]
    fn a_call_on_another_version_of_the_pinned_prompt_or_model_refuses_the_answers() {
        let runs = BTreeMap::from([(
            "r1".to_owned(),
            run(vec![
                TracedCall {
                    prompt_version: Some("q".repeat(64)),
                    ..on_the_pins()
                },
                TracedCall {
                    model_version: Some("v6".to_owned()),
                    prompt_name: None,
                    prompt_version: None,
                    ..on_the_pins()
                },
            ]),
        )]);

        let refused = trace_answers(
            &variant(),
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &runs,
            None,
            &Witnesses::default(),
        )
        .expect_err("the trace contradicts the pins");

        assert_eq!(refused.len(), 2, "{refused:?}");
        assert!(refused[0].contains(&"q".repeat(64)) && refused[0].contains(&"p".repeat(64)));
        assert!(refused[1].contains("v6") && refused[1].contains("v7"));
    }

    #[test]
    fn a_run_naming_another_variant_or_result_is_not_this_one_s() {
        let runs = BTreeMap::from([(
            "r1".to_owned(),
            TracedRun {
                trace_id: None,
                variant_id: Some("baseline".to_owned()),
                evaluation_id: Some("answers-baseline".to_owned()),
                calls: vec![on_the_pins()],
                ..TracedRun::default()
            },
        )]);

        let refused = trace_answers(
            &variant(),
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &runs,
            None,
            &Witnesses::default(),
        )
        .expect_err("another variant's run");

        assert!(
            refused
                .iter()
                .any(|said| said.contains("names variant baseline"))
        );
        assert!(
            refused
                .iter()
                .any(|said| said.contains("answered for answers-baseline"))
        );
    }

    #[test]
    fn a_model_named_without_its_version_is_neither_a_sighting_nor_a_contradiction() {
        let mut pins = variant();
        pins.prompt = None;
        let runs = BTreeMap::from([(
            "r1".to_owned(),
            run(vec![TracedCall {
                model_version: None,
                ..on_the_pins()
            }]),
        )]);

        let rows = trace_answers(
            &pins,
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &runs,
            None,
            &Witnesses::default(),
        )
        .expect("no version is no contradiction");

        let trace = GenerationTrace::of(&rows);
        assert_eq!((trace.on_prompt, trace.on_model), (None, Some(0)));
    }

    fn workflow_variant() -> VariantManifest {
        VariantManifest {
            model: None,
            prompt: None,
            workflow: Some(VersionReference {
                name: "capitals-app".to_owned(),
                version: "w".repeat(64),
            }),
            ..variant()
        }
    }

    fn shape(edges: &[(&str, &str)]) -> Topology {
        Topology::read(&serde_json::json!({
            "nodes": ["retrieve", "answer"],
            "edges": edges.iter().map(|(from, to)| [from, to]).collect::<Vec<_>>(),
        }))
        .expect("a shape")
    }

    fn workflow_run(declared: &Topology, nodes_run: &[&str]) -> TracedRun {
        TracedRun {
            workflow: Some("capitals-app".to_owned()),
            workflow_topology: Some(declared.digest()),
            nodes_run: nodes_run.iter().map(|node| (*node).to_owned()).collect(),
            ..run(Vec::new())
        }
    }

    #[test]
    fn a_run_that_declared_the_pinned_shape_and_stayed_on_it_is_seen_executing_it() {
        let pinned = shape(&[("retrieve", "answer")]);
        let runs = BTreeMap::from([
            (
                "r1".to_owned(),
                workflow_run(&pinned, &["retrieve", "answer"]),
            ),
            (
                "r2".to_owned(),
                TracedRun {
                    workflow_topology: None,
                    ..workflow_run(&pinned, &["answer"])
                },
            ),
        ]);

        let rows = trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[answer("c1", Some("r1")), answer("c2", Some("r2"))],
            &runs,
            Some(&pinned),
            &Witnesses::default(),
        )
        .expect("nothing contradicts the pinned shape");
        let trace = GenerationTrace::of(&rows);

        assert_eq!(
            trace.on_workflow,
            Some(1),
            "a run that declared nothing is not seen on it"
        );
        assert_eq!(
            trace.shortfall(),
            ["1 of 2 seen runs show no execution of the pinned workflow".to_owned()]
        );
    }

    #[test]
    fn a_run_declaring_the_pinned_workflow_in_another_shape_or_stepping_off_it_is_refused() {
        let pinned = shape(&[("retrieve", "answer")]);
        let runs = BTreeMap::from([
            (
                "other-shape".to_owned(),
                workflow_run(&shape(&[("answer", "retrieve")]), &[]),
            ),
            (
                "off-the-graph".to_owned(),
                workflow_run(&pinned, &["retrieve", "improvise"]),
            ),
        ]);

        let refused = trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[
                answer("c1", Some("other-shape")),
                answer("c2", Some("off-the-graph")),
            ],
            &runs,
            Some(&pinned),
            &Witnesses::default(),
        )
        .expect_err("both contradict the pinned declaration");

        assert!(refused[0].contains("another shape"), "{refused:?}");
        assert!(refused[1].contains("improvise"), "{refused:?}");
    }

    #[test]
    fn a_run_that_started_a_node_before_what_leads_into_it_completed_is_refused() {
        let pinned = shape(&[("retrieve", "answer")]);
        let steps = |order: &[(&str, bool)]| -> Vec<StepSeen> {
            order
                .iter()
                .map(|(node, started)| {
                    if *started {
                        StepSeen::Started((*node).to_owned())
                    } else {
                        StepSeen::Completed((*node).to_owned())
                    }
                })
                .collect()
        };
        let runs = BTreeMap::from([
            (
                "in-order".to_owned(),
                TracedRun {
                    node_steps: steps(&[
                        ("retrieve", true),
                        ("retrieve", false),
                        ("answer", true),
                        ("answer", false),
                    ]),
                    ..workflow_run(&pinned, &["retrieve", "answer"])
                },
            ),
            (
                "answered-first".to_owned(),
                TracedRun {
                    node_steps: steps(&[
                        ("answer", true),
                        ("retrieve", true),
                        ("retrieve", false),
                        ("answer", false),
                        ("answer", true),
                    ]),
                    ..workflow_run(&pinned, &["answer", "retrieve"])
                },
            ),
        ]);

        let rows = trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[answer("c1", Some("in-order"))],
            &runs,
            Some(&pinned),
            &Witnesses::default(),
        )
        .expect("the order the declaration leads");
        assert_eq!(rows[0].on_workflow, Some(true));

        let refused = trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[answer("c2", Some("answered-first"))],
            &runs,
            Some(&pinned),
            &Witnesses::default(),
        )
        .expect_err("answer started before retrieve completed");
        assert_eq!(refused.len(), 1, "named once: {refused:?}");
        assert!(
            refused[0].contains("started answer before retrieve had completed"),
            "{refused:?}"
        );
    }

    /// Steps written as `node:s`, `node:c` or `node:f` — started, completed,
    /// failed.
    fn stepped(steps: &[&str]) -> Vec<StepSeen> {
        steps
            .iter()
            .map(|step| {
                let (node, phase) = step.split_once(':').expect("node:phase");
                match phase {
                    "s" => StepSeen::Started(node.to_owned()),
                    "c" => StepSeen::Completed(node.to_owned()),
                    _ => StepSeen::Failed(node.to_owned()),
                }
            })
            .collect()
    }

    fn traversed(pinned: &Topology, steps: &[&str]) -> Result<Vec<TracedAnswer>, Vec<String>> {
        let nodes: Vec<&str> = pinned.nodes.iter().map(String::as_str).collect();
        let runs = BTreeMap::from([(
            "r".to_owned(),
            TracedRun {
                node_steps: stepped(steps),
                ..workflow_run(pinned, &nodes)
            },
        )]);
        trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[answer("c1", Some("r"))],
            &runs,
            Some(pinned),
            &Witnesses::default(),
        )
    }

    #[test]
    fn a_declared_loop_goes_round_as_its_nodes_complete_and_a_retry_needs_no_second_completion() {
        let pinned = Topology::read(&serde_json::json!({
            "nodes": ["plan", "act", "report"],
            "edges": [["plan", "act"], ["act", "plan"], ["act", "report"]]
        }))
        .expect("a shape");

        let rows = traversed(
            &pinned,
            &[
                "plan:s", "plan:c", "act:s", "act:f", "act:s", "act:c", "plan:s", "plan:c",
                "act:s", "act:c", "report:s", "report:c",
            ],
        )
        .expect("round the loop twice, with a retry, then out");

        assert_eq!(rows[0].on_workflow, Some(true));
    }

    #[test]
    fn a_node_run_twice_for_one_completion_or_again_when_nothing_leads_back_is_refused() {
        let pinned = shape(&[("retrieve", "answer")]);

        let twice = traversed(
            &pinned,
            &[
                "retrieve:s",
                "retrieve:c",
                "answer:s",
                "answer:s",
                "answer:c",
            ],
        )
        .expect_err("one retrieval sends the run on to one answer");
        assert_eq!(twice.len(), 1, "{twice:?}");
        assert!(
            twice[0].contains("started answer again with no completion of retrieve since"),
            "{twice:?}"
        );

        let again = traversed(
            &pinned,
            &[
                "retrieve:s",
                "retrieve:c",
                "answer:s",
                "answer:c",
                "retrieve:s",
            ],
        )
        .expect_err("nothing leads back into retrieve");
        assert!(
            again[0].contains("started retrieve again, and nothing in the declaration"),
            "{again:?}"
        );

        let failed_unadmitted = traversed(&pinned, &["answer:s", "answer:f", "answer:s"])
            .expect_err("a failure gives back only what its start used");
        assert_eq!(failed_unadmitted.len(), 1, "{failed_unadmitted:?}");
        assert!(failed_unadmitted[0].contains("before retrieve had completed"));
    }

    #[test]
    fn a_repeating_node_runs_as_often_as_it_likes_once_something_leads_into_it() {
        let pinned = Topology::read(&serde_json::json!({
            "nodes": ["retrieve", {"id": "answer", "repeats": true}],
            "edges": [["retrieve", "answer"]]
        }))
        .expect("a shape");

        let rows = traversed(
            &pinned,
            &[
                "retrieve:s",
                "retrieve:c",
                "answer:s",
                "answer:s",
                "answer:c",
                "answer:c",
                "answer:s",
                "answer:c",
            ],
        )
        .expect("one per item, side by side");
        assert_eq!(rows[0].on_workflow, Some(true));

        let early = traversed(&pinned, &["answer:s", "retrieve:s"])
            .expect_err("its first start still waits for what leads into it");
        assert!(
            early[0].contains("before retrieve had completed"),
            "{early:?}"
        );
    }

    #[test]
    fn a_declared_bound_holds_a_loop_and_a_repeating_node_to_as_many_starts() {
        let pinned = Topology::read(&serde_json::json!({
            "nodes": [
                {"id": "plan", "at_most": 2},
                "act",
                {"id": "summarise", "repeats": true, "at_most": 3}
            ],
            "edges": [["plan", "act"], ["act", "plan"], ["act", "summarise"]]
        }))
        .expect("a shape");

        let rows = traversed(
            &pinned,
            &[
                "plan:s",
                "plan:c",
                "act:s",
                "act:c",
                "plan:s",
                "plan:c",
                "act:s",
                "act:c",
                "summarise:s",
                "summarise:s",
                "summarise:s",
            ],
        )
        .expect("twice round the loop, and three items");
        assert_eq!(rows[0].on_workflow, Some(true));

        let looped = traversed(
            &pinned,
            &[
                "plan:s", "plan:c", "act:s", "act:c", "plan:s", "plan:c", "act:s", "act:c",
                "plan:s",
            ],
        )
        .expect_err("a third time round");
        assert!(
            looped[0].contains("started plan more than 2 times"),
            "{looped:?}"
        );
        let repeated = traversed(
            &pinned,
            &[
                "plan:s",
                "plan:c",
                "act:s",
                "act:c",
                "summarise:s",
                "summarise:s",
                "summarise:s",
                "summarise:s",
            ],
        )
        .expect_err("a fourth item");
        assert_eq!(repeated.len(), 1, "{repeated:?}");
        assert!(repeated[0].contains("started summarise more than 3 times"));
    }

    #[test]
    fn a_bound_on_the_way_back_holds_a_cycle_through_several_nodes_to_its_rounds() {
        let pinned = Topology::read(&serde_json::json!({
            "nodes": ["draft", "write", "review", "publish"],
            "edges": [
                ["draft", "write"],
                ["write", "review"],
                {"from": "review", "to": "write", "at_most": 2},
                ["review", "publish"]
            ]
        }))
        .expect("a shape");
        let round = ["write:s", "write:c", "review:s", "review:c"];
        let rounds = |times: usize, tail: &[&'static str]| {
            let mut steps = vec!["draft:s", "draft:c"];
            for _ in 0..times {
                steps.extend(round);
            }
            steps.extend(tail);
            steps
        };

        let rows = traversed(&pinned, &rounds(3, &["publish:s", "publish:c"]))
            .expect("written once and sent back twice");
        assert_eq!(rows[0].on_workflow, Some(true));
        let retried = traversed(
            &pinned,
            &[
                "draft:s",
                "draft:c",
                "write:s",
                "write:c",
                "review:s",
                "review:c",
                "write:s",
                "write:f",
                "write:s",
                "write:c",
                "review:s",
                "review:c",
                "write:s",
                "write:c",
                "review:s",
                "review:c",
                "publish:s",
            ],
        )
        .expect("a failed start gives its turn back, so a retry is not another round");
        assert_eq!(retried[0].on_workflow, Some(true));

        let looped = traversed(&pinned, &rounds(4, &[])).expect_err("sent back a third time");
        assert_eq!(looped.len(), 1, "{looped:?}");
        assert!(
            looped[0].contains("went from review to write more than 2 times"),
            "{looped:?}"
        );
    }

    #[test]
    fn a_bound_two_ways_back_share_holds_the_cycle_to_its_rounds_whichever_way_each_went() {
        let declared = |bounds: serde_json::Value| {
            Topology::read(&serde_json::json!({
                "nodes": ["write", "review", "fix", "publish"],
                "edges": [
                    ["write", "review"],
                    ["review", "write"],
                    ["review", "fix"],
                    ["fix", "review"],
                    ["review", "publish"]
                ],
                "bounds": bounds
            }))
            .expect("a shape")
        };
        let together = declared(serde_json::json!([
            {"edges": [["review", "write"], ["fix", "review"]], "at_most": 2}
        ]));
        let apart = declared(serde_json::json!([
            {"edges": [["review", "write"]], "at_most": 2},
            {"edges": [["fix", "review"]], "at_most": 2}
        ]));
        let back_to_write = ["write:s", "write:c", "review:s", "review:c"];
        let through_fix = ["fix:s", "fix:c", "review:s", "review:c"];
        let run = |rounds: &[&[&'static str; 4]]| {
            let mut steps = vec!["write:s", "write:c", "review:s", "review:c"];
            for round in rounds {
                steps.extend(round.iter());
            }
            steps.extend(["publish:s", "publish:c"]);
            steps
        };

        let twice = run(&[&back_to_write, &through_fix]);
        assert_eq!(
            traversed(&together, &twice).expect("once each way")[0].on_workflow,
            Some(true)
        );
        let four = run(&[&back_to_write, &through_fix, &back_to_write, &through_fix]);
        assert_eq!(
            traversed(&apart, &four).expect("twice each way, each within its own bound")[0]
                .on_workflow,
            Some(true),
            "two bounds apart let the cycle go round as often as both allow"
        );
        let refused = traversed(&together, &four).expect_err("four rounds in all");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(
            refused[0].contains("went along fix to review and review to write more than 2 times"),
            "{refused:?}"
        );

        let retried = run(&[
            &back_to_write,
            &["fix:s", "fix:c", "review:s", "review:f"],
            &["review:s", "review:c", "publish:s", "publish:c"],
        ]);
        assert_eq!(
            traversed(&together, &retried[..retried.len() - 2]).expect("a retry is no round")[0]
                .on_workflow,
            Some(true)
        );
    }

    #[test]
    fn a_run_whose_steps_outgrew_its_fold_is_not_seen_on_the_workflow_and_says_why() {
        let pinned = shape(&[("retrieve", "answer")]);
        let runs = BTreeMap::from([(
            "long".to_owned(),
            TracedRun {
                node_steps: stepped(&["retrieve:s", "retrieve:c"]),
                node_steps_dropped: true,
                ..workflow_run(&pinned, &["retrieve", "answer"])
            },
        )]);

        let rows = trace_answers(
            &workflow_variant(),
            "variant",
            "answers",
            &[answer("c1", Some("long"))],
            &runs,
            Some(&pinned),
            &Witnesses::default(),
        )
        .expect("what was read is in order, and the rest is counted");
        let trace = GenerationTrace::of(&rows);

        assert_eq!((rows[0].on_workflow, trace.steps_unread), (Some(false), 1));
        assert!(
            trace
                .shortfall()
                .iter()
                .any(|said| said.contains("more node steps than a run's fold keeps")),
            "{:?}",
            trace.shortfall()
        );
    }

    #[test]
    fn a_serving_host_s_word_counts_only_under_another_credential_and_contradicts_under_one() {
        let mut pins = variant();
        pins.prompt = None;
        let served = |publisher: &str, version: &str| TracedCall {
            model: Some("capitals-model".to_owned()),
            model_version: Some(version.to_owned()),
            served_model: Some("capitals-model-q4".to_owned()),
            published_by: Some(publisher.to_owned()),
            ..TracedCall::default()
        };
        let runs = BTreeMap::from([
            (
                "witnessed".to_owned(),
                TracedRun {
                    served_for_it: vec![served("serving", "v7")],
                    ..run(vec![TracedCall {
                        served_model: Some("capitals-model-q4".to_owned()),
                        ..on_the_pins()
                    }])
                },
            ),
            (
                "self-reported".to_owned(),
                TracedRun {
                    served_for_it: vec![served("worker", "v7")],
                    ..run(vec![on_the_pins()])
                },
            ),
        ]);

        let rows = trace_answers(
            &pins,
            "variant",
            "answers",
            &[
                answer("c1", Some("witnessed")),
                answer("c2", Some("self-reported")),
            ],
            &runs,
            None,
            &Witnesses::default(),
        )
        .expect("nothing contradicts the pins");
        let trace = GenerationTrace::of(&rows);
        assert_eq!(
            (trace.on_model, trace.witnessed_model),
            (Some(2), Some(1)),
            "a serving run the worker's own credential published is the worker's word"
        );
        assert!(trace.complete() && !trace.witnessed());
        assert_eq!(
            trace.self_witnessed, 1,
            "one token on both hosts is said so"
        );
        assert_eq!(trace.witnesses, ["serving"]);
        assert!(
            trace
                .unwitnessed()
                .iter()
                .any(|said| said.contains("a token of its own")),
            "{:?}",
            trace.unwitnessed()
        );
        assert_eq!(
            trace.served,
            [GenerationServed {
                model: "capitals-model-q4".to_owned(),
                answers: 1
            }]
        );

        let contradicted = BTreeMap::from([(
            "r1".to_owned(),
            TracedRun {
                served_for_it: vec![served("serving", "v6")],
                ..run(vec![on_the_pins()])
            },
        )]);
        let refused = trace_answers(
            &pins,
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &contradicted,
            None,
            &Witnesses::default(),
        )
        .expect_err("the serving host served another version");
        assert!(
            refused[0].contains("serving says it served") && refused[0].contains("v6"),
            "{refused:?}"
        );
    }

    #[test]
    fn a_gateway_that_matched_the_pinned_prompt_witnesses_it_and_one_that_did_not_refuses_it() {
        let mut pins = variant();
        pins.model = None;
        let relayed = |publisher: &str, verified: bool| TracedCall {
            model: Some("gpt-4o".to_owned()),
            prompt_name: Some("capitals".to_owned()),
            prompt_version: Some("p".repeat(64)),
            prompt_verified: Some(verified),
            published_by: Some(publisher.to_owned()),
            ..TracedCall::default()
        };
        let run_with = |calls: Vec<TracedCall>| TracedRun {
            served_for_it: calls,
            ..run(vec![on_the_pins()])
        };
        let runs = BTreeMap::from([
            (
                "matched".to_owned(),
                run_with(vec![relayed("gateway", true)]),
            ),
            (
                "unlisted".to_owned(),
                run_with(vec![relayed("somebody-else", true)]),
            ),
        ]);
        let rows = trace_answers(
            &pins,
            "variant",
            "answers",
            &[
                answer("c1", Some("matched")),
                answer("c2", Some("unlisted")),
            ],
            &runs,
            None,
            &Witnesses::named(vec!["gateway".to_owned()]),
        )
        .expect("nothing contradicts the pins");
        let trace = GenerationTrace::of(&rows);
        assert_eq!(
            (trace.witnessed_prompt, trace.witnesses.as_slice()),
            (Some(1), ["gateway".to_owned()].as_slice()),
            "a credential the deployment did not name witnesses nothing"
        );

        let drifted =
            BTreeMap::from([("r1".to_owned(), run_with(vec![relayed("gateway", false)]))]);
        let refused = trace_answers(
            &pins,
            "variant",
            "answers",
            &[answer("c1", Some("r1"))],
            &drifted,
            None,
            &Witnesses::default(),
        )
        .expect_err("the request did not hold the pinned template");
        assert!(
            refused[0].contains("does not hold that version's template"),
            "{refused:?}"
        );
    }

    #[test]
    fn an_answer_is_witnessed_only_as_a_reply_the_witness_relayed_to_a_request_holding_its_input() {
        use aiwatcher_core::witness::{Said, digest, key_for};
        let mut pins = variant();
        pins.model = None;
        let key = key_for("gateway-secret");
        let forged = key_for("application-secret");
        let question = |country: &str| format!("What is the capital of {country}?");
        let relayed = |replied: Vec<String>, asked: Vec<String>| TracedCall {
            model: Some("gpt-4o".to_owned()),
            prompt_name: Some("capitals".to_owned()),
            prompt_version: Some("p".repeat(64)),
            prompt_verified: Some(true),
            prompt_exact: Some(true),
            published_by: Some("gateway".to_owned()),
            rendered: vec![digest(&key, Said::Replied, &question("France"))],
            replied,
            asked,
            ..TracedCall::default()
        };
        let run_with = |call: TracedCall| TracedRun {
            served_for_it: vec![call],
            ..run(vec![on_the_pins()])
        };
        let run_of = |calls: Vec<TracedCall>| TracedRun {
            served_for_it: calls,
            ..run(vec![on_the_pins()])
        };
        let runs = BTreeMap::from([
            (
                "through".to_owned(),
                run_with(relayed(
                    vec![digest(&key, Said::Replied, "Paris")],
                    vec![digest(&key, Said::Asked, &question("France"))],
                )),
            ),
            (
                "around".to_owned(),
                run_with(relayed(
                    vec![digest(&key, Said::Replied, "Lyon")],
                    vec![digest(&key, Said::Asked, &question("Japan"))],
                )),
            ),
            (
                "forged".to_owned(),
                run_with(relayed(
                    vec![digest(&forged, Said::Replied, "Paris")],
                    vec![digest(&forged, Said::Asked, &question("Peru"))],
                )),
            ),
            (
                "told-to-repeat".to_owned(),
                run_with(relayed(
                    vec![digest(&key, Said::Replied, "Paris")],
                    vec![
                        digest(&key, Said::Asked, &question("France")),
                        digest(&key, Said::Asked, "Paris"),
                    ],
                )),
            ),
            (
                "padded".to_owned(),
                run_with(TracedCall {
                    prompt_exact: Some(false),
                    ..relayed(
                        vec![digest(&key, Said::Replied, "Paris")],
                        vec![digest(&key, Said::Asked, &question("France"))],
                    )
                }),
            ),
            (
                // The answer rides in a value the application made.
                "hinted".to_owned(),
                run_with(TracedCall {
                    rendered: vec![
                        digest(&key, Said::Replied, &question("France")),
                        digest(&key, Said::Replied, "Think of Paris"),
                    ],
                    ..relayed(
                        vec![digest(&key, Said::Replied, "Paris")],
                        vec![digest(&key, Said::Asked, &question("France"))],
                    )
                }),
            ),
            (
                // A second call rendered with the first one's reply.
                "chained".to_owned(),
                run_of(vec![
                    TracedCall {
                        rendered: vec![digest(&key, Said::Replied, "It is Paris, I think")],
                        ..relayed(vec![digest(&key, Said::Replied, "Paris")], Vec::new())
                    },
                    relayed(
                        vec![digest(&key, Said::Replied, "It is Paris, I think")],
                        vec![digest(&key, Said::Asked, &question("France"))],
                    ),
                ]),
            ),
            (
                // Two calls each rendered with nothing but the other's reply.
                "circular".to_owned(),
                run_of(vec![
                    TracedCall {
                        rendered: vec![digest(&key, Said::Replied, "Paris")],
                        ..relayed(vec![digest(&key, Said::Replied, "It is Paris")], Vec::new())
                    },
                    TracedCall {
                        rendered: vec![digest(&key, Said::Replied, "It is Paris")],
                        ..relayed(vec![digest(&key, Said::Replied, "Paris")], Vec::new())
                    },
                ]),
            ),
        ]);
        let inputs = BTreeMap::from([
            (
                "c1".to_owned(),
                serde_json::json!({"question": question("France")}),
            ),
            (
                "c2".to_owned(),
                serde_json::json!({"question": question("France")}),
            ),
            (
                "c3".to_owned(),
                serde_json::json!({"question": question("Peru")}),
            ),
            (
                "c4".to_owned(),
                serde_json::json!({"question": question("France")}),
            ),
            (
                "c5".to_owned(),
                serde_json::json!({"question": question("France")}),
            ),
            (
                "c6".to_owned(),
                serde_json::json!({"question": question("France")}),
            ),
            (
                "c7".to_owned(),
                serde_json::json!({"question": question("France")}),
            ),
            (
                "c8".to_owned(),
                serde_json::json!({"question": question("France")}),
            ),
        ]);
        let witnesses = Witnesses::named(vec!["gateway".to_owned()])
            .keyed([("gateway".to_owned(), key)])
            .asked(inputs);

        let rows = trace_answers(
            &pins,
            "variant",
            "answers",
            &[
                answer("c1", Some("through")),
                answer("c2", Some("around")),
                answer("c3", Some("forged")),
                answer("c4", Some("told-to-repeat")),
                answer("c5", Some("padded")),
                answer("c6", Some("hinted")),
                answer("c7", Some("chained")),
                answer("c8", Some("circular")),
            ],
            &runs,
            None,
            &witnesses,
        )
        .expect("nothing contradicts the pins");
        let trace = GenerationTrace::of(&rows);

        assert_eq!(
            (rows[0].witnessed_answer, rows[0].witnessed_input),
            (Some(true), Some(true))
        );
        assert_eq!(
            (rows[1].witnessed_answer, rows[1].witnessed_input),
            (Some(false), Some(false)),
            "an answer the gateway never relayed, to a question about somewhere else"
        );
        assert_eq!(
            (rows[2].witnessed_answer, rows[2].witnessed_input),
            (Some(false), Some(false)),
            "digests under a key that is not the witness's say nothing"
        );
        assert_eq!(
            (
                trace.witnessed_answer,
                trace.witnessed_input,
                trace.witnessed_prompt
            ),
            (Some(6), Some(5), Some(8))
        );
        assert_eq!(
            [&rows[0], &rows[3], &rows[4], &rows[5]].map(|row| row.witnessed_exchange),
            [Some(true), Some(false), Some(false), Some(false)],
            "the answer in the request, words beside the prompt, or a value the application \
             made witness no exchange"
        );
        assert_eq!(
            [&rows[6], &rows[7]].map(|row| row.witnessed_exchange),
            [Some(true), Some(false)],
            "a reply fed on counts from where the input went in, and nothing counts from nowhere"
        );
        assert_eq!(trace.witnessed_exchange, Some(2));
        assert!(trace.witnessed() && !trace.answers_witnessed());
        assert!(
            trace.unwitnessed_answers()[0].contains("made around the gateway"),
            "{:?}",
            trace.unwitnessed_answers()
        );
        assert!(!format!("{witnesses:?}").contains(&hex::encode(key)));
    }

    #[test]
    fn an_answer_holding_a_wide_integer_is_compared_digit_for_digit_as_the_generation_spelled_it() {
        use aiwatcher_core::witness::{Said, digest, key_for};
        let mut pins = variant();
        pins.prompt = None;
        let key = key_for("gateway-secret");
        let exact = "123456789012345678901234567890";
        let neighbour = "123456789012345678901234567891";
        let replied = |text: &str| TracedRun {
            served_for_it: vec![TracedCall {
                model: Some("capitals-model".to_owned()),
                model_version: Some("v7".to_owned()),
                published_by: Some("gateway".to_owned()),
                replied: vec![digest(&key, Said::Replied, text)],
                ..TracedCall::default()
            }],
            ..run(vec![on_the_pins()])
        };
        let runs = BTreeMap::from([
            ("said-it".to_owned(), replied(exact)),
            ("said-another".to_owned(), replied(neighbour)),
        ]);
        let rounded = |case: &str, run: &str| RecordedAnswer {
            answer: serde_json::from_str(exact).expect("a number"),
            ..answer(case, Some(run))
        };
        let witnesses = Witnesses::named(vec!["gateway".to_owned()])
            .keyed([("gateway".to_owned(), key)])
            .spelled(BTreeMap::from([
                ("c1".to_owned(), exact.to_owned()),
                ("c2".to_owned(), exact.to_owned()),
            ]));

        let rows = trace_answers(
            &pins,
            "variant",
            "answers",
            &[rounded("c1", "said-it"), rounded("c2", "said-another")],
            &runs,
            None,
            &witnesses,
        )
        .expect("nothing contradicts the pins");

        assert_eq!(
            [rows[0].witnessed_answer, rows[1].witnessed_answer],
            [Some(true), Some(false)],
            "one integer a double cannot tell from the next is not the next"
        );
    }
}
