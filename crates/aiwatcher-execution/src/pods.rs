//! A pod of a step's own: what a step asks for, and the operator's templates
//! that answer it (ADR_0029).
//!
//! Two halves, and only one of them is written by whoever saves a workflow. A
//! [`PodRequest`] names a template, an image and at most two quantities. A
//! [`PodTemplate`] is everything else — volumes, secrets, a service account —
//! and the operator writes it into a file no route writes. The pairing is the
//! unit of trust: which code may meet this template's secrets is answered by
//! this template's own list of images, and never by a list the whole
//! deployment shares.
//!
//! Pure: parsing and checking, and no cluster. Registration asks
//! [`refusals`] with no credential at all, which is why the serve role can
//! read the same file the launcher does.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use utoipa::ToSchema;

use crate::claim::AttemptKey;
use crate::plan::{ExecutionPlan, RuntimeBinding};

/// The variables aiwatcher sets on a launched pod's container: the attempt it
/// is for, where the API is, and the name it claims under. A template that sets
/// one is refused, because the pod would then claim somebody else's attempt or
/// report to somebody else's API.
pub const OWNED_ENV: [&str; 3] = [
    "AIWATCHER_ATTEMPT",
    "AIWATCHER_URL",
    "AIWATCHER_WORKER_NAME",
];

/// Five minutes from the Job's creation to the pod's claim, unless the
/// template says otherwise — an image pull and a scheduling decision, which is
/// minutes on a cold node and seconds on a warm one.
///
/// Public because the watch reads an allowance for a Job whose template is no
/// longer configured, and the allowance is what decides that a pod never got
/// going: absent, it would decide "never" (ADR_0029).
pub const DEFAULT_START_ALLOWANCE_SECONDS: u64 = 300;

/// What a step asks for when it wants a pod of its own.
///
/// Nested under `pod` rather than flattened into the step, because serde's
/// `flatten` and `deny_unknown_fields` do not compose — and it denies unknown
/// fields itself, so a `namespace`, a `secret`, a `serviceAccount` or a `gpu`
/// is refused by name rather than stored and ignored. Those are the template's,
/// or are not built yet.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PodRequest {
    /// The operator's template, by name.
    pub template: String,
    /// `registry/repository[:tag][@digest]`. The repository has to be on the
    /// template's list, exactly; the tag or digest is the step's own.
    pub image: String,
    /// CPU as a Kubernetes quantity (`500m`, `2`), the request and the limit
    /// both. Absent takes the template's defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// Memory as a Kubernetes quantity (`512Mi`, `4Gi`), the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

impl PodRequest {
    /// What is wrong with this request before any template is read.
    ///
    /// The part of the answer a definition carries on its own — a template
    /// name that could be one, an image that parses, quantities that are
    /// quantities — so the compiler refuses it in every deployment, including
    /// one with no templates at all.
    #[must_use]
    pub fn problems(&self, step: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if !is_label(&self.template) {
            problems.push(format!(
                "{step}: pod.template '{}' is not a template name — lower-case letters, digits \
                 and '-', at most 63",
                self.template
            ));
        }
        if let Err(why) = ImageRef::parse(&self.image) {
            problems.push(format!(
                "{step}: pod.image '{}' is not an image reference: {why}",
                self.image
            ));
        }
        for (resource, value) in self.quantities() {
            if let Err(why) = resource.parse(value) {
                problems.push(format!(
                    "{step}: pod.{resource} '{value}' is not a Kubernetes quantity of {resource}: \
                     {why}"
                ));
            }
        }
        problems
    }

    fn quantities(&self) -> impl Iterator<Item = (Resource, &str)> {
        [
            (Resource::Cpu, self.cpu.as_deref()),
            (Resource::Memory, self.memory.as_deref()),
        ]
        .into_iter()
        .filter_map(|(resource, value)| value.map(|value| (resource, value)))
    }
}

/// An image reference, split into what an allowlist compares and what it does
/// not.
///
/// Docker's grammar, with Docker's defaults written out: `python:3.13` is
/// `docker.io/library/python`, so an entry and a step that spell one image two
/// ways still compare equal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub registry: String,
    pub repository: String,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

impl ImageRef {
    /// # Errors
    /// Why the text is not an image reference, in words.
    pub fn parse(reference: &str) -> Result<Self, String> {
        if reference.is_empty() || reference.len() > 512 {
            return Err("it must be between 1 and 512 characters".to_owned());
        }
        if reference
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("it holds whitespace".to_owned());
        }
        let (rest, digest) = match reference.split_once('@') {
            Some((rest, digest)) => {
                if !is_digest(digest) {
                    return Err(format!("'{digest}' is not a digest (algorithm:hex)"));
                }
                (rest, Some(digest.to_owned()))
            }
            None => (reference, None),
        };
        // A colon after the last slash is a tag; one before it is a port.
        let (name, tag) = match rest.rfind(':') {
            Some(at) if !rest[at..].contains('/') => (&rest[..at], Some(&rest[at + 1..])),
            _ => (rest, None),
        };
        if let Some(tag) = tag
            && !is_tag(tag)
        {
            return Err(format!("'{tag}' is not a tag"));
        }
        let mut components: Vec<&str> = name.split('/').collect();
        let registry = match components.first() {
            Some(first)
                if components.len() > 1
                    && (first.contains('.') || first.contains(':') || *first == "localhost") =>
            {
                let host = components.remove(0).to_ascii_lowercase();
                if !is_host(&host) {
                    return Err(format!("'{host}' is not a registry"));
                }
                if host == "index.docker.io" {
                    "docker.io".to_owned()
                } else {
                    host
                }
            }
            _ => "docker.io".to_owned(),
        };
        if let Some(bad) = components.iter().find(|part| !is_path_component(part)) {
            return Err(format!(
                "'{bad}' is not a repository path component — lower-case letters and digits, \
                 separated by '.', '_', '__' or dashes"
            ));
        }
        let mut repository = components.join("/");
        if registry == "docker.io" && components.len() == 1 {
            repository = format!("library/{repository}");
        }
        Ok(Self {
            registry,
            repository,
            tag: tag.map(str::to_owned),
            digest,
        })
    }

    /// `registry/repository`: what an allowlist entry names.
    #[must_use]
    pub fn name(&self) -> String {
        format!("{}/{}", self.registry, self.repository)
    }

    /// The same repository, exactly — never a prefix of one. An entry
    /// `ghcr.io/planner/import` admitting `ghcr.io/planner/import-debug` would
    /// be a permission invented by spelling, the corpus-name rule applied to
    /// images.
    #[must_use]
    pub fn same_repository(&self, other: &Self) -> bool {
        self.registry == other.registry && self.repository == other.repository
    }
}

/// The two resources a step may ask for, and how each is counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    /// In millicores.
    Cpu,
    /// In bytes.
    Memory,
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
        })
    }
}

impl Resource {
    /// A Kubernetes quantity of this resource, in millicores or in bytes.
    ///
    /// A fraction rounds up, as Kubernetes rounds it. Zero is refused: a
    /// request for no CPU is a pod the scheduler places anywhere and the kernel
    /// starves, and nobody asks for that on purpose. So are the forms nobody
    /// writes by hand — an exponent, milli-bytes — which would only be one more
    /// grammar to get wrong.
    ///
    /// # Errors
    /// Why the text is not a quantity of this resource.
    pub fn parse(self, raw: &str) -> Result<u128, String> {
        let end = raw
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(raw.len());
        let (number, suffix) = raw.split_at(end);
        let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
        if (whole.is_empty() && fraction.is_empty())
            || number.ends_with('.')
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return Err("it does not start with a number".to_owned());
        }
        if whole.len() > 18 || fraction.len() > 9 {
            return Err("it has more digits than Kubernetes keeps".to_owned());
        }
        let multiplier: u128 = match (self, suffix) {
            (Self::Cpu, "") => 1_000,
            (Self::Cpu, "m") => 1,
            (Self::Memory, "") => 1,
            (Self::Memory, "k") => 1_000,
            (Self::Memory, "M") => 1_000_000,
            (Self::Memory, "G") => 1_000_000_000,
            (Self::Memory, "T") => 1_000_000_000_000,
            (Self::Memory, "P") => 1_000_000_000_000_000,
            (Self::Memory, "E") => 1_000_000_000_000_000_000,
            (Self::Memory, "Ki") => 1 << 10,
            (Self::Memory, "Mi") => 1 << 20,
            (Self::Memory, "Gi") => 1 << 30,
            (Self::Memory, "Ti") => 1 << 40,
            (Self::Memory, "Pi") => 1 << 50,
            (Self::Memory, "Ei") => 1 << 60,
            (Self::Cpu, _) => {
                return Err(format!("'{suffix}' is not a unit of CPU; use m or none"));
            }
            (Self::Memory, _) => {
                return Err(format!(
                    "'{suffix}' is not a unit of memory; use Ki, Mi, Gi, Ti, k, M, G, T or none"
                ));
            }
        };
        let scale = 10u128.pow(u32::try_from(fraction.len()).unwrap_or(9));
        let whole: u128 = if whole.is_empty() {
            0
        } else {
            whole
                .parse()
                .map_err(|_| "it does not start with a number")?
        };
        let fraction: u128 = if fraction.is_empty() {
            0
        } else {
            fraction
                .parse()
                .map_err(|_| "it does not start with a number")?
        };
        let value = whole
            .checked_mul(scale)
            .and_then(|whole| whole.checked_add(fraction))
            .and_then(|mantissa| mantissa.checked_mul(multiplier))
            .map(|scaled| scaled.div_ceil(scale))
            .ok_or("it is larger than any node")?;
        if value == 0 {
            return Err("it must be above zero".to_owned());
        }
        Ok(value)
    }
}

/// A CPU and a memory quantity, each optional, as Kubernetes writes them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceValues {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

impl ResourceValues {
    #[must_use]
    pub fn get(&self, resource: Resource) -> Option<&str> {
        match resource {
            Resource::Cpu => self.cpu.as_deref(),
            Resource::Memory => self.memory.as_deref(),
        }
    }
}

/// A template's resources: what a step gets when it asks for nothing, and the
/// most it may ask for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateResources {
    #[serde(default)]
    pub requests: ResourceValues,
    #[serde(default)]
    pub limits: ResourceValues,
    /// The ceiling, and both halves are required: a template with no ceiling
    /// lets whoever saves a workflow ask for the whole node.
    #[serde(default)]
    pub max: ResourceValues,
}

/// What the container is given: a request and a limit per resource.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resources {
    pub requests: ResourceValues,
    pub limits: ResourceValues,
}

/// One operator's template (ADR_0029).
///
/// Refuses unknown fields, so `image:` written for `images:` is a start-up
/// failure naming it rather than a template that admits nothing.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PodTemplate {
    /// The repositories this template runs, without a tag or digest — the tag
    /// is the step's. Matched exactly, never by prefix.
    pub images: Vec<String>,
    #[serde(default)]
    pub resources: TemplateResources,
    /// What the container runs: the application's own worker, in `run-attempt`
    /// mode. aiwatcher adds the attempt through the environment, not here.
    pub command: Vec<String>,
    /// How long a pod has, from its Job's creation, to claim its attempt.
    /// Added to the step's timeout for the Job's own deadline.
    #[serde(default = "default_start_allowance")]
    pub start_allowance_seconds: u64,
    /// A pod-spec fragment the operator writes in full: volumes, env from
    /// secrets, a service account, node selectors, tolerations, a security
    /// context. At most one container, and none of the fields aiwatcher fills
    /// in — [`PodTemplate::problems`] says which.
    #[serde(default)]
    pub pod: Map<String, Value>,
}

fn default_start_allowance() -> u64 {
    DEFAULT_START_ALLOWANCE_SECONDS
}

impl PodTemplate {
    /// Whether this template runs `image`: its repository is on the list.
    #[must_use]
    pub fn admits(&self, image: &ImageRef) -> bool {
        self.images
            .iter()
            .filter_map(|entry| ImageRef::parse(entry).ok())
            .any(|entry| entry.same_repository(image))
    }

    /// What the container asks for under this template.
    ///
    /// A step's own quantity is its request **and** its limit, so what it
    /// asked for is what it gets and never more; whatever it left out is the
    /// template's default, request and limit as the operator wrote them.
    #[must_use]
    pub fn resources_for(&self, request: &PodRequest) -> Resources {
        let pick = |asked: &Option<String>, default: &Option<String>| {
            asked.clone().or_else(|| default.clone())
        };
        let defaults = &self.resources;
        Resources {
            requests: ResourceValues {
                cpu: pick(&request.cpu, &defaults.requests.cpu),
                memory: pick(&request.memory, &defaults.requests.memory),
            },
            limits: ResourceValues {
                cpu: pick(&request.cpu, &defaults.limits.cpu),
                memory: pick(&request.memory, &defaults.limits.memory),
            },
        }
    }

    /// Everything wrong with this template, each problem naming it.
    #[must_use]
    pub fn problems(&self, name: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if !is_label(name) {
            problems.push(format!(
                "{name}: a template's name is a label on every Job it starts — lower-case \
                 letters, digits and '-', at most 63"
            ));
        }
        if self.images.is_empty() {
            problems.push(format!(
                "{name}: images is empty, so the template runs nothing"
            ));
        }
        for entry in &self.images {
            match ImageRef::parse(entry) {
                Err(why) => problems.push(format!(
                    "{name}: images entry '{entry}' is not an image reference: {why}"
                )),
                Ok(image) if image.tag.is_some() || image.digest.is_some() => {
                    problems.push(format!(
                        "{name}: images entry '{entry}' names a tag or a digest — an entry is a \
                         repository, and which version runs is the step's"
                    ));
                }
                Ok(_) => {}
            }
        }
        if self.command.is_empty() || self.command.iter().any(String::is_empty) {
            problems.push(format!(
                "{name}: command is empty — it is what the container runs, the application's \
                 worker in run-attempt mode"
            ));
        }
        if !(1..=3_600).contains(&self.start_allowance_seconds) {
            problems.push(format!(
                "{name}: start_allowance_seconds must be between 1 and 3600"
            ));
        }
        problems.extend(self.resource_problems(name));
        problems.extend(pod_problems(name, &self.pod));
        problems
    }

    fn resource_problems(&self, name: &str) -> Vec<String> {
        let mut problems = Vec::new();
        let resources = &self.resources;
        for resource in [Resource::Cpu, Resource::Memory] {
            let mut read = |section: &str, value: Option<&str>| {
                let value = value?;
                match resource.parse(value) {
                    Ok(parsed) => Some(parsed),
                    Err(why) => {
                        problems.push(format!(
                            "{name}: resources.{section}.{resource} '{value}' is not a \
                             Kubernetes quantity of {resource}: {why}"
                        ));
                        None
                    }
                }
            };
            let request = read("requests", resources.requests.get(resource));
            let limit = read("limits", resources.limits.get(resource));
            let max = read("max", resources.max.get(resource));
            if resources.max.get(resource).is_none() {
                problems.push(format!(
                    "{name}: resources.max.{resource} is required — a step may ask for more \
                     {resource}, and the ceiling is what stops it"
                ));
            }
            for (section, value) in [("requests", request), ("limits", limit)] {
                if let (Some(value), Some(max)) = (value, max)
                    && value > max
                {
                    problems.push(format!(
                        "{name}: resources.{section}.{resource} is above resources.max.{resource}, \
                         so a step asking for nothing would be refused"
                    ));
                }
            }
            if let (Some(request), Some(limit)) = (request, limit)
                && request > limit
            {
                problems.push(format!(
                    "{name}: resources.requests.{resource} is above resources.limits.{resource}"
                ));
            }
        }
        problems
    }
}

/// The fields of a pod aiwatcher fills in, and why, refused where the operator
/// wrote them.
fn pod_problems(name: &str, pod: &Map<String, Value>) -> Vec<String> {
    let mut problems = Vec::new();
    for (field, why) in [
        (
            "restartPolicy",
            "every launched pod is Never, because the engine owns a pod's retries",
        ),
        (
            "activeDeadlineSeconds",
            "aiwatcher sets the Job's deadline from the step's timeout and the start allowance",
        ),
        (
            "backoffLimit",
            "it is a Job field, and aiwatcher sets it to 0",
        ),
        (
            "ttlSecondsAfterFinished",
            "it is a Job field, and aiwatcher sets it",
        ),
    ] {
        if pod.contains_key(field) {
            problems.push(format!("{name}: pod.{field} is aiwatcher's — {why}"));
        }
    }
    let Some(containers) = pod.get("containers") else {
        return problems;
    };
    let Some([container]) = containers.as_array().map(Vec::as_slice) else {
        problems.push(format!(
            "{name}: pod.containers holds anything but exactly one container — a launched pod \
             runs one, and init containers go under initContainers"
        ));
        return problems;
    };
    let Some(container) = container.as_object() else {
        problems.push(format!("{name}: pod.containers[0] is not an object"));
        return problems;
    };
    for (field, why) in [
        ("image", "it is the step's, checked against images"),
        ("resources", "they are the step's, bounded by resources.max"),
        ("command", "it is the template's command"),
        ("args", "the template's command is the whole of what runs"),
    ] {
        if container.contains_key(field) {
            problems.push(format!(
                "{name}: pod.containers[0].{field} is aiwatcher's — {why}"
            ));
        }
    }
    let owned = container
        .get("env")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .filter(|variable| OWNED_ENV.contains(variable));
    for variable in owned {
        problems.push(format!(
            "{name}: pod.containers[0].env sets {variable}, which aiwatcher sets — it says which \
             attempt the pod claims and where it reports"
        ));
    }
    problems
}

/// Every template this deployment has, read from `AIWATCHER_POD_TEMPLATES`.
///
/// A map from name to template. Both roles read it: the serve role checks a
/// step against it at registration, and the work role checks again before it
/// starts a pod, because the file is configuration and can change after a
/// definition was saved.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct PodTemplates {
    templates: BTreeMap<String, PodTemplate>,
}

/// Why a templates file was refused.
#[derive(Debug, Error)]
pub enum PodTemplatesError {
    #[error("the pod templates are not JSON of the expected shape: {0}")]
    Malformed(String),
    #[error("{}", .0.join("; "))]
    Refused(Vec<String>),
}

impl PodTemplates {
    /// Read and check a templates file, every problem at once.
    ///
    /// # Errors
    /// [`PodTemplatesError::Malformed`] for a file that is not the shape, and
    /// [`PodTemplatesError::Refused`] naming each template and field that is
    /// wrong — a malformed file fails the start rather than being skipped,
    /// because a template that silently did not load refuses every step that
    /// names it while looking exactly like one that had.
    pub fn parse(bytes: &[u8]) -> Result<Self, PodTemplatesError> {
        let templates: Self = serde_json::from_slice(bytes)
            .map_err(|error| PodTemplatesError::Malformed(error.to_string()))?;
        let problems: Vec<String> = templates
            .templates
            .iter()
            .flat_map(|(name, template)| template.problems(name))
            .collect();
        if problems.is_empty() {
            Ok(templates)
        } else {
            Err(PodTemplatesError::Refused(problems))
        }
    }

    /// Templates built in code, unchecked — for a test, which checks what it
    /// means to.
    #[must_use]
    pub fn from_templates(templates: BTreeMap<String, PodTemplate>) -> Self {
        Self { templates }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PodTemplate> {
        self.templates.get(name)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Why `request` may not run under these templates, each problem naming
    /// the step, the value and the template. Empty is allowed.
    #[must_use]
    pub fn refusals(&self, step: &str, request: &PodRequest) -> Vec<String> {
        let Some(template) = self.templates.get(&request.template) else {
            let configured = if self.templates.is_empty() {
                "none".to_owned()
            } else {
                self.templates
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return vec![format!(
                "{step}: pod template '{}' is not configured here; this deployment has {configured}",
                request.template
            )];
        };
        let mut problems = Vec::new();
        match ImageRef::parse(&request.image) {
            Ok(image) if template.admits(&image) => {}
            Ok(image) => problems.push(format!(
                "{step}: image '{}' is not on pod template '{}''s list — its repository {} has to \
                 be one of {}, exactly",
                request.image,
                request.template,
                image.name(),
                template.images.join(", ")
            )),
            Err(why) => problems.push(format!(
                "{step}: pod.image '{}' is not an image reference: {why}",
                request.image
            )),
        }
        for (resource, asked) in request.quantities() {
            let ceiling = template.resources.max.get(resource);
            match (
                resource.parse(asked),
                ceiling.map(|max| resource.parse(max)),
            ) {
                (Ok(asked_value), Some(Ok(max))) if asked_value > max => {
                    problems.push(format!(
                        "{step}: {resource} '{asked}' is above pod template '{}''s ceiling of {}",
                        request.template,
                        ceiling.unwrap_or_default()
                    ));
                }
                (Err(why), _) => problems.push(format!(
                    "{step}: pod.{resource} '{asked}' is not a Kubernetes quantity of \
                     {resource}: {why}"
                )),
                _ => {}
            }
        }
        problems
    }
}

/// The name of the one Job an attempt gets (ADR_0029).
///
/// Derived from the attempt's key, so a second launcher asking for the same
/// Job is told it already exists: Kubernetes' name uniqueness is the
/// compare-and-set, and no lease is needed. A retry is a new attempt number,
/// so it gets a new name and a new Job. Hashed rather than spelled out,
/// because a Job's name is also a label on its pods — at most 63 characters of
/// `[a-z0-9-]` — and an execution id and a step id are neither. The attempt
/// itself rides on the Job as an annotation, for whoever reads `kubectl`.
#[must_use]
pub fn job_name(key: &AttemptKey) -> String {
    let digest = crate::digest(key.idempotency_key().as_bytes());
    format!("aiwatcher-{}", &digest[..32])
}

/// Why a plan's pods may not run under `templates`, every step at once.
///
/// `None` is a deployment that configured no templates, and every step asking
/// for a pod is refused naming the variable — a definition this deployment
/// could never launch is not one to store.
#[must_use]
pub fn refusals(plan: &ExecutionPlan, templates: Option<&PodTemplates>) -> Vec<String> {
    plan.steps
        .iter()
        .filter_map(|step| match &step.runtime {
            RuntimeBinding::ContainerJob(spec) => Some((step.id.as_str(), &spec.pod)),
            _ => None,
        })
        .flat_map(|(step, pod)| match templates {
            Some(templates) => templates.refusals(step, pod),
            None => vec![format!(
                "{step}: asks for a pod from template '{}', and this deployment configures no pod \
                 templates (AIWATCHER_POD_TEMPLATES)",
                pod.template
            )],
        })
        .collect()
}

/// A DNS label: what a Kubernetes name or label value can hold.
fn is_label(value: &str) -> bool {
    (1..=63).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

/// `host[:port]`.
fn is_host(value: &str) -> bool {
    let (host, port) = value.split_once(':').unwrap_or((value, "1"));
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|b| b.is_ascii_digit())
}

/// `[a-z0-9]+` runs joined by `.`, `_`, `__` or a run of dashes.
fn is_path_component(value: &str) -> bool {
    let bytes = value.as_bytes();
    let alnum = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if bytes.is_empty() || !bytes.first().is_some_and(alnum) || !bytes.last().is_some_and(alnum) {
        return false;
    }
    let mut separator = String::new();
    for &b in bytes {
        if alnum(&b) {
            let fine = separator.is_empty()
                || matches!(separator.as_str(), "." | "_" | "__")
                || separator.bytes().all(|s| s == b'-');
            if !fine {
                return false;
            }
            separator.clear();
        } else if matches!(b, b'.' | b'_' | b'-') {
            separator.push(char::from(b));
        } else {
            return false;
        }
    }
    true
}

fn is_tag(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// `algorithm:encoded`, OCI's grammar with the encoded half at least 32 long.
fn is_digest(value: &str) -> bool {
    let Some((algorithm, encoded)) = value.split_once(':') else {
        return false;
    };
    !algorithm.is_empty()
        && algorithm.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'+' | b'.' | b'_' | b'-')
        })
        && encoded.len() >= 32
        && encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'=' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn templates() -> PodTemplates {
        PodTemplates::parse(
            json!({
                "planner-import": {
                    "images": ["ghcr.io/planner/import", "python"],
                    "resources": {
                        "requests": {"cpu": "250m", "memory": "512Mi"},
                        "limits": {"cpu": "1", "memory": "1Gi"},
                        "max": {"cpu": "2", "memory": "4Gi"}
                    },
                    "command": ["python", "-m", "aiwatcher_sdk.worker", "run-attempt"],
                    "pod": {
                        "serviceAccountName": "planner-import",
                        "containers": [{
                            "name": "step",
                            "envFrom": [{"secretRef": {"name": "planner-import-token"}}]
                        }]
                    }
                }
            })
            .to_string()
            .as_bytes(),
        )
        .expect("a template the chart's example would render")
    }

    fn asking(image: &str) -> PodRequest {
        PodRequest {
            template: "planner-import".to_owned(),
            image: image.to_owned(),
            cpu: None,
            memory: None,
        }
    }

    #[test]
    fn one_attempt_names_one_job_and_its_retry_names_another() {
        use crate::state::ExecutionId;
        let first = AttemptKey::new(ExecutionId::new("exec-1"), "Stage_One/x", 1);
        let name = job_name(&first);
        assert_eq!(
            name,
            job_name(&first.clone()),
            "derived, so a second launcher asks for the same Job"
        );
        assert_ne!(
            name,
            job_name(&AttemptKey::new(
                ExecutionId::new("exec-1"),
                "Stage_One/x",
                2
            )),
            "a retry is a new attempt, a new name and a new Job"
        );
        // Whatever the ids hold, the name is a label a pod can carry.
        assert!(is_label(&name), "{name}");
    }

    #[test]
    fn an_image_is_compared_by_its_repository_and_its_tag_or_digest_is_free() {
        let templates = templates();
        for allowed in [
            "ghcr.io/planner/import:1.4",
            "ghcr.io/planner/import",
            "ghcr.io/planner/import@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            // Docker's defaults, written out: one image, three spellings.
            "python:3.13",
            "docker.io/library/python:3.13",
            "index.docker.io/library/python",
        ] {
            assert!(
                templates.refusals("import", &asking(allowed)).is_empty(),
                "{allowed} is on the list"
            );
        }
    }

    #[test]
    fn a_repository_that_only_begins_with_an_allowed_one_is_not_allowed() {
        // The corpus-name rule, for images: a prefix match would be a
        // permission invented by spelling.
        let templates = templates();
        for refused in [
            "ghcr.io/planner/import-debug:1",
            "ghcr.io/planner/import/sidecar:1",
            "ghcr.io/planner:1",
            "docker.io/planner/import:1",
            "pythonista:1",
        ] {
            let problems = templates.refusals("import", &asking(refused));
            assert_eq!(problems.len(), 1, "{refused}: {problems:?}");
            assert!(
                problems[0].contains(refused) && problems[0].contains("planner-import"),
                "names the value and the template: {problems:?}"
            );
        }
    }

    #[test]
    fn an_unknown_template_is_refused_naming_the_ones_there_are() {
        let problems = templates().refusals(
            "import",
            &PodRequest {
                template: "scratch".to_owned(),
                ..asking("python:3.13")
            },
        );
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("'scratch'") && problems[0].contains("planner-import"));
    }

    #[test]
    fn a_step_may_ask_for_up_to_the_ceiling_and_is_refused_above_it() {
        let templates = templates();
        let at_the_ceiling = PodRequest {
            cpu: Some("2000m".to_owned()),
            memory: Some("4Gi".to_owned()),
            ..asking("python:3.13")
        };
        assert!(templates.refusals("import", &at_the_ceiling).is_empty());

        let above = PodRequest {
            cpu: Some("2.5".to_owned()),
            memory: Some("5G".to_owned()),
            ..asking("ghcr.io/planner/import-debug:1")
        };
        let problems = templates.refusals("import", &above);
        // Every problem at once: the image, the CPU and the memory.
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(
            problems
                .iter()
                .any(|p| p.contains("cpu '2.5'") && p.contains("ceiling of 2"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("memory '5G'") && p.contains("ceiling of 4Gi"))
        );
    }

    #[test]
    fn what_a_step_asks_for_is_its_request_and_its_limit_and_the_rest_is_the_templates() {
        let templates = templates();
        let template = templates.get("planner-import").expect("the template");
        let resources = template.resources_for(&PodRequest {
            cpu: Some("2".to_owned()),
            ..asking("python:3.13")
        });
        assert_eq!(resources.requests.cpu.as_deref(), Some("2"));
        assert_eq!(resources.limits.cpu.as_deref(), Some("2"));
        assert_eq!(resources.requests.memory.as_deref(), Some("512Mi"));
        assert_eq!(resources.limits.memory.as_deref(), Some("1Gi"));
    }

    #[test]
    fn quantities_are_compared_by_value_whatever_their_unit() {
        assert_eq!(Resource::Cpu.parse("500m"), Ok(500));
        assert_eq!(Resource::Cpu.parse("0.5"), Ok(500));
        assert_eq!(Resource::Cpu.parse("2"), Ok(2_000));
        assert_eq!(Resource::Memory.parse("1Gi"), Ok(1 << 30));
        assert_eq!(Resource::Memory.parse("1.5Gi"), Ok(3 << 29));
        assert_eq!(Resource::Memory.parse("1G"), Ok(1_000_000_000));
        // Rounded up, as Kubernetes rounds a fraction of a millicore.
        assert_eq!(Resource::Cpu.parse("0.0001"), Ok(1));
        for refused in ["", "0", "m", "2 cores", "1Gb", "1e3", "1.", "1..5", "500mi"] {
            assert!(Resource::Cpu.parse(refused).is_err(), "cpu {refused:?}");
        }
        for refused in ["", "0", "1Gb", "1gi", "100m"] {
            assert!(
                Resource::Memory.parse(refused).is_err(),
                "memory {refused:?}"
            );
        }
    }

    #[test]
    fn an_image_reference_parses_into_what_a_list_compares() {
        let image = ImageRef::parse("localhost:5000/planner/import:1.4").expect("parses");
        assert_eq!(image.registry, "localhost:5000");
        assert_eq!(image.repository, "planner/import");
        assert_eq!(image.tag.as_deref(), Some("1.4"));
        assert_eq!(
            ImageRef::parse("python").expect("parses").name(),
            "docker.io/library/python"
        );
        for refused in [
            "",
            "ghcr.io/Planner/import",
            "ghcr.io/planner/import:",
            "ghcr.io/planner//import",
            "ghcr.io/planner/import@sha256:short",
            "ghcr.io/planner/import:1 ",
            "ghcr.io/planner/-import",
        ] {
            assert!(ImageRef::parse(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn a_template_setting_what_aiwatcher_fills_in_is_refused_naming_the_field() {
        let error = PodTemplates::parse(
            json!({
                "scratch": {
                    "images": ["ghcr.io/planner/import:1.4"],
                    "resources": {"requests": {"cpu": "4"}, "max": {"cpu": "2"}},
                    "command": ["worker"],
                    "pod": {
                        "restartPolicy": "OnFailure",
                        "containers": [{
                            "image": "ghcr.io/planner/import:1.4",
                            "command": ["sh"],
                            "env": [{"name": "AIWATCHER_ATTEMPT", "value": "x/y/1"}]
                        }]
                    }
                }
            })
            .to_string()
            .as_bytes(),
        )
        .expect_err("refused");
        let PodTemplatesError::Refused(problems) = error else {
            panic!("a shape serde reads: {error}");
        };
        for named in [
            "images entry 'ghcr.io/planner/import:1.4' names a tag",
            "pod.restartPolicy",
            "pod.containers[0].image",
            "pod.containers[0].command",
            "AIWATCHER_ATTEMPT",
            "resources.max.memory is required",
            "resources.requests.cpu is above resources.max.cpu",
        ] {
            assert!(
                problems.iter().any(|problem| problem.contains(named)),
                "{named} not named: {problems:?}"
            );
        }
        assert!(
            problems
                .iter()
                .all(|problem| problem.starts_with("scratch: "))
        );
    }

    #[test]
    fn a_template_with_two_containers_or_a_mistyped_field_does_not_load() {
        let two = PodTemplates::parse(
            json!({"scratch": {
                "images": ["python"], "command": ["worker"],
                "resources": {"max": {"cpu": "1", "memory": "1Gi"}},
                "pod": {"containers": [{}, {}]}
            }})
            .to_string()
            .as_bytes(),
        )
        .expect_err("two containers");
        assert!(two.to_string().contains("exactly one container"), "{two}");

        let mistyped = PodTemplates::parse(
            json!({"scratch": {"image": ["python"], "command": ["worker"]}})
                .to_string()
                .as_bytes(),
        )
        .expect_err("image for images");
        assert!(
            matches!(&mistyped, PodTemplatesError::Malformed(why) if why.contains("image")),
            "{mistyped}"
        );
    }

    #[test]
    fn a_step_asking_for_a_pod_where_no_template_is_configured_is_refused_naming_the_variable() {
        let plan = crate::definition::WorkflowSpec {
            name: "house/import".to_owned(),
            version: "1".to_owned(),
            steps: vec![
                serde_json::from_value(json!({
                    "id": "acquire", "task_ref": "acquire@1", "queue": "planner",
                    "timeout_seconds": 60,
                    "pod": {"template": "planner-import", "image": "python:3.13"}
                }))
                .expect("a step"),
            ],
        }
        .compile()
        .expect("compiles: the request is well formed");

        let problems = refusals(&plan, None);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].contains("AIWATCHER_POD_TEMPLATES"),
            "{problems:?}"
        );
        assert!(refusals(&plan, Some(&templates())).is_empty());
    }
}
