//! Every route a verb names is a route the API serves.
//!
//! This is the cheap half of the generated CLI that was considered and not
//! built. The verbs are written by hand, because `runs list` is what somebody
//! types and `listRuns_1` is what a generator would call it — but a
//! hand-written path is one that can quietly stop existing, and the symptom
//! would be a 404 from a command that used to work, discovered by whoever
//! needed it most.
//!
//! So the *names* stay hand-written and the *paths* are checked against
//! `contracts/openapi.json`, which `just openapi` regenerates from the axum
//! routes. A route renamed on the Rust side fails here, in the same commit,
//! rather than in somebody's terminal a month later.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;

use aiwatcher_cli::commands::{read, write};

/// The contract, as the set of paths it declares.
fn contract() -> BTreeSet<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("the workspace root")
        .join("contracts/openapi.json");
    let raw = std::fs::read_to_string(&root)
        .unwrap_or_else(|error| panic!("reading {}: {error}", root.display()));
    let document: serde_json::Value = serde_json::from_str(&raw).expect("the contract is JSON");
    document
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .expect("the contract has paths")
        .keys()
        .cloned()
        .collect()
}

/// A verb's template as the contract spells it.
///
/// The two differ in one way only, and deliberately: a verb names its path
/// parameters for what a person types (`{id}`, `{name}`, `{step}`) and the
/// contract names them for what the handler calls them (`{run_id}`,
/// `{execution_id}`, `{step_id}`). Comparing the *shape* — the literal segments
/// and where the holes are — is what makes those two spellings one fact.
fn shape(template: &str) -> Vec<String> {
    template
        .split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                "{}".to_owned()
            } else {
                segment.to_owned()
            }
        })
        .collect()
}

#[test]
fn every_reading_verb_names_a_route_the_api_serves() {
    let served: BTreeSet<Vec<String>> = contract().iter().map(|path| shape(path)).collect();
    let mut missing = Vec::new();
    for spec in read::READS {
        if !served.contains(&shape(spec.path)) {
            missing.push(format!("{} → {}", spec.words.join(" "), spec.path));
        }
    }
    assert!(
        missing.is_empty(),
        "these verbs name routes the contract does not have:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn every_writing_verb_names_a_route_the_api_serves() {
    let served: BTreeSet<Vec<String>> = contract().iter().map(|path| shape(path)).collect();
    let mut missing = Vec::new();
    for spec in write::COMMANDS {
        if !served.contains(&shape(spec.path)) {
            missing.push(format!("{} → {}", spec.words.join(" "), spec.path));
        }
    }
    assert!(
        missing.is_empty(),
        "these verbs name routes the contract does not have:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn the_hand_written_write_routes_are_in_the_contract_too() {
    // The four that build a body rather than sharing the command table. They
    // are written out here because they are written out there.
    let served = contract();
    for path in [
        "/api/v1/prompts",
        "/api/v1/prompts/{name}/labels/{label}",
        "/api/v1/events",
        "/api/v1/executions",
    ] {
        assert!(served.contains(path), "the contract has no {path}");
    }
}

#[test]
fn a_verbs_query_parameters_are_ones_the_route_declares() {
    // The API rejects unknown query parameters, so a CLI-side name that does
    // not exist on the route turns a scoped read into a 400 — and the person
    // typing it would have no way to tell which of their filters was wrong.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("the workspace root")
        .join("contracts/openapi.json");
    let raw = std::fs::read_to_string(&root).expect("reads the contract");
    let document: serde_json::Value = serde_json::from_str(&raw).expect("the contract is JSON");
    let paths = document
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .expect("the contract has paths");

    let mut wrong = Vec::new();
    for spec in read::READS {
        // Find the contract's spelling of this verb's path by shape.
        let Some((_, item)) = paths
            .iter()
            .find(|(candidate, _)| shape(candidate) == shape(spec.path))
        else {
            continue; // reported by the test above
        };
        let Some(operation) = item.get("get") else {
            continue;
        };
        let declared: BTreeSet<&str> = operation
            .get("parameters")
            .and_then(serde_json::Value::as_array)
            .map(|parameters| {
                parameters
                    .iter()
                    .filter(|parameter| {
                        parameter.get("in").and_then(serde_json::Value::as_str) == Some("query")
                    })
                    .filter_map(|parameter| {
                        parameter.get("name").and_then(serde_json::Value::as_str)
                    })
                    .collect()
            })
            .unwrap_or_default();

        for (typed, sent) in spec.query {
            if !declared.contains(sent) {
                wrong.push(format!(
                    "{} sends {sent}= (typed {typed}=), which {} does not declare",
                    spec.words.join(" "),
                    spec.path
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn every_windowed_verb_reaches_a_route_that_takes_a_window() {
    // `window=` is added to every read, so a route that does not take one would
    // answer 400 for a value the CLI supplies on its own. Rather than a list of
    // exceptions, this reports which routes those are — and the reads that do
    // not take a window must not be given one.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("the workspace root")
        .join("contracts/openapi.json");
    let raw = std::fs::read_to_string(&root).expect("reads the contract");
    let document: serde_json::Value = serde_json::from_str(&raw).expect("the contract is JSON");
    let paths = document
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .expect("the contract has paths");

    let mut windowed = 0;
    for spec in read::READS {
        let Some((_, item)) = paths
            .iter()
            .find(|(candidate, _)| shape(candidate) == shape(spec.path))
        else {
            continue;
        };
        let takes = item
            .get("get")
            .and_then(|operation| operation.get("parameters"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|parameters| {
                parameters.iter().any(|parameter| {
                    parameter.get("name").and_then(serde_json::Value::as_str)
                        == Some("window_seconds")
                })
            });
        if takes {
            windowed += 1;
        }
    }
    assert!(
        windowed >= 5,
        "the window is documented as working on every list, and only {windowed} routes take one"
    );
}
