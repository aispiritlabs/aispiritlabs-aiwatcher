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
    /// Where each of those values was placed: the digest of the placeholder's
    /// name and of the value, made as a reply's are — what a judging call's
    /// reply naming a placeholder is read back through.
    pub placed: Vec<(String, String)>,
    /// Values the caller took out of another of its values in steps the
    /// witness repeated: each value's digest and the digest of the one it came
    /// out of.
    pub derived: Vec<(String, String)>,
    /// What a way of taking an answer that knows more than the reply took out
    /// of each reply, and the digest of that way ([`aiwatcher_core::witness::Said::Taking`]).
    pub taken: Vec<String>,
    pub taking: Option<String>,
    /// The caller said how it takes its answer out, and that way took nothing
    /// out of any reply: a reply it could not read, rather than one it passed
    /// over.
    pub took_nothing: bool,
    /// When the witness reported the call, in Unix milliseconds on its own
    /// clock — once it had relayed the reply, so calls ordered by it are in
    /// the order their replies came back.
    pub started_ms: Option<i64>,
}

/// One tool call a witness relayed for a run, as its span says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TracedTool {
    pub name: Option<String>,
    /// The credential both ends of the call's span were published under.
    pub published_by: Option<String>,
    /// The witness's digests of each part of the call's arguments, and of what
    /// the tool returned, made as a reply's are.
    pub arguments: Vec<String>,
    pub returned: Vec<String>,
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
    /// How the variant pins taking an answer out of a reply, where its
    /// generation config says.
    taking: Option<serde_json::Value>,
    /// The response schema the variant pins, which names the parts an answer
    /// made of several replies may have.
    shaped: Option<serde_json::Value>,
    /// The words the variant's generation config pins joining replies into a
    /// text answer with (`answer_joined`).
    joined: Option<Joining>,
    /// How the variant's generation config pins choosing an answer among its
    /// run's replies (`answer_chosen`).
    chosen: Option<Choosing>,
    /// Calls witnesses relayed while the measurement ran, in whatever run
    /// named them — what an application could have asked a case's question
    /// in before, or beside, the run it answers in.
    elsewhere: Vec<CallElsewhere>,
    /// The measurement's start was not in the log's fold, so no such call was
    /// looked for.
    elsewhere_unread: bool,
}

/// A call a witness relayed while a measurement ran, and the run it named as
/// its caller, if any.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallElsewhere {
    pub caller_run_id: Option<String>,
    pub call: TracedCall,
}

/// How a variant pins joining several replies into one text answer: the words
/// between them, and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Joining {
    /// `{"separator": ", "}`: replies one after another, this between each two.
    Separator(String),
    /// `{"template": "{{ capital }}, in {{ country }}"}`: a reply in each
    /// placeholder, and the template's own words around them.
    Template(Vec<String>),
}

/// Occurrences of a pinned join's words an answer may hold before it is not
/// read for them at all: each split of it is tried.
const MOST_JOINS: usize = 64;

impl Joining {
    fn read(pin: &serde_json::Value) -> Option<Self> {
        if let Some(separator) = pin.get("separator").and_then(serde_json::Value::as_str) {
            return (!separator.is_empty()).then(|| Self::Separator(separator.to_owned()));
        }
        let template = pin.get("template")?.as_str()?;
        let placeholder = regex::Regex::new(r"\{\{\s*[a-zA-Z][a-zA-Z0-9_]*\s*\}\}").ok()?;
        let mut literals = Vec::new();
        let mut from = 0;
        for found in placeholder.find_iter(template) {
            if template[..found.start()].ends_with('{') {
                continue;
            }
            literals.push(template[from..found.start()].to_owned());
            from = found.end();
        }
        literals.push(template[from..].to_owned());
        // Two replies with nothing between them could be split anywhere, and a
        // template with no placeholder joins nothing.
        (literals.len() > 1
            && literals[1..literals.len() - 1]
                .iter()
                .all(|l| !l.is_empty()))
        .then_some(Self::Template(literals))
    }

    /// The pieces of `text` between the pinned words, for the first way of
    /// splitting it along them whose every piece `is_part` accepts — every way
    /// is tried, so a reply holding the words itself is still found. `None`
    /// where no way does, or the words occur more often than are read.
    fn split(&self, text: &str, is_part: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
        let at = |words: &str| -> Option<Vec<usize>> {
            let mut found = Vec::new();
            let mut from = 0;
            while let Some(offset) = text[from..].find(words) {
                found.push(from + offset);
                if found.len() > MOST_JOINS {
                    return None;
                }
                from += offset
                    + text[from + offset..]
                        .chars()
                        .next()
                        .map_or(1, char::len_utf8);
            }
            Some(found)
        };
        let piece = |from: usize, to: usize| {
            let piece = &text[from..to];
            (!piece.trim().is_empty() && is_part(piece)).then(|| piece.to_owned())
        };
        match self {
            Self::Separator(separator) => {
                let places = at(separator)?;
                // From each place a piece may begin, the pieces before it.
                let mut reached: BTreeMap<usize, Vec<String>> = BTreeMap::from([(0, Vec::new())]);
                while let Some((from, before)) = reached.pop_first() {
                    if let Some(last) = piece(from, text.len()) {
                        return Some([before, vec![last]].concat());
                    }
                    for place in places.iter().filter(|place| **place > from) {
                        let next = place + separator.len();
                        if !reached.contains_key(&next)
                            && let Some(found) = piece(from, *place)
                        {
                            reached.insert(next, [before.clone(), vec![found]].concat());
                        }
                    }
                }
                None
            }
            Self::Template(literals) => {
                let (first, rest) = literals.split_first()?;
                let (last, between) = rest.split_last()?;
                let end = text.len().checked_sub(last.len())?;
                if !text.starts_with(first.as_str())
                    || !text.ends_with(last.as_str())
                    || end < first.len()
                {
                    return None;
                }
                let mut reached: Vec<(usize, Vec<String>)> = vec![(first.len(), Vec::new())];
                for words in between {
                    let places = at(words)?;
                    let mut next: BTreeMap<usize, Vec<String>> = BTreeMap::new();
                    for (from, before) in &reached {
                        for place in places
                            .iter()
                            .filter(|place| **place > *from && **place <= end)
                        {
                            let after = place + words.len();
                            if !next.contains_key(&after)
                                && let Some(found) = piece(*from, *place)
                            {
                                next.insert(after, [before.clone(), vec![found]].concat());
                            }
                        }
                    }
                    reached = next.into_iter().collect();
                }
                reached.into_iter().find_map(|(from, before)| {
                    (from <= end)
                        .then(|| piece(from, end))
                        .flatten()
                        .map(|found| [before, vec![found]].concat())
                })
            }
        }
    }
}

/// How a variant pins choosing its answer among replies its run's calls gave
/// that went into nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Choosing {
    /// `"first"`: the reply the witness relayed first.
    First,
    /// `{"most_of": n}`: exactly `n` such replies, and the answer's more often
    /// than any other.
    MostOf(usize),
    /// `{"judged": {"prompt": {"name": …, "version": …}, "pick": rule}}`: the
    /// candidates rendered into one call on this prompt version, whose reply,
    /// taken out by this rule, names the placeholder the answer was placed in.
    Judged {
        prompt: String,
        version: String,
        pick: serde_json::Value,
    },
}

impl Choosing {
    fn read(pin: &serde_json::Value) -> Option<Self> {
        if pin.as_str() == Some("first") {
            return Some(Self::First);
        }
        if let Some(judged) = pin.get("judged") {
            let prompt = judged.get("prompt")?;
            let text = |value: Option<&serde_json::Value>| {
                value
                    .and_then(serde_json::Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(ToOwned::to_owned)
            };
            return Some(Self::Judged {
                prompt: text(prompt.get("name"))?,
                version: text(prompt.get("version"))?,
                pick: judged.get("pick").filter(|rule| rule.is_object())?.clone(),
            });
        }
        pin.get("most_of")
            .and_then(serde_json::Value::as_u64)
            .filter(|n| *n > 0)
            .and_then(|n| usize::try_from(n).ok())
            .map(Self::MostOf)
    }

    /// The prompt version a judging call is made on, where one is pinned.
    fn judge_prompt(&self) -> Option<(&str, &str)> {
        match self {
            Self::Judged {
                prompt, version, ..
            } => Some((prompt.as_str(), version.as_str())),
            Self::First | Self::MostOf(_) => None,
        }
    }
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

    /// The calls witnesses relayed from when the measurement started until the
    /// traces step looked, in any run: a case's input asked on the pinned
    /// prompt in a run other than its answer's is a question the application
    /// could have chosen its answer's run by.
    #[must_use]
    pub fn asked_elsewhere(mut self, calls: Vec<CallElsewhere>) -> Self {
        self.elsewhere = calls;
        self.elsewhere_unread = false;
        self
    }

    /// When the measurement started is not known, so calls asked elsewhere
    /// during it could not be looked for: no answer is an exchange.
    #[must_use]
    pub fn elsewhere_unread(mut self) -> Self {
        self.elsewhere = Vec::new();
        self.elsewhere_unread = true;
        self
    }

    /// What the variant's generation config pins about making an answer out
    /// of replies, and the response schema it pins: how an answer is taken
    /// out of a reply (`answer_from`) — a way that knows more than the reply
    /// counts only where it is this one — the words replies are joined into a
    /// text answer with (`answer_joined`), how an answer is chosen among
    /// replies that went into nothing else (`answer_chosen`), and the shape an
    /// answer made of several replies has. Each absent, or not one of the
    /// shapes read, pins nothing, and what it would allow counts nowhere.
    #[must_use]
    pub fn pinned(
        mut self,
        config: Option<&serde_json::Value>,
        shaped: Option<serde_json::Value>,
    ) -> Self {
        let field = |name: &str| config.and_then(|config| config.get(name));
        self.taking = field("answer_from")
            .filter(|rule| rule.is_object())
            .cloned();
        self.joined = field("answer_joined").and_then(Joining::read);
        self.chosen = field("answer_chosen").and_then(Choosing::read);
        self.shaped = shaped;
        self
    }

    /// What a call replied, as a witness under `key` says: its replies, and
    /// what a way of taking that knows more than them took, where that way is
    /// the one the variant pins.
    fn replies_of<'a>(
        &self,
        call: &'a TracedCall,
        key: &[u8; 32],
    ) -> impl Iterator<Item = &'a String> {
        taken_by(call, key, self.taking.as_ref())
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

/// What a call replied, as a witness under `key` says: its replies, and what a
/// way of taking that knows more than them took, where that way is `rule`.
fn taken_by<'a>(
    call: &'a TracedCall,
    key: &[u8; 32],
    rule: Option<&serde_json::Value>,
) -> impl Iterator<Item = &'a String> {
    use aiwatcher_core::witness::{Said, canonical, digest};
    let pinned = rule.is_some_and(|rule| {
        call.taking.as_deref() == Some(digest(key, Said::Taking, &canonical(rule)).as_str())
    });
    call.replied
        .iter()
        .chain(call.taken.iter().filter(move |_| pinned))
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
    /// Tool calls such runs say they relayed for this one.
    pub tools_served_for_it: Vec<TracedTool>,
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
    /// the answer not in it — with values each accounted for: the case's input
    /// or a part of it, the reply of another call so made, what a tool a
    /// witness relayed returned to arguments so accounted for, or a value taken
    /// out of one of those in steps the witness repeated. A reply taken by a
    /// rule that knows more than it — a label's word — counts where the
    /// variant's generation config pins that rule, and an answer made of
    /// several replies counts where each part the pinned response schema names
    /// is such a reply, and a text answer made of several where the words
    /// between them are the ones the generation config pins joining them with.
    /// Where the run's witnessed calls gave replies that went into nothing
    /// else, the answer is one only where the way of choosing the generation
    /// config pins picks it ([`Self::chosen`]). Absent when the variant pins
    /// no prompt, which is what says what a request should hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_exchange: Option<bool>,
    /// It would be such an exchange, but its run's witnessed calls gave other
    /// replies that went into nothing else — into neither the answer, nor a
    /// call or a tool a witness relayed — so the application chose among them,
    /// and the variant pins no way of choosing that picks this one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chosen: bool,
    /// It would be such an exchange, but witnessed calls on the pinned prompt
    /// asked its case's input this many times in runs other than its own while
    /// the measurement ran — replies the application could have chosen the run
    /// it answered in by, before it answered or beside it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub asked_elsewhere: usize,
    /// It would be such an exchange, but when the measurement started was not
    /// in the log's fold, so calls asked elsewhere during it were not looked
    /// for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub elsewhere_unread: bool,
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
    /// Edge bounds of the pinned workflow that hold nothing a run could do,
    /// in words ([`Topology::idle_bounds`]): named, and never a refusal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_idle_bounds: Vec<String>,
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
    /// Edge bounds of the pinned workflow that hold nothing a run could do,
    /// in words: a bound no smaller than the times its source may complete, or
    /// than a bound it shares. Measured all the same — such a bound is true —
    /// and usually meant for another edge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub idle_bounds: Vec<String>,
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
    /// Answers a witnessed call relayed as its reply — or, part by part in the
    /// pinned response schema's shape, several calls — to a request that was
    /// nothing but the pinned prompt, rendered with values each accounted for
    /// ([`TracedAnswer::witnessed_exchange`]); absent when the variant pins no
    /// prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnessed_exchange: Option<usize>,
    /// Answers that would be an exchange but were chosen among replies their
    /// run's witnessed calls gave that went into nothing else, which no way of
    /// choosing the variant pins picks ([`TracedAnswer::chosen`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub chosen: usize,
    /// Answers that would be an exchange but whose cases' inputs witnessed
    /// calls on the pinned prompt asked in other runs while the measurement ran
    /// ([`TracedAnswer::asked_elsewhere`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub asked_elsewhere: usize,
    /// Answers that would be an exchange but for which calls asked elsewhere
    /// were not looked for, the measurement's start not being in the fold.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub elsewhere_unread: usize,
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
            idle_bounds: {
                let mut idle: Vec<String> = rows
                    .iter()
                    .flat_map(|row| row.workflow_idle_bounds.iter().cloned())
                    .collect();
                idle.sort();
                idle.dedup();
                idle
            },
            witnessed_model: counted(|row| row.witnessed_model),
            witnessed_prompt: counted(|row| row.witnessed_prompt),
            witnessed_answer: counted(|row| row.witnessed_answer),
            witnessed_input: counted(|row| row.witnessed_input),
            witnessed_exchange: counted(|row| row.witnessed_exchange),
            chosen: rows.iter().filter(|row| row.chosen).count(),
            asked_elsewhere: rows.iter().filter(|row| row.asked_elsewhere > 0).count(),
            elsewhere_unread: rows.iter().filter(|row| row.elsewhere_unread).count(),
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
        let denied = self.chosen + self.asked_elsewhere + self.elsewhere_unread;
        if let Some(on) = self.witnessed_exchange
            && on + denied < self.answers
        {
            said.push(format!(
                "{} of {} answers had no witnessed call relaying them — or each part the pinned \
                 response schema names, or each reply joined in the words the pinned \
                 answer_joined names — as its reply to a request that was nothing but the \
                 pinned prompt, rendered with their case's input, a reply or a tool's result \
                 relayed for a call so made, or a value taken out of one of those in steps the \
                 witness repeated — words beside those, a label's word the variant does not pin, \
                 or a value the application made, such as an answer to repeat, witness nothing",
                self.answers - on - denied,
                self.answers
            ));
        }
        if self.chosen > 0 {
            said.push(format!(
                "{} of {} answers were chosen among replies their run's witnessed calls gave that \
                 went into nothing else — a choice the application made, which counts only where \
                 the generation config pins answer_chosen and that way picks the answer",
                self.chosen, self.answers
            ));
        }
        if self.asked_elsewhere > 0 {
            said.push(format!(
                "{} of {} answers' cases were asked on the pinned prompt in other runs while this \
                 measurement ran — replies the application could have seen before it answered, \
                 and chosen the run it answered in by",
                self.asked_elsewhere, self.answers
            ));
        }
        if self.elsewhere_unread > 0 {
            said.push(format!(
                "{} of {} answers could not be held to calls asked in other runs, because when \
                 this measurement started is not in the log's fold",
                self.elsewhere_unread, self.answers
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
/// application added: on the pinned prompt — or the one a pinned way of
/// choosing asks a judging call on — found to be nothing but its
/// template rendered, with every value it was rendered with accounted for — a
/// part of the case's input, a reply of another such call, what a tool a
/// witness relayed returned to arguments so accounted for, or a value taken out
/// of one of those in steps the witness repeated. Found by adding a call or a
/// tool once all it was made of is accounted for, until none is left to add —
/// so a chain of calls and tools feeding each other on is counted, and two
/// calls whose values are only each other's replies are not.
fn grounded_calls(
    run: &TracedRun,
    variant: &VariantManifest,
    witnesses: &Witnesses,
    input: Option<&serde_json::Value>,
) -> Vec<bool> {
    use aiwatcher_core::witness::{Said, carried_as, digest};
    let calls = &run.served_for_it;
    let tools = &run.tools_served_for_it;
    let mut grounded = vec![false; calls.len()];
    let (Some(pin), Some(input)) = (&variant.prompt, input) else {
        return grounded;
    };
    let witness_key = |published_by: Option<&str>| {
        let witness = published_by?;
        let admitted = run
            .published_by
            .as_deref()
            .is_some_and(|publisher| publisher != witness)
            && witnesses.admits(witness);
        witnesses.keys.get(witness).filter(|_| admitted)
    };
    let call_keys: Vec<Option<&[u8; 32]>> = calls
        .iter()
        .map(|call| {
            let judging = witnesses
                .chosen
                .as_ref()
                .and_then(Choosing::judge_prompt)
                .is_some_and(|(name, version)| {
                    call.prompt_name.as_deref() == Some(name)
                        && call.prompt_version.as_deref() == Some(version)
                });
            let made_of_the_prompt = (judging
                || (call.prompt_name.as_deref() == Some(pin.name.as_str())
                    && call.prompt_version.as_deref() == Some(pin.version.as_str())))
                && call.prompt_verified == Some(true)
                && call.prompt_exact == Some(true)
                && !call.rendered.is_empty();
            witness_key(call.published_by.as_deref()).filter(|_| made_of_the_prompt)
        })
        .collect();
    let tool_keys: Vec<Option<&[u8; 32]>> = tools
        .iter()
        .map(|tool| witness_key(tool.published_by.as_deref()))
        .collect();
    let carried = carried_as(input);
    let mut accounted: std::collections::BTreeSet<String> = call_keys
        .iter()
        .chain(&tool_keys)
        .flatten()
        .flat_map(|key| carried.iter().map(|text| digest(key, Said::Replied, text)))
        .collect();
    let mut grounded_tools = vec![false; tools.len()];
    loop {
        let mut added = false;
        for (at, call) in calls.iter().enumerate() {
            let Some(key) = call_keys[at].filter(|_| !grounded[at]) else {
                continue;
            };
            let mut known: std::collections::BTreeSet<&str> =
                accounted.iter().map(String::as_str).collect();
            // A value taken out of an accounted value is accounted, and so on
            // down a chain of them.
            while let Some((value, _)) = call.derived.iter().find(|(value, source)| {
                known.contains(source.as_str()) && !known.contains(value.as_str())
            }) {
                known.insert(value);
            }
            if call
                .rendered
                .iter()
                .all(|value| known.contains(value.as_str()))
            {
                grounded[at] = true;
                added = true;
                accounted.extend(witnesses.replies_of(call, key).cloned());
            }
        }
        for (at, tool) in tools.iter().enumerate() {
            if grounded_tools[at] || tool_keys[at].is_none() {
                continue;
            }
            if tool.arguments.iter().all(|part| accounted.contains(part)) {
                grounded_tools[at] = true;
                added = true;
                accounted.extend(tool.returned.iter().cloned());
            }
        }
        if !added {
            return grounded;
        }
    }
}

/// The parts of an answer made of several replies, each as a reply is compared
/// — text as itself, a number or a flag as its canonical JSON — where every key
/// it uses is one the pinned response schema names at that place. `None` for an
/// answer that is not an object or a list, or uses a key the schema does not
/// name.
fn parts_by_schema(
    answer: &aiwatcher_core::exact::ExactValue,
    schema: &serde_json::Value,
) -> Option<Vec<String>> {
    use aiwatcher_core::exact::ExactValue;
    fn walk(value: &ExactValue, schema: &serde_json::Value, into: &mut Vec<String>) -> bool {
        match value {
            ExactValue::Object(fields) => {
                let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
                    return false;
                };
                fields.iter().all(|(key, field)| {
                    properties
                        .get(key)
                        .is_some_and(|inner| walk(field, inner, into))
                })
            }
            ExactValue::Array(items) => schema
                .get("items")
                .is_some_and(|inner| items.iter().all(|item| walk(item, inner, into))),
            ExactValue::Text(text) => {
                if !text.trim().is_empty() {
                    into.push(text.clone());
                }
                true
            }
            ExactValue::Number(number) => {
                into.push(aiwatcher_core::witness::exact_number(number));
                true
            }
            ExactValue::Bool(flag) => {
                into.push(flag.to_string());
                true
            }
            ExactValue::Null => true,
        }
    }
    if !matches!(answer, ExactValue::Object(_) | ExactValue::Array(_)) {
        return None;
    }
    let mut parts = Vec::new();
    (walk(answer, schema, &mut parts) && !parts.is_empty()).then_some(parts)
}

/// Whether an answer made of `answered_by`'s replies was not chosen among
/// others, or was chosen the way the variant pins.
///
/// The replies that count are those the run's witnessed calls gave that went
/// into nothing else a witness saw — no call it relayed was rendered with one
/// or took a value out of one, and no tool it relayed was handed one whose
/// result went on in turn — except a call whose way of taking its answer, the
/// same as the answer's, took nothing out of it, which the application could
/// not have read. Where every one of them is the answer's, nothing was chosen.
/// Otherwise the variant's `answer_chosen` decides, over those replies all
/// made of nothing the application added and for an answer that is one reply:
/// `first` wants the answer's to be the one the witness relayed first, alone,
/// `most_of: n` wants exactly `n` of them, the answer's given more often than
/// any other, and `judged` wants the one besides the answer's to be a call on
/// the judging prompt it pins, taking its answer out the way it pins, whose
/// reply names exactly one placeholder — and the value placed there to be the
/// answer's reply.
fn chosen_as_pinned(
    run: &TracedRun,
    witnesses: &Witnesses,
    grounded: &[bool],
    answered_by: &std::collections::BTreeSet<usize>,
    in_parts: bool,
) -> bool {
    let calls = &run.served_for_it;
    let key_of = |published_by: Option<&str>| {
        let witness = published_by?;
        let admitted = run
            .published_by
            .as_deref()
            .is_some_and(|publisher| publisher != witness)
            && witnesses.admits(witness);
        witnesses.keys.get(witness).filter(|_| admitted)
    };
    let replies: Vec<Vec<&String>> = calls
        .iter()
        .map(|call| {
            key_of(call.published_by.as_deref())
                .map(|key| witnesses.replies_of(call, key).collect())
                .unwrap_or_default()
        })
        .collect();
    let mut went_on: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for call in calls
        .iter()
        .filter(|call| key_of(call.published_by.as_deref()).is_some())
    {
        went_on.extend(call.rendered.iter().map(String::as_str));
        went_on.extend(call.derived.iter().map(|(_, source)| source.as_str()));
    }
    // A tool's arguments went on only where what it returned did, into a call
    // or into another tool that did: handed to a tool whose result went into
    // nothing, a reply went into nothing either.
    let tools: Vec<&TracedTool> = run
        .tools_served_for_it
        .iter()
        .filter(|tool| key_of(tool.published_by.as_deref()).is_some())
        .collect();
    let mut onward = vec![false; tools.len()];
    loop {
        let mut added = false;
        for (at, tool) in tools.iter().enumerate() {
            if !onward[at]
                && tool
                    .returned
                    .iter()
                    .any(|returned| went_on.contains(returned.as_str()))
            {
                onward[at] = true;
                added = true;
                went_on.extend(tool.arguments.iter().map(String::as_str));
            }
        }
        if !added {
            break;
        }
    }
    let takings: std::collections::BTreeSet<&str> = answered_by
        .iter()
        .filter_map(|at| calls[*at].taking.as_deref())
        .collect();
    let finals: Vec<usize> = (0..calls.len())
        .filter(|at| !replies[*at].is_empty())
        .filter(|at| {
            answered_by.contains(at)
                || !replies[*at]
                    .iter()
                    .any(|reply| went_on.contains(reply.as_str()))
        })
        .filter(|at| {
            !(calls[*at].took_nothing
                && calls[*at]
                    .taking
                    .as_deref()
                    .is_some_and(|taking| takings.contains(taking)))
        })
        .collect();
    if finals.iter().all(|at| answered_by.contains(at)) {
        return true;
    }
    if in_parts || !finals.iter().all(|at| grounded[*at]) {
        return false;
    }
    match &witnesses.chosen {
        None => false,
        Some(Choosing::Judged {
            prompt,
            version,
            pick,
        }) => {
            // One call judged, and nothing else went unused: the candidates
            // went into it, and its reply names where the answer was placed.
            let unanswered: Vec<usize> = finals
                .iter()
                .copied()
                .filter(|at| !answered_by.contains(at))
                .collect();
            let [judge] = unanswered[..] else {
                return false;
            };
            let call = &calls[judge];
            let Some(key) = key_of(call.published_by.as_deref()) else {
                return false;
            };
            use aiwatcher_core::witness::{Said, canonical, digest};
            if call.prompt_name.as_deref() != Some(prompt.as_str())
                || call.prompt_version.as_deref() != Some(version.as_str())
                || call.taking.as_deref()
                    != Some(digest(key, Said::Taking, &canonical(pick)).as_str())
            {
                return false;
            }
            let said: std::collections::BTreeSet<&String> =
                taken_by(call, key, Some(pick)).collect();
            let named: std::collections::BTreeSet<&str> = call
                .placed
                .iter()
                .filter(|(name, _)| said.contains(name))
                .map(|(_, value)| value.as_str())
                .collect();
            let [picked] = named.into_iter().collect::<Vec<_>>()[..] else {
                return false;
            };
            answered_by
                .iter()
                .all(|at| replies[*at].iter().any(|reply| reply.as_str() == picked))
        }
        Some(Choosing::First) => {
            let mut timed: Vec<(i64, usize)> = Vec::with_capacity(finals.len());
            for at in &finals {
                let Some(started) = calls[*at].started_ms else {
                    return false;
                };
                timed.push((started, *at));
            }
            timed.sort_unstable();
            let first = timed[0].0;
            let at_first: Vec<usize> = timed
                .iter()
                .take_while(|(started, _)| *started == first)
                .map(|(_, at)| *at)
                .collect();
            at_first.iter().all(|at| answered_by.contains(at))
        }
        Some(Choosing::MostOf(n)) => {
            if finals.len() != *n {
                return false;
            }
            // Replies that share a digest are one reply given again.
            let mut group: Vec<usize> = (0..finals.len()).collect();
            fn root(group: &mut [usize], at: usize) -> usize {
                let mut at = at;
                while group[at] != at {
                    group[at] = group[group[at]];
                    at = group[at];
                }
                at
            }
            for one in 0..finals.len() {
                for other in (one + 1)..finals.len() {
                    if replies[finals[one]]
                        .iter()
                        .any(|reply| replies[finals[other]].contains(reply))
                    {
                        let (a, b) = (root(&mut group, one), root(&mut group, other));
                        group[a] = b;
                    }
                }
            }
            let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
            let mut answers = std::collections::BTreeSet::new();
            for (position, at) in finals.iter().enumerate() {
                let root = root(&mut group, position);
                *sizes.entry(root).or_default() += 1;
                if answered_by.contains(at) {
                    answers.insert(root);
                }
            }
            let [answer] = answers.into_iter().collect::<Vec<_>>()[..] else {
                return false;
            };
            sizes
                .iter()
                .all(|(root, size)| *root == answer || *size < sizes[&answer])
        }
    }
}

/// How many calls witnesses relayed while the measurement ran, in runs other
/// than `run_id` and than `alike` — the runs of cases asking the same — asked
/// the case's whole input on the pinned prompt, or the judging prompt a pinned
/// way of choosing names, of the pinned model where one is pinned: its text, or
/// every text in it.
fn asked_elsewhere(
    variant: &VariantManifest,
    witnesses: &Witnesses,
    input: Option<&serde_json::Value>,
    run_id: &str,
    run: &TracedRun,
    alike: &std::collections::BTreeSet<&str>,
) -> usize {
    use aiwatcher_core::witness::{Said, asked_as, digest};
    let (Some(pin), Some(input)) = (&variant.prompt, input) else {
        return 0;
    };
    let judging = witnesses.chosen.as_ref().and_then(Choosing::judge_prompt);
    witnesses
        .elsewhere
        .iter()
        .filter(|elsewhere| {
            elsewhere
                .caller_run_id
                .as_deref()
                .is_none_or(|caller| caller != run_id && !alike.contains(caller))
        })
        .filter(|elsewhere| {
            let call = &elsewhere.call;
            let Some(witness) = call.published_by.as_deref() else {
                return false;
            };
            let admitted = run
                .published_by
                .as_deref()
                .is_some_and(|publisher| publisher != witness)
                && witnesses.admits(witness);
            let Some(key) = witnesses.keys.get(witness).filter(|_| admitted) else {
                return false;
            };
            let on = |name: &str, version: &str| {
                call.prompt_name.as_deref() == Some(name)
                    && call.prompt_version.as_deref() == Some(version)
            };
            if call.prompt_verified != Some(true)
                || !(on(&pin.name, &pin.version)
                    || judging.is_some_and(|(name, version)| on(name, version)))
                || variant
                    .model
                    .as_ref()
                    .is_some_and(|model| call.model.as_deref() != Some(model.name.as_str()))
            {
                return false;
            }
            let holds = |text: &String| call.asked.contains(&digest(key, Said::Asked, text));
            match asked_as(input).split_first() {
                None => false,
                Some((whole, parts)) => {
                    holds(whole) || (!parts.is_empty() && parts.iter().all(holds))
                }
            }
        })
        .count()
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
    let idle_bounds = workflow.map(Topology::idle_bounds).unwrap_or_default();
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
            chosen: false,
            asked_elsewhere: 0,
            elsewhere_unread: false,
            witnessed_by: Vec::new(),
            self_witnessed: false,
            served_models: Vec::new(),
            models: Vec::new(),
            workflow_undeclared: variant.workflow.is_some() && workflow.is_none(),
            workflow_steps_unread: false,
            workflow_idle_bounds: idle_bounds.clone(),
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
            // The calls whose replies the answer is made of, and whether it is
            // made of several parts rather than one reply.
            let mut answered_by = std::collections::BTreeSet::new();
            let mut in_parts = false;
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
                    let replies: Vec<String> = witnesses.replies_of(call, key).cloned().collect();
                    let replied = holds(&replies, Said::Replied, answered.clone());
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
                        answered_by.insert(at);
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
            // An answer made of several replies — in the shape the variant pins,
            // or joined in the words it pins: an exchange when each part is one,
            // each from a call so made that did not hold it in its request.
            let replying = |part: &str| -> Vec<usize> {
                use aiwatcher_core::witness::{Said, digest};
                run.served_for_it
                    .iter()
                    .enumerate()
                    .filter(|(at, call)| {
                        let Some(key) = call
                            .published_by
                            .as_deref()
                            .and_then(|witness| witnesses.keys.get(witness))
                            .filter(|_| grounded[*at])
                        else {
                            return false;
                        };
                        witnesses
                            .replies_of(call, key)
                            .any(|reply| *reply == digest(key, Said::Replied, part))
                            && !call.asked.contains(&digest(key, Said::Asked, part))
                    })
                    .map(|(at, _)| at)
                    .collect()
            };
            if row.witnessed_exchange == Some(false) {
                let shaped = witnesses.shaped.as_ref().and_then(|schema| {
                    let exact = witnesses
                        .spelled
                        .get(&answer.case_id)
                        .and_then(|text| aiwatcher_core::exact::ExactValue::parse(text))
                        .unwrap_or_else(|| {
                            aiwatcher_core::exact::ExactValue::from_value(&answer.answer)
                        });
                    parts_by_schema(&exact, schema)
                        .filter(|parts| parts.iter().all(|part| !replying(part).is_empty()))
                });
                let joined = || {
                    let joining = witnesses.joined.as_ref()?;
                    let text = answer.answer.as_str()?;
                    joining.split(text, &|piece| !replying(piece).is_empty())
                };
                if let Some(parts) = shaped.or_else(joined) {
                    row.witnessed_exchange = Some(true);
                    in_parts = true;
                    for part in &parts {
                        answered_by.extend(replying(part));
                    }
                }
            }
            // Replies that went into nothing else: the application may have
            // chosen the answer among them.
            if row.witnessed_exchange == Some(true)
                && !chosen_as_pinned(run, witnesses, &grounded, &answered_by, in_parts)
            {
                row.witnessed_exchange = Some(false);
                row.chosen = true;
            }
            // The same question asked in another run while the measurement ran:
            // the application could have chosen this run by what came back.
            if row.witnessed_exchange == Some(true) {
                if witnesses.elsewhere_unread {
                    row.witnessed_exchange = Some(false);
                    row.elsewhere_unread = true;
                } else {
                    let input = witnesses.inputs.get(&answer.case_id);
                    let alike: std::collections::BTreeSet<&str> = answers
                        .iter()
                        .filter(|other| {
                            other.case_id != answer.case_id
                                && input.is_some()
                                && witnesses.inputs.get(&other.case_id) == input
                        })
                        .filter_map(|other| other.run_id.as_deref())
                        .collect();
                    let asked = asked_elsewhere(variant, witnesses, input, run_id, run, &alike);
                    if asked > 0 {
                        row.witnessed_exchange = Some(false);
                        row.asked_elsewhere = asked;
                    }
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
            placed: Vec::new(),
            derived: Vec::new(),
            taken: Vec::new(),
            taking: None,
            took_nothing: false,
            started_ms: None,
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
                idle_bounds: Vec::new(),
                witnessed_model: Some(0),
                witnessed_prompt: Some(0),
                witnessed_answer: Some(0),
                witnessed_input: Some(0),
                witnessed_exchange: Some(0),
                chosen: 0,
                asked_elsewhere: 0,
                elsewhere_unread: 0,
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
    fn a_bound_two_ways_back_share_holds_the_loop_to_its_rounds_whichever_way_each_went() {
        let declared = |bounds: serde_json::Value| {
            Topology::read(&serde_json::json!({
                "nodes": ["write", "review", "fix", "publish"],
                "edges": [
                    ["write", "review"],
                    ["review", "write"],
                    ["review", "fix"],
                    ["fix", "write"],
                    ["review", "publish"]
                ],
                "bounds": bounds
            }))
            .expect("a shape")
        };
        let together = declared(serde_json::json!([
            {"edges": [["review", "write"], ["fix", "write"]], "at_most": 2}
        ]));
        assert!(
            together.misbounded().is_empty(),
            "both lead back into write"
        );
        let apart = declared(serde_json::json!([
            {"edges": [["review", "write"]], "at_most": 2},
            {"edges": [["fix", "write"]], "at_most": 2}
        ]));
        let back_to_write: &[&'static str] = &["write:s", "write:c", "review:s", "review:c"];
        let through_fix: &[&'static str] = &[
            "fix:s", "fix:c", "write:s", "write:c", "review:s", "review:c",
        ];
        let run = |rounds: &[&[&'static str]]| {
            let mut steps = vec!["write:s", "write:c", "review:s", "review:c"];
            for round in rounds {
                steps.extend(round.iter());
            }
            steps.extend(["publish:s", "publish:c"]);
            steps
        };

        let twice = run(&[back_to_write, through_fix]);
        assert_eq!(
            traversed(&together, &twice).expect("once each way")[0].on_workflow,
            Some(true)
        );
        let four = run(&[back_to_write, through_fix, back_to_write, through_fix]);
        assert_eq!(
            traversed(&apart, &four).expect("twice each way, each within its own bound")[0]
                .on_workflow,
            Some(true),
            "two bounds apart let the cycle go round as often as both allow"
        );
        let refused = traversed(&together, &four).expect_err("four rounds in all");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(
            refused[0].contains("went along fix to write and review to write more than 2 times"),
            "{refused:?}"
        );

        let retried = run(&[
            back_to_write,
            &["fix:s", "fix:c", "write:s", "write:f"],
            &[
                "write:s",
                "write:c",
                "review:s",
                "review:c",
                "publish:s",
                "publish:c",
            ],
        ]);
        assert_eq!(
            traversed(&together, &retried[..retried.len() - 2]).expect("a retry is no round")[0]
                .on_workflow,
            Some(true)
        );
    }

    #[test]
    fn a_pinned_bound_that_holds_nothing_is_still_measured_and_the_trace_names_it() {
        let pinned = Topology::read(&serde_json::json!({
            "nodes": ["retrieve", "answer"],
            "edges": [{"from": "retrieve", "to": "answer", "at_most": 1}]
        }))
        .expect("a shape");

        let rows = traversed(
            &pinned,
            &["retrieve:s", "retrieve:c", "answer:s", "answer:c"],
        )
        .expect("a bound that holds nothing is true, and refuses no run");
        let trace = GenerationTrace::of(&rows);

        assert_eq!(rows[0].on_workflow, Some(true));
        assert_eq!(
            trace.idle_bounds,
            [
                "the bound of at most 1 on retrieve to answer holds nothing, since retrieve \
              completes at most once on this shape"
            ]
        );
        assert!(
            trace.complete(),
            "a bound that holds nothing is no shortfall"
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
    fn a_value_cut_from_the_input_a_tool_s_result_a_pinned_label_and_a_composed_answer_are_accounted()
     {
        use aiwatcher_core::witness::{Said, canonical, digest, key_for};
        let mut pins = variant();
        pins.model = None;
        let key = key_for("gateway-secret");
        let said = |text: &str| digest(&key, Said::Replied, text);
        let question = "What is the capital of France?";
        let call = |rendered: &[&str], replied: &[&str]| TracedCall {
            model: Some("gpt-4o".to_owned()),
            prompt_name: Some("capitals".to_owned()),
            prompt_version: Some("p".repeat(64)),
            prompt_verified: Some(true),
            prompt_exact: Some(true),
            published_by: Some("gateway".to_owned()),
            rendered: rendered.iter().map(|text| said(text)).collect(),
            replied: replied.iter().map(|text| said(text)).collect(),
            ..TracedCall::default()
        };
        let tool = |arguments: &[&str], returned: &[&str]| TracedTool {
            name: Some("search".to_owned()),
            published_by: Some("gateway".to_owned()),
            arguments: arguments.iter().map(|text| said(text)).collect(),
            returned: returned.iter().map(|text| said(text)).collect(),
        };
        let with = |calls: Vec<TracedCall>, tools: Vec<TracedTool>| TracedRun {
            served_for_it: calls,
            tools_served_for_it: tools,
            ..run(vec![on_the_pins()])
        };
        let found = r#"{"capital":"Paris"}"#;
        let labels =
            serde_json::json!({"steps": [{"strip": "."}, {"map": {"A": "Paris", "B": "Lyon"}}]});
        let labelled = |rule: &serde_json::Value| TracedCall {
            taken: vec![said("Paris")],
            taking: Some(digest(&key, Said::Taking, &canonical(rule))),
            ..call(&[question], &["A"])
        };
        let runs = BTreeMap::from([
            (
                "cut".to_owned(),
                with(
                    vec![TracedCall {
                        derived: vec![(said("France"), said(question))],
                        ..call(&["France", question], &["Paris"])
                    }],
                    Vec::new(),
                ),
            ),
            (
                "cut-from-nothing".to_owned(),
                with(
                    vec![TracedCall {
                        derived: vec![(said("France"), said("Think of Paris"))],
                        ..call(&["France", "Think of Paris"], &["Paris"])
                    }],
                    Vec::new(),
                ),
            ),
            (
                // The model asks for a search, the gateway relays it, and the
                // next call is rendered with what the tool returned.
                "searched".to_owned(),
                with(
                    vec![
                        call(&[found], &["Paris"]),
                        call(&[question], &[r#"{"query": "France"}"#, "France"]),
                    ],
                    vec![tool(&["France"], &[found])],
                ),
            ),
            (
                "searched-for-the-answer".to_owned(),
                with(
                    vec![call(&[found], &["Paris"]), call(&[question], &["France"])],
                    vec![tool(&["Paris"], &[found])],
                ),
            ),
            (
                "labelled".to_owned(),
                with(vec![labelled(&labels)], Vec::new()),
            ),
            (
                "labelled-otherwise".to_owned(),
                with(
                    vec![labelled(&serde_json::json!({"map": {"A": "Paris"}}))],
                    Vec::new(),
                ),
            ),
            (
                "composed".to_owned(),
                with(
                    vec![
                        call(&[question], &["Paris"]),
                        call(&[question], &["France"]),
                    ],
                    Vec::new(),
                ),
            ),
            (
                "composed-with-more".to_owned(),
                with(vec![call(&[question], &["Paris"])], Vec::new()),
            ),
        ]);
        let cases = [
            "cut",
            "cut-from-nothing",
            "searched",
            "searched-for-the-answer",
            "labelled",
            "labelled-otherwise",
            "composed",
            "composed-with-more",
        ];
        let witnesses = Witnesses::named(vec!["gateway".to_owned()])
            .keyed([("gateway".to_owned(), key)])
            .asked(
                cases
                    .iter()
                    .map(|case| {
                        (
                            (*case).to_owned(),
                            serde_json::json!({"question": question}),
                        )
                    })
                    .collect(),
            )
            .pinned(
                Some(&serde_json::json!({"answer_from": labels.clone()})),
                Some(serde_json::json!({
                    "type": "object",
                    "properties": {"capital": {"type": "string"}, "country": {"type": "string"}}
                })),
            );
        let mut answers: Vec<RecordedAnswer> =
            cases.iter().map(|case| answer(case, Some(case))).collect();
        answers[6].answer = serde_json::json!({"capital": "Paris", "country": "France"});
        answers[7].answer = serde_json::json!({"capital": "Paris", "hint": "Paris"});

        let rows = trace_answers(
            &pins, "variant", "answers", &answers, &runs, None, &witnesses,
        )
        .expect("nothing contradicts the pins");

        assert_eq!(
            rows.iter()
                .map(|row| (row.case_id.as_str(), row.witnessed_exchange))
                .collect::<Vec<_>>(),
            [
                ("cut", Some(true)),
                ("cut-from-nothing", Some(false)),
                ("searched", Some(true)),
                ("searched-for-the-answer", Some(false)),
                ("labelled", Some(true)),
                ("labelled-otherwise", Some(false)),
                ("composed", Some(true)),
                ("composed-with-more", Some(false)),
            ],
            "a value taken out of the input, a tool's result for arguments the model gave, a \
             label's word the variant pins, and parts the pinned schema names are accounted; \
             a source nobody gave, arguments the application made, a rule the variant does not \
             pin and a part the schema does not name are not"
        );
        assert_eq!(
            rows[5].witnessed_answer,
            Some(false),
            "a label's word under a rule nobody pinned is not the model's reply"
        );
    }

    #[test]
    fn replies_joined_in_the_pinned_words_and_an_answer_chosen_the_pinned_way_are_exchanges() {
        use aiwatcher_core::witness::{Said, digest, key_for};
        let mut pins = variant();
        pins.model = None;
        let key = key_for("gateway-secret");
        let said = |text: &str| digest(&key, Said::Replied, text);
        let question = "What is the capital of France?";
        let call = |replied: &str, at: i64| TracedCall {
            model: Some("gpt-4o".to_owned()),
            prompt_name: Some("capitals".to_owned()),
            prompt_version: Some("p".repeat(64)),
            prompt_verified: Some(true),
            prompt_exact: Some(true),
            published_by: Some("gateway".to_owned()),
            rendered: vec![said(question)],
            replied: vec![said(replied)],
            started_ms: Some(at),
            ..TracedCall::default()
        };
        let traced = |config: serde_json::Value, cases: Vec<(&str, &str, Vec<TracedCall>)>| {
            let runs: BTreeMap<String, TracedRun> = cases
                .iter()
                .map(|(case, _, calls)| {
                    (
                        (*case).to_owned(),
                        TracedRun {
                            served_for_it: calls.clone(),
                            ..run(vec![on_the_pins()])
                        },
                    )
                })
                .collect();
            let witnesses = Witnesses::named(vec!["gateway".to_owned()])
                .keyed([("gateway".to_owned(), key)])
                .asked(
                    cases
                        .iter()
                        .map(|(case, _, _)| {
                            (
                                (*case).to_owned(),
                                serde_json::json!({"question": question}),
                            )
                        })
                        .collect(),
                )
                .pinned(Some(&config), None);
            let answers: Vec<RecordedAnswer> = cases
                .iter()
                .map(|(case, answered, _)| RecordedAnswer {
                    answer: serde_json::json!(answered),
                    ..answer(case, Some(case))
                })
                .collect();
            trace_answers(
                &pins, "variant", "answers", &answers, &runs, None, &witnesses,
            )
            .expect("nothing contradicts the pins")
        };
        let brief = |rows: Vec<TracedAnswer>| {
            rows.into_iter()
                .map(|row| (row.case_id, row.witnessed_exchange, row.chosen))
                .collect::<Vec<_>>()
        };
        let taking = |call: TracedCall, rule: &str, nothing: bool| TracedCall {
            taking: Some(digest(&key, Said::Taking, rule)),
            took_nothing: nothing,
            ..call
        };

        let rows = brief(traced(
            serde_json::json!({"answer_joined": {"separator": ", "}, "answer_chosen": {"most_of": 3}}),
            vec![
                (
                    "joined",
                    "Paris, France",
                    vec![call("Paris", 1), call("France", 2)],
                ),
                (
                    "joined-with-its-own-words",
                    "Paris, the capital, France",
                    vec![call("Paris, the capital", 1), call("France", 2)],
                ),
                (
                    "joined-otherwise",
                    "Paris and France",
                    vec![call("Paris", 1), call("France", 2)],
                ),
                (
                    "joined-with-a-reply-to-spare",
                    "Paris, France",
                    vec![call("Paris", 1), call("France", 2), call("Lyon", 3)],
                ),
                (
                    "most-of-three",
                    "Paris",
                    vec![call("Paris", 1), call("Lyon", 2), call("Paris", 3)],
                ),
                (
                    "three-apart",
                    "Paris",
                    vec![call("Paris", 1), call("Lyon", 2), call("Nice", 3)],
                ),
                (
                    "most-of-four",
                    "Paris",
                    vec![
                        call("Paris", 1),
                        call("Lyon", 2),
                        call("Paris", 3),
                        call("Paris", 4),
                    ],
                ),
            ],
        ));
        assert_eq!(
            rows,
            [
                ("joined".to_owned(), Some(true), false),
                ("joined-with-its-own-words".to_owned(), Some(true), false),
                ("joined-otherwise".to_owned(), Some(false), false),
                ("joined-with-a-reply-to-spare".to_owned(), Some(false), true),
                ("most-of-three".to_owned(), Some(true), false),
                ("three-apart".to_owned(), Some(false), true),
                ("most-of-four".to_owned(), Some(false), true),
            ],
            "replies joined in the pinned words, a reply joined holding them itself, and the \
             answer most of the pinned number of replies gave are exchanges; other words, a \
             reply a joined answer left over, a tie and another number of replies are not"
        );

        let rows = brief(traced(
            serde_json::json!({"answer_joined": {"template": "{{ capital }} is in {{ country }}."}, "answer_chosen": "first"}),
            vec![
                (
                    "templated",
                    "Paris is in France.",
                    vec![call("Paris", 1), call("France", 2)],
                ),
                (
                    "templated-otherwise",
                    "Paris, in France",
                    vec![call("Paris", 1), call("France", 2)],
                ),
                ("first", "Paris", vec![call("Paris", 1), call("Lyon", 2)]),
                (
                    "not-first",
                    "Paris",
                    vec![call("Lyon", 1), call("Paris", 2)],
                ),
                ("together", "Paris", vec![call("Paris", 1), call("Lyon", 1)]),
            ],
        ));
        assert_eq!(
            rows.iter()
                .map(|(case, exchange, _)| (case.as_str(), *exchange))
                .collect::<Vec<_>>(),
            [
                ("templated", Some(true)),
                ("templated-otherwise", Some(false)),
                ("first", Some(true)),
                ("not-first", Some(false)),
                ("together", Some(false)),
            ],
            "a template's own words around the replies, and the reply the witness relayed \
             first alone"
        );

        let rows = traced(
            serde_json::json!({"temperature": 0}),
            vec![
                (
                    "one-of-two",
                    "Paris",
                    vec![call("Paris", 1), call("Lyon", 2)],
                ),
                (
                    "again-the-same",
                    "Paris",
                    vec![call("Paris", 1), call("Paris", 2)],
                ),
                (
                    "unreadable",
                    "Paris",
                    vec![
                        taking(call("Paris", 2), "rule", false),
                        taking(call("It is Lyon", 1), "rule", true),
                    ],
                ),
                (
                    "peeked",
                    "Paris",
                    vec![
                        taking(call("Paris", 2), "rule", false),
                        taking(call("It is Lyon", 1), "another rule", true),
                    ],
                ),
            ],
        );
        let trace = GenerationTrace::of(&rows);
        assert_eq!(
            brief(rows),
            [
                ("one-of-two".to_owned(), Some(false), true),
                ("again-the-same".to_owned(), Some(true), false),
                ("unreadable".to_owned(), Some(true), false),
                ("peeked".to_owned(), Some(false), true),
            ],
            "with no way of choosing pinned, a reply chosen over another is no exchange — \
             unless the other said the same, or the answer's own way of taking it read nothing \
             out of it"
        );
        let said = trace.unwitnessed_answers();
        assert!(
            said.iter()
                .any(|line| line.starts_with("2 of 4 answers were chosen among replies"))
                && !said
                    .iter()
                    .any(|line| line.contains("no witnessed call relaying")),
            "a choice is said as a choice, and not as an answer no call relayed: {said:?}"
        );
    }

    #[test]
    fn an_answer_a_pinned_judge_named_is_an_exchange_and_one_asked_elsewhere_or_left_in_a_tool_is_not()
     {
        use aiwatcher_core::witness::{Said, canonical, digest, key_for};
        let mut pins = variant();
        pins.model = None;
        let key = key_for("gateway-secret");
        let said = |text: &str| digest(&key, Said::Replied, text);
        let question = "What is the capital of France?";
        let judge_version = "j".repeat(64);
        let pick = serde_json::json!({"json_pointer": "/best"});
        let call = |replied: &str, at: i64| TracedCall {
            model: Some("gpt-4o".to_owned()),
            prompt_name: Some("capitals".to_owned()),
            prompt_version: Some("p".repeat(64)),
            prompt_verified: Some(true),
            prompt_exact: Some(true),
            published_by: Some("gateway".to_owned()),
            rendered: vec![said(question)],
            asked: vec![digest(&key, Said::Asked, question)],
            replied: vec![said(replied)],
            started_ms: Some(at),
            ..TracedCall::default()
        };
        let judge = |named: &str, version: &str, rule: &serde_json::Value| TracedCall {
            prompt_name: Some("pick-best".to_owned()),
            prompt_version: Some(version.to_owned()),
            rendered: vec![said("Paris"), said("Lyon")],
            asked: Vec::new(),
            placed: vec![
                (said("first"), said("Paris")),
                (said("second"), said("Lyon")),
            ],
            taking: Some(digest(&key, Said::Taking, &canonical(rule))),
            replied: vec![said(&format!("{{\"best\":\"{named}\"}}")), said(named)],
            ..call("", 9)
        };
        let traced = |config: serde_json::Value,
                      elsewhere: Option<Vec<CallElsewhere>>,
                      cases: Vec<(&str, Vec<TracedCall>, Vec<TracedTool>)>| {
            let runs: BTreeMap<String, TracedRun> = cases
                .iter()
                .map(|(case, calls, tools)| {
                    (
                        (*case).to_owned(),
                        TracedRun {
                            served_for_it: calls.clone(),
                            tools_served_for_it: tools.clone(),
                            ..run(vec![on_the_pins()])
                        },
                    )
                })
                .collect();
            let witnesses = Witnesses::named(vec!["gateway".to_owned()])
                .keyed([("gateway".to_owned(), key)])
                .asked(
                    cases
                        .iter()
                        .map(|(case, _, _)| {
                            (
                                (*case).to_owned(),
                                serde_json::json!({"question": question}),
                            )
                        })
                        .collect(),
                )
                .pinned(Some(&config), None);
            let witnesses = match elsewhere {
                Some(calls) => witnesses.asked_elsewhere(calls),
                None => witnesses.elsewhere_unread(),
            };
            let answers: Vec<RecordedAnswer> = cases
                .iter()
                .map(|(case, _, _)| RecordedAnswer {
                    answer: serde_json::json!("Paris"),
                    ..answer(case, Some(case))
                })
                .collect();
            trace_answers(
                &pins, "variant", "answers", &answers, &runs, None, &witnesses,
            )
            .expect("nothing contradicts the pins")
        };
        fn brief(rows: &[TracedAnswer]) -> Vec<(&str, Option<bool>, bool)> {
            rows.iter()
                .map(|row| (row.case_id.as_str(), row.witnessed_exchange, row.chosen))
                .collect()
        }
        let judged = serde_json::json!({"answer_chosen": {"judged": {
            "prompt": {"name": "pick-best", "version": judge_version},
            "pick": pick
        }}});
        let candidates = || vec![call("Paris", 1), call("Lyon", 2)];
        let with = |mut calls: Vec<TracedCall>, more: Vec<TracedCall>| {
            calls.extend(more);
            calls
        };

        let rows = traced(
            judged.clone(),
            Some(Vec::new()),
            vec![
                (
                    "judged",
                    with(candidates(), vec![judge("first", &judge_version, &pick)]),
                    Vec::new(),
                ),
                (
                    "judged-the-other",
                    with(candidates(), vec![judge("second", &judge_version, &pick)]),
                    Vec::new(),
                ),
                (
                    "judged-twice",
                    with(
                        candidates(),
                        vec![
                            judge("first", &judge_version, &pick),
                            judge("first", &judge_version, &pick),
                        ],
                    ),
                    Vec::new(),
                ),
                (
                    "judged-on-another-prompt",
                    with(candidates(), vec![judge("first", &"k".repeat(64), &pick)]),
                    Vec::new(),
                ),
                (
                    "judged-taken-otherwise",
                    with(
                        candidates(),
                        vec![judge(
                            "first",
                            &judge_version,
                            &serde_json::json!({"line": -1}),
                        )],
                    ),
                    Vec::new(),
                ),
            ],
        );
        assert_eq!(
            brief(&rows),
            [
                ("judged", Some(true), false),
                ("judged-the-other", Some(false), true),
                ("judged-twice", Some(false), true),
                ("judged-on-another-prompt", Some(false), true),
                ("judged-taken-otherwise", Some(false), true),
            ],
            "the reply placed where one call on the pinned judging prompt, taken the pinned \
             way, says is an exchange; another placeholder, a judge asked twice, on another \
             prompt or read another way is the application's choice"
        );

        let to_a_tool = |returned: &str| TracedTool {
            name: Some("atlas".to_owned()),
            published_by: Some("gateway".to_owned()),
            arguments: vec![said("Lyon")],
            returned: vec![said(returned)],
        };
        let rendering = |value: &str| TracedCall {
            rendered: vec![said(value)],
            asked: Vec::new(),
            ..call("Paris", 3)
        };
        let rows = traced(
            serde_json::json!({}),
            Some(Vec::new()),
            vec![
                ("left-in-a-tool", candidates(), vec![to_a_tool("noted")]),
                (
                    "carried-on-by-a-tool",
                    with(candidates(), vec![rendering("Europe")]),
                    vec![to_a_tool("Europe")],
                ),
            ],
        );
        assert_eq!(
            brief(&rows),
            [
                ("left-in-a-tool", Some(false), true),
                ("carried-on-by-a-tool", Some(true), false),
            ],
            "a reply handed to a tool whose result went into nothing went into nothing; one \
             whose result a call was rendered with went on"
        );

        let elsewhere = |caller: Option<&str>, asked: &str, version: &str| CallElsewhere {
            caller_run_id: caller.map(ToOwned::to_owned),
            call: TracedCall {
                asked: vec![digest(&key, Said::Asked, asked)],
                prompt_version: Some(version.to_owned()),
                ..call("Lyon", 0)
            },
        };
        let pinned = "p".repeat(64);
        let alone = |case: &'static str| (case, vec![call("Paris", 1)], Vec::new());
        let rows = traced(
            serde_json::json!({}),
            Some(vec![
                elsewhere(Some("peeked"), question, &pinned),
                elsewhere(Some("asked-again"), question, &pinned),
                elsewhere(None, question, &pinned),
                elsewhere(Some("somewhere"), "What is the capital of Peru?", &pinned),
                elsewhere(Some("somewhere"), question, &"q".repeat(64)),
            ]),
            vec![alone("peeked"), alone("asked-again")],
        );
        assert_eq!(
            rows.iter()
                .map(|row| (
                    row.case_id.as_str(),
                    row.witnessed_exchange,
                    row.asked_elsewhere
                ))
                .collect::<Vec<_>>(),
            [("peeked", Some(false), 1), ("asked-again", Some(false), 1)],
            "the case's question on the pinned prompt in a run that is neither its answer's nor \
             that of a case asking the same — here only the call naming no run — is asked \
             elsewhere; another question or another prompt is not"
        );
        let unread = traced(serde_json::json!({}), None, vec![alone("unread")]);
        assert!(unread[0].elsewhere_unread && unread[0].witnessed_exchange == Some(false));
        let trace = GenerationTrace::of(&[rows, unread].concat());
        let said = trace.unwitnessed_answers();
        assert!(
            said.iter().any(|line| line.starts_with(
                "2 of 3 answers' cases were asked on the pinned prompt in other runs"
            )) && said
                .iter()
                .any(|line| line.starts_with("1 of 3 answers could not be held"))
                && !said
                    .iter()
                    .any(|line| line.contains("no witnessed call relaying")),
            "{said:?}"
        );
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
