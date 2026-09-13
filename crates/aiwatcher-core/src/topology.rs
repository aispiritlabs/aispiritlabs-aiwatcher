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
//! the run has items — which changes what a run may do on the shape, so it is
//! part of the digest; a declaration with no such node digests as it always
//! did.

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
}

impl Topology {
    /// Read a declaration: `nodes` as IDs or objects naming one by `id`, `node`
    /// or `name`, and `edges` as pairs or objects naming `from`/`source` and
    /// `to`/`target` — the shapes the workflow fold reads. `None` when it
    /// declares no node, which is a heartbeat rather than a shape.
    #[must_use]
    pub fn read(declaration: &Value) -> Option<Self> {
        let mut repeats = BTreeSet::new();
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
                    Some(id.to_owned())
                }
                _ => None,
            })
            .filter(|id| !id.is_empty())
            .collect();
        if nodes.is_empty() {
            return None;
        }
        let edges = declaration
            .get("edges")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|edge| {
                let text = |value: Option<&Value>| {
                    value
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(ToOwned::to_owned)
                };
                match edge {
                    Value::Array(pair) => Some((text(pair.first())?, text(pair.get(1))?)),
                    Value::Object(fields) => Some((
                        text(fields.get("from").or_else(|| fields.get("source")))?,
                        text(fields.get("to").or_else(|| fields.get("target")))?,
                    )),
                    _ => None,
                }
            })
            .collect();
        Some(Self {
            nodes,
            edges,
            repeats,
        })
    }

    /// The sha256 of the shape: sorted node IDs and sorted edges — and the
    /// repeating nodes, where there are any — labelled so no other digest in
    /// this system can be mistaken for it.
    #[must_use]
    pub fn digest(&self) -> String {
        let edges: Vec<[&str; 2]> = self
            .edges
            .iter()
            .map(|(from, to)| [from.as_str(), to.as_str()])
            .collect();
        let canonical = if self.repeats.is_empty() {
            serde_json::json!([1, "aiwatcher.workflow.topology", self.nodes, edges])
        } else {
            serde_json::json!([
                1,
                "aiwatcher.workflow.topology",
                self.nodes,
                edges,
                self.repeats
            ])
        };
        hex::encode(Sha256::digest(canonical.to_string().as_bytes()))
    }

    /// Where a run may enter the shape: each set is a part nothing outside it
    /// leads into — a node no edge enters, or a cycle entered from nowhere
    /// else — and entering starts one of its nodes, once.
    #[must_use]
    pub fn entries(&self) -> Vec<BTreeSet<String>> {
        let mut next: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (from, to) in &self.edges {
            next.entry(from.as_str()).or_default().push(to.as_str());
        }
        let reach = |start: &str| {
            let mut seen = BTreeSet::from([start.to_owned()]);
            let mut queue = vec![start];
            while let Some(node) = queue.pop() {
                for to in next.get(node).into_iter().flatten() {
                    if seen.insert((*to).to_owned()) {
                        queue.push(to);
                    }
                }
            }
            seen
        };
        let reaches: BTreeMap<&str, BTreeSet<String>> = self
            .nodes
            .iter()
            .map(|node| (node.as_str(), reach(node)))
            .collect();
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
                    reaches[node.as_str()].contains(*other)
                        && reaches[other.as_str()].contains(node)
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
        assert_eq!(entries, [vec!["rank".to_owned()], vec!["retrieve".to_owned()]]);

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
    fn a_declaration_naming_no_node_is_no_shape() {
        assert!(Topology::read(&json!({"name": "capitals-app", "steps": ["answer"]})).is_none());
        assert!(Topology::read(&json!({"nodes": []})).is_none());
    }
}
