//! A declared workflow's shape, as one digest every reader computes.
//!
//! A producer's `workflow.declared` carries a `version` it chose, which the
//! catalog compares for equality and never reads. A variant pins a workflow by
//! the digest of the declaration it was measured with. Neither says whether a
//! run executed *that* shape, so both are read here into the same thing — the
//! node IDs and the edges between them, nothing a canvas adds — and digested the
//! same way, which lets a run's own declaration be compared with the pinned one
//! rather than a version string with a version string.

use std::collections::BTreeSet;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The nodes and edges of a declaration, without names, kinds or labels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Topology {
    pub nodes: BTreeSet<String>,
    pub edges: BTreeSet<(String, String)>,
}

impl Topology {
    /// Read a declaration: `nodes` as IDs or objects naming one by `id`, `node`
    /// or `name`, and `edges` as pairs or objects naming `from`/`source` and
    /// `to`/`target` — the shapes the workflow fold reads. `None` when it
    /// declares no node, which is a heartbeat rather than a shape.
    #[must_use]
    pub fn read(declaration: &Value) -> Option<Self> {
        let nodes: BTreeSet<String> = declaration
            .get("nodes")?
            .as_array()?
            .iter()
            .filter_map(|node| match node {
                Value::String(id) => Some(id.clone()),
                Value::Object(fields) => ["id", "node", "name"]
                    .iter()
                    .find_map(|key| fields.get(*key).and_then(Value::as_str))
                    .map(ToOwned::to_owned),
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
        Some(Self { nodes, edges })
    }

    /// The sha256 of the shape: sorted node IDs and sorted edges, labelled so
    /// no other digest in this system can be mistaken for it.
    #[must_use]
    pub fn digest(&self) -> String {
        let edges: Vec<[&str; 2]> = self
            .edges
            .iter()
            .map(|(from, to)| [from.as_str(), to.as_str()])
            .collect();
        let canonical = serde_json::json!([1, "aiwatcher.workflow.topology", self.nodes, edges]);
        hex::encode(Sha256::digest(canonical.to_string().as_bytes()))
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
    fn a_declaration_naming_no_node_is_no_shape() {
        assert!(Topology::read(&json!({"name": "capitals-app", "steps": ["answer"]})).is_none());
        assert!(Topology::read(&json!({"nodes": []})).is_none());
    }
}
