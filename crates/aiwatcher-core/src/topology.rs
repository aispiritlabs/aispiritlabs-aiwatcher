//! A declared workflow's shape, as one digest every reader computes.
//!
//! A producer's `workflow.declared` carries a `version` it chose, which the
//! catalog compares for equality and never reads. A variant pins a workflow by
//! the digest of the declaration it was measured with. Neither says whether a
//! run executed *that* shape, so both are read here into the same thing — the
//! node IDs and the edges between them, nothing a canvas adds — and digested the
//! same way, which lets a run's own declaration be compared with the pinned one
//! rather than a version string with a version string.
//!
//! A node may say it `repeats` — a stage run once per item, as many times as
//! the run has items — and how many times at most it may start (`at_most`),
//! which bounds a declared loop and a repeating node alike. An edge may say how
//! many times at most a run may follow it (`at_most` on the edge), which bounds
//! the rounds of a cycle through several nodes by the edge that leads back —
//! and several edges may share one bound (`bounds`), which is how a loop with
//! more than one way back into its head is held to its rounds whichever way
//! each one took —
//! which is all such a bound may hold, so one on anything but the ways back
//! into one loop's head is named ([`Topology::misbounded`]).
//! Each changes what a run may do on the shape, so each is part of the digest;
//! a declaration without any digests as it always did.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The nodes and edges of a declaration, without names, kinds or labels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Topology {
    pub nodes: BTreeSet<String>,
    pub edges: BTreeSet<(String, String)>,
    /// Nodes declared `"repeats": true`: once something leads into one, it
    /// may run any number of times, side by side.
    pub repeats: BTreeSet<String>,
    /// Nodes declared `"at_most": n`: how many times a run may start one,
    /// retries included.
    pub at_most: BTreeMap<String, u64>,
    /// Edges declared `"at_most": n`: how many times a run may follow one — a
    /// start of its target that the completion of its source led to, and that
    /// did not fail and give its turn back.
    pub edges_at_most: BTreeMap<(String, String), u64>,
    /// Edges declared together under one bound, `"bounds": [{"edges": [...],
    /// "at_most": n}]`: how many times a run may follow any of them, counted
    /// together. A bound of one edge is that edge's own `at_most`.
    pub bounds: BTreeMap<BTreeSet<(String, String)>, u64>,
}

impl Topology {
    /// Read a declaration: `nodes` as IDs or objects naming one by `id`, `node`
    /// or `name`, and `edges` as pairs or objects naming `from`/`source` and
    /// `to`/`target` — the shapes the workflow fold reads. `None` when it
    /// declares no node, which is a heartbeat rather than a shape.
    #[must_use]
    pub fn read(declaration: &Value) -> Option<Self> {
        let mut repeats = BTreeSet::new();
        let mut at_most = BTreeMap::new();
        let mut edges_at_most = BTreeMap::new();
        let nodes: BTreeSet<String> = declaration
            .get("nodes")?
            .as_array()?
            .iter()
            .filter_map(|node| match node {
                Value::String(id) => Some(id.clone()),
                Value::Object(fields) => {
                    let id = ["id", "node", "name"]
                        .iter()
                        .find_map(|key| fields.get(*key).and_then(Value::as_str))
                        .filter(|id| !id.is_empty())?;
                    if fields.get("repeats").and_then(Value::as_bool) == Some(true) {
                        repeats.insert(id.to_owned());
                    }
                    if let Some(bound) = fields
                        .get("at_most")
                        .and_then(Value::as_u64)
                        .filter(|n| *n > 0)
                    {
                        at_most.insert(id.to_owned(), bound);
                    }
                    Some(id.to_owned())
                }
                _ => None,
            })
            .filter(|id| !id.is_empty())
            .collect();
        if nodes.is_empty() {
            return None;
        }
        let text = |value: Option<&Value>| {
            value
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(ToOwned::to_owned)
        };
        let edge_of = |edge: &Value| match edge {
            Value::Array(pair) => Some((text(pair.first())?, text(pair.get(1))?)),
            Value::Object(fields) => Some((
                text(fields.get("from").or_else(|| fields.get("source")))?,
                text(fields.get("to").or_else(|| fields.get("target")))?,
            )),
            _ => None,
        };
        let edges: BTreeSet<(String, String)> = declaration
            .get("edges")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|edge| {
                let read = edge_of(edge)?;
                if let Some(bound) = edge
                    .get("at_most")
                    .and_then(Value::as_u64)
                    .filter(|n| *n > 0)
                {
                    edges_at_most.insert(read.clone(), bound);
                }
                Some(read)
            })
            .collect();
        let mut bounds: BTreeMap<BTreeSet<(String, String)>, u64> = BTreeMap::new();
        for bound in declaration
            .get("bounds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(at_most) = bound
                .get("at_most")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
            else {
                continue;
            };
            // Only edges the declaration has: a bound on a way the shape does
            // not lead holds nothing a run could do.
            let shared: BTreeSet<(String, String)> = bound
                .get("edges")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(edge_of)
                .filter(|edge| edges.contains(edge))
                .collect();
            let mut single = shared.iter();
            match (single.next(), single.next()) {
                (None, _) => {}
                (Some(edge), None) => {
                    let held = edges_at_most.entry(edge.clone()).or_insert(at_most);
                    *held = (*held).min(at_most);
                }
                (Some(_), Some(_)) => {
                    let held = bounds.entry(shared).or_insert(at_most);
                    *held = (*held).min(at_most);
                }
            }
        }
        Some(Self {
            nodes,
            edges,
            repeats,
            at_most,
            edges_at_most,
            bounds,
        })
    }

    /// The sha256 of the shape: sorted node IDs and sorted edges — and the
    /// repeating nodes, the nodes' bounds, the edges' bounds and the bounds
    /// several edges share, each where it or one after it is declared —
    /// labelled so no other digest in this system can be mistaken for it.
    #[must_use]
    pub fn digest(&self) -> String {
        let edges: Vec<[&str; 2]> = self
            .edges
            .iter()
            .map(|(from, to)| [from.as_str(), to.as_str()])
            .collect();
        let mut canonical = vec![
            serde_json::json!(1),
            serde_json::json!("aiwatcher.workflow.topology"),
            serde_json::json!(self.nodes),
            serde_json::json!(edges),
        ];
        let shared = !self.bounds.is_empty();
        let edge_bounds = !self.edges_at_most.is_empty() || shared;
        if !self.repeats.is_empty() || !self.at_most.is_empty() || edge_bounds {
            canonical.push(serde_json::json!(self.repeats));
        }
        if !self.at_most.is_empty() || edge_bounds {
            let bounds: Vec<(&str, u64)> = self
                .at_most
                .iter()
                .map(|(node, bound)| (node.as_str(), *bound))
                .collect();
            canonical.push(serde_json::json!(bounds));
        }
        if edge_bounds {
            let bounds: Vec<(&str, &str, u64)> = self
                .edges_at_most
                .iter()
                .map(|((from, to), bound)| (from.as_str(), to.as_str(), *bound))
                .collect();
            canonical.push(serde_json::json!(bounds));
        }
        if shared {
            let bounds: Vec<(Vec<[&str; 2]>, u64)> = self
                .bounds
                .iter()
                .map(|(edges, bound)| {
                    (
                        edges
                            .iter()
                            .map(|(from, to)| [from.as_str(), to.as_str()])
                            .collect(),
                        *bound,
                    )
                })
                .collect();
            canonical.push(serde_json::json!(bounds));
        }
        hex::encode(Sha256::digest(
            serde_json::Value::Array(canonical).to_string().as_bytes(),
        ))
    }

    /// Every node each node reaches along the declared edges, itself included.
    fn reaches(&self) -> BTreeMap<&str, BTreeSet<&str>> {
        let mut next: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (from, to) in &self.edges {
            next.entry(from.as_str()).or_default().push(to.as_str());
        }
        self.nodes
            .iter()
            .map(|start| {
                let mut seen = BTreeSet::from([start.as_str()]);
                let mut queue = vec![start.as_str()];
                while let Some(node) = queue.pop() {
                    for to in next.get(node).into_iter().flatten() {
                        if seen.insert(*to) {
                            queue.push(to);
                        }
                    }
                }
                (start.as_str(), seen)
            })
            .collect()
    }

    /// Where a run may enter the shape: each set is a part nothing outside it
    /// leads into — a node no edge enters, or a cycle entered from nowhere
    /// else — and entering starts one of its nodes, once.
    #[must_use]
    pub fn entries(&self) -> Vec<BTreeSet<String>> {
        let reaches = self.reaches();
        let mut placed = BTreeSet::new();
        let mut entries = Vec::new();
        for node in &self.nodes {
            if placed.contains(node) {
                continue;
            }
            let part: BTreeSet<String> = self
                .nodes
                .iter()
                .filter(|other| {
                    reaches[node.as_str()].contains(other.as_str())
                        && reaches[other.as_str()].contains(node.as_str())
                })
                .cloned()
                .collect();
            placed.extend(part.iter().cloned());
            if !self
                .edges
                .iter()
                .any(|(from, to)| part.contains(to) && !part.contains(from))
            {
                entries.push(part);
            }
        }
        entries
    }

    /// Each bound several edges share that is not on the ways back of one
    /// loop, in words. A shared bound counts a loop's rounds whichever way back
    /// each took, and a round is a return to the loop's head — so every edge
    /// under it has to lead back to where it left (its target reaches its
    /// source), and all of them into one node, the head. An edge that leads
    /// nowhere back is followed once per completion of its source and goes
    /// round nothing; edges leading back into two nodes go round two loops —
    /// separate cycles, or two loops through a node they share — and a count
    /// of both is the rounds of neither. Two bodies returning into one head are
    /// one loop, and a body's own rounds are its edge's own `at_most`. Empty
    /// when every shared bound is on one loop's ways back; a bound of one edge
    /// is that edge's own `at_most` and is not asked.
    #[must_use]
    pub fn misbounded(&self) -> Vec<String> {
        let reaches = self.reaches();
        let spelled = |edges: &mut dyn Iterator<Item = &(String, String)>| {
            edges
                .map(|(from, to)| format!("{from} to {to}"))
                .collect::<Vec<_>>()
                .join(" and ")
        };
        let mut said = Vec::new();
        for (edges, at_most) in &self.bounds {
            let nowhere_back: Vec<&(String, String)> = edges
                .iter()
                .filter(|(from, to)| {
                    !reaches
                        .get(to.as_str())
                        .is_some_and(|reached| reached.contains(from.as_str()))
                })
                .collect();
            if !nowhere_back.is_empty() {
                said.push(format!(
                    "the bound of at most {at_most} that {} share is on {}, which leads nowhere \
                     back",
                    spelled(&mut edges.iter()),
                    spelled(&mut nowhere_back.into_iter())
                ));
                continue;
            }
            let heads: BTreeSet<&str> = edges.iter().map(|(_, to)| to.as_str()).collect();
            if heads.len() > 1 {
                said.push(format!(
                    "the bound of at most {at_most} that {} share leads back into {}, the heads of \
                     different loops, whose rounds are counted apart",
                    spelled(&mut edges.iter()),
                    heads.into_iter().collect::<Vec<_>>().join(" and into ")
                ));
            }
        }
        said
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn two_spellings_of_one_shape_are_one_digest_and_another_edge_is_another() {
        let short = Topology::read(&json!({
            "nodes": ["retrieve", "answer"],
            "edges": [["retrieve", "answer"]]
        }))
        .expect("a shape");
        let long = Topology::read(&json!({
            "name": "capitals-app",
            "nodes": [{"id": "answer", "kind": "chain"}, {"name": "retrieve"}],
            "edges": [{"from": "retrieve", "to": "answer", "label": "documents"}]
        }))
        .expect("a shape");
        assert_eq!(short.digest(), long.digest());

        let reversed = Topology::read(&json!({
            "nodes": ["retrieve", "answer"],
            "edges": [["answer", "retrieve"]]
        }))
        .expect("a shape");
        assert_ne!(short.digest(), reversed.digest());
    }

    #[test]
    fn a_repeating_node_is_part_of_the_shape_and_a_declaration_without_one_digests_as_before() {
        let plain = json!({"nodes": ["retrieve", "answer"], "edges": [["retrieve", "answer"]]});
        let repeating = json!({
            "nodes": ["retrieve", {"id": "answer", "repeats": true}],
            "edges": [["retrieve", "answer"]]
        });
        let plain = Topology::read(&plain).expect("a shape");
        let repeating = Topology::read(&repeating).expect("a shape");

        assert_eq!(
            plain.digest(),
            "8075765af5b44ded505202eca0096fe4d6f30a8423fdbd226f22ab53d9b8dd3d",
            "the digest every declaration without a repeating node already had"
        );
        assert_eq!(repeating.repeats, BTreeSet::from(["answer".to_owned()]));
        assert_ne!(plain.digest(), repeating.digest());
        let bounded = Topology::read(&json!({
            "nodes": ["retrieve", {"id": "answer", "repeats": true, "at_most": 3}],
            "edges": [["retrieve", "answer"]]
        }))
        .expect("a shape");
        assert_eq!(bounded.at_most.get("answer"), Some(&3));
        assert_ne!(
            bounded.digest(),
            repeating.digest(),
            "a bound is part of the shape"
        );
        let rounds = Topology::read(&json!({
            "nodes": ["retrieve", "answer"],
            "edges": [["retrieve", "answer"], {"from": "answer", "to": "retrieve", "at_most": 2}]
        }))
        .expect("a shape");
        let unbounded = Topology::read(&json!({
            "nodes": ["retrieve", "answer"],
            "edges": [["retrieve", "answer"], ["answer", "retrieve"]]
        }))
        .expect("a shape");
        assert_eq!(
            rounds
                .edges_at_most
                .get(&("answer".to_owned(), "retrieve".to_owned())),
            Some(&2)
        );
        assert_eq!(rounds.edges, unbounded.edges);
        assert_ne!(
            rounds.digest(),
            unbounded.digest(),
            "an edge's bound is part of the shape"
        );
        let two_ways_back = json!({
            "nodes": ["write", "review", "fix"],
            "edges": [["write", "review"], ["review", "write"], ["review", "fix"], ["fix", "review"]],
        });
        let before = Topology::read(&two_ways_back).expect("a shape").digest();
        let mut shared = two_ways_back.clone();
        shared["bounds"] = json!([
            {"edges": [["review", "write"], {"from": "fix", "to": "review"}, ["nowhere", "write"]], "at_most": 3},
            {"edges": [["review", "write"]], "at_most": 5},
            {"edges": [], "at_most": 2},
        ]);
        let shared = Topology::read(&shared).expect("a shape");
        assert_eq!(
            shared.bounds,
            BTreeMap::from([(
                BTreeSet::from([
                    ("fix".to_owned(), "review".to_owned()),
                    ("review".to_owned(), "write".to_owned()),
                ]),
                3,
            )]),
            "an edge the shape lacks bounds nothing, and a bound of one edge is its own"
        );
        assert_eq!(
            shared
                .edges_at_most
                .get(&("review".to_owned(), "write".to_owned())),
            Some(&5)
        );
        assert_ne!(
            shared.digest(),
            before,
            "a shared bound is part of the shape"
        );
        let mut spelled = two_ways_back;
        spelled["edges"][1] = json!({"from": "review", "to": "write", "at_most": 5});
        spelled["bounds"] =
            json!([{"edges": [["fix", "review"], ["review", "write"]], "at_most": 3}]);
        assert_eq!(
            Topology::read(&spelled).expect("a shape").digest(),
            shared.digest(),
            "one bound, however it was spelled"
        );
    }

    #[test]
    fn a_node_nothing_enters_and_a_cycle_entered_from_nowhere_are_where_a_run_may_begin() {
        let dag = Topology::read(&json!({
            "nodes": ["retrieve", "rank", "answer"],
            "edges": [["retrieve", "answer"], ["rank", "answer"]]
        }))
        .expect("a shape");
        let entries: Vec<Vec<String>> = dag
            .entries()
            .into_iter()
            .map(|part| part.into_iter().collect())
            .collect();
        assert_eq!(
            entries,
            [vec!["rank".to_owned()], vec!["retrieve".to_owned()]]
        );

        let conversation = Topology::read(&json!({
            "nodes": ["planner", "executor", "report"],
            "edges": [["planner", "executor"], ["executor", "planner"], ["executor", "report"]]
        }))
        .expect("a shape");
        let entries = conversation.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0],
            BTreeSet::from(["executor".to_owned(), "planner".to_owned()])
        );
    }

    #[test]
    fn a_shared_bound_holds_only_where_every_edge_under_it_leads_back_into_one_head() {
        let loops = json!({
            "nodes": ["write", "review", "fix", "publish", "draft", "check"],
            "edges": [
                ["write", "review"], ["review", "write"], ["review", "fix"], ["fix", "write"],
                ["fix", "review"], ["review", "publish"], ["draft", "check"], ["check", "draft"]
            ],
        });
        let bounded = |bounds: serde_json::Value| {
            let mut declaration = loops.clone();
            declaration["bounds"] = bounds;
            Topology::read(&declaration).expect("a shape").misbounded()
        };
        assert!(
            bounded(json!([{"edges": [["review", "write"], ["fix", "write"]], "at_most": 3}]))
                .is_empty(),
            "two ways back into one head are one loop's"
        );
        assert!(
            bounded(json!([{"edges": [["review", "publish"]], "at_most": 1}])).is_empty(),
            "a bound of one edge is that edge's own"
        );
        assert_eq!(
            bounded(json!([{"edges": [["review", "write"], ["review", "publish"]], "at_most": 3}])),
            [
                "the bound of at most 3 that review to publish and review to write share is on \
              review to publish, which leads nowhere back"
            ]
        );
        assert_eq!(
            bounded(json!([{"edges": [["review", "write"], ["fix", "review"]], "at_most": 3}])),
            [
                "the bound of at most 3 that fix to review and review to write share leads back \
              into review and into write, the heads of different loops, whose rounds are counted \
              apart"
            ],
            "two loops through the node they share, write and review round one and review and \
             fix the other"
        );
        assert_eq!(
            bounded(json!([{"edges": [["review", "write"], ["check", "draft"]], "at_most": 2}])),
            [
                "the bound of at most 2 that check to draft and review to write share leads back \
              into draft and into write, the heads of different loops, whose rounds are counted \
              apart"
            ]
        );
    }

    #[test]
    fn a_declaration_naming_no_node_is_no_shape() {
        assert!(Topology::read(&json!({"name": "capitals-app", "steps": ["answer"]})).is_none());
        assert!(Topology::read(&json!({"nodes": []})).is_none());
    }
}
