//! The one Job an attempt gets, as a pure function of what it is for
//! (ADR_0029).
//!
//! JSON rather than `k8s-openapi`'s types, so it is built and tested in every
//! build: whether a manifest says `backoffLimit: 0` is a question about this
//! file, and asking it should not need the `kube` feature. The cluster client
//! reads it back into a typed Job before it sends it.
//!
//! What is aiwatcher's here is the container's image, command, resources and
//! the attempt's environment, the pod's `restartPolicy` and the Job's own
//! fields. Everything else in the pod is the operator's template, copied as
//! written — and the template was refused at start if it set any of the
//! former, so nothing here overwrites a choice somebody made.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use aiwatcher_execution::pods::{PodRequest, PodTemplate, ResourceValues, job_name};
use aiwatcher_execution::{AttemptKey, ExecutionId};

/// How long a finished Job stays when no launcher is left to delete it.
///
/// A backstop, not the clean-up: once a Job's log is kept, the launcher
/// deletes it (2.4). An hour is long enough to read a pod that failed and
/// short enough that a namespace does not fill with them.
pub const FINISHED_JOB_TTL_SECONDS: u64 = 3_600;

/// The labels every launched Job and pod carries — what a NetworkPolicy
/// admits them to the API by, and what `kubectl get -l` finds them by.
pub const LABELS: [(&str, &str); 2] = [
    ("app.kubernetes.io/managed-by", "aiwatcher"),
    ("app.kubernetes.io/component", "step"),
];

/// Which template a Job was started from.
pub const TEMPLATE_LABEL: &str = "aiwatcher.dev/template";

/// Which attempt a Job is for — annotations rather than labels, because an
/// execution id and a step id are not label values.
///
/// Three of them rather than one `<execution>/<step>/<attempt>`, because the
/// watch reads this back off the cluster to say which attempt a dead pod held
/// (ADR_0029). An execution id comes from a request and a step id from a
/// canvas, and neither is checked against a path grammar anywhere: the joined
/// key is a string to write and not one to parse. An annotation's value is
/// arbitrary text, so each part rides whole.
pub const EXECUTION_ANNOTATION: &str = "aiwatcher.dev/execution";
pub const STEP_ANNOTATION: &str = "aiwatcher.dev/step";
pub const ATTEMPT_ANNOTATION: &str = "aiwatcher.dev/attempt";

/// The container's name when the template's pod names none.
const CONTAINER: &str = "step";

/// Everything one Job is built from.
#[derive(Clone, Copy, Debug)]
pub struct JobRequest<'a> {
    pub key: &'a AttemptKey,
    pub template_name: &'a str,
    pub template: &'a PodTemplate,
    pub pod: &'a PodRequest,
    /// The step's own timeout. The Job's deadline is this plus the template's
    /// start allowance, so a pod that took its whole allowance to start still
    /// gets the step's full time.
    pub timeout_seconds: u64,
    /// Where the pod reports: what its worker is told in `AIWATCHER_URL`.
    pub api_url: &'a str,
}

/// The `batch/v1` Job for one attempt.
#[must_use]
pub fn job(request: &JobRequest<'_>) -> Value {
    let mut labels = Map::new();
    for (name, value) in LABELS {
        labels.insert(name.to_owned(), value.into());
    }
    labels.insert(TEMPLATE_LABEL.to_owned(), request.template_name.into());
    let annotations = annotations(request.key);

    json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {
            "name": job_name(request.key),
            "labels": labels,
            "annotations": annotations,
        },
        "spec": {
            // The engine owns a pod's retries, as it owns every step's: a
            // Kubernetes retry would be a second attempt the store never
            // dispatched, running under the first one's key.
            "backoffLimit": 0,
            "activeDeadlineSeconds": request
                .template
                .start_allowance_seconds
                .saturating_add(request.timeout_seconds),
            "ttlSecondsAfterFinished": FINISHED_JOB_TTL_SECONDS,
            "template": {
                "metadata": { "labels": labels, "annotations": annotations },
                "spec": pod_spec(request),
            },
        },
    })
}

/// The three annotations one attempt is named by, and the only place that
/// decides how a Job carries it. [`attempt_of`] is the inverse, beside it.
fn annotations(key: &AttemptKey) -> Value {
    json!({
        EXECUTION_ANNOTATION: key.execution_id.as_str(),
        STEP_ANNOTATION: key.step_id,
        ATTEMPT_ANNOTATION: key.attempt.to_string(),
    })
}

/// The attempt a Job's annotations name.
///
/// `None` for a Job that does not carry all three — something else's, whatever
/// label selector found it — which the watch skips rather than guesses at.
#[must_use]
pub fn attempt_of(annotations: &BTreeMap<String, String>) -> Option<AttemptKey> {
    Some(AttemptKey::new(
        ExecutionId::new(annotations.get(EXECUTION_ANNOTATION)?.clone()),
        annotations.get(STEP_ANNOTATION)?.clone(),
        annotations.get(ATTEMPT_ANNOTATION)?.parse().ok()?,
    ))
}

/// The operator's pod, with aiwatcher's fields filled in.
fn pod_spec(request: &JobRequest<'_>) -> Value {
    let mut spec = request.template.pod.clone();
    let mut container = spec
        .remove("containers")
        .and_then(|containers| containers.as_array()?.first()?.as_object().cloned())
        .unwrap_or_default();
    container.entry("name").or_insert_with(|| CONTAINER.into());
    container.insert("image".to_owned(), request.pod.image.clone().into());
    container.insert("command".to_owned(), json!(request.template.command));

    let resources = request.template.resources_for(request.pod);
    let mut given = Map::new();
    for (section, values) in [
        ("requests", &resources.requests),
        ("limits", &resources.limits),
    ] {
        if let Some(quantities) = quantities(values) {
            given.insert(section.to_owned(), quantities);
        }
    }
    if !given.is_empty() {
        container.insert("resources".to_owned(), Value::Object(given));
    }

    // Aiwatcher's first, then the operator's — which may not name these three
    // (the template was refused if it did), and may refer to them.
    let mut env = vec![
        json!({ "name": "AIWATCHER_ATTEMPT", "value": request.key.idempotency_key() }),
        json!({ "name": "AIWATCHER_URL", "value": request.api_url }),
        // The pod's own name, from the downward API: unique per pod, and the
        // name its claim is held under and a person finds it by.
        json!({
            "name": "AIWATCHER_WORKER_NAME",
            "valueFrom": { "fieldRef": { "fieldPath": "metadata.name" } },
        }),
    ];
    if let Some(Value::Array(theirs)) = container.remove("env") {
        env.extend(theirs);
    }
    container.insert("env".to_owned(), Value::Array(env));

    spec.insert("containers".to_owned(), json!([container]));
    spec.insert("restartPolicy".to_owned(), "Never".into());
    // No token unless the template asks for one: whatever runs in the pod can
    // read what the pod holds, and a step's code has no business with the
    // cluster's API.
    spec.entry("automountServiceAccountToken")
        .or_insert(Value::Bool(false));
    Value::Object(spec)
}

fn quantities(values: &ResourceValues) -> Option<Value> {
    let mut map = Map::new();
    if let Some(cpu) = &values.cpu {
        map.insert("cpu".to_owned(), cpu.clone().into());
    }
    if let Some(memory) = &values.memory {
        map.insert("memory".to_owned(), memory.clone().into());
    }
    (!map.is_empty()).then_some(Value::Object(map))
}

#[cfg(test)]
mod tests {
    use super::*;

    use aiwatcher_execution::ExecutionId;
    use aiwatcher_execution::pods::{OWNED_ENV, PodTemplates};

    fn template(pod: Value) -> PodTemplate {
        let templates = PodTemplates::parse(
            json!({
                "planner-import": {
                    "images": ["ghcr.io/planner/import"],
                    "resources": {
                        "requests": {"cpu": "250m", "memory": "512Mi"},
                        "limits": {"cpu": "1", "memory": "1Gi"},
                        "max": {"cpu": "2", "memory": "4Gi"}
                    },
                    "command": ["python", "-m", "aiwatcher_sdk.worker", "run-attempt",
                                "--queue", "planner-import", "--task", "planner.stages:parse"],
                    "start_allowance_seconds": 120,
                    "pod": pod
                }
            })
            .to_string()
            .as_bytes(),
        )
        .expect("a template the chart's example renders");
        templates
            .get("planner-import")
            .cloned()
            .expect("the template")
    }

    fn planner_pod() -> Value {
        json!({
            "serviceAccountName": "planner-import",
            "nodeSelector": {"workload": "import"},
            "volumes": [{"name": "scratch", "emptyDir": {}}],
            "containers": [{
                "name": "stage",
                "envFrom": [{"secretRef": {"name": "planner-import-token"}}],
                "env": [{"name": "PLANNER_MODE", "value": "import"}],
                "volumeMounts": [{"name": "scratch", "mountPath": "/scratch"}]
            }]
        })
    }

    fn key() -> AttemptKey {
        AttemptKey::new(ExecutionId::new("import-7"), "parse", 2)
    }

    fn built(template: &PodTemplate, cpu: Option<&str>) -> Value {
        let pod = PodRequest {
            template: "planner-import".to_owned(),
            image: "ghcr.io/planner/import:1.4".to_owned(),
            cpu: cpu.map(str::to_owned),
            memory: None,
        };
        job(&JobRequest {
            key: &key(),
            template_name: "planner-import",
            template,
            pod: &pod,
            timeout_seconds: 900,
            api_url: "http://aiwatcher-server:8080",
        })
    }

    #[test]
    fn kubernetes_never_retries_or_restarts_a_launched_pod() {
        // The engine owns a pod's retries: a Kubernetes retry would be a
        // second attempt nobody dispatched, running under the first one's key.
        let job = built(&template(planner_pod()), None);
        assert_eq!(job["metadata"]["name"], job_name(&key()));
        assert_eq!(job["spec"]["backoffLimit"], 0);
        assert_eq!(job["spec"]["template"]["spec"]["restartPolicy"], "Never");
        assert_eq!(
            job["spec"]["activeDeadlineSeconds"], 1_020,
            "the start allowance plus the step's own timeout"
        );
        assert_eq!(
            job["spec"]["ttlSecondsAfterFinished"],
            FINISHED_JOB_TTL_SECONDS
        );
    }

    #[test]
    fn the_container_runs_the_steps_image_under_the_templates_command_and_resources() {
        let template = template(planner_pod());
        let job = built(&template, Some("1500m"));
        let container = &job["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["name"], "stage", "the operator's name is kept");
        assert_eq!(container["image"], "ghcr.io/planner/import:1.4");
        assert_eq!(container["command"], json!(template.command));
        // What the step asked for is its request and its limit; what it left
        // out is the template's.
        assert_eq!(
            container["resources"],
            json!({
                "requests": {"cpu": "1500m", "memory": "512Mi"},
                "limits": {"cpu": "1500m", "memory": "1Gi"}
            })
        );
    }

    #[test]
    fn the_pod_is_told_its_attempt_where_the_api_is_and_its_own_name() {
        let job = built(&template(planner_pod()), None);
        let container = &job["spec"]["template"]["spec"]["containers"][0];
        let env = container["env"].as_array().expect("env");
        let names: Vec<&str> = env
            .iter()
            .filter_map(|entry| entry["name"].as_str())
            .collect();
        assert_eq!(&names[..3], &OWNED_ENV[..], "aiwatcher's three, first");
        assert_eq!(env[0]["value"], "import-7/parse/2");
        assert_eq!(env[1]["value"], "http://aiwatcher-server:8080");
        assert_eq!(
            env[2]["valueFrom"]["fieldRef"]["fieldPath"],
            "metadata.name"
        );
        // The operator's are kept after them: the token, and their own.
        assert_eq!(names[3], "PLANNER_MODE");
        assert_eq!(
            container["envFrom"][0]["secretRef"]["name"],
            "planner-import-token"
        );
    }

    #[test]
    fn everything_else_in_the_pod_is_the_operators_as_written() {
        let job = built(&template(planner_pod()), None);
        let spec = &job["spec"]["template"]["spec"];
        assert_eq!(spec["serviceAccountName"], "planner-import");
        assert_eq!(spec["nodeSelector"]["workload"], "import");
        assert_eq!(spec["volumes"][0]["name"], "scratch");
        assert_eq!(
            spec["containers"][0]["volumeMounts"][0]["mountPath"],
            "/scratch"
        );
        assert_eq!(
            spec["automountServiceAccountToken"], false,
            "no cluster token unless the template asks for one"
        );

        let asks = template(json!({"automountServiceAccountToken": true}));
        assert_eq!(
            built(&asks, None)["spec"]["template"]["spec"]["automountServiceAccountToken"],
            true
        );
    }

    #[test]
    fn a_template_whose_pod_names_no_container_gets_one() {
        let job = built(&template(json!({})), None);
        let containers = job["spec"]["template"]["spec"]["containers"]
            .as_array()
            .expect("containers");
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0]["name"], CONTAINER);
    }

    #[test]
    fn a_job_and_its_pod_say_what_they_are_for() {
        let job = built(&template(planner_pod()), None);
        for metadata in [&job["metadata"], &job["spec"]["template"]["metadata"]] {
            for (name, value) in LABELS {
                assert_eq!(metadata["labels"][name], value);
            }
            assert_eq!(metadata["labels"][TEMPLATE_LABEL], "planner-import");
            assert_eq!(metadata["annotations"][EXECUTION_ANNOTATION], "import-7");
            assert_eq!(metadata["annotations"][STEP_ANNOTATION], "parse");
            assert_eq!(metadata["annotations"][ATTEMPT_ANNOTATION], "2");
        }
    }

    #[test]
    fn the_attempt_a_job_is_for_is_read_back_off_its_annotations() {
        let job = built(&template(planner_pod()), None);
        let carried: BTreeMap<String, String> = job["metadata"]["annotations"]
            .as_object()
            .expect("annotations")
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.as_str().expect("an annotation is text").to_owned(),
                )
            })
            .collect();
        assert_eq!(attempt_of(&carried), Some(key()));

        // A step id with a separator in it survives the round trip, which is
        // the reason the key is not one annotation.
        let slashed = AttemptKey::new(ExecutionId::new("import/7"), "parse/rows", 2);
        let written: BTreeMap<String, String> =
            serde_json::from_value(annotations(&slashed)).expect("annotations are a map of text");
        assert_eq!(attempt_of(&written), Some(slashed));

        // And a Job that carries none of this is nobody's attempt.
        assert_eq!(attempt_of(&BTreeMap::new()), None);
    }
}
