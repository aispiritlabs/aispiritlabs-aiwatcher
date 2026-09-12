//! The other cluster: this host, running each Job as a process of its own.
//!
//! One attempt is one process, started from the same manifest a real cluster
//! is sent — the container's command, its environment and its directory — so
//! the launcher, the derived name, the claim by key, the lease, the watch and
//! the log are the ones [`super`] already decided, and nothing about a plan
//! changes. What a `container_job` step *means* is where it runs, and a
//! deployment with no cluster can now mean this host.
//!
//! What it keeps of the Job contract: exactly one process per attempt, a start
//! allowance while it waits for a slot, the pod's own word for how it ended
//! and its last [`super::log::TAIL_BYTES`], and a delete that kills it.
//!
//! What it cannot keep, and says so rather than pretending: the image is
//! recorded and never run — the command is this host's — and so are the
//! resource requests and limits, the security context, the service account and
//! the node selector. A step over its memory limit is not killed here; it
//! takes the host's memory. Everything the *program* could read or
//! authenticate as is refused instead of ignored: a template naming `envFrom`,
//! a volume, or an environment value from anywhere but the downward API's own
//! name ends the attempt with a message naming it. And a process dies with the
//! server that started it, where a Job outlives a launcher's restart — which
//! is the whole reason this is a development backend and the chart offers it
//! nowhere.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use aiwatcher_execution::AttemptKey;

use super::log::{Kept, Tail, bounded};
use super::manifest::{self, Program};
use super::{Cluster, ClusterError, Created, Observed, Phase};

/// How many of this host's processes may run at once.
///
/// Four: enough to prove a fan-out, and small enough that a laptop running the
/// server, the panel and a browser beside them does not swap. A cluster's
/// answer to this is a scheduler; here it is a number, and an attempt over it
/// waits in the same `Pending` a pod waits in.
pub const DEFAULT_LIMIT: usize = 4;

/// The variables a launched process does not inherit.
///
/// Everything else of the host's environment is inherited, because a program
/// run outside an image has no `PATH`, `HOME` or interpreter path of its own —
/// but this server's own configuration would reach its children and a worker
/// reading `AIWATCHER_WORKFLOW_STORE` or `AIWATCHER_POD_TEMPLATES` is a worker
/// configured as a server. The three the manifest sets are applied after this.
const NOT_INHERITED: &str = "AIWATCHER_";

/// This host, as a cluster of one.
#[derive(Debug)]
pub struct ProcessCluster {
    limit: usize,
    started: Mutex<BTreeMap<String, Started>>,
}

/// One Job of this cluster, and how far it got.
#[derive(Debug)]
struct Started {
    key: AttemptKey,
    template: Option<String>,
    /// When the Job was created rather than when its process began — the clock
    /// the start allowance runs on, so one queued behind the limit is overdue
    /// on the same terms as a pod nothing would schedule.
    created_at: OffsetDateTime,
    stage: Stage,
}

#[derive(Debug)]
enum Stage {
    /// Created, and waiting for a slot on this host.
    Queued(Program),
    Running {
        printed: Arc<Mutex<Tail>>,
        /// How it ended, written once by the task that waited for it.
        ended: Arc<Mutex<Option<String>>>,
        stop: CancellationToken,
    },
    Ended {
        reason: String,
        printed: Kept,
    },
}

impl ProcessCluster {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            started: Mutex::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }
}

impl Drop for ProcessCluster {
    /// Stop every process this host started.
    ///
    /// What makes the module's promise true: a step's process goes with the
    /// server, where a Job outlives a launcher and is picked up by the next
    /// one. A `SIGKILL` runs no destructor, so a child survives that — the
    /// backstop there is `kill_on_drop` and, after it, a lease that lapses.
    fn drop(&mut self) {
        for job in self.started.get_mut().values() {
            if let Stage::Running { stop, .. } = &job.stage {
                stop.cancel();
            }
        }
    }
}

/// Reap what has ended and start what a free slot admits.
///
/// Called wherever this cluster is asked anything, so nothing here needs a
/// loop of its own: the launcher's pass is the clock, and a slot freed by a
/// process that ended is filled on the next one.
async fn sweep(started: &mut BTreeMap<String, Started>, limit: usize) {
    for job in started.values_mut() {
        let (reason, printed) = {
            let Stage::Running { printed, ended, .. } = &job.stage else {
                continue;
            };
            let Some(reason) = ended.lock().await.take() else {
                continue;
            };
            (reason, printed.lock().await.kept())
        };
        job.stage = Stage::Ended { reason, printed };
    }
    let mut running = started
        .values()
        .filter(|job| matches!(job.stage, Stage::Running { .. }))
        .count();
    for (name, job) in started.iter_mut() {
        if running >= limit {
            break;
        }
        let Stage::Queued(program) = &job.stage else {
            continue;
        };
        let stage = match start(name, program) {
            Ok(stage) => {
                running += 1;
                stage
            }
            // Queued, and by the time there was room the program could not be
            // run at all. The watch ends the attempt with this, which is what
            // it does for a pod the cluster could not start.
            Err(ClusterError::Refused(why) | ClusterError::Unavailable(why)) => Stage::Ended {
                reason: why,
                printed: Kept::default(),
            },
        };
        job.stage = stage;
    }
}

/// Start one program, and watch it from a task of its own.
fn start(name: &str, program: &Program) -> Result<Stage, ClusterError> {
    let Some((head, rest)) = program.command.split_first() else {
        return Err(ClusterError::Refused(
            "the container names no command to run".to_owned(),
        ));
    };
    let mut command = Command::new(head);
    command.args(rest);
    for inherited in std::env::vars().map(|(name, _)| name) {
        if inherited.starts_with(NOT_INHERITED) {
            command.env_remove(inherited);
        }
    }
    for (variable, value) in &program.environment {
        command.env(variable, value);
    }
    if let Some(directory) = &program.directory {
        command.current_dir(directory);
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The process goes when this server does. A Job outlives a launcher
        // and is picked up again by the next one; a child left behind here
        // would hold an attempt nothing is watching, on somebody's laptop.
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| match error.kind() {
            // Permanent until somebody edits the template, which is what
            // `Refused` means: the attempt ends rather than being retried.
            ErrorKind::NotFound | ErrorKind::PermissionDenied => {
                ClusterError::Refused(format!("this host cannot run '{head}': {error}"))
            }
            _ => ClusterError::Unavailable(format!("'{head}' could not be started: {error}")),
        })?;
    let printed = Arc::new(Mutex::new(Tail::new(super::log::TAIL_BYTES)));
    let ended = Arc::new(Mutex::new(None));
    let stop = CancellationToken::new();
    tokio::spawn(supervise(
        child,
        Arc::clone(&printed),
        Arc::clone(&ended),
        stop.clone(),
    ));
    tracing::info!(job = %name, program = %head, "a step's process was started on this host");
    Ok(Stage::Running {
        printed,
        ended,
        stop,
    })
}

/// Read what one process prints and wait for it to end.
///
/// Both pipes into one tail, because that is what `kubectl logs` shows and
/// what the catalog keeps. They are drained to their end before the ending is
/// written down: a traceback printed on the way out is the part somebody is
/// reading.
async fn supervise(
    mut child: Child,
    printed: Arc<Mutex<Tail>>,
    ended: Arc<Mutex<Option<String>>>,
    stop: CancellationToken,
) {
    let out = child
        .stdout
        .take()
        .map(|pipe| tokio::spawn(drain(pipe, Arc::clone(&printed))));
    let error = child
        .stderr
        .take()
        .map(|pipe| tokio::spawn(drain(pipe, Arc::clone(&printed))));
    let status = tokio::select! {
        status = child.wait() => status,
        () = stop.cancelled() => {
            // No grace: a delete is asked for once the attempt this process
            // held has already ended, and a cancel wants the pod stopped now.
            let _ = child.start_kill();
            child.wait().await
        }
    };
    for reader in [out, error].into_iter().flatten() {
        let _ = reader.await;
    }
    *ended.lock().await = Some(ending(status));
}

async fn drain<R: AsyncRead + Unpin>(mut pipe: R, into: Arc<Mutex<Tail>>) {
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => into.lock().await.push(&chunk[..read]),
        }
    }
}

/// The pod's own word for how it ended, in a cluster's vocabulary.
///
/// A signal rather than a code is what an out-of-memory kill looks like from
/// here — the host's, not a limit's, since this backend sets none.
fn ending(status: std::io::Result<ExitStatus>) -> String {
    let status = match status {
        Ok(status) => status,
        Err(error) => return format!("the process could not be waited for: {error}"),
    };
    if status.success() {
        return "Completed".to_owned();
    }
    if let Some(code) = status.code() {
        return format!("Error (exit {code})");
    }
    signalled(status)
}

#[cfg(unix)]
fn signalled(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;

    status.signal().map_or_else(
        || "Terminated".to_owned(),
        |signal| format!("Signalled (signal {signal})"),
    )
}

#[cfg(not(unix))]
fn signalled(_status: ExitStatus) -> String {
    "Terminated".to_owned()
}

#[async_trait]
impl Cluster for ProcessCluster {
    async fn create_job(&self, manifest: &Value) -> Result<Created, ClusterError> {
        let name = manifest
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .ok_or_else(|| ClusterError::Refused("the manifest has no name".to_owned()))?
            .to_owned();
        let key = manifest::attempt_of(&manifest::annotations_of(manifest)).ok_or_else(|| {
            ClusterError::Refused("the manifest does not say which attempt it is for".to_owned())
        })?;
        let program = manifest::program(manifest, &name).map_err(ClusterError::Refused)?;
        let mut started = self.started.lock().await;
        sweep(&mut started, self.limit).await;
        if started.contains_key(&name) {
            // The name is derived from the attempt, so this is this launcher
            // before a restart — or the same pass twice.
            return Ok(Created::AlreadyExisted);
        }
        let running = started
            .values()
            .filter(|job| matches!(job.stage, Stage::Running { .. }))
            .count();
        // Started here rather than left to the sweep, so a command this host
        // does not have is a refusal the caller can end the attempt with
        // instead of an infrastructure failure it retries three times.
        let stage = if running < self.limit {
            start(&name, &program)?
        } else {
            Stage::Queued(program)
        };
        started.insert(
            name,
            Started {
                key,
                template: manifest::labels_of(manifest)
                    .get(manifest::TEMPLATE_LABEL)
                    .cloned(),
                created_at: OffsetDateTime::now_utc(),
                stage,
            },
        );
        Ok(Created::New)
    }

    async fn jobs(&self) -> Result<Vec<Observed>, ClusterError> {
        let mut started = self.started.lock().await;
        sweep(&mut started, self.limit).await;
        Ok(started
            .iter()
            .map(|(name, job)| Observed {
                name: name.clone(),
                key: job.key.clone(),
                template: job.template.clone(),
                created_at: job.created_at,
                pod: match &job.stage {
                    // A cluster says `Unschedulable` for this; the launcher
                    // reads the word and the start allowance decides.
                    Stage::Queued(_) => Phase::Live {
                        reason: Some(format!(
                            "this host runs {} step processes at once and all of them are busy",
                            self.limit
                        )),
                    },
                    Stage::Running { .. } => Phase::Live { reason: None },
                    Stage::Ended { reason, .. } => Phase::Ended {
                        reason: reason.clone(),
                    },
                },
            })
            .collect())
    }

    async fn log(&self, job: &str, at_most: usize) -> Result<Option<Kept>, ClusterError> {
        let mut started = self.started.lock().await;
        sweep(&mut started, self.limit).await;
        let Some(job) = started.get(job) else {
            return Ok(None);
        };
        Ok(match &job.stage {
            // Nothing has printed anything: there is no pod yet.
            Stage::Queued(_) => None,
            Stage::Running { printed, .. } => Some(bounded(printed.lock().await.kept(), at_most)),
            Stage::Ended { printed, .. } => Some(bounded(printed.clone(), at_most)),
        })
    }

    async fn delete(&self, job: &str) -> Result<(), ClusterError> {
        let mut started = self.started.lock().await;
        if let Some(Started {
            stage: Stage::Running { stop, .. },
            ..
        }) = started.remove(job)
        {
            // The task that is waiting for it does the killing, because it
            // holds the child — and it is what writes down that it ended.
            stop.cancel();
        }
        sweep(&mut started, self.limit).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use serde_json::json;

    use aiwatcher_execution::ExecutionId;
    use aiwatcher_execution::pods::{PodRequest, PodTemplates};

    use super::super::manifest::JobRequest;

    /// One Job, built by the same function a real cluster is sent.
    ///
    /// Through the template parser rather than by hand, so a manifest this
    /// backend is asked to run is one a cluster would have accepted — the two
    /// backends read one manifest, and a test that wrote its own would stop
    /// saying so the day the builder changed.
    fn manifest_for(step: &str, command: &[&str], pod: Value) -> Value {
        let templates = PodTemplates::parse(
            json!({
                "e2e": {
                    "images": ["aiwatcher-stage"],
                    // A ceiling this backend cannot enforce, and the same
                    // template file either way: what a step may ask for is a
                    // fact about the template, not about who runs it.
                    "resources": {"max": {"cpu": "2", "memory": "1Gi"}},
                    "command": command,
                    "pod": pod,
                }
            })
            .to_string()
            .as_bytes(),
        )
        .expect("a template this host can run");
        let template = templates.get("e2e").expect("the template");
        let key = AttemptKey::new(ExecutionId::new("run-1"), step, 1);
        super::super::manifest::job(&JobRequest {
            key: &key,
            template_name: "e2e",
            template,
            pod: &PodRequest {
                template: "e2e".to_owned(),
                image: "aiwatcher-stage:e2e".to_owned(),
                cpu: None,
                memory: None,
            },
            timeout_seconds: 60,
            api_url: "http://127.0.0.1:8080",
        })
    }

    fn stage(command: &[&str]) -> Value {
        manifest_for("analyze", command, json!({}))
    }

    fn named(manifest: &Value) -> String {
        manifest
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .expect("a Job has a name")
            .to_owned()
    }

    async fn phase_of(cluster: &ProcessCluster, name: &str) -> Phase {
        cluster
            .jobs()
            .await
            .expect("this host answers")
            .into_iter()
            .find(|job| job.name == name)
            .map(|job| job.pod)
            .expect("the Job that was created")
    }

    /// Wait for one process to end, the way the launcher's pass would.
    async fn ended(cluster: &ProcessCluster, name: &str) -> String {
        for _ in 0..500 {
            if let Phase::Ended { reason } = phase_of(cluster, name).await {
                return reason;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{name} never ended");
    }

    async fn printed(cluster: &ProcessCluster, name: &str) -> String {
        let kept = cluster
            .log(name, super::super::log::TAIL_BYTES)
            .await
            .expect("this host answers")
            .expect("a process that ran printed something");
        String::from_utf8(kept.bytes).expect("its output is text")
    }

    #[tokio::test]
    async fn a_step_process_runs_the_container_s_command_and_both_its_streams_are_the_log() {
        // Both, in one tail: that is what `kubectl logs` shows and what the
        // catalog keeps, and a traceback is printed on the error stream.
        let manifest = stage(&[
            "/bin/sh",
            "-c",
            "echo the stage ran; echo and complained >&2",
        ]);
        let cluster = ProcessCluster::new(2);
        assert_eq!(
            cluster.create_job(&manifest).await.expect("it starts"),
            Created::New
        );
        let name = named(&manifest);
        assert_eq!(ended(&cluster, &name).await, "Completed");
        let output = printed(&cluster, &name).await;
        assert!(output.contains("the stage ran"), "{output}");
        assert!(output.contains("and complained"), "{output}");

        cluster.delete(&name).await.expect("it goes");
        assert!(
            cluster.jobs().await.expect("this host answers").is_empty(),
            "a deleted Job is gone from this host as it is from a cluster"
        );
    }

    #[tokio::test]
    async fn a_step_process_that_failed_says_what_it_exited_with() {
        // `Error` on its own tells nobody anything, which is the reason the
        // cluster backend keeps the exit code too.
        let manifest = stage(&["/bin/sh", "-c", "echo nearly; exit 3"]);
        let cluster = ProcessCluster::new(2);
        cluster.create_job(&manifest).await.expect("it starts");
        assert_eq!(ended(&cluster, &named(&manifest)).await, "Error (exit 3)");
    }

    #[tokio::test]
    async fn a_process_is_told_its_attempt_its_api_and_its_own_name() {
        // The three the manifest sets, and the third from the downward API —
        // which is the name the claim is held under, so a worker that read an
        // empty one would claim under a name nobody can find.
        let manifest = stage(&[
            "/bin/sh",
            "-c",
            "printf '%s|%s|%s' \"$AIWATCHER_ATTEMPT\" \"$AIWATCHER_URL\" \"$AIWATCHER_WORKER_NAME\"",
        ]);
        let cluster = ProcessCluster::new(2);
        cluster.create_job(&manifest).await.expect("it starts");
        let name = named(&manifest);
        assert_eq!(ended(&cluster, &name).await, "Completed");
        assert_eq!(
            printed(&cluster, &name).await,
            format!("run-1/analyze/1|http://127.0.0.1:8080|{name}")
        );
    }

    #[tokio::test]
    async fn the_same_attempt_asked_for_twice_is_one_process() {
        // The name is derived from the attempt, so the second ask is this
        // launcher before a restart, or one pass overlapping another.
        let manifest = stage(&["/bin/sh", "-c", "sleep 0.2"]);
        let cluster = ProcessCluster::new(2);
        assert_eq!(
            cluster.create_job(&manifest).await.expect("it starts"),
            Created::New
        );
        assert_eq!(
            cluster.create_job(&manifest).await.expect("it is there"),
            Created::AlreadyExisted
        );
        assert_eq!(cluster.jobs().await.expect("this host answers").len(), 1);
    }

    #[tokio::test]
    async fn an_attempt_over_this_host_s_limit_waits_for_a_slot_instead_of_starting() {
        // A cluster answers this with a scheduler and says `Unschedulable`
        // meanwhile; here it is a number, and the waiting attempt is `Live`
        // with the cluster's own kind of reason — so the launcher's start
        // allowance decides about it and nothing else has to change.
        let first = manifest_for("acquire", &["/bin/sh", "-c", "sleep 0.4"], json!({}));
        let second = manifest_for("normalize", &["/bin/sh", "-c", "echo second"], json!({}));
        let cluster = ProcessCluster::new(1);
        cluster.create_job(&first).await.expect("it starts");
        cluster.create_job(&second).await.expect("it is created");

        assert_eq!(
            phase_of(&cluster, &named(&first)).await,
            Phase::Live { reason: None },
            "the one slot is running the first"
        );
        let Phase::Live { reason: Some(why) } = phase_of(&cluster, &named(&second)).await else {
            panic!("the second waits for a slot rather than running");
        };
        assert!(why.contains("busy"), "{why}");

        assert_eq!(ended(&cluster, &named(&first)).await, "Completed");
        assert_eq!(ended(&cluster, &named(&second)).await, "Completed");
        assert_eq!(printed(&cluster, &named(&second)).await, "second\n");
    }

    #[tokio::test]
    async fn deleting_a_running_process_kills_it_rather_than_forgetting_it() {
        // The one thing a cancel is for. A pool that dropped the row and left
        // the program running would report a cancelled run in seconds and go
        // on doing the work it cancelled.
        let marker = std::env::temp_dir().join(format!("aiwatcher-kill-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let script = format!(
            "while true; do echo working >> {}; sleep 0.05; done",
            marker.display()
        );
        let manifest = stage(&["/bin/sh", "-c", &script]);
        let cluster = ProcessCluster::new(2);
        cluster.create_job(&manifest).await.expect("it starts");
        let name = named(&manifest);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            phase_of(&cluster, &name).await,
            Phase::Live { reason: None }
        );

        cluster.delete(&name).await.expect("it goes");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let written = std::fs::metadata(&marker)
            .expect("it had written something")
            .len();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            std::fs::metadata(&marker).expect("the file stays").len(),
            written,
            "nothing was written after the delete: the process is gone"
        );
        let _ = std::fs::remove_file(&marker);
    }

    #[tokio::test]
    async fn a_command_this_host_does_not_have_is_refused_rather_than_retried() {
        // `Refused` is what ends the attempt as the caller's mistake. As an
        // infrastructure failure it would be tried three times, ten minutes
        // apart, over a path that will never exist.
        let manifest = stage(&["/nowhere/aiwatcher-stage", "run-attempt"]);
        let error = ProcessCluster::new(2)
            .create_job(&manifest)
            .await
            .expect_err("this host cannot run it");
        assert!(
            matches!(&error, ClusterError::Refused(why) if why.contains("/nowhere/aiwatcher-stage")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_template_that_reads_a_secret_is_refused_naming_how() {
        // Ignored instead, the program would run without the credential it
        // was written to hold and fail somewhere else entirely.
        let manifest = manifest_for(
            "analyze",
            &["/bin/sh", "-c", "true"],
            json!({
                "containers": [{
                    "envFrom": [{"secretRef": {"name": "planner-import-token"}}],
                }]
            }),
        );
        let error = ProcessCluster::new(2)
            .create_job(&manifest)
            .await
            .expect_err("this host cannot read a Secret");
        assert!(
            matches!(&error, ClusterError::Refused(why) if why.contains("envFrom")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_template_that_mounts_a_volume_is_refused() {
        let manifest = manifest_for(
            "analyze",
            &["/bin/sh", "-c", "true"],
            json!({
                "volumes": [{"name": "scratch", "emptyDir": {}}],
                "containers": [{"volumeMounts": [{"name": "scratch", "mountPath": "/scratch"}]}]
            }),
        );
        let error = ProcessCluster::new(2)
            .create_job(&manifest)
            .await
            .expect_err("this host mounts nothing");
        assert!(
            matches!(&error, ClusterError::Refused(why) if why.contains("volume")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn an_environment_value_from_anywhere_but_the_pod_s_own_name_is_refused() {
        let manifest = manifest_for(
            "analyze",
            &["/bin/sh", "-c", "true"],
            json!({
                "containers": [{
                    "env": [{
                        "name": "PLANNER_TOKEN",
                        "valueFrom": {"secretKeyRef": {"name": "planner", "key": "token"}},
                    }]
                }]
            }),
        );
        let error = ProcessCluster::new(2)
            .create_job(&manifest)
            .await
            .expect_err("this host cannot read it");
        assert!(
            matches!(&error, ClusterError::Refused(why) if why.contains("PLANNER_TOKEN")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_manifest_that_is_not_a_pod_is_refused_the_way_a_cluster_refuses_one() {
        let addressed = json!({
            super::super::manifest::EXECUTION_ANNOTATION: "run-1",
            super::super::manifest::STEP_ANNOTATION: "analyze",
            super::super::manifest::ATTEMPT_ANNOTATION: "1",
        });
        for (manifest, expected) in [
            (
                json!({"metadata": {"name": "step", "annotations": addressed}}),
                "no pod spec",
            ),
            (
                json!({
                    "metadata": {"name": "step", "annotations": addressed},
                    "spec": {"template": {"spec": {"containers": [{}]}}},
                }),
                "no command",
            ),
        ] {
            let error = ProcessCluster::new(2)
                .create_job(&manifest)
                .await
                .expect_err("it is not a Job this host can run");
            assert!(
                matches!(&error, ClusterError::Refused(why) if why.contains(expected)),
                "{error}"
            );
        }
    }
}
