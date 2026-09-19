//! The operator's command line.
//!
//! Four verbs and one rule: `plan` and `verify` read, `apply` and `resume`
//! write, and the writing pair need a manifest somebody has already read.
//!
//! ```text
//! aiwatcher-migrate plan   --store fs:DIR --org UUID --project UUID [--prefix family=value] [--out FILE]
//! aiwatcher-migrate verify --store fs:DIR --manifest FILE [--iam SPEC --principal P:S]
//! aiwatcher-migrate apply  --store fs:DIR --manifest FILE --checkpoint FILE --iam SPEC --principal P:S --confirm
//! aiwatcher-migrate resume --store fs:DIR --manifest FILE --checkpoint FILE --iam SPEC --principal P:S --confirm
//! ```
//!
//! `apply` and `resume` are the same operation: an execution reads the
//! checkpoint if there is one, and a first run is one with nothing to read.
//! Two words because an operator typing `resume` after a failure is saying
//! what they believe happened, and the tool can then say so back — it refuses
//! `resume` with no checkpoint and `apply` with one.
//!
//! `--iam` is `postgres:URL` in a deployment, and `fixture:FILE` for a
//! rehearsal. A fixture is not an authority: every receipt it admits carries
//! `non_authoritative_iam`, and no cutover may be declared on one.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_iam::{OrganizationId, Principal, ProjectId, ProjectScope};
use aiwatcher_migration::authority::{Fixture, FixtureAuthority, Offline, TargetAuthority};
use aiwatcher_migration::execute::{Destination, Options, execute, load_checkpoint, survey};
use aiwatcher_migration::manifest::{Audience, Manifest};
use aiwatcher_migration::plan::{Source, plan};
use aiwatcher_prompts::adapters::fs::FileObjectStore;

const USAGE: &str = "\
aiwatcher-migrate — move an authored registry into one named IAM project

  plan   --store fs:DIR --org UUID --project UUID [--prefix family=value]... [--out FILE]
  verify --store fs:DIR --manifest FILE [--iam SPEC --principal PROVIDER:SUBJECT]
  apply  --store fs:DIR --manifest FILE --checkpoint FILE --iam SPEC \\
         --principal PROVIDER:SUBJECT --confirm [--exclusive-access]
  resume --store fs:DIR --manifest FILE --checkpoint FILE --iam SPEC \\
         --principal PROVIDER:SUBJECT --confirm [--exclusive-access]

  --store       fs:DIRECTORY — an existing object-store snapshot directory
  --iam         postgres:URL (the deployment's own store) or fixture:FILE
                (a rehearsal; never a cutover)
  --confirm     required to write anything at all

plan and verify write nothing. apply and resume overwrite nothing: a target key
holding different bytes stops the run and is reported.
";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("aiwatcher-migrate: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let verb = arguments.next().unwrap_or_default();
    let flags = Flags::parse(arguments)?;
    match verb.as_str() {
        "plan" => do_plan(&flags).await,
        "verify" => do_verify(&flags).await,
        "apply" => do_execute(&flags, false).await,
        "resume" => do_execute(&flags, true).await,
        "help" | "--help" | "-h" | "" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}").into()),
    }
}

async fn do_plan(flags: &Flags) -> Result<(), Box<dyn std::error::Error>> {
    let store = flags.store().await?;
    let mut source = Source::new(flags.require("--store")?);
    for (family, prefix) in &flags.prefixes {
        source = source.with_prefix(family, prefix);
    }
    let manifest = plan(&store, &source, flags.scope()?).await?;
    let authority = flags.authority().await?;
    let survey = survey(&store, &manifest, authority.as_ref()).await?;
    let document = serde_json::json!({ "manifest": manifest, "survey": survey });
    match flags.get("--out") {
        Some(path) => {
            tokio::fs::write(path, serde_json::to_vec_pretty(&manifest)?).await?;
            println!("{}", serde_json::to_string_pretty(&document)?);
            eprintln!("the manifest alone is written to {path}");
        }
        None => println!("{}", serde_json::to_string_pretty(&document)?),
    }
    Ok(())
}

async fn do_verify(flags: &Flags) -> Result<(), Box<dyn std::error::Error>> {
    let store = flags.store().await?;
    let manifest = read_manifest(flags).await?;
    let authority = flags.authority().await?;
    let survey = survey(&store, &manifest, authority.as_ref()).await?;
    println!("{}", serde_json::to_string_pretty(&survey)?);
    say_who_reaches_it(&survey.audience);
    Ok(())
}

/// Say, on stderr, who will be able to open what this copies.
///
/// On stderr and beside the JSON rather than only inside it, because it is the
/// one consequence of a migration that nothing in the bytes shows and that no
/// later command asks about: a project's data is reachable by whoever holds a
/// live grant on that project, and **a copy is not a share**. The tool creates
/// no grant; an operator reads this and decides.
fn say_who_reaches_it(audience: &Audience) {
    match audience {
        Audience::Read {
            holders,
            evaluated_at,
        } if holders.is_empty() => eprintln!(
            "nobody but this operator holds a live grant on the target (read at {evaluated_at}). \
             What is copied there is reachable by them and by nobody else until somebody grants \
             it explicitly — this tool creates no grant"
        ),
        Audience::Read {
            holders,
            evaluated_at,
        } => {
            eprintln!(
                "{} live grant(s) on the target (read at {evaluated_at}); after this copy they \
                 reach it and nobody else does:",
                holders.len()
            );
            for holder in holders {
                eprintln!("  {} {:?}", holder.grantee, holder.role);
            }
        }
        Audience::NotRead { reason } => eprintln!(
            "who reaches the target is unknown to this run: {reason}. A cutover declared without \
             it is a cutover declared without reading its consequence"
        ),
    }
}

async fn do_execute(flags: &Flags, resuming: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !flags.present("--confirm") {
        return Err("writing needs --confirm; plan and verify write nothing".into());
    }
    if !flags.prefixes.is_empty() {
        // The prefixes an execution uses are the ones inside the manifest,
        // because that is what was reviewed. Accepting them here and ignoring
        // them would read as configuring the run.
        return Err(
            "--prefix belongs to `plan`; an execution reads the prefixes out of the manifest \
             it was given"
                .into(),
        );
    }
    let store = flags.store().await?;
    let manifest = read_manifest(flags).await?;
    let destination = Destination {
        label: flags.require("--store")?.to_owned(),
        checkpoint: PathBuf::from(flags.require("--checkpoint")?),
    };
    let existing = load_checkpoint(&destination.checkpoint, &manifest, &destination.label).await?;
    match (resuming, &existing) {
        (true, None) => {
            return Err(format!(
                "resume found no checkpoint at {}; a first run is `apply`",
                destination.checkpoint.display()
            )
            .into());
        }
        (false, Some(checkpoint)) => {
            return Err(format!(
                "apply found a checkpoint at {} covering {} objects; continue it with `resume`",
                destination.checkpoint.display(),
                checkpoint.done.len()
            )
            .into());
        }
        _ => (),
    }
    let authority = flags.authority().await?;
    let receipt = execute(
        &store,
        &manifest,
        &destination,
        authority.as_ref(),
        Options {
            exclusive_access: flags.present("--exclusive-access"),
        },
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    say_who_reaches_it(&receipt.audience);
    if !receipt.cutover_ready {
        eprintln!(
            "this run is not a cutover: {} blocker(s) stand. Nothing may be switched over or \
             removed on the strength of it",
            receipt.blockers.len()
        );
    }
    Ok(())
}

async fn read_manifest(flags: &Flags) -> Result<Manifest, Box<dyn std::error::Error>> {
    let path = flags.require("--manifest")?;
    let bytes = tokio::fs::read(path).await?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if !manifest.identity_holds()? {
        return Err(
            format!("{path} no longer matches the id it was reviewed under; re-plan it").into(),
        );
    }
    Ok(manifest)
}

/// `--flag value`, `--flag=value` and bare `--flag`.
struct Flags {
    values: BTreeMap<String, String>,
    present: Vec<String>,
    prefixes: BTreeMap<String, String>,
}

impl Flags {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        let mut present = Vec::new();
        let mut prefixes = BTreeMap::new();
        let arguments: Vec<String> = arguments.collect();
        let mut index = 0;
        while index < arguments.len() {
            let argument = &arguments[index];
            if !argument.starts_with("--") {
                return Err(format!("unexpected argument {argument:?}\n\n{USAGE}"));
            }
            let (name, inline) = match argument.split_once('=') {
                Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
                None => (argument.clone(), None),
            };
            let value = match inline {
                Some(value) => Some(value),
                None => arguments
                    .get(index + 1)
                    .filter(|next| !next.starts_with("--"))
                    .cloned()
                    .inspect(|_| index += 1),
            };
            match (name.as_str(), value) {
                ("--prefix", Some(pair)) => {
                    let (family, prefix) = pair
                        .split_once('=')
                        .ok_or_else(|| "--prefix takes family=prefix".to_owned())?;
                    prefixes.insert(family.to_owned(), prefix.to_owned());
                }
                (_, Some(value)) => {
                    values.insert(name, value);
                }
                (_, None) => present.push(name),
            }
            index += 1;
        }
        Ok(Self {
            values,
            present,
            prefixes,
        })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn present(&self, name: &str) -> bool {
        self.present.iter().any(|flag| flag == name)
    }

    fn require(&self, name: &str) -> Result<&str, String> {
        self.get(name)
            .ok_or_else(|| format!("{name} is required\n\n{USAGE}"))
    }

    fn scope(&self) -> Result<ProjectScope, Box<dyn std::error::Error>> {
        Ok(ProjectScope {
            organization: OrganizationId(self.require("--org")?.parse()?),
            project: ProjectId(self.require("--project")?.parse()?),
        })
    }

    async fn store(&self) -> Result<Arc<dyn ObjectStore>, Box<dyn std::error::Error>> {
        let spec = self.require("--store")?;
        let directory = spec.strip_prefix("fs:").ok_or_else(|| {
            format!("--store takes fs:DIRECTORY; {spec:?} names no store this tool can open")
        })?;
        if !std::path::Path::new(directory).is_dir() {
            return Err(format!("{directory} is not an existing snapshot directory").into());
        }
        Ok(Arc::new(FileObjectStore::open(directory).await?))
    }

    fn principal(&self) -> Result<Principal, Box<dyn std::error::Error>> {
        let raw = self.require("--principal")?;
        let (provider, subject) = raw.split_once(':').ok_or_else(|| {
            "--principal takes PROVIDER:SUBJECT — the exact pair the control plane compares, \
             never an email address or a group name"
                .to_owned()
        })?;
        Ok(Principal::new(provider, subject)?)
    }

    /// Who says the target project exists.
    async fn authority(&self) -> Result<Box<dyn TargetAuthority>, Box<dyn std::error::Error>> {
        let Some(spec) = self.get("--iam") else {
            return Ok(Box::new(Offline::default()));
        };
        if let Some(path) = spec.strip_prefix("fixture:") {
            let fixture: Fixture = serde_json::from_slice(&std::fs::read(path)?)?;
            return Ok(Box::new(FixtureAuthority::new(
                fixture,
                self.principal()?,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )));
        }
        if let Some(url) = spec.strip_prefix("postgres:") {
            return postgres_authority(url, self.principal()?).await;
        }
        Err(format!("--iam takes postgres:URL or fixture:FILE; {spec:?} is neither").into())
    }
}

#[cfg(feature = "postgres")]
async fn postgres_authority(
    url: &str,
    principal: Principal,
) -> Result<Box<dyn TargetAuthority>, Box<dyn std::error::Error>> {
    // `--iam postgres:URL` strips the scheme off the connection string, so it
    // goes back on here rather than asking an operator to write it twice.
    let store =
        aiwatcher_iam::postgres::PostgresIamStore::connect(&format!("postgres:{url}"), 2).await?;
    Ok(Box::new(aiwatcher_migration::authority::IamAuthority::new(
        Arc::new(store),
        principal,
        "postgres",
        true,
    )))
}

#[cfg(not(feature = "postgres"))]
async fn postgres_authority(
    _url: &str,
    _principal: Principal,
) -> Result<Box<dyn TargetAuthority>, Box<dyn std::error::Error>> {
    Err("this build has no PostgreSQL IAM adapter; rebuild with --features postgres".into())
}
