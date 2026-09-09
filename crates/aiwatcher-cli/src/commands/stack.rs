//! `aiwatcher up` — the local stack, from one command.
//!
//! What "the stack" is, and what it is not. The **server** runs in this
//! process, not as a child of it: there is one binary and running it twice to
//! watch itself would be theatre. The **broker** and the **query service** are
//! other people's programs, so they are children, started only when they are
//! asked for and stopped when this exits.
//!
//! The design rule that shapes the whole file is that an absent dependency is
//! reported, never worked around. Docker missing is a sentence naming Docker;
//! PHP missing is a sentence naming PHP; and in both cases the server still
//! comes up, because the built-in write-ahead log is a real durable log and the
//! Query tab is not the reason anybody installs this. What would be worse than
//! either is starting, half working, and leaving somebody to discover which
//! half from an empty screen.
//!
//! There is deliberately no code here that builds Iggy from source. It is
//! available as a container image and — on Linux — as a release binary; on this
//! machine, compiling somebody else's server as a side effect of `up` is a
//! surprise, not a convenience. When Docker is not there, the command says so
//! and names the one flag that changes the answer.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use aiwatcher_auth::AuthMode;
use aiwatcher_auth::identity::Role;
use aiwatcher_auth::local::LocalAuth;
use aiwatcher_server::config::{BackendKind, Config as ServerConfig};

use crate::paths::Paths;
use crate::{Args, CliError};

/// Where a local instance listens unless told otherwise.
pub const DEFAULT_URL: &str = "http://127.0.0.1:8080";

/// The container image `up` starts a broker from.
///
/// Pinned, and the same tag `just iggy-up` uses: an image that floated would
/// make "it worked yesterday" a question about somebody else's release train.
const IGGY_IMAGE: &str = "apache/iggy:0.9.0-edge.5";

/// The name the container is given, so a second `up` reuses it rather than
/// racing a port with itself.
const IGGY_CONTAINER: &str = "aiwatcher-iggy";

/// Iggy's HTTP API. The TCP port the SDK speaks is 8090; this is the one the
/// `iggy` bus feature uses, because it needs `reqwest` and nothing else.
const IGGY_HTTP_PORT: u16 = 3000;

/// Which log a local instance runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Log {
    /// The built-in write-ahead log. Durable, single node, nothing to start.
    Wal,
    /// Apache Iggy, in a container this command supervises.
    Iggy,
    /// Nothing survives a restart. For a demo.
    Memory,
}

impl std::str::FromStr for Log {
    type Err = CliError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "wal" | "file" | "builtin" => Ok(Self::Wal),
            "iggy" | "broker" => Ok(Self::Iggy),
            "memory" | "none" => Ok(Self::Memory),
            other => Err(CliError::Usage(format!(
                "log={other:?}; expected wal, iggy or memory"
            ))),
        }
    }
}

/// What `up` was asked for.
#[derive(Debug)]
struct Plan {
    log: Log,
    flow: bool,
    port: u16,
    /// Whether to bind something other than the loopback interface, which the
    /// local credential's whole premise is that we do not.
    listen: String,
}

impl Plan {
    fn read(args: &Args) -> Result<Self, CliError> {
        let log = args.value("log").unwrap_or("wal").parse()?;
        let port = match args.value("port") {
            None => 8080,
            Some(raw) => raw
                .parse()
                .map_err(|_| CliError::Usage(format!("port={raw:?} is not a port number")))?,
        };
        Ok(Self {
            log,
            // On when the service is there to be started and nobody said
            // otherwise: the Query tab is part of what this stack is for, and
            // an absent PHP is reported rather than preempted.
            flow: !matches!(args.value("flow"), Some("off" | "no" | "false")),
            port,
            listen: args.value("listen").unwrap_or("127.0.0.1").to_owned(),
        })
    }
}

/// A child this command started and is responsible for stopping.
struct Child {
    name: &'static str,
    handle: std::process::Child,
}

/// Everything `up` started, stopped in reverse on the way out.
///
/// A guard rather than a sequence of kills at the end of `run`, because the
/// interesting exit is the one that did not reach the end of `run` — a bind
/// that failed, a configuration the server refused. Without this, those leave a
/// PHP process holding port 8081 and the next `up` fails for a reason that has
/// nothing to do with what went wrong.
struct Started {
    children: Vec<Child>,
    /// Whether this invocation started the container, as opposed to finding
    /// one already running. Only the starter stops it.
    started_iggy: bool,
}

impl Drop for Started {
    fn drop(&mut self) {
        for child in self.children.iter_mut().rev() {
            let _ = child.handle.kill();
            let _ = child.handle.wait();
            eprintln!("stopped {}", child.name);
        }
        if self.started_iggy {
            let _ = docker(&["rm", "-f", IGGY_CONTAINER]);
            eprintln!("stopped the broker");
        }
    }
}

/// Dispatch `up | down | status`.
///
/// # Errors
///
/// Whatever stopped the stack from coming up.
pub async fn run(args: &Args, paths: &Paths) -> Result<(), CliError> {
    match args.word(0) {
        Some("up") => up(args, paths).await,
        Some("down") => down(),
        Some("status") => status(args, paths).await,
        other => Err(CliError::Usage(format!(
            "{:?} is not a stack command",
            other.unwrap_or("")
        ))),
    }
}

/// Start everything and serve until interrupted.
async fn up(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let plan = Plan::read(args)?;
    // Before anything is started: a token that cannot be generated is a stack
    // nobody could talk to, and finding that out after a container is up is
    // strictly worse than finding it out now.
    let secret = crate::commands::token::ensure(paths, false)?;

    let mut started = Started {
        children: Vec::new(),
        started_iggy: false,
    };

    if plan.log == Log::Iggy {
        started.started_iggy = start_iggy()?;
    }
    if plan.flow
        && let Some(child) = start_flow()?
    {
        started.children.push(child);
    }

    let config = server_config(&plan, paths, &secret)?;
    announce(&plan, paths, &config);

    // The guard lives until this returns, so a server that refuses to start
    // takes the children with it.
    crate::commands::serve::run(config)
        .await
        .map_err(CliError::Other)
}

/// Stop what an `up` left behind.
///
/// For the case `up`'s own guard cannot cover: a hard kill, or a container
/// somebody started with `just iggy-up` and wants gone.
fn down() -> Result<(), CliError> {
    match docker(&["rm", "-f", IGGY_CONTAINER]) {
        Ok(true) => println!("stopped the broker"),
        Ok(false) | Err(_) => println!("no broker container to stop"),
    }
    println!("the server and the query service stop with the `up` that started them");
    Ok(())
}

/// What is running, and what a command would talk to.
async fn status(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let config = crate::config::Config::load(&paths.config_file())?;
    let resolved = config.resolve(args.value("profile"), DEFAULT_URL, paths.token_file());
    let client = crate::client::Client::new(&resolved, Duration::from_secs(3))?;

    let server = client.ready().await;
    let broker = docker(&["inspect", "-f", "{{.State.Running}}", IGGY_CONTAINER]).unwrap_or(false);
    let flow = reachable("127.0.0.1:8081");

    if crate::Format::from_args(args)? == crate::Format::Json {
        println!(
            "{}",
            serde_json::json!({
                "profile": resolved.name,
                "url": resolved.profile.url,
                "server": server,
                "broker": broker,
                "query_service": flow,
                "token": client.has_token(),
            })
        );
        return Ok(());
    }
    println!("profile        {}", resolved.name);
    println!("url            {}", resolved.profile.url);
    println!("server         {}", mark(server));
    println!("broker         {}", mark(broker));
    println!("query service  {}", mark(flow));
    println!("token          {}", mark(client.has_token()));
    Ok(())
}

const fn mark(up: bool) -> &'static str {
    if up { "up" } else { "down" }
}

/// The configuration a local instance runs under.
///
/// Built by reading the environment and then overriding what `up` decides,
/// rather than by setting environment variables and re-reading: this workspace
/// forbids `unsafe`, and `std::env::set_var` is `unsafe` in edition 2024. The
/// order is the right one anyway — a variable somebody exported deliberately
/// still reaches everything `up` does not have an opinion about.
fn server_config(plan: &Plan, paths: &Paths, secret: &str) -> Result<ServerConfig, CliError> {
    let mut config = ServerConfig::from_env().map_err(|error| CliError::Other(error.into()))?;

    config.listen = format!("{}:{}", plan.listen, plan.port)
        .parse()
        .map_err(|_| CliError::Usage(format!("listen={:?} is not an address", plan.listen)))?;
    config.data_dir = paths.data_dir.display().to_string();
    config.bus = match plan.log {
        Log::Wal => BackendKind::Wal,
        Log::Memory => BackendKind::Memory,
        Log::Iggy => BackendKind::Laser,
    };
    // A local instance is the one place a producer has nowhere else to publish:
    // there is no cluster, and the SDK reaches this process over HTTP.
    config.ingest_enabled = true;

    // The local database, when this build has one. Overriding rather than
    // defaulting, because `up` is the command that has an opinion: a build with
    // the feature compiled in has it for this. A variable somebody exported
    // deliberately still wins, which is why it is read back first.
    #[cfg(feature = "duckdb")]
    if std::env::var("AIWATCHER_WORKFLOW_STORE").is_err() {
        config.workflow_store = aiwatcher_server::config::WorkflowStoreKind::Duckdb;
    }
    if plan.flow {
        config.flow_url = Some("http://127.0.0.1:8081".to_owned());
    }

    config.auth.mode = AuthMode::Local;
    config.auth.local =
        Some(LocalAuth::new(secret, Role::Admin).map_err(|error| CliError::Other(error.into()))?);

    config
        .validate()
        .map_err(|error| CliError::Other(error.into()))?;
    Ok(config)
}

/// Say what is about to happen, before it takes over the terminal.
fn announce(plan: &Plan, paths: &Paths, config: &ServerConfig) {
    println!("aiwatcher");
    println!("  api     http://{}", config.listen);
    println!("  panel   http://{}/", config.listen);
    println!(
        "  log     {}",
        match plan.log {
            Log::Wal => "write-ahead log, in the data directory",
            Log::Iggy => "apache iggy, in a container",
            Log::Memory => "memory — nothing survives a restart",
        }
    );
    println!("  data    {}", paths.data_dir.display());
    println!(
        "  store   {}",
        match config.workflow_store {
            aiwatcher_server::config::WorkflowStoreKind::Memory => "memory".to_owned(),
            aiwatcher_server::config::WorkflowStoreKind::File => "files, one per run".to_owned(),
            aiwatcher_server::config::WorkflowStoreKind::Postgres => "postgresql".to_owned(),
            aiwatcher_server::config::WorkflowStoreKind::Duckdb =>
                format!("duckdb ({} — aiwatcher sql)", paths.database().display()),
        }
    );
    println!(
        "  token   {} (aiwatcher token show)",
        paths.token_file().display()
    );
    println!();
}

// ── The children ─────────────────────────────────────────────────────────────

/// Start the broker, or explain why there is none.
///
/// Returns whether *this* invocation started it: a container somebody else is
/// using is one this command must not stop on the way out.
fn start_iggy() -> Result<bool, CliError> {
    if !have("docker") {
        return Err(CliError::Environment(
            "log=iggy needs Docker, which is not on this machine. Apache publishes the \
             broker as a container image and, for Linux only, as a release binary; there is \
             no macOS build and this command will not compile one for you. \
             Run `aiwatcher up` without log=iggy to use the built-in write-ahead log, \
             which is durable and needs nothing."
                .into(),
        ));
    }
    if docker(&["inspect", "-f", "{{.State.Running}}", IGGY_CONTAINER]).unwrap_or(false) {
        println!("broker already running; leaving it alone");
        return Ok(false);
    }
    let _ = docker(&["rm", "-f", IGGY_CONTAINER]);

    // Every flag is load-bearing; the broker fails differently without each.
    // They are the ones `just iggy-up` established, plus the HTTP port, which
    // is the transport the `iggy` bus feature speaks.
    let ports = format!("{IGGY_HTTP_PORT}:{IGGY_HTTP_PORT}");
    let ok = docker(&[
        "run",
        "-d",
        "--name",
        IGGY_CONTAINER,
        // Its runtime is io_uring, and the default seccomp profile blocks
        // io_uring_setup, io_uring_enter and io_uring_register.
        "--security-opt",
        "seccomp=unconfined",
        "-e",
        "IGGY_TCP_ADDRESS=0.0.0.0:8090",
        "-e",
        "IGGY_HTTP_ADDRESS=0.0.0.0:3000",
        "-e",
        "IGGY_HTTP_ENABLED=true",
        // "numa:auto" binds shard memory to a NUMA node, which fails inside a
        // container VM and takes the server down.
        "-e",
        "IGGY_SYSTEM_SHARDING_CPU_ALLOCATION=2",
        "-e",
        "IGGY_SYSTEM_SHARDING_PIN_CORES=false",
        // Without these the server generates a random root password and only
        // logs it, so every login is "Invalid credentials".
        "-e",
        "IGGY_ROOT_USERNAME=iggy",
        "-e",
        "IGGY_ROOT_PASSWORD=iggy",
        "-p",
        "8090:8090",
        "-p",
        &ports,
        IGGY_IMAGE,
    ])?;
    if !ok {
        return Err(CliError::Environment(
            "the broker container did not start; `docker logs aiwatcher-iggy` says why".into(),
        ));
    }
    wait_for("127.0.0.1:8090", Duration::from_secs(30)).map_err(|_| {
        CliError::Environment(
            "the broker did not answer within 30s; `docker logs aiwatcher-iggy` says why".into(),
        )
    })?;
    println!("broker up on :8090 (tcp) and :3000 (http)");
    Ok(true)
}

/// Start the query service, if this checkout has one and PHP is installed.
///
/// `None` rather than an error for either absence: the Query tab is one tab,
/// and a missing optional service must not be the reason a local instance does
/// not come up.
fn start_flow() -> Result<Option<Child>, CliError> {
    let root = PathBuf::from("services/flow");
    if !root.join("public").is_dir() {
        return Ok(None);
    }
    if !root.join("vendor").is_dir() {
        println!("query service: dependencies not installed (`just flow-install`); skipping");
        return Ok(None);
    }
    if !have("php") {
        println!("query service: PHP is not on this machine; skipping");
        return Ok(None);
    }
    let handle = Command::new("php")
        .args(["-S", "127.0.0.1:8081", "-t", "public"])
        .current_dir(&root)
        .env("AIWATCHER_URL", DEFAULT_URL)
        // `php -S` handles one request at a time unless told otherwise, and the
        // panel polls this service's health while a query is running — so the
        // single-worker default deadlocks the two against each other.
        .env("PHP_CLI_SERVER_WORKERS", "4")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| CliError::Environment(format!("starting the query service: {error}")))?;
    println!("query service up on :8081");
    Ok(Some(Child {
        name: "the query service",
        handle,
    }))
}

// ── Talking to the machine ───────────────────────────────────────────────────

/// Whether a program is on the path.
fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Run docker, and report whether it said yes.
///
/// `inspect` is used as a question, so a non-zero exit is an answer rather than
/// a failure — which is why this returns a bool instead of erroring on it.
fn docker(args: &[&str]) -> Result<bool, CliError> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .map_err(|error| CliError::Environment(format!("running docker: {error}")))?;
    if !output.status.success() {
        return Ok(false);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim();
    // `inspect -f {{.State.Running}}` answers the literal word.
    Ok(trimmed != "false")
}

/// Whether something is listening.
fn reachable(address: &str) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let Ok(mut candidates) = address.to_socket_addrs() else {
        return false;
    };
    candidates
        .any(|candidate| TcpStream::connect_timeout(&candidate, Duration::from_millis(300)).is_ok())
}

/// Block until something is listening, or give up.
fn wait_for(address: &str, budget: Duration) -> Result<(), CliError> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if reachable(address) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    Err(CliError::Environment(format!("{address} never answered")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_is_named_in_the_words_a_person_would_use() {
        assert_eq!("wal".parse::<Log>().expect("parses"), Log::Wal);
        assert_eq!("iggy".parse::<Log>().expect("parses"), Log::Iggy);
        assert_eq!("memory".parse::<Log>().expect("parses"), Log::Memory);
    }

    #[test]
    fn an_unknown_log_is_refused_by_name_rather_than_defaulted() {
        // Defaulting would start a stack that is not the one somebody asked
        // for, and the difference is whether their events survive a restart.
        let error = "kafka".parse::<Log>().expect_err("refuses");
        assert!(error.to_string().contains("kafka"), "{error}");
    }

    #[test]
    fn the_default_plan_needs_nothing_running() {
        let plan = Plan::read(&Args::parse(["up"]).expect("parses")).expect("reads");
        assert_eq!(plan.log, Log::Wal, "the default stack needs a broker");
        assert_eq!(plan.port, 8080);
        assert_eq!(plan.listen, "127.0.0.1");
    }

    #[test]
    fn the_query_service_can_be_turned_off_without_turning_the_log_off() {
        let plan = Plan::read(&Args::parse(["up", "flow=off"]).expect("parses")).expect("reads");
        assert!(!plan.flow);
        assert_eq!(plan.log, Log::Wal);
    }

    #[test]
    fn a_port_that_is_not_a_number_is_refused_before_anything_starts() {
        let error =
            Plan::read(&Args::parse(["up", "port=eighty"]).expect("parses")).expect_err("refuses");
        assert!(error.to_string().contains("eighty"), "{error}");
    }
}
