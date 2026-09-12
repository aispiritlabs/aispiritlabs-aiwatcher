//! The third cluster: this machine's container engine, one container per
//! attempt.
//!
//! Between the other two, and the reason it exists is what each of them cannot
//! do. A cluster runs the step's **image** under the template's **limits** and
//! needs a cluster; a process ([`super::process`]) needs nothing and runs
//! neither. A container runs both, on a machine that has Docker and no
//! Kubernetes — which is most machines this is developed on, and a deployment
//! small enough to be one host.
//!
//! It keeps no state of its own, which the process backend cannot: the engine
//! **is** the record, the way the cluster is. A container is found again by the
//! labels the manifest carries, so a launcher that restarted still reads back
//! what it started, and a container outlives the server that asked for it.
//!
//! What it cannot keep: a node selector, a service account, a `Secret` or a
//! volume — everything about a pod that is a *cluster's* answer rather than a
//! container runtime's. The first two are ignored, the last two refused with
//! the rest of what a program would read ([`manifest::program`]).
//!
//! Why this drives the command line rather than the engine's socket, and why
//! swap is pinned to the memory limit, are in ADR_0029's amendments with the
//! rest of what a backend that is not a cluster may and may not do.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::process::Stdio;

use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::process::Command;

use aiwatcher_execution::pods::Resource;

use super::log::{Kept, Tail, bounded};
use super::manifest;
use super::{Cluster, ClusterError, Created, Observed, Phase};

/// The client this drives. Resolved on `PATH`, so an engine whose client is
/// Docker-compatible is a symlink away.
const ENGINE: &str = "docker";

/// The filter every container is found again by — the manifest's own label,
/// written the way this engine takes one, so a selector that had drifted from
/// the manifest would quietly watch nothing.
///
/// `label=` is part of it: without the prefix the engine refuses the filter
/// rather than matching everything, which is a listing that fails every pass
/// while the launches go on working.
fn selector() -> String {
    let (name, value) = manifest::LABELS[0];
    format!("label={name}={value}")
}

/// This machine's container engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct DockerCluster;

impl DockerCluster {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// What the engine says it is, or why it could not be asked.
    ///
    /// Asked once at start-up so an absent client or a daemon that is not
    /// running is one line naming it, rather than a warning every pass about a
    /// call that never had a chance.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Unavailable`] when the engine could not be reached.
    pub async fn version() -> Result<String, ClusterError> {
        Ok(engine(&["version", "--format", "{{.Server.Version}}"])
            .await?
            .trim()
            .to_owned())
    }
}

#[async_trait]
impl Cluster for DockerCluster {
    async fn create_job(&self, manifest: &Value) -> Result<Created, ClusterError> {
        let name = manifest
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .ok_or_else(|| ClusterError::Refused("the manifest has no name".to_owned()))?;
        if manifest::attempt_of(&manifest::annotations_of(manifest)).is_none() {
            return Err(ClusterError::Refused(
                "the manifest does not say which attempt it is for".to_owned(),
            ));
        }
        let arguments = run_arguments(manifest, name).map_err(ClusterError::Refused)?;
        match engine(&arguments.iter().map(String::as_str).collect::<Vec<_>>()).await {
            Ok(_) => Ok(Created::New),
            Err(ClusterError::Refused(why)) if why.contains("is already in use") => {
                // The name is derived from the attempt, so somebody asked
                // first: another launcher, or this one before a restart.
                Ok(Created::AlreadyExisted)
            }
            Err(error) => Err(error),
        }
    }

    async fn jobs(&self) -> Result<Vec<Observed>, ClusterError> {
        let listed = engine(&["ps", "--all", "--quiet", "--filter", &selector()]).await?;
        let ids: Vec<&str> = listed.split_whitespace().collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // One inspect for all of them: a fan-out is many containers of one run,
        // and a call per container per pass is what the cluster backend
        // deliberately does not do either.
        let mut arguments = vec!["inspect"];
        arguments.extend_from_slice(&ids);
        let inspected: Value = serde_json::from_str(&engine(&arguments).await?)
            .map_err(|error| ClusterError::Unavailable(format!("inspect said: {error}")))?;
        Ok(inspected
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(observed)
            .collect())
    }

    async fn log(&self, job: &str, at_most: usize) -> Result<Option<Kept>, ClusterError> {
        // Both streams in the engine's own order, which is what `docker logs`
        // writes to two of them and the catalog keeps as one. The whole log is
        // read and the end of it kept: a bound in lines bounds nothing, since
        // one line of a structured logger can be megabytes.
        let printed = match shell(r#"exec "$1" logs "$2" 2>&1"#, &[ENGINE, job]).await {
            Ok(printed) => printed,
            // No container to read: it was never created, or somebody removed
            // it. An absence rather than a failure.
            Err(ClusterError::Refused(why)) if missing(&why) => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut tail = Tail::new(at_most);
        tail.push(printed.as_bytes());
        Ok(Some(bounded(tail.finish(), at_most)))
    }

    async fn delete(&self, job: &str) -> Result<(), ClusterError> {
        // Forced, because a delete is asked for once the attempt has ended and
        // a cancel wants the container stopped now; with its volumes, because
        // an anonymous volume left behind is a disk nobody is watching.
        match engine(&["rm", "--force", "--volumes", job]).await {
            Ok(_) => Ok(()),
            // Somebody got there first, which is the same outcome.
            Err(ClusterError::Refused(why)) if missing(&why) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// One container, as the engine describes it.
///
/// `None` for anything the label selector reached that does not say which
/// attempt it is for — skipped rather than guessed at, as on a cluster.
fn observed(container: &Value) -> Option<Observed> {
    let labels = container
        .pointer("/Config/Labels")
        .and_then(Value::as_object)
        .map(|written| {
            written
                .iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    Some(Observed {
        // Named as the engine gives it, which is the name with its leading
        // slash taken off — that is what every other call takes.
        name: container
            .get("Name")
            .and_then(Value::as_str)?
            .trim_start_matches('/')
            .to_owned(),
        key: manifest::attempt_of(&labels)?,
        template: labels.get(manifest::TEMPLATE_LABEL).cloned(),
        created_at: created_at(container),
        pod: phase_of(container.get("State")),
    })
}

/// When the container was created, or now for one the engine did not date.
///
/// Now rather than the epoch, for the reason the cluster backend gives: this is
/// the clock a start allowance runs on, and the epoch reads as overdue the
/// moment it is seen.
fn created_at(container: &Value) -> OffsetDateTime {
    container
        .get("Created")
        .and_then(Value::as_str)
        .and_then(|at| OffsetDateTime::parse(at, &Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc)
}

/// How far one container got.
///
/// The engine answers in one object what a cluster answers in two, and the
/// precedence is the same: whether it ended, and then why.
fn phase_of(state: Option<&Value>) -> Phase {
    let Some(state) = state else {
        return Phase::Live { reason: None };
    };
    let status = state.get("Status").and_then(Value::as_str).unwrap_or("");
    let code = state.get("ExitCode").and_then(Value::as_i64).unwrap_or(0);
    match status {
        "exited" | "dead" => Phase::Ended {
            reason: ended_reason(state, code),
        },
        // Created and not started is this engine's `Pending`: the launcher's
        // start allowance is what decides about one that stays there.
        "created" => Phase::Live {
            reason: Some("the container is created and has not started".to_owned()),
        },
        "paused" | "restarting" | "removing" => Phase::Live {
            reason: Some(format!("the container is {status}")),
        },
        _ => Phase::Live { reason: None },
    }
}

/// The container's own word for how it ended.
///
/// `OOMKilled` is the engine's own flag and the word is the cluster's, so an
/// attempt that went over its limit reads the same on both backends — which is
/// the point of running the image under the template's limits at all.
fn ended_reason(state: &Value, code: i64) -> String {
    if state
        .get("OOMKilled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return format!("OOMKilled (exit {code})");
    }
    if let Some(error) = state
        .get("Error")
        .and_then(Value::as_str)
        .filter(|error| !error.is_empty())
    {
        return format!("{error} (exit {code})");
    }
    if code == 0 {
        return "Completed".to_owned();
    }
    format!("Error (exit {code})")
}

/// Everything `docker run` is told for one attempt.
///
/// A pure function of the manifest, so what a container is started with is a
/// question about this file and is asked in every build — the shape
/// [`manifest::job`] already has, one layer down.
///
/// # Errors
///
/// Why this manifest cannot be run in a container here, in the words the
/// attempt ends with.
pub fn run_arguments(manifest: &Value, name: &str) -> Result<Vec<String>, String> {
    let program = manifest::program(manifest, name)?;
    let container = manifest
        .pointer("/spec/template/spec/containers/0")
        .ok_or_else(|| "the pod spec has no container".to_owned())?;
    let image = container
        .get("image")
        .and_then(Value::as_str)
        .ok_or_else(|| "the container names no image".to_owned())?;

    let mut arguments = vec![
        "run".to_owned(),
        "--detach".to_owned(),
        "--name".to_owned(),
        name.to_owned(),
        // The engine owns no retries here: the store owns every step's, so a
        // restart would be an attempt nothing dispatched, under the first
        // one's key. `backoffLimit: 0`, in this engine's words.
        "--restart=no".to_owned(),
        format!("--pull={}", pull(container)),
    ];
    for written in [
        manifest::labels_of(manifest),
        manifest::annotations_of(manifest),
    ] {
        for (label, value) in written {
            arguments.push("--label".to_owned());
            arguments.push(format!("{label}={value}"));
        }
    }
    for (variable, value) in program.environment {
        arguments.push("--env".to_owned());
        arguments.push(format!("{variable}={value}"));
    }
    if let Some(directory) = program.directory {
        arguments.push("--workdir".to_owned());
        arguments.push(directory);
    }
    arguments.extend(limits(container)?);
    arguments.push(image.to_owned());
    arguments.extend(program.command);
    Ok(arguments)
}

/// The container's limits, as the engine counts them.
///
/// The *limits* and not the requests: a request is a scheduler's word and this
/// engine schedules nothing, so a container that asked for half a core and may
/// have one gets the one it may have.
fn limits(container: &Value) -> Result<Vec<String>, String> {
    let mut given = Vec::new();
    if let Some(cpu) = container
        .pointer("/resources/limits/cpu")
        .and_then(Value::as_str)
    {
        let millicores = Resource::Cpu.parse(cpu)?;
        given.push(format!(
            "--cpus={}.{:03}",
            millicores / 1000,
            millicores % 1000
        ));
    }
    if let Some(memory) = container
        .pointer("/resources/limits/memory")
        .and_then(Value::as_str)
    {
        let bytes = Resource::Memory.parse(memory)?;
        given.push(format!("--memory={bytes}"));
        // Pinned to the limit: a pod has no swap, and this engine's default is
        // to grant as much again — under which a stage over its limit is slow
        // rather than stopped.
        given.push(format!("--memory-swap={bytes}"));
    }
    Ok(given)
}

/// What the container's `imagePullPolicy` means to this engine.
///
/// The template's field, honoured rather than ignored, because a locally built
/// image that was never pushed is exactly the case this backend is for — and
/// `missing` is what a cluster means by the default.
fn pull(container: &Value) -> &'static str {
    match container
        .get("imagePullPolicy")
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "Always" => "always",
        "Never" => "never",
        _ => "missing",
    }
}

/// Whether a refusal is the engine saying there is no such container.
fn missing(why: &str) -> bool {
    why.contains("No such container") || why.contains("no such container")
}

/// One call to the engine's client, or why it did not happen.
async fn engine(arguments: &[&str]) -> Result<String, ClusterError> {
    let mut command = Command::new(ENGINE);
    command.args(arguments);
    output(command, ENGINE).await
}

/// One call whose two streams have to arrive as one, which the client cannot be
/// asked for directly.
///
/// `script` names its arguments positionally, so nothing read off a manifest is
/// ever part of the text a shell parses.
async fn shell(script: &str, arguments: &[&str]) -> Result<String, ClusterError> {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script).arg("sh").args(arguments);
    output(command, "sh").await
}

async fn output(mut command: Command, program: &str) -> Result<String, ClusterError> {
    let outcome = command
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| match error.kind() {
            ErrorKind::NotFound => ClusterError::Unavailable(format!(
                "'{program}' is not on this host's PATH, so no container can be started"
            )),
            _ => ClusterError::Unavailable(format!("'{program}' could not be run: {error}")),
        })?;
    if outcome.status.success() {
        return Ok(String::from_utf8_lossy(&outcome.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&outcome.stderr).trim().to_owned();
    // An engine that could not be reached answers again on the next pass; one
    // that read the request and will not have it says the same thing every
    // time, until somebody edits the template.
    if said.contains("Cannot connect to the Docker daemon") || said.contains("daemon is not") {
        return Err(ClusterError::Unavailable(said));
    }
    Err(ClusterError::Refused(said))
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    use aiwatcher_execution::pods::{PodRequest, PodTemplates};
    use aiwatcher_execution::{AttemptKey, ExecutionId};

    use super::super::manifest::JobRequest;

    fn manifest_for(memory: Option<&str>, pod: Value) -> Value {
        let templates = PodTemplates::parse(
            json!({
                "e2e": {
                    "images": ["aiwatcher-stage"],
                    "resources": {
                        "requests": {"cpu": "50m", "memory": "96Mi"},
                        "limits": {"cpu": "1", "memory": "256Mi"},
                        "max": {"cpu": "2", "memory": "512Mi"},
                    },
                    "command": ["python", "-m", "aiwatcher_sdk.worker", "run-attempt"],
                    "pod": pod,
                }
            })
            .to_string()
            .as_bytes(),
        )
        .expect("a template");
        let template = templates.get("e2e").expect("the template");
        let key = AttemptKey::new(ExecutionId::new("run-1"), "analyze", 2);
        super::super::manifest::job(&JobRequest {
            key: &key,
            template_name: "e2e",
            template,
            pod: &PodRequest {
                template: "e2e".to_owned(),
                image: "aiwatcher-stage:e2e".to_owned(),
                cpu: None,
                memory: memory.map(str::to_owned),
            },
            timeout_seconds: 60,
            api_url: "http://host.docker.internal:8080",
        })
    }

    fn arguments(manifest: &Value) -> Vec<String> {
        run_arguments(manifest, "aiwatcher-abc").expect("an image this engine can run")
    }

    fn value_after(arguments: &[String], flag: &str) -> Option<String> {
        arguments
            .iter()
            .position(|argument| argument == flag)
            .and_then(|at| arguments.get(at + 1))
            .cloned()
    }

    #[test]
    fn a_container_runs_the_step_s_image_with_the_attempt_in_its_environment() {
        let arguments = arguments(&manifest_for(None, json!({})));
        assert_eq!(
            value_after(&arguments, "--name").as_deref(),
            Some("aiwatcher-abc")
        );
        // The image, then the command: what the engine's own argument order
        // means, and what the template and the step each decided.
        let image = arguments
            .iter()
            .position(|argument| argument == "aiwatcher-stage:e2e")
            .expect("the step's image");
        assert_eq!(
            &arguments[image + 1..],
            ["python", "-m", "aiwatcher_sdk.worker", "run-attempt"]
        );
        let environment: Vec<&String> = arguments
            .iter()
            .zip(arguments.iter().skip(1))
            .filter_map(|(flag, value)| (flag == "--env").then_some(value))
            .collect();
        assert!(
            environment.contains(&&"AIWATCHER_ATTEMPT=run-1/analyze/2".to_owned())
                && environment
                    .contains(&&"AIWATCHER_URL=http://host.docker.internal:8080".to_owned())
                // The downward API's own name, answered with the container's:
                // the name the claim is held under.
                && environment.contains(&&"AIWATCHER_WORKER_NAME=aiwatcher-abc".to_owned()),
            "{environment:?}"
        );
    }

    #[test]
    fn a_container_is_found_again_by_the_labels_the_manifest_carries() {
        let arguments = arguments(&manifest_for(None, json!({})));
        let labels: Vec<&String> = arguments
            .iter()
            .zip(arguments.iter().skip(1))
            .filter_map(|(flag, value)| (flag == "--label").then_some(value))
            .collect();
        for expected in [
            "app.kubernetes.io/managed-by=aiwatcher",
            "aiwatcher.dev/template=e2e",
            "aiwatcher.dev/execution=run-1",
            "aiwatcher.dev/step=analyze",
            "aiwatcher.dev/attempt=2",
        ] {
            assert!(
                labels.contains(&&expected.to_owned()),
                "{expected}: {labels:?}"
            );
        }
        // And the listing finds them by that label, in the engine's own
        // spelling: a filter with no `label=` is one the engine refuses.
        assert_eq!(selector(), "label=app.kubernetes.io/managed-by=aiwatcher");
    }

    #[test]
    fn a_step_s_memory_ask_is_a_ceiling_with_no_swap_under_it() {
        // Docker's own default grants as much swap again, under which a stage
        // over its limit is slow rather than stopped — and being stopped is
        // the whole reason a limit is set.
        let arguments = arguments(&manifest_for(Some("192Mi"), json!({})));
        assert!(
            arguments.contains(&"--memory=201326592".to_owned())
                && arguments.contains(&"--memory-swap=201326592".to_owned()),
            "{arguments:?}"
        );
        // The template's ceiling, since the step asked for no cpu.
        assert!(
            arguments.contains(&"--cpus=1.000".to_owned()),
            "{arguments:?}"
        );
        assert!(arguments.contains(&"--restart=no".to_owned()));
    }

    #[test]
    fn the_template_s_pull_policy_is_honoured_rather_than_ignored() {
        // A locally built image that was never pushed is the case this backend
        // is for, and `Always` would fail on it every time.
        for (policy, expected) in [
            ("IfNotPresent", "--pull=missing"),
            ("Always", "--pull=always"),
            ("Never", "--pull=never"),
        ] {
            let manifest = manifest_for(None, json!({"containers": [{"imagePullPolicy": policy}]}));
            assert!(
                arguments(&manifest).contains(&expected.to_owned()),
                "{policy}"
            );
        }
        // Absent is what a cluster means by the default.
        assert!(arguments(&manifest_for(None, json!({}))).contains(&"--pull=missing".to_owned()));
    }

    #[test]
    fn a_template_this_engine_cannot_honour_is_refused_naming_what() {
        for (pod, expected) in [
            (
                json!({"containers": [{"envFrom": [{"secretRef": {"name": "planner"}}]}]}),
                "envFrom",
            ),
            (
                json!({
                    "volumes": [{"name": "scratch", "emptyDir": {}}],
                    "containers": [{}],
                }),
                "volumes",
            ),
            (
                json!({"containers": [{"env": [{
                    "name": "PLANNER_TOKEN",
                    "valueFrom": {"secretKeyRef": {"name": "planner", "key": "token"}},
                }]}]}),
                "PLANNER_TOKEN",
            ),
        ] {
            let why = run_arguments(&manifest_for(None, pod), "aiwatcher-abc")
                .expect_err("this engine cannot honour it");
            assert!(why.contains(expected), "{why}");
        }
    }

    #[test]
    fn a_container_that_went_over_its_limit_says_so_in_the_cluster_s_own_words() {
        // What makes the memory phase of the gate the same assertion on both
        // backends: the engine's own flag, read as the cluster's word.
        let killed = json!({"Status": "exited", "ExitCode": 137, "OOMKilled": true, "Error": ""});
        assert_eq!(
            phase_of(Some(&killed)),
            Phase::Ended {
                reason: "OOMKilled (exit 137)".to_owned()
            }
        );
    }

    #[test]
    fn how_a_container_ended_is_read_the_way_a_pod_s_ending_is() {
        for (state, expected) in [
            (
                json!({"Status": "exited", "ExitCode": 0, "OOMKilled": false, "Error": ""}),
                Phase::Ended {
                    reason: "Completed".to_owned(),
                },
            ),
            (
                json!({"Status": "exited", "ExitCode": 3, "OOMKilled": false, "Error": ""}),
                Phase::Ended {
                    reason: "Error (exit 3)".to_owned(),
                },
            ),
            (
                json!({"Status": "dead", "ExitCode": 1, "Error": "no space left on device"}),
                Phase::Ended {
                    reason: "no space left on device (exit 1)".to_owned(),
                },
            ),
            (
                json!({"Status": "running", "ExitCode": 0}),
                Phase::Live { reason: None },
            ),
        ] {
            assert_eq!(phase_of(Some(&state)), expected, "{state}");
        }
        // Created and not started is this engine's `Pending`, and the launcher
        // reads the word: its start allowance is what ends one that stays.
        let Phase::Live { reason: Some(why) } =
            phase_of(Some(&json!({"Status": "created", "ExitCode": 0})))
        else {
            panic!("a created container has not ended");
        };
        assert!(why.contains("has not started"), "{why}");
    }

    #[test]
    fn a_container_names_the_attempt_it_was_started_for() {
        let inspected = json!({
            "Name": "/aiwatcher-abc",
            "Created": "2026-09-12T09:41:02.123456789Z",
            "Config": {"Labels": {
                "aiwatcher.dev/execution": "run-1",
                "aiwatcher.dev/step": "analyze",
                "aiwatcher.dev/attempt": "2",
                "aiwatcher.dev/template": "e2e",
            }},
            "State": {"Status": "running"},
        });
        let container = observed(&inspected).expect("a container of this release");
        assert_eq!(container.name, "aiwatcher-abc", "without the leading slash");
        assert_eq!(
            container.key,
            AttemptKey::new(ExecutionId::new("run-1"), "analyze", 2)
        );
        assert_eq!(container.template.as_deref(), Some("e2e"));
        assert_eq!(container.created_at.year(), 2026);

        // Something else the selector reached is skipped rather than guessed
        // at, exactly as on a cluster.
        assert!(
            observed(&json!({"Name": "/somebody-elses", "Config": {"Labels": {}}})).is_none(),
            "a container that does not say which attempt it is for"
        );
    }
}
