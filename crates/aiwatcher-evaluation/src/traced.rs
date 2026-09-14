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
    /// The same texts normalised before they were digested
    /// (`aiwatcher_core::witness::normalized`).
    pub asked_normalized: Vec<String>,
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
    /// The sha256 of the code that answered it, where the witness answered it
    /// with a function of its own and said so.
    pub code: Option<String>,
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
    /// The code the variant's generation config pins each tool a witness
    /// answers to (`tool_code`: a name and the sha256 of its source).
    tool_code: BTreeMap<String, String>,
    /// The placeholders of the judging prompt that way names, in the order its
    /// text places them — what an order a judge was shown is held to.
    judge_places: Option<Vec<String>>,
    /// Calls witnesses relayed while the measurement ran, in whatever run
    /// named them — what an application could have asked a case's question
    /// in before, or beside, the run it answers in.
    elsewhere: Vec<CallElsewhere>,
    /// The measurement's start was not in the log's fold, so no such call was
    /// looked for.
    elsewhere_unread: bool,
    /// Calls were looked for from a moment the index of questions asked does
    /// not reach back to — before it began, or past its retention — so those
    /// before this one were not.
    elsewhere_unread_before: Option<String>,
    /// How long before the measurement's start calls asked elsewhere were
    /// looked for, in seconds.
    asked_since_seconds: Option<u64>,
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
    /// `{"judged": {"prompt": {"name": …, "version": …}, "pick": rule,
    /// "order": …}}`: the candidates rendered into one call on this prompt
    /// version, whose reply, taken out by this rule, names the placeholder the
    /// answer was placed in — in the order `order` pins, where it pins one.
    Judged {
        prompt: String,
        version: String,
        pick: serde_json::Value,
        order: Option<JudgedOrder>,
    },
}

/// The order a pinned judge is shown the candidates in, which the application
/// does not choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JudgedOrder {
    /// `"witnessed"`: in the order of their values' digests under the witness's
    /// key, placeholder by placeholder as the judging prompt places them — an
    /// order the application, holding no key, cannot arrange in advance.
    Witnessed,
    /// `"both_ways"`: asked twice, the second time in the reverse order, both
    /// naming the answer.
    BothWays,
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
            // An order this build does not read pins nothing it could keep.
            let order = match judged.get("order") {
                None => None,
                Some(order) => match order.as_str()? {
                    "witnessed" => Some(JudgedOrder::Witnessed),
                    "both_ways" => Some(JudgedOrder::BothWays),
                    _ => return None,
                },
            };
            return Some(Self::Judged {
                prompt: text(prompt.get("name"))?,
                version: text(prompt.get("version"))?,
                pick: judged.get("pick").filter(|rule| rule.is_object())?.clone(),
                order,
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

    /// Calls asked elsewhere were looked for from this many seconds before the
    /// measurement started, which a gate's policy may hold a result to.
    #[must_use]
    pub fn asked_since(mut self, seconds: u64) -> Self {
        self.asked_since_seconds = Some(seconds);
        self
    }

    /// Calls asked elsewhere were looked for, but what was read reaches back
    /// only to `date`, short of where the run pinned they be read from: no
    /// answer is an exchange, and the trace names the date.
    #[must_use]
    pub fn elsewhere_unread_before(mut self, date: impl Into<String>) -> Self {
        self.elsewhere_unread_before = Some(date.into());
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
        self.tool_code = field("tool_code")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(name, code)| {
                code.as_str()
                    .filter(|code| code.len() == 64 && code.bytes().all(|b| b.is_ascii_hexdigit()))
                    .map(|code| (name.clone(), code.to_ascii_lowercase()))
            })
            .collect();
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

    /// The judging prompt version whose placeholders an order the variant pins
    /// is held to — `(name, version)`, where it pins one.
    #[must_use]
    pub fn judge_ordered_on(&self) -> Option<(&str, &str)> {
        match &self.chosen {
            Some(Choosing::Judged {
                prompt,
                version,
                order: Some(_),
                ..
            }) => Some((prompt.as_str(), version.as_str())),
            _ => None,
        }
    }

    /// The text of that judging prompt version, whose placeholders' order the
    /// order a judge was shown is held to.
    #[must_use]
    pub fn judged_with(mut self, template: &str) -> Self {
        self.judge_places = Some(aiwatcher_core::prompts::variables_in_order(template));
        self
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
    /// The names of the tools the run's own telemetry says it called: its own
    /// word, which accounts for no value.
    pub tools_called: Vec<String>,
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
    /// It is such an exchange, and witnessed calls on another prompt or model
    /// asked its case's input this many times in runs other than its own —
    /// counted, and denying nothing unless a gate's policy says so, since
    /// traffic on other prompts asks what cases ask all the time.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub asked_elsewhere_unpinned: usize,
    /// It would be such an exchange, but when the measurement started was not
    /// in the log's fold, so calls asked elsewhere during it were not looked
    /// for — or, with a date, those before it were not.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub elsewhere_unread: bool,
    /// The moment before which calls asked elsewhere were not read, where
    /// they were read from one after the moment the run pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elsewhere_unread_before: Option<String>,
    /// How long before the measurement's start calls asked elsewhere were
    /// looked for, in seconds; absent where none were, the variant pinning no
    /// prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_since_seconds: Option<u64>,
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
    /// A judge the variant pins picked it among its run's replies, shown the
    /// candidates in the order the application placed them in: the variant
    /// pins no `order`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub judged_unordered: bool,
    /// Why the way of choosing the variant pins did not pick it, where it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice_refused: Option<String>,
    /// A call replied the answer to a request holding a value nothing accounts
    /// for, and the run called these tools with no witness: where that value
    /// may have come from, never taken for where it did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unaccounted_tools: Vec<String>,
    /// The run it names never reached the log, and its client's count of the
    /// runs it opened for this result passes over one that never arrived
    /// ([`lost_or_unknown`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lost_in_transport: bool,
    /// The run it names never reached the log, and no client's count of the
    /// runs it opened for this result passes over one it could be.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unknown_run: bool,
}

/// One client's count of the runs it opened answering a result, at one attempt
/// of generating the answers, as the log's reader holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunsCounted {
    pub client: String,
    pub attempt: Option<u32>,
    /// Runs it opened: its own count, or one past the highest number read.
    pub opened: u64,
    /// Of those, the runs whose start arrived.
    pub arrived: u64,
}

/// Say of each answer naming a run the log never received whether it may be a
/// run lost in transport — a run its client opened for this result and whose
/// start never arrived, one for each such number — or a run nobody opened.
///
/// Only the counts of the latest attempt at generating the answers are read,
/// since that is the attempt whose answers these are, and an earlier attempt's
/// lost runs are nobody's answer. A named run whose start arrived (`started`)
/// was not lost and is neither. With no count at all — a client that numbers
/// none — nothing is said.
pub fn lost_or_unknown(
    rows: &mut [TracedAnswer],
    counted: &[RunsCounted],
    started: impl Fn(&str) -> bool,
) {
    let Some(latest) = counted.iter().map(|count| count.attempt).max() else {
        return;
    };
    let mut lost: u64 = counted
        .iter()
        .filter(|count| count.attempt == latest)
        .map(|count| count.opened.saturating_sub(count.arrived))
        .sum();
    for row in rows.iter_mut().filter(|row| !row.seen) {
        let Some(run_id) = row.run_id.as_deref() else {
            continue;
        };
        if started(run_id) {
            continue;
        }
        if lost > 0 {
            lost -= 1;
            row.lost_in_transport = true;
        } else {
            row.unknown_run = true;
        }
    }
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
    /// Tools called with no witness in the runs of answers holding a value
    /// nothing accounted for, each once: where such a value may have come from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unaccounted_tools: Vec<String>,
    /// Of the runs not on the log, those its client's count says were opened
    /// for this result and never arrived: lost in transport.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lost_in_transport: usize,
    /// Of the runs not on the log, those no client's count passes over: runs
    /// no client opened for this result, such as a run ID made up.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unknown_runs: usize,
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
    /// Of those, why the way of choosing the variant pins picked none of them,
    /// each reason once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices_refused: Vec<String>,
    /// Exchanges a judge the variant pins picked among their runs' replies,
    /// shown the candidates in the order the application placed them in —
    /// the variant pins no `order` a judge is shown them in.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub judged_unordered: usize,
    /// Answers that would be an exchange but whose cases' inputs witnessed
    /// calls on the pinned prompt asked in other runs while the measurement ran
    /// ([`TracedAnswer::asked_elsewhere`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub asked_elsewhere: usize,
    /// Exchanges whose cases' inputs witnessed calls on another prompt or
    /// model asked in other runs ([`TracedAnswer::asked_elsewhere_unpinned`]):
    /// exchanges all the same, unless a gate's policy denies them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub asked_elsewhere_unpinned: usize,
    /// Answers that would be an exchange but for which calls asked elsewhere
    /// were not looked for, the measurement's start not being in the fold —
    /// or not before `elsewhere_unread_before`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub elsewhere_unread: usize,
    /// Where calls asked elsewhere were read only from a moment after the one
    /// the run pinned: that moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elsewhere_unread_before: Option<String>,
    /// How long before the measurement's start the step read calls asked
    /// elsewhere from, in seconds — `0` from the start itself; absent where it
    /// read none, the variant pinning no prompt. What a gate's
    /// `asked_since_seconds` holds a result to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_since_seconds: Option<u64>,
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
            lost_in_transport: rows.iter().filter(|row| row.lost_in_transport).count(),
            unaccounted_tools: {
                let mut named: Vec<String> = rows
                    .iter()
                    .flat_map(|row| row.unaccounted_tools.iter().cloned())
                    .collect();
                named.sort();
                named.dedup();
                named
            },
            unknown_runs: rows.iter().filter(|row| row.unknown_run).count(),
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
            choices_refused: {
                let mut refused: Vec<String> = rows
                    .iter()
                    .filter_map(|row| row.choice_refused.clone())
                    .collect();
                refused.sort();
                refused.dedup();
                refused
            },
            judged_unordered: rows.iter().filter(|row| row.judged_unordered).count(),
            asked_elsewhere: rows.iter().filter(|row| row.asked_elsewhere > 0).count(),
            elsewhere_unread: rows.iter().filter(|row| row.elsewhere_unread).count(),
            asked_elsewhere_unpinned: rows
                .iter()
                .filter(|row| row.asked_elsewhere_unpinned > 0)
                .count(),
            elsewhere_unread_before: rows
                .iter()
                .filter_map(|row| row.elsewhere_unread_before.clone())
                .max(),
            asked_since_seconds: rows.iter().filter_map(|row| row.asked_since_seconds).min(),
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
        if self.lost_in_transport > 0 {
            said.push(format!(
                "{} of those runs were lost in transport: their client counted as many runs \
                 opened for this result whose start never arrived",
                self.lost_in_transport
            ));
        }
        if self.unknown_runs > 0 {
            said.push(format!(
                "{} of those runs are none a client opened for this result: no client's count \
                 passes over a run they could be, so the answers name runs made up or opened \
                 for something else",
                self.unknown_runs
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

    /// Why calls asked elsewhere were not read from as long before the
    /// measurement as `wanted` seconds, in words; `None` where they were.
    #[must_use]
    pub fn asked_since_short_of(&self, wanted: u64) -> Option<String> {
        match self.asked_since_seconds {
            Some(read) if read >= wanted => None,
            Some(read) => Some(format!(
                "calls asked elsewhere were read from {read} s before the measurement started, \
                 not the {wanted} s wanted: a case asked in between was not looked for"
            )),
            None => Some(format!(
                "the result records no lookback for calls asked elsewhere — the variant pinning \
                 no prompt, none was looked for, or it was measured before a result recorded \
                 one — so none was from {wanted} s before the measurement started"
            )),
        }
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
        said.extend(self.choices_refused.iter().cloned());
        if !self.unaccounted_tools.is_empty() {
            said.push(format!(
                "answers holding a value nothing accounted for came from runs that called {} \
                 with no witness, which may be where it came from — a tool the gateway relays or \
                 answers, or one on a host digesting under the witness's key, is accounted for",
                self.unaccounted_tools.join(", ")
            ));
        }
        if self.asked_elsewhere > 0 {
            said.push(format!(
                "{} of {} answers' cases were asked on the pinned prompt in other runs while this \
                 measurement ran or before it, from when the run reads — replies the application \
                 could have seen before it answered, and chosen the run it answered in by",
                self.asked_elsewhere, self.answers
            ));
        }
        if self.elsewhere_unread > 0 {
            match &self.elsewhere_unread_before {
                Some(date) => said.push(format!(
                    "{} of {} answers could not be held to calls asked in other runs before {date}, \
                     which the index of questions asked does not reach back to",
                    self.elsewhere_unread, self.answers
                )),
                None => said.push(format!(
                    "{} of {} answers could not be held to calls asked in other runs, because \
                     when this measurement started is not in the log's fold",
                    self.elsewhere_unread, self.answers
                )),
            }
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
    // A tool the variant pins the code of accounts for what it returned only
    // where the witness says that code answered it.
    let tool_keys: Vec<Option<&[u8; 32]>> = tools
        .iter()
        .map(|tool| {
            witness_key(tool.published_by.as_deref()).filter(|_| {
                tool.name
                    .as_ref()
                    .and_then(|name| witnesses.tool_code.get(name))
                    .is_none_or(|pinned| tool.code.as_ref() == Some(pinned))
            })
        })
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
) -> Choice {
    let calls = &run.served_for_it;
    let key_of = |published_by: Option<&str>| witness_key(run, witnesses, published_by);
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
        return Choice::Picked;
    }
    if in_parts || !finals.iter().all(|at| grounded[*at]) {
        return Choice::Chosen(None);
    }
    let picked = match &witnesses.chosen {
        None => false,
        Some(Choosing::Judged {
            prompt,
            version,
            pick,
            order,
        }) => {
            return judged_as_pinned(
                &JudgedPin {
                    prompt,
                    version,
                    pick,
                    order: *order,
                },
                calls,
                &replies,
                &finals,
                answered_by,
                witnesses,
                run,
            );
        }
        Some(Choosing::First) => {
            let mut timed: Vec<(i64, usize)> = Vec::with_capacity(finals.len());
            for at in &finals {
                let Some(started) = calls[*at].started_ms else {
                    return Choice::Chosen(None);
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
                return Choice::Chosen(None);
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
                return Choice::Chosen(None);
            };
            sizes
                .iter()
                .all(|(root, size)| *root == answer || *size < sizes[&answer])
        }
    };
    if picked {
        Choice::Picked
    } else {
        Choice::Chosen(None)
    }
}

/// The key a witness's digests in `run` are made under: a credential the
/// deployment admits, other than the one the run was published under.
fn witness_key<'w>(
    run: &TracedRun,
    witnesses: &'w Witnesses,
    published_by: Option<&str>,
) -> Option<&'w [u8; 32]> {
    let witness = published_by?;
    let admitted = run
        .published_by
        .as_deref()
        .is_some_and(|publisher| publisher != witness)
        && witnesses.admits(witness);
    witnesses.keys.get(witness).filter(|_| admitted)
}

/// A way of choosing by a judge, as the variant pins it.
struct JudgedPin<'a> {
    prompt: &'a str,
    version: &'a str,
    pick: &'a serde_json::Value,
    order: Option<JudgedOrder>,
}

/// What choosing an answer among replies that went into nothing else came to.
#[derive(Debug, PartialEq, Eq)]
enum Choice {
    /// Nothing was chosen, or the way the variant pins picked the answer.
    Picked,
    /// A pinned judge picked it, shown the candidates in the order the
    /// application placed them in, since the variant pins no `order`.
    JudgedUnordered,
    /// No way the variant pins picked it — and why, where the way can say.
    Chosen(Option<String>),
}

/// Whether the judge a variant pins picked the answer: the calls besides the
/// answer's that went into nothing else are that many calls on the judging
/// prompt, taken out the pinned way, each naming one placeholder whose value
/// is the answer's reply — one call, or two asked both ways — shown the
/// candidates in the order `order` pins where it pins one.
fn judged_as_pinned(
    pin: &JudgedPin<'_>,
    calls: &[TracedCall],
    replies: &[Vec<&String>],
    finals: &[usize],
    answered_by: &std::collections::BTreeSet<usize>,
    witnesses: &Witnesses,
    run: &TracedRun,
) -> Choice {
    use aiwatcher_core::witness::{Said, canonical, digest};
    let key_of = |published_by: Option<&str>| witness_key(run, witnesses, published_by);
    let judges: Vec<usize> = finals
        .iter()
        .copied()
        .filter(|at| !answered_by.contains(at))
        .collect();
    let asked = match pin.order {
        Some(JudgedOrder::BothWays) => 2,
        Some(JudgedOrder::Witnessed) | None => 1,
    };
    if judges.len() != asked {
        let went = match judges.len() {
            1 => "1 call went".to_owned(),
            count => format!("{count} calls went"),
        };
        return Choice::Chosen(pin.order.map(|order| match order {
            JudgedOrder::BothWays => format!(
                "the variant pins asking the judge {} both ways, and {went} into nothing else",
                pin.prompt
            ),
            JudgedOrder::Witnessed => format!(
                "the variant pins asking the judge {} once, in the witnessed order, and {went} \
                 into nothing else",
                pin.prompt
            ),
        }));
    }
    // The value placed where a judging call's reply says: a call on the pinned
    // prompt, taking its answer out the pinned way, naming one placeholder.
    let picked_by = |judge: usize| -> Option<&str> {
        let call = &calls[judge];
        let key = key_of(call.published_by.as_deref())?;
        if call.prompt_name.as_deref() != Some(pin.prompt)
            || call.prompt_version.as_deref() != Some(pin.version)
            || call.taking.as_deref()
                != Some(digest(key, Said::Taking, &canonical(pin.pick)).as_str())
        {
            return None;
        }
        let said: std::collections::BTreeSet<&String> =
            taken_by(call, key, Some(pin.pick)).collect();
        let named: std::collections::BTreeSet<&str> = call
            .placed
            .iter()
            .filter(|(name, _)| said.contains(name))
            .map(|(_, value)| value.as_str())
            .collect();
        let [picked] = named.into_iter().collect::<Vec<_>>()[..] else {
            return None;
        };
        Some(picked)
    };
    let mut picks = Vec::with_capacity(judges.len());
    for judge in &judges {
        let Some(picked) = picked_by(*judge) else {
            return Choice::Chosen(None);
        };
        picks.push(picked);
    }
    if picks.iter().any(|picked| *picked != picks[0]) {
        return Choice::Chosen(Some(format!(
            "the judge {} named different replies when asked both ways",
            pin.prompt
        )));
    }
    if !answered_by
        .iter()
        .all(|at| replies[*at].iter().any(|reply| reply.as_str() == picks[0]))
    {
        return Choice::Chosen(None);
    }
    let Some(order) = pin.order else {
        return Choice::JudgedUnordered;
    };
    // The candidates a judge was shown, in the order the judging prompt's
    // placeholders place them: the values placed that are replies of the
    // run's other calls.
    let candidates: std::collections::BTreeSet<&str> = (0..calls.len())
        .filter(|at| !judges.contains(at))
        .flat_map(|at| replies[at].iter().map(|reply| reply.as_str()))
        .collect();
    let shown = |judge: usize| -> Result<Vec<&str>, String> {
        let call = &calls[judge];
        let Some(key) = key_of(call.published_by.as_deref()) else {
            return Err(String::new());
        };
        let Some(places) = &witnesses.judge_places else {
            return Err(format!(
                "the order the judge was shown the candidates in could not be held to {} at {}, \
                 whose text this step could not read",
                pin.prompt, pin.version
            ));
        };
        let position: BTreeMap<String, usize> = places
            .iter()
            .enumerate()
            .map(|(at, name)| (digest(key, Said::Replied, name), at))
            .collect();
        let mut placed = Vec::new();
        for (name, value) in call
            .placed
            .iter()
            .filter(|(_, value)| candidates.contains(value.as_str()))
        {
            let Some(at) = position.get(name) else {
                return Err(format!(
                    "the judge was shown a candidate where {} at {} places nothing",
                    pin.prompt, pin.version
                ));
            };
            placed.push((*at, value.as_str()));
        }
        placed.sort_unstable();
        Ok(placed.into_iter().map(|(_, value)| value).collect())
    };
    let mut orders = Vec::with_capacity(judges.len());
    for judge in &judges {
        match shown(*judge) {
            Ok(order) => orders.push(order),
            Err(why) => return Choice::Chosen((!why.is_empty()).then_some(why)),
        }
    }
    match order {
        JudgedOrder::Witnessed => {
            if orders[0].windows(2).all(|pair| pair[0] <= pair[1]) {
                Choice::Picked
            } else {
                Choice::Chosen(Some(format!(
                    "the judge {} was shown the candidates in an order the application placed \
                     them in, not the witnessed order the variant pins",
                    pin.prompt
                )))
            }
        }
        JudgedOrder::BothWays => {
            let mut reversed = orders[1].clone();
            reversed.reverse();
            if orders[0] == reversed {
                Choice::Picked
            } else {
                Choice::Chosen(Some(format!(
                    "the judge {} was not shown the candidates the second time in the reverse of \
                     the first order",
                    pin.prompt
                )))
            }
        }
    }
}

/// How many calls witnesses relayed while the measurement ran — or from the
/// moment the run pinned they be read from — in runs other than `run_id` and
/// than `alike` (the runs of cases asking the same) asked the case's whole
/// input: its text, or every text in it, as it was asked or normalised. The
/// first count is of calls on the pinned prompt, or the judging prompt a pinned
/// way of choosing names, of the pinned model where one is pinned; the second
/// of every other such call — another prompt, another model, or none.
fn asked_elsewhere(
    variant: &VariantManifest,
    witnesses: &Witnesses,
    input: Option<&serde_json::Value>,
    run_id: &str,
    run: &TracedRun,
    alike: &std::collections::BTreeSet<&str>,
) -> (usize, usize) {
    use aiwatcher_core::witness::{Said, asked_as, digest, normalized};
    let (Some(pin), Some(input)) = (&variant.prompt, input) else {
        return (0, 0);
    };
    let judging = witnesses.chosen.as_ref().and_then(Choosing::judge_prompt);
    let texts = asked_as(input);
    let (mut pinned, mut unpinned) = (0, 0);
    for elsewhere in witnesses.elsewhere.iter().filter(|elsewhere| {
        elsewhere
            .caller_run_id
            .as_deref()
            .is_none_or(|caller| caller != run_id && !alike.contains(caller))
    }) {
        let call = &elsewhere.call;
        let Some(key) = witness_key(run, witnesses, call.published_by.as_deref()) else {
            continue;
        };
        let holds = |text: &String| {
            call.asked.contains(&digest(key, Said::Asked, text))
                || call
                    .asked_normalized
                    .contains(&digest(key, Said::Asked, &normalized(text)))
        };
        let asked = match texts.split_first() {
            None => false,
            Some((whole, parts)) => holds(whole) || (!parts.is_empty() && parts.iter().all(holds)),
        };
        if !asked {
            continue;
        }
        let on = |name: &str, version: &str| {
            call.prompt_name.as_deref() == Some(name)
                && call.prompt_version.as_deref() == Some(version)
        };
        if call.prompt_verified == Some(true)
            && (on(&pin.name, &pin.version)
                || judging.is_some_and(|(name, version)| on(name, version)))
            && variant
                .model
                .as_ref()
                .is_none_or(|model| call.model.as_deref() == Some(model.name.as_str()))
        {
            pinned += 1;
        } else {
            unpinned += 1;
        }
    }
    (pinned, unpinned)
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
            asked_elsewhere_unpinned: 0,
            elsewhere_unread_before: None,
            asked_since_seconds: variant
                .prompt
                .as_ref()
                .map(|_| witnesses.asked_since_seconds.unwrap_or(0)),
            witnessed_by: Vec::new(),
            self_witnessed: false,
            served_models: Vec::new(),
            models: Vec::new(),
            workflow_undeclared: variant.workflow.is_some() && workflow.is_none(),
            workflow_steps_unread: false,
            workflow_idle_bounds: idle_bounds.clone(),
            judged_unordered: false,
            choice_refused: None,
            unaccounted_tools: Vec::new(),
            lost_in_transport: false,
            unknown_run: false,
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
            // A tool the variant pins the code of, answered by a witness with
            // other code: what it returned is not the variant's.
            for tool in &run.tools_served_for_it {
                let (Some(name), Some(code), Some(witness)) =
                    (&tool.name, &tool.code, tool.published_by.as_deref())
                else {
                    continue;
                };
                if witness_key(run, witnesses, Some(witness)).is_none() {
                    continue;
                }
                if let Some(pinned) = witnesses.tool_code.get(name)
                    && pinned != code
                {
                    said(format!(
                        "{witness} answered the tool {name} with code {code}, and the variant \
                         pins {pinned}"
                    ));
                }
            }
            // The calls whose replies the answer is made of, and whether it is
            // made of several parts rather than one reply.
            let mut answered_by = std::collections::BTreeSet::new();
            let mut in_parts = false;
            // A call replied the answer to a request holding a value nothing
            // accounts for.
            let mut unaccounted = false;
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
                    unaccounted |= replied && !grounded[at];
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
            // Where a value nothing accounted for may have come from: a tool the
            // run called with no witness — its own telemetry's word, a host
            // with no key, or code other than the pinned. Named, never taken
            // for the source.
            if unaccounted && row.witnessed_exchange != Some(true) {
                let mut named: Vec<String> = run.tools_called.clone();
                named.extend(
                    run.tools_served_for_it
                        .iter()
                        .filter(|tool| {
                            witness_key(run, witnesses, tool.published_by.as_deref()).is_none()
                                || tool
                                    .name
                                    .as_ref()
                                    .and_then(|name| witnesses.tool_code.get(name))
                                    .is_some_and(|pinned| tool.code.as_ref() != Some(pinned))
                        })
                        .filter_map(|tool| tool.name.clone()),
                );
                named.sort();
                named.dedup();
                row.unaccounted_tools = named;
            }
            // Replies that went into nothing else: the application may have
            // chosen the answer among them.
            if row.witnessed_exchange == Some(true) {
                match chosen_as_pinned(run, witnesses, &grounded, &answered_by, in_parts) {
                    Choice::Picked => {}
                    Choice::JudgedUnordered => row.judged_unordered = true,
                    Choice::Chosen(why) => {
                        row.witnessed_exchange = Some(false);
                        row.chosen = true;
                        row.choice_refused = why;
                    }
                }
            }
            // The same question asked in another run while the measurement ran:
            // the application could have chosen this run by what came back.
            if row.witnessed_exchange == Some(true) {
                if witnesses.elsewhere_unread || witnesses.elsewhere_unread_before.is_some() {
                    row.witnessed_exchange = Some(false);
                    row.elsewhere_unread = true;
                    row.elsewhere_unread_before
                        .clone_from(&witnesses.elsewhere_unread_before);
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
                    let (asked, unpinned) =
                        asked_elsewhere(variant, witnesses, input, run_id, run, &alike);
                    if asked > 0 {
                        row.witnessed_exchange = Some(false);
                        row.asked_elsewhere = asked;
                    } else {
                        row.asked_elsewhere_unpinned = unpinned;
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
            asked_normalized: Vec::new(),
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
                lost_in_transport: 0,
                unknown_runs: 0,
                unaccounted_tools: Vec::new(),
                witnessed_model: Some(0),
                witnessed_prompt: Some(0),
                witnessed_answer: Some(0),
                witnessed_input: Some(0),
                witnessed_exchange: Some(0),
                chosen: 0,
                choices_refused: Vec::new(),
                judged_unordered: 0,
                asked_elsewhere: 0,
                elsewhere_unread: 0,
                asked_elsewhere_unpinned: 0,
                elsewhere_unread_before: None,
                asked_since_seconds: Some(0),
                self_witnessed: 0,
                witnesses: Vec::new(),
                served: Vec::new(),
            }
        );
        assert!(!trace.complete());
        assert_eq!(
            trace.asked_since_short_of(0),
            None,
            "a variant pinning a prompt looked from the start"
        );
        let looked = GenerationTrace::of(
            &trace_answers(
                &variant(),
                "variant",
                "answers",
                &answers,
                &runs,
                None,
                &Witnesses::default().asked_since(600),
            )
            .expect("nothing contradicts the pins"),
        );
        assert_eq!(looked.asked_since_short_of(600), None);
        assert!(
            looked
                .asked_since_short_of(3_600)
                .is_some_and(|reason| reason.contains("from 600 s")),
            "ten minutes back is short of an hour"
        );
        let unlooked = GenerationTrace {
            asked_since_seconds: None,
            ..trace.clone()
        };
        assert!(
            unlooked
                .asked_since_short_of(0)
                .is_some_and(|reason| reason.contains("pinning no prompt")),
            "a result that looked for nothing asked elsewhere holds no lookback at all"
        );
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
    fn a_run_not_on_the_log_is_lost_in_transport_where_its_client_counted_one_that_never_arrived() {
        let runs = BTreeMap::from([("seen".to_owned(), run(Vec::new()))]);
        let mut rows = trace_answers(
            &VariantManifest {
                model: None,
                prompt: None,
                ..variant()
            },
            "variant",
            "answers",
            &[
                answer("c1", Some("seen")),
                answer("c2", Some("lost")),
                answer("c3", Some("made-up")),
                answer("c4", Some("still-running")),
                answer("c5", None),
            ],
            &runs,
            None,
            &Witnesses::default(),
        )
        .expect("nothing contradicts the pins");
        let counted = |attempt: u32, opened: u64, arrived: u64| RunsCounted {
            client: "worker".to_owned(),
            attempt: Some(attempt),
            opened,
            arrived,
        };
        lost_or_unknown(&mut rows, &[counted(1, 9, 2), counted(2, 4, 3)], |run_id| {
            run_id == "still-running"
        });
        let trace = GenerationTrace::of(&rows);

        assert_eq!(
            (trace.lost_in_transport, trace.unknown_runs),
            (1, 1),
            "the second attempt passed over one run, and the first attempt's lost runs are no answer's"
        );
        assert!(rows[1].lost_in_transport && rows[2].unknown_run);
        assert!(!rows[3].lost_in_transport && !rows[3].unknown_run);
        let said = trace.shortfall();
        assert!(
            said.iter()
                .any(|sentence| sentence.starts_with("1 of those runs were lost in transport")),
            "{said:?}"
        );
        assert!(
            said.iter()
                .any(|sentence| sentence.starts_with("1 of those runs are none a client opened")),
            "{said:?}"
        );

        let mut uncounted = rows.clone();
        for row in &mut uncounted {
            (row.lost_in_transport, row.unknown_run) = (false, false);
        }
        lost_or_unknown(&mut uncounted, &[], |_| false);
        assert!(
            uncounted
                .iter()
                .all(|row| !row.lost_in_transport && !row.unknown_run),
            "a client that numbers nothing says nothing"
        );
    }

    #[test]
    fn a_tool_s_value_is_accounted_only_from_the_pinned_code_and_an_unwitnessed_tool_is_named() {
        use aiwatcher_core::witness::{Said, digest, key_for};
        let mut pins = variant();
        pins.model = None;
        let key = key_for("gateway-secret");
        let said = |text: &str| digest(&key, Said::Replied, text);
        let question = "What is the capital of France?";
        let found = r#"{"capital":"Paris"}"#;
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
        let searched = |code: Option<&str>, witnessed: bool| TracedRun {
            served_for_it: vec![call(&[found], &["Paris"]), call(&[question], &["France"])],
            tools_served_for_it: if witnessed {
                vec![TracedTool {
                    name: Some("search".to_owned()),
                    published_by: Some("gateway".to_owned()),
                    arguments: vec![said("France")],
                    returned: vec![said(found)],
                    code: code.map(ToOwned::to_owned),
                }]
            } else {
                Vec::new()
            },
            tools_called: if witnessed {
                Vec::new()
            } else {
                vec!["search".to_owned()]
            },
            ..run(vec![on_the_pins()])
        };
        let pinned = "a".repeat(64);
        let traced = |cases: Vec<(&str, TracedRun)>| {
            let witnesses = Witnesses::named(vec!["gateway".to_owned()])
                .keyed([("gateway".to_owned(), key)])
                .asked(
                    cases
                        .iter()
                        .map(|(case, _)| {
                            (
                                (*case).to_owned(),
                                serde_json::json!({"question": question}),
                            )
                        })
                        .collect(),
                )
                .pinned(
                    Some(&serde_json::json!({"tool_code": {"search": pinned}})),
                    None,
                );
            let answers: Vec<RecordedAnswer> = cases
                .iter()
                .map(|(case, _)| answer(case, Some(case)))
                .collect();
            let runs: BTreeMap<String, TracedRun> = cases
                .into_iter()
                .map(|(case, run)| (case.to_owned(), run))
                .collect();
            trace_answers(
                &pins, "variant", "answers", &answers, &runs, None, &witnesses,
            )
        };

        let rows = traced(vec![
            ("pinned-code", searched(Some(&pinned), true)),
            ("no-code-said", searched(None, true)),
            ("in-the-application", searched(None, false)),
        ])
        .expect("nothing contradicts the pins");
        let trace = GenerationTrace::of(&rows);
        assert_eq!(
            rows.iter()
                .map(|row| (
                    row.case_id.as_str(),
                    row.witnessed_exchange,
                    row.unaccounted_tools.clone()
                ))
                .collect::<Vec<_>>(),
            [
                ("pinned-code", Some(true), Vec::new()),
                ("no-code-said", Some(false), vec!["search".to_owned()]),
                ("in-the-application", Some(false), vec!["search".to_owned()]),
            ],
            "a tool answered by the pinned code accounts for what it returned; one whose code \
             nobody said, or that the application ran itself, is named as where the value may \
             have come from"
        );
        assert!(
            trace
                .unwitnessed_answers()
                .iter()
                .any(|line| line.contains("runs that called search with no witness")),
            "{:?}",
            trace.unwitnessed_answers()
        );

        let refused = traced(vec![("other-code", searched(Some(&"b".repeat(64)), true))])
            .expect_err("code other than the pinned answered the tool");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(
            refused[0].contains(&format!(
                "gateway answered the tool search with code {}, and the variant pins {pinned}",
                "b".repeat(64)
            )),
            "{refused:?}"
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
            code: None,
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
    fn a_judge_pinned_to_an_order_picks_an_exchange_only_in_the_witnessed_order_or_both_ways() {
        use aiwatcher_core::witness::{Said, canonical, digest, key_for};
        let mut pins = variant();
        pins.model = None;
        let key = key_for("gateway-secret");
        let said = |text: &str| digest(&key, Said::Replied, text);
        let question = "What is the capital of France?";
        let judge_version = "j".repeat(64);
        let template = "Which answers {{ question }} better: {{ first }} or {{ second }}?";
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
        // A judge shown `first` and `second` in these places, naming one.
        let judge = |first: &str, second: &str, named: &str| TracedCall {
            prompt_name: Some("pick-best".to_owned()),
            prompt_version: Some(judge_version.clone()),
            rendered: vec![said(question), said(first), said(second)],
            asked: Vec::new(),
            placed: vec![
                (said("question"), said(question)),
                (said("first"), said(first)),
                (said("second"), said(second)),
            ],
            taking: Some(digest(&key, Said::Taking, &canonical(&pick))),
            replied: vec![said(&format!("{{\"best\":\"{named}\"}}")), said(named)],
            ..call("", 9)
        };
        let traced =
            |order: Option<&str>, template: Option<&str>, cases: Vec<(&str, Vec<TracedCall>)>| {
                let mut judged = serde_json::json!({
                    "prompt": {"name": "pick-best", "version": judge_version},
                    "pick": pick
                });
                if let Some(order) = order {
                    judged["order"] = serde_json::json!(order);
                }
                let runs: BTreeMap<String, TracedRun> = cases
                    .iter()
                    .map(|(case, calls)| {
                        (
                            (*case).to_owned(),
                            TracedRun {
                                served_for_it: calls.clone(),
                                ..run(vec![on_the_pins()])
                            },
                        )
                    })
                    .collect();
                let mut witnesses = Witnesses::named(vec!["gateway".to_owned()])
                    .keyed([("gateway".to_owned(), key)])
                    .asked(
                        cases
                            .iter()
                            .map(|(case, _)| {
                                (
                                    (*case).to_owned(),
                                    serde_json::json!({"question": question}),
                                )
                            })
                            .collect(),
                    )
                    .pinned(
                        Some(&serde_json::json!({"answer_chosen": {"judged": judged}})),
                        None,
                    )
                    .asked_elsewhere(Vec::new());
                assert_eq!(
                    witnesses.judge_ordered_on(),
                    order.map(|_| ("pick-best", judge_version.as_str()))
                );
                if let Some(template) = template {
                    witnesses = witnesses.judged_with(template);
                }
                let answers: Vec<RecordedAnswer> = cases
                    .iter()
                    .map(|(case, _)| RecordedAnswer {
                        answer: serde_json::json!("Paris"),
                        ..answer(case, Some(case))
                    })
                    .collect();
                trace_answers(
                    &pins, "variant", "answers", &answers, &runs, None, &witnesses,
                )
                .expect("nothing contradicts the pins")
            };
        // The witnessed order: the candidates' digests, ascending.
        let (low, high) = if said("Paris") < said("Lyon") {
            ("Paris", "Lyon")
        } else {
            ("Lyon", "Paris")
        };
        let places = |value: &str| if value == low { "first" } else { "second" };
        let with = |more: Vec<TracedCall>| {
            let mut calls = vec![call("Paris", 1), call("Lyon", 2)];
            calls.extend(more);
            calls
        };

        let rows = traced(
            Some("witnessed"),
            Some(template),
            vec![
                ("in-order", with(vec![judge(low, high, places("Paris"))])),
                (
                    "out-of-order",
                    with(vec![judge(
                        high,
                        low,
                        if high == "Paris" { "first" } else { "second" },
                    )]),
                ),
                (
                    "asked-twice",
                    with(vec![
                        judge(low, high, places("Paris")),
                        judge(low, high, places("Paris")),
                    ]),
                ),
            ],
        );
        let trace = GenerationTrace::of(&rows);
        assert_eq!(
            rows.iter()
                .map(|row| (row.case_id.as_str(), row.witnessed_exchange, row.chosen))
                .collect::<Vec<_>>(),
            [
                ("in-order", Some(true), false),
                ("out-of-order", Some(false), true),
                ("asked-twice", Some(false), true),
            ]
        );
        assert!(
            trace.unwitnessed_answers().iter().any(|line| line.contains(
                "was shown the candidates in an order the application placed them in, not the \
                 witnessed order"
            )),
            "{:?}",
            trace.unwitnessed_answers()
        );
        assert!(
            rows[2]
                .choice_refused
                .as_deref()
                .is_some_and(|why| why.contains("once, in the witnessed order, and 2 calls")),
            "{:?}",
            rows[2].choice_refused
        );

        let unread = traced(
            Some("witnessed"),
            None,
            vec![("unread", with(vec![judge(low, high, places("Paris"))]))],
        );
        assert_eq!(unread[0].witnessed_exchange, Some(false));
        assert!(
            unread[0]
                .choice_refused
                .as_deref()
                .is_some_and(|why| why.contains("whose text this step could not read"))
        );

        let other = |value: &str| if value == "Paris" { "Lyon" } else { "Paris" };
        let rows = traced(
            Some("both_ways"),
            Some(template),
            vec![
                (
                    "both-ways",
                    with(vec![
                        judge("Paris", "Lyon", "first"),
                        judge("Lyon", "Paris", "second"),
                    ]),
                ),
                (
                    "same-order-twice",
                    with(vec![
                        judge("Paris", "Lyon", "first"),
                        judge("Paris", "Lyon", "first"),
                    ]),
                ),
                (
                    "named-apart",
                    with(vec![
                        judge("Paris", "Lyon", "first"),
                        judge("Lyon", "Paris", "first"),
                    ]),
                ),
                (
                    "asked-once",
                    with(vec![judge("Paris", other("Paris"), "first")]),
                ),
                (
                    "asked-thrice",
                    with(vec![
                        judge("Paris", "Lyon", "first"),
                        judge("Lyon", "Paris", "second"),
                        judge("Paris", "Lyon", "first"),
                    ]),
                ),
            ],
        );
        assert_eq!(
            rows.iter()
                .map(|row| (row.case_id.as_str(), row.witnessed_exchange, row.chosen))
                .collect::<Vec<_>>(),
            [
                ("both-ways", Some(true), false),
                ("same-order-twice", Some(false), true),
                ("named-apart", Some(false), true),
                ("asked-once", Some(false), true),
                ("asked-thrice", Some(false), true),
            ],
            "asked both ways and naming one reply each time is an exchange; the same order \
             twice, two replies named, or a judge asked once or three times is the \
             application's choice"
        );
        let refused: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.choice_refused.as_deref())
            .collect();
        assert!(refused[0].contains("not shown the candidates the second time in the reverse"));
        assert!(refused[1].contains("named different replies when asked both ways"));
        assert!(refused[2].contains("both ways, and 1 call went"));
        assert!(refused[3].contains("both ways, and 3 calls"));

        let rows = traced(
            None,
            None,
            vec![(
                "unordered",
                with(vec![judge(
                    high,
                    low,
                    if high == "Paris" { "first" } else { "second" },
                )]),
            )],
        );
        let trace = GenerationTrace::of(&rows);
        assert_eq!(
            (rows[0].witnessed_exchange, trace.judged_unordered),
            (Some(true), 1),
            "with no order pinned a judge's pick is an exchange, and says the application \
             placed the candidates"
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
            code: None,
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
    fn a_case_asked_in_other_words_is_found_and_one_asked_on_another_prompt_is_only_counted() {
        use aiwatcher_core::witness::{Said, digest, key_for, normalized};
        let mut pins = variant();
        pins.model = None;
        let key = key_for("gateway-secret");
        let question = "What is the capital of France?";
        let asked_as = |text: &str| digest(&key, Said::Asked, text);
        let call = |replied: &str| TracedCall {
            model: Some("gpt-4o".to_owned()),
            prompt_name: Some("capitals".to_owned()),
            prompt_version: Some("p".repeat(64)),
            prompt_verified: Some(true),
            prompt_exact: Some(true),
            published_by: Some("gateway".to_owned()),
            rendered: vec![digest(&key, Said::Replied, question)],
            asked: vec![asked_as(question)],
            replied: vec![digest(&key, Said::Replied, replied)],
            started_ms: Some(1),
            ..TracedCall::default()
        };
        let elsewhere = |call: TracedCall| CallElsewhere {
            caller_run_id: Some("somewhere".to_owned()),
            call: TracedCall {
                replied: Vec::new(),
                ..call
            },
        };
        let traced = |witnesses: Witnesses| {
            let runs = BTreeMap::from([(
                "case".to_owned(),
                TracedRun {
                    served_for_it: vec![call("Paris")],
                    ..run(vec![on_the_pins()])
                },
            )]);
            let witnesses = witnesses
                .keyed([("gateway".to_owned(), key)])
                .asked(BTreeMap::from([(
                    "case".to_owned(),
                    serde_json::json!({"question": question}),
                )]))
                .pinned(Some(&serde_json::json!({})), None);
            trace_answers(
                &pins,
                "variant",
                "answers",
                &[RecordedAnswer {
                    answer: serde_json::json!("Paris"),
                    ..answer("case", Some("case"))
                }],
                &runs,
                None,
                &witnesses,
            )
            .expect("nothing contradicts the pins")
        };

        let in_other_words = traced(Witnesses::named(Vec::new()).asked_elsewhere(vec![elsewhere(
            TracedCall {
                asked: vec![asked_as("WHAT is the capital of  France")],
                asked_normalized: vec![asked_as(&normalized("WHAT is the capital of  France"))],
                ..call("Lyon")
            },
        )]));
        assert_eq!(
            (
                in_other_words[0].witnessed_exchange,
                in_other_words[0].asked_elsewhere
            ),
            (Some(false), 1),
            "the same question in another case and spacing is the same question asked"
        );

        let on_another_prompt = traced(Witnesses::named(Vec::new()).asked_elsewhere(vec![
            elsewhere(TracedCall {
                prompt_name: Some("chit-chat".to_owned()),
                prompt_version: Some("c".repeat(64)),
                ..call("Lyon")
            }),
            elsewhere(TracedCall {
                prompt_name: None,
                prompt_version: None,
                prompt_verified: None,
                ..call("Lyon")
            }),
        ]));
        let trace = GenerationTrace::of(&on_another_prompt);
        assert_eq!(
            (
                on_another_prompt[0].witnessed_exchange,
                on_another_prompt[0].asked_elsewhere_unpinned,
                trace.asked_elsewhere_unpinned
            ),
            (Some(true), 2, 1),
            "on another prompt, or none, a case asked is counted and denies nothing"
        );

        let unread = traced(
            Witnesses::named(Vec::new())
                .asked_elsewhere(Vec::new())
                .elsewhere_unread_before("2026-09-01T00:00:00Z"),
        );
        let trace = GenerationTrace::of(&unread);
        assert_eq!(unread[0].witnessed_exchange, Some(false));
        assert_eq!(
            trace.elsewhere_unread_before.as_deref(),
            Some("2026-09-01T00:00:00Z")
        );
        assert!(
            trace.unwitnessed_answers().iter().any(|line| line.contains(
                "calls asked in other runs before 2026-09-01T00:00:00Z, which the index"
            )),
            "{:?}",
            trace.unwitnessed_answers()
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
