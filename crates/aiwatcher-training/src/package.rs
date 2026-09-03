//! What a serving runtime is handed, and what it is allowed to assume.
//!
//! [plan.md](../../../plan.md) sequences this before any loader, and the
//! reason is the one ADR_0011 gives for `OptimizationRecord::verdict`: the
//! side that produces an artifact is the wrong side to decide what the artifact
//! means. A checkpoint URI is a pointer and nothing else — it does not say what
//! framework wrote it, what shape it eats, what it hands back, what it needs to
//! run, or whether the bytes at that address are the bytes anybody measured.
//! A runtime given only the URI has to guess all six, and every one of those
//! guesses is a way to load the wrong model and serve it confidently.
//!
//! So a package is a **declaration on the version**, written by whoever
//! trained it, and every field here exists because a serving process would
//! otherwise infer it:
//!
//! ```text
//!  runtime + version   which loader, and which build of it
//!  entry_point         what inside the artifact to load
//!  inputs / outputs    the shape, so a request can be refused rather than
//!                      reshaped into something that predicts nonsense
//!  preprocessing       what the trainer did to its inputs, named
//!  dependencies        what has to be present for the entry point to import
//!  artifacts           every file, with a digest — see below
//!  resources           what it needs, so a scheduler can refuse rather than
//!                      thrash
//! ```
//!
//! Three rules carry it.
//!
//! **Every artifact has a digest, and a package with none is refused.** This
//! is the same rule as `put_blob` hashing what it received: an address is not
//! an identity, and `s3://models/latest.pt` can be different bytes tomorrow.
//! The whole point of the model registry is that a span naming a version can
//! be traced to the images it learned from, and that chain is only as strong
//! as "these are the weights".
//!
//! **A runtime is declared, never sniffed.** A loader chosen by looking at the
//! file is a loader that can be chosen by whoever wrote the file.
//! [`Runtime::Python`] exists and is the one a control plane must not load
//! in-process, which is why it is a named variant rather than a fallback: the
//! shape of the danger is visible in the manifest rather than discovered at
//! `torch.load` time.
//!
//! **A package is optional and, once given, complete.** Versions registered
//! before this existed have none, and a runtime that meets one says so rather
//! than guessing — the same choice ADR_0019 makes about a licence nobody
//! recorded. What is refused is a *half* package: a declared runtime with an
//! artifact carrying no digest is worse than no declaration at all, because it
//! reads as one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{Error, Result, validate_slug};

/// Artifacts one package may name. A model is weights, a config and a
/// tokeniser, not a directory listing.
pub const MAX_ARTIFACTS: usize = 32;

/// Which loader reads this, and therefore what it is allowed to do.
///
/// Deliberately small, and deliberately ordered by how much of somebody else's
/// code runs when the artifact is opened. The first two are data formats with
/// a fixed interpreter; the third is a program.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// A JSON array of numbers plus the shape the package declares. No graph,
    /// no operators, no code — a linear model.
    ///
    /// The smallest thing that is still a runtime, and it is here because this
    /// repository ships one: `just e2e-train` fits an eight-weight classifier
    /// and `just serve-model` loads it. Having a name for it is what
    /// keeps the demo from declaring itself as ONNX in order to be loadable,
    /// which would make the first example anybody reads a lie about the field
    /// that decides which loader runs.
    Weights,
    /// A serialized graph plus weights, read by one interpreter with a fixed
    /// operator set.
    ///
    /// The second profile `aiwatcher_sdk.serving` implements, and the one that
    /// showed what a package is for when the artifact describes *itself*: a
    /// graph carries its own input and output names, element types and shapes,
    /// so the loader **cross-checks** [`ModelPackage::inputs`] and
    /// [`ModelPackage::outputs`] against it rather than trusting them. A
    /// disagreement is not a typo — it means the package describes a different
    /// model, and this version's held-out score belongs to that one.
    Onnx,
    /// TorchScript: a graph, not a pickle. Still torch's own deserializer.
    TorchScript,
    /// Anything whose entry point is code the package brought with it —
    /// Transformers, a custom `predict.py`, a pickled estimator.
    ///
    /// A named variant rather than a fallback, so that "this package runs
    /// somebody else's code" is a fact a scheduler can read *before* it opens
    /// anything. Never loaded in a control-plane process; see
    /// [`Runtime::executes_packaged_code`].
    Python,
    /// Declared by nobody. A runtime that meets this refuses to load rather
    /// than picking one, because picking one is the mistake this enum exists
    /// to prevent.
    #[default]
    Unspecified,
}

impl Runtime {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Weights => "weights",
            Self::Onnx => "onnx",
            Self::TorchScript => "torchscript",
            Self::Python => "python",
            Self::Unspecified => "unspecified",
        }
    }

    /// Whether loading this runs code the package supplied.
    ///
    /// The one question a host process has to answer before it opens
    /// anything. `true` means the package must be loaded in an isolated
    /// process — never in the API, which holds the object store's credentials
    /// and every registry behind them.
    #[must_use]
    pub const fn executes_packaged_code(self) -> bool {
        matches!(self, Self::Python)
    }
}

/// A number a request has to look like.
///
/// The shape is a list rather than a string so a server can check it; `None`
/// in a dimension is "any", which is how a batch axis is written. A runtime
/// that validates against this refuses a wrong request instead of reshaping it
/// into something that predicts confidently and wrongly.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TensorSpec {
    pub name: String,
    /// `float32`, `int64`, `uint8`. Free text, because the vocabulary is the
    /// runtime's and an enum here would be one framework's list imposed on
    /// every other.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dtype: String,
    /// `[null, 3, 224, 224]`. A `null` is a free dimension.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<Option<u64>>)]
    pub shape: Vec<Option<u64>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// For a classifier: what index 0, 1, 2 … mean.
    ///
    /// On the package rather than in the serving code, because a label order
    /// that lives in a deployment is a label order that silently permutes when
    /// somebody retrains — every metric stays finite and nothing says so. The
    /// same failure `ExportDataset` checks `schema_version` for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
}

/// One file a package is made of.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    /// What the runtime calls it: `weights`, `tokenizer`, `config`.
    pub name: String,
    /// Where the bytes are. A pointer, like every other artifact in this
    /// workspace — the registry stores no weights.
    pub uri: String,
    /// `sha256` of the bytes, lowercase hex. Required, and the reason this
    /// type exists: an address is not an identity.
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_type: String,
}

impl ArtifactRef {
    fn validate(&self) -> Result<()> {
        validate_slug(&self.name, "an artifact name")?;
        if self.uri.trim().is_empty() {
            return Err(Error::Invalid(format!(
                "the artifact {} has no uri",
                self.name
            )));
        }
        if self.digest.len() != 64 || !self.digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Invalid(format!(
                "the artifact {} has no sha256 digest. An address is not an identity: \
                 's3://models/latest.pt' is different bytes tomorrow, and a version whose weights \
                 cannot be checked is a provenance chain with a hole in it",
                self.name
            )));
        }
        if self.digest.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(Error::Invalid(format!(
                "the artifact {}'s digest must be lowercase hex; uppercase would address the same \
                 bytes twice",
                self.name
            )));
        }
        Ok(())
    }
}

/// What a package needs in order to run at all.
///
/// Declared so a scheduler can refuse rather than thrash. A model that needs
/// 24 GB of accelerator memory placed on a node with 16 does not fail at load
/// — it fails at the first request under load, at which point the previous
/// version is already gone.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    /// Accelerators, and how much memory each needs. `None` means it runs on a
    /// CPU, which is a claim rather than a default: a package that needs one
    /// and says nothing gets scheduled somewhere it cannot run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpus: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_memory_mb: Option<u32>,
}

/// Everything a serving runtime needs and is not allowed to guess.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelPackage {
    pub runtime: Runtime,
    /// The build of that runtime the artifact was written by: `2.4.1`,
    /// `1.17`. An ONNX graph using an opset the server's runtime does not have
    /// fails at load; saying which one it needs turns that into a deployment
    /// decision rather than a crash loop.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runtime_version: String,
    /// What inside the package to load: a file name, a module path, a
    /// `module:function`. Interpreted by the loader for that runtime and by
    /// nothing else.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entry_point: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<TensorSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<TensorSpec>,
    /// What the trainer did to its inputs, in its own words: `resize:512`,
    /// `normalize:imagenet`, `edge-grid:8`.
    ///
    /// Free text and deliberately not executable. A package that shipped
    /// preprocessing *code* would be a package that runs code in whatever
    /// opens it, which is the thing [`Runtime::executes_packaged_code`] exists
    /// to keep visible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preprocessing: Vec<String>,
    /// Package name to version specifier. What has to be present for the
    /// entry point to import.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    /// Every file, each with its digest. At least one.
    pub artifacts: Vec<ArtifactRef>,
    #[serde(default)]
    pub resources: ResourceRequest,
}

impl ModelPackage {
    /// # Errors
    /// [`Error::Invalid`] when the runtime is undeclared, there are no
    /// artifacts, or any artifact has no `sha256`.
    pub fn validate(&self) -> Result<()> {
        if self.runtime == Runtime::Unspecified {
            return Err(Error::Invalid(
                "a package has to name its runtime. A loader chosen by looking at the file is a \
                 loader chosen by whoever wrote the file"
                    .to_owned(),
            ));
        }
        if self.artifacts.is_empty() {
            return Err(Error::Invalid(
                "a package with no artifacts names nothing to load".to_owned(),
            ));
        }
        if self.artifacts.len() > MAX_ARTIFACTS {
            return Err(Error::Invalid(format!(
                "a package may name {MAX_ARTIFACTS} artifacts; this one names {}",
                self.artifacts.len()
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for artifact in &self.artifacts {
            artifact.validate()?;
            if !seen.insert(artifact.name.as_str()) {
                return Err(Error::Invalid(format!(
                    "two artifacts are both called {}; a loader resolves them by name",
                    artifact.name
                )));
            }
        }
        Ok(())
    }

    /// The artifact a loader starts from.
    ///
    /// `weights` by name when there is one, else the only artifact, else
    /// nothing — a package with three files and no `weights` has to say which
    /// through [`entry_point`](Self::entry_point) rather than have one picked
    /// for it.
    #[must_use]
    pub fn primary(&self) -> Option<&ArtifactRef> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.name == "weights")
            .or(match self.artifacts.as_slice() {
                [only] => Some(only),
                _ => None,
            })
    }

    /// `sha256` over every artifact digest, in declared order.
    ///
    /// What a running server reports as "the model I have", and what a health
    /// check compares against the registry's answer. One number rather than a
    /// list, for the same reason an export version is one number: comparing it
    /// is an equality rather than a review.
    #[must_use]
    pub fn digest(&self) -> String {
        let material: Vec<&str> = self
            .artifacts
            .iter()
            .map(|artifact| artifact.digest.as_str())
            .collect();
        crate::digest(material.join("\0").as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(name: &str) -> ArtifactRef {
        ArtifactRef {
            name: name.to_owned(),
            uri: format!("s3://models/{name}.bin"),
            digest: "ab".repeat(32),
            size_bytes: Some(1024),
            content_type: String::new(),
        }
    }

    fn package() -> ModelPackage {
        ModelPackage {
            runtime: Runtime::Onnx,
            runtime_version: "1.17".to_owned(),
            entry_point: "model.onnx".to_owned(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            preprocessing: Vec::new(),
            dependencies: BTreeMap::new(),
            artifacts: vec![artifact("weights")],
            resources: ResourceRequest::default(),
        }
    }

    #[test]
    fn an_artifact_with_no_digest_is_refused_because_an_address_is_not_an_identity() {
        let mut broken = package();
        broken.artifacts[0].digest = String::new();
        let error = broken.validate().expect_err("a digest is required");
        assert!(error.to_string().contains("an identity"), "{error}");
    }

    #[test]
    fn a_package_that_does_not_name_its_runtime_is_refused() {
        let mut broken = package();
        broken.runtime = Runtime::Unspecified;
        let error = broken.validate().expect_err("the runtime is required");
        assert!(
            error
                .to_string()
                .contains("chosen by whoever wrote the file"),
            "{error}"
        );
    }

    #[test]
    fn a_host_can_tell_before_loading_whether_a_package_runs_its_own_code() {
        assert!(!Runtime::Weights.executes_packaged_code());
        assert!(!Runtime::Onnx.executes_packaged_code());
        assert!(!Runtime::TorchScript.executes_packaged_code());
        assert!(Runtime::Python.executes_packaged_code());
    }

    #[test]
    fn the_package_digest_changes_when_any_artifact_does() {
        let one = package();
        let mut other = package();
        other.artifacts[0].digest = "cd".repeat(32);
        assert_ne!(one.digest(), other.digest());
        assert_eq!(one.digest(), package().digest());
    }

    #[test]
    fn two_artifacts_of_one_name_are_refused_because_a_loader_resolves_by_name() {
        let mut broken = package();
        broken.artifacts.push(artifact("weights"));
        let error = broken.validate().expect_err("names are keys");
        assert!(error.to_string().contains("both called"), "{error}");
    }
}
