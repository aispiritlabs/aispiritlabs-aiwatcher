//! The Flyte engine.
//!
//! Two things aiwatcher cannot learn from its own log: what a cluster is able
//! to run, and what inputs that thing declares. Nothing publishes an event
//! about a workflow nobody has started yet, and a workflow's input interface
//! is not visible in its output. Both live in the orchestrator, so both are
//! read from it — and nothing else here is.
//!
//! ## Transport
//!
//! Flyte's control plane is one gRPC service with a grpc-gateway in front of
//! it, and every method has an HTTP mapping under `/api/v1/`. That gateway is
//! what this speaks: it is a stable documented contract, it needs no code
//! generation, and it keeps `prost`, `tonic` and the whole `flyteidl` surface
//! out of a build that would use six messages of it. Flyte 2 kept the mapping.
//!
//! ## Two ports, one adapter
//!
//! [`FlyteEngine`] implements `WorkflowEngine` *and* `WorkflowRunner`. A
//! deployment whose orchestrator is Flyte should not have to configure two
//! endpoints to get both the picker and the rerun button, and the alternative
//! — an HTTP runner pointed at a shim that then talks to Flyte — is a second
//! thing to deploy for no new capability.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use aiwatcher_core::engine::{
    CatalogQuery, EngineCatalog, EngineDescription, EngineExecution, EnginePhase, EngineRef,
    EngineWorkflow, EntityKind, LaunchAccepted, LaunchError, LaunchRequest, PipelineStage,
    WorkflowEngine,
};
use aiwatcher_core::ports::{PortError, PortResult, RerunAccepted, RerunRequest, WorkflowRunner};

use crate::literals::{Interface, field, text};

const TARGET: &str = "flyte";

/// The label an execution started from here carries.
///
/// It is how a launch made in the panel is joined to the events the workflow
/// then publishes: aiwatcher sets it, and a producer that reads its own
/// execution labels can put the same value in `workflow_run_id`. Nothing
/// breaks when the producer does not — the launch is still made, and the
/// execution is still linked to Flyte's console — the join is simply not
/// there, which is exactly what the panel shows.
pub const RUN_ID_LABEL: &str = "aiwatcher-workflow-run-id";

/// The input name aiwatcher fills in automatically when an entity declares it.
///
/// A workflow that wants to be correlated declares this parameter and passes
/// it to `aiwatcher.workflow_run_id`; one that does not declare it is
/// unaffected, because an input the entity never declared is refused rather
/// than sent.
pub const RUN_ID_INPUTS: [&str; 2] = ["aiwatcher_workflow_run_id", "workflow_run_id"];

/// How to reach one Flyte control plane.
#[derive(Clone)]
pub struct FlyteConfig {
    /// `http://flyteadmin.flyte:80`. No trailing slash; one is trimmed.
    pub endpoint: String,
    /// The project and domain a request uses when it names neither.
    pub project: String,
    pub domain: String,
    /// A pre-issued bearer token. Mutually exclusive with client credentials
    /// in practice, and checked at the point of use rather than at
    /// construction so a deployment can hold both while it migrates.
    pub token: Option<String>,
    /// The service-account flow. All three are needed together.
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    /// Explicit, never discovered. Taking the token endpoint from a document
    /// the same host serves would hand the choice of who mints aiwatcher's
    /// credentials to whoever answered the request — the same reasoning as
    /// `ProviderMetadata::discover` refusing an issuer mismatch.
    pub token_url: Option<String>,
    pub scopes: String,
    /// Flyte's console, for links out. Absent means the panel renders no link
    /// rather than a broken one.
    pub console_url: Option<String>,
    pub timeout: Duration,
}

impl Default for FlyteConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            project: "flytesnacks".to_owned(),
            domain: "development".to_owned(),
            token: None,
            client_id: None,
            client_secret: None,
            token_url: None,
            // What flytectl asks for. A narrower scope is a deployment's
            // choice to make in its identity provider.
            scopes: "all".to_owned(),
            console_url: None,
            // The same ten seconds every other outbound adapter here uses.
            timeout: Duration::from_secs(10),
        }
    }
}

impl std::fmt::Debug for FlyteConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Neither secret may reach a log through a derived `Debug` three
        // layers up on something that happens to hold this.
        f.debug_struct("FlyteConfig")
            .field("endpoint", &self.endpoint)
            .field("project", &self.project)
            .field("domain", &self.domain)
            .field("token", &self.token.is_some())
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.is_some())
            .field("token_url", &self.token_url)
            .field("console_url", &self.console_url)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// A token and when it stops being one.
#[derive(Debug)]
struct CachedToken {
    value: String,
    /// Monotonic, so a clock adjustment cannot make a live token look expired
    /// or a dead one look fresh.
    expires_at: Instant,
}

/// Reads a Flyte control plane's inventory, and starts one of its entries.
pub struct FlyteEngine {
    http: reqwest::Client,
    config: FlyteConfig,
    token: tokio::sync::Mutex<Option<CachedToken>>,
}

impl std::fmt::Debug for FlyteEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlyteEngine")
            .field("endpoint", &self.config.endpoint)
            .field("project", &self.config.project)
            .field("domain", &self.config.domain)
            .finish_non_exhaustive()
    }
}

impl FlyteEngine {
    /// Fails only if the HTTP client cannot be built.
    ///
    /// Flyte is not contacted here, and that is deliberate: an engine that
    /// refused to start because the orchestrator was down would take
    /// aiwatcher down with it, and observability that stops when the thing it
    /// observes stops is the wrong way round. The `/api/v1/engine` route
    /// reports reachability instead.
    pub fn new(config: FlyteConfig) -> PortResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| PortError::Other {
                target: TARGET,
                source: Box::new(error),
            })?;
        Ok(Self {
            http,
            config: FlyteConfig {
                endpoint: config.endpoint.trim_end_matches('/').to_owned(),
                ..config
            },
            token: tokio::sync::Mutex::new(None),
        })
    }

    /// The bearer to present, minting one if the cached one is spent.
    async fn bearer(&self) -> PortResult<Option<String>> {
        if let Some(token) = &self.config.token {
            return Ok(Some(token.clone()));
        }
        let (Some(client_id), Some(client_secret), Some(token_url)) = (
            self.config.client_id.as_ref(),
            self.config.client_secret.as_ref(),
            self.config.token_url.as_ref(),
        ) else {
            // An unauthenticated Flyte — the sandbox, and a cluster that puts
            // authentication in front of the service instead of in it.
            return Ok(None);
        };

        let mut cached = self.token.lock().await;
        if let Some(token) = cached
            .as_ref()
            .filter(|token| token.expires_at > Instant::now())
        {
            return Ok(Some(token.value.clone()));
        }

        let response = self
            .http
            .post(token_url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("scope", self.config.scopes.as_str()),
            ])
            .basic_auth(client_id, Some(client_secret))
            .send()
            .await
            .map_err(|source| PortError::Unavailable {
                target: TARGET,
                message: format!("the token endpoint could not be reached: {source}"),
            })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            // Truncated and classified like any other call: a token endpoint
            // that is down is worth retrying, and one that refuses these
            // credentials will refuse them identically forever.
            return Err(classify(status, truncated(&body, 300)));
        }
        let parsed: Value = serde_json::from_str(&body).map_err(|source| PortError::Rejected {
            target: TARGET,
            message: format!("the token endpoint did not answer with JSON: {source}"),
        })?;
        let value = parsed
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| PortError::Rejected {
                target: TARGET,
                message: "the token endpoint answered without an access_token".to_owned(),
            })?
            .to_owned();
        let lifetime = parsed
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(3_600);
        // A minute of headroom. Presenting a token that expires in flight is
        // a 401 on a launch somebody just made.
        let expires_at = Instant::now() + Duration::from_secs(lifetime.saturating_sub(60).max(30));
        *cached = Some(CachedToken {
            value: value.clone(),
            expires_at,
        });
        Ok(Some(value))
    }

    /// Drop the cached token so the next call mints a fresh one.
    async fn forget_token(&self) {
        *self.token.lock().await = None;
    }

    async fn send(
        &self,
        build: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
    ) -> PortResult<Value> {
        let mut attempt = 0;
        loop {
            let mut request = build(&self.http);
            if let Some(token) = self.bearer().await? {
                request = request.bearer_auth(token);
            }
            let response = request
                .send()
                .await
                .map_err(|source| PortError::Unavailable {
                    target: TARGET,
                    message: source.to_string(),
                })?;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if status.is_success() {
                if body.trim().is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_str(&body).map_err(|source| PortError::Rejected {
                    target: TARGET,
                    message: format!("Flyte answered with something that is not JSON: {source}"),
                });
            }
            // A token that expired between two calls is the one failure here
            // worth a second attempt: the credentials are fine and the cached
            // value is not. Once, and only when there is a mint to retry with.
            if status == reqwest::StatusCode::UNAUTHORIZED
                && attempt == 0
                && self.config.client_id.is_some()
            {
                attempt += 1;
                self.forget_token().await;
                continue;
            }
            return Err(classify(status, truncated(&body, 500)));
        }
    }

    async fn get(&self, path: &str, query: Vec<(String, String)>) -> PortResult<Value> {
        let url = format!("{}{path}", self.config.endpoint);
        self.send(move |http| http.get(&url).query(&query)).await
    }

    async fn post(&self, path: &str, body: Value) -> PortResult<Value> {
        let url = format!("{}{path}", self.config.endpoint);
        self.send(move |http| http.post(&url).json(&body)).await
    }

    fn project_of(&self, requested: Option<&str>) -> String {
        requested
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.config.project)
            .to_owned()
    }

    fn domain_of(&self, requested: Option<&str>) -> String {
        requested
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.config.domain)
            .to_owned()
    }

    fn console_link(&self, project: &str, domain: &str, execution: &str) -> Option<String> {
        self.config.console_url.as_ref().map(|console| {
            format!(
                "{}/console/projects/{project}/domains/{domain}/executions/{execution}",
                console.trim_end_matches('/')
            )
        })
    }

    fn entity_link(&self, reference: &EngineRef) -> Option<String> {
        self.config.console_url.as_ref().map(|console| {
            format!(
                "{}/console/projects/{}/domains/{}/launch_plans/{}",
                console.trim_end_matches('/'),
                reference.project,
                reference.domain,
                reference.name
            )
        })
    }

    /// The newest registered version of one launch plan, whole.
    async fn newest_launch_plan(&self, reference: &EngineRef) -> PortResult<Option<Value>> {
        let path = format!(
            "/api/v1/launch_plans/{}/{}/{}",
            reference.project, reference.domain, reference.name
        );
        let page = self
            .get(
                &path,
                vec![
                    ("limit".to_owned(), "1".to_owned()),
                    ("sort_by.key".to_owned(), "created_at".to_owned()),
                    ("sort_by.direction".to_owned(), "DESCENDING".to_owned()),
                ],
            )
            .await?;
        Ok(field(&page, "launch_plans")
            .and_then(Value::as_array)
            .and_then(|plans| plans.first())
            .cloned())
    }

    /// One exact version.
    async fn launch_plan(&self, reference: &EngineRef) -> PortResult<Option<Value>> {
        let Some(version) = &reference.version else {
            return self.newest_launch_plan(reference).await;
        };
        let path = format!(
            "/api/v1/launch_plans/{}/{}/{}/{version}",
            reference.project, reference.domain, reference.name
        );
        match self.get(&path, Vec::new()).await {
            Ok(plan) => Ok(Some(plan)),
            // Flyte answers 404 for a version that was never registered, which
            // is a question with an answer rather than a broken orchestrator.
            Err(PortError::Rejected { message, .. }) if message.starts_with("404") => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// One launch plan document as something a picker can render.
fn read_launch_plan(
    plan: &Value,
    fallback: &EngineRef,
    description: &str,
    url: Option<String>,
) -> EngineWorkflow {
    let id = field(plan, "id");
    let reference = EngineRef {
        kind: EntityKind::LaunchPlan,
        project: id
            .map(|id| text(id, "project"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback.project.clone()),
        domain: id
            .map(|id| text(id, "domain"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback.domain.clone()),
        name: id
            .map(|id| text(id, "name"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback.name.clone()),
        version: id
            .map(|id| text(id, "version"))
            .filter(|value| !value.is_empty()),
    };
    let closure = field(plan, "closure");
    let interface = Interface::read(closure.and_then(|closure| field(closure, "expected_inputs")));
    let description = if description.is_empty() {
        // flytekit registers the docstring on the entity, and the launch plan
        // carries the workflow's one under its metadata.
        field(plan, "spec")
            .and_then(|spec| field(spec, "entity_metadata"))
            .map(|metadata| text(metadata, "description"))
            .unwrap_or_default()
    } else {
        description.to_owned()
    };
    let state = closure
        .map(|closure| text(closure, "state"))
        .unwrap_or_default();
    EngineWorkflow {
        id: reference.render(),
        name: reference.name.clone(),
        project: reference.project.clone(),
        domain: reference.domain.clone(),
        version: reference.version.clone().unwrap_or_default(),
        kind: EntityKind::LaunchPlan,
        // The name decides, and the description only breaks a tie it left
        // open. Reading both as one string lets "fine-tune on a **curated**
        // dataset" file a training workflow under curation, which is the one
        // mistake this hint must not make — it is what the Data Curation
        // picker defaults to.
        stage_hint: PipelineStage::guess(&reference.name)
            .or_else(|| PipelineStage::guess(&description)),
        parameters: interface.describe(),
        // `ACTIVE`, or `1` when the gateway rendered the enum as a number.
        // Anything else — including a closure that said nothing — is not
        // active, which is the safe way round for a flag the panel labels.
        active: state == "ACTIVE" || state == "1",
        updated_at: closure.and_then(|closure| {
            let stamp = text(closure, "updated_at");
            let stamp = if stamp.is_empty() {
                text(closure, "created_at")
            } else {
                stamp
            };
            time::OffsetDateTime::parse(&stamp, &time::format_description::well_known::Rfc3339).ok()
        }),
        url,
        description,
    }
}

#[async_trait]
impl WorkflowEngine for FlyteEngine {
    fn describe(&self) -> EngineDescription {
        EngineDescription {
            kind: TARGET.to_owned(),
            project: self.config.project.clone(),
            domain: self.config.domain.clone(),
            console_url: self.config.console_url.clone(),
        }
    }

    /// Names first, then one version each.
    ///
    /// The obvious listing — every launch plan in the project — returns every
    /// *version* of every launch plan, which is a picker showing `house_import`
    /// fifty-seven times. Flyte's named-entity listing is the deduplicated one
    /// and it pages cleanly, so the catalog is that listing plus the newest
    /// version of each name. It costs one request per row, which is why the
    /// page is small and why `limit` is capped.
    async fn catalog(&self, query: &CatalogQuery) -> PortResult<EngineCatalog> {
        let project = self.project_of(query.project.as_deref());
        let domain = self.domain_of(query.domain.as_deref());
        let limit = query.limit.clamp(1, 100);

        let mut parameters = vec![
            ("limit".to_owned(), limit.to_string()),
            ("sort_by.key".to_owned(), "name".to_owned()),
            ("sort_by.direction".to_owned(), "ASCENDING".to_owned()),
        ];
        if let Some(token) = &query.token {
            parameters.push(("token".to_owned(), token.clone()));
        }
        if let Some(search) = query.search.as_ref().filter(|search| !search.is_empty()) {
            // Narrowed by the engine where it can be. The same search is
            // applied again below, because it also matches a description and
            // this filter does not.
            parameters.push(("filters".to_owned(), format!("contains(name,{search})")));
        }

        let listing = self
            .get(
                &format!("/api/v1/named_entities/LAUNCH_PLAN/{project}/{domain}"),
                parameters,
            )
            .await?;
        let entities = field(&listing, "entities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut workflows = Vec::with_capacity(entities.len());
        for entity in entities {
            let id = field(&entity, "id");
            let name = id.map(|id| text(id, "name")).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let metadata = field(&entity, "metadata");
            let description = metadata
                .map(|metadata| text(metadata, "description"))
                .unwrap_or_default();
            // An archived entity is one somebody deliberately took out of
            // circulation. Listing it would be offering to start it.
            if metadata
                .map(|metadata| text(metadata, "state"))
                .is_some_and(|state| state.contains("ARCHIVED") || state == "1")
            {
                continue;
            }
            let reference = EngineRef {
                kind: EntityKind::LaunchPlan,
                project: project.clone(),
                domain: domain.clone(),
                name,
                version: None,
            };
            if !reference.is_well_formed() {
                continue;
            }
            let Some(plan) = self.newest_launch_plan(&reference).await? else {
                // A named entity with no registered version. Real, and not
                // launchable; a row offering to start it would be a lie.
                continue;
            };
            workflows.push(read_launch_plan(
                &plan,
                &reference,
                &description,
                self.entity_link(&reference),
            ));
        }

        if let Some(search) = query.search.as_ref().filter(|search| !search.is_empty()) {
            let needle = search.to_ascii_lowercase();
            workflows.retain(|workflow| {
                workflow.name.to_ascii_lowercase().contains(&needle)
                    || workflow.description.to_ascii_lowercase().contains(&needle)
            });
        }
        if let Some(stage) = query.stage {
            workflows.retain(|workflow| workflow.stage_hint == Some(stage));
        }

        Ok(EngineCatalog {
            workflows,
            next_token: field(&listing, "token")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .map(ToOwned::to_owned),
        })
    }

    async fn workflow(&self, reference: &EngineRef) -> PortResult<Option<EngineWorkflow>> {
        let reference = EngineRef {
            project: self.project_of(Some(&reference.project)),
            domain: self.domain_of(Some(&reference.domain)),
            ..reference.clone()
        };
        if !reference.is_well_formed() {
            return Err(PortError::Rejected {
                target: TARGET,
                message: format!("{reference} is not a well-formed reference"),
            });
        }
        Ok(self
            .launch_plan(&reference)
            .await?
            .map(|plan| read_launch_plan(&plan, &reference, "", self.entity_link(&reference))))
    }

    async fn launch(&self, request: LaunchRequest) -> PortResult<LaunchAccepted> {
        let reference = EngineRef::from_str(&request.workflow)?;
        // The interface is read now rather than trusted from the caller. A
        // panel that has been open since before a redeploy is rendering a
        // stale form, and binding against what it remembers would send a
        // literal typed against an interface that no longer exists.
        let plan = self
            .launch_plan(&reference)
            .await?
            .ok_or_else(|| LaunchError::Invalid(format!("{reference} is not registered")))?;
        let plan_id = field(&plan, "id").ok_or_else(|| PortError::Rejected {
            target: TARGET,
            message: "Flyte returned a launch plan with no id".to_owned(),
        })?;
        let project = text(plan_id, "project");
        let domain = text(plan_id, "domain");
        let name = text(plan_id, "name");
        // Always pinned. A launch that named no version resolves to one here,
        // so what ran is answerable later — an execution recorded against
        // "whatever was current" is not a comparison anybody can repeat.
        let version = text(plan_id, "version");

        let interface = Interface::read(
            field(&plan, "closure").and_then(|closure| field(closure, "expected_inputs")),
        );
        let mut inputs = request.inputs.clone();
        if let Some(run_id) = &request.workflow_run_id {
            // Filled in only where the entity asked for it. An input it never
            // declared is refused rather than sent, so this cannot break a
            // workflow that has never heard of aiwatcher.
            let declared = interface.describe();
            for candidate in RUN_ID_INPUTS {
                if declared.iter().any(|parameter| parameter.name == candidate) {
                    inputs
                        .entry(candidate.to_owned())
                        .or_insert_with(|| Value::String(run_id.clone()));
                    break;
                }
            }
        }
        let literals = interface.bind(&reference.render(), &inputs)?;

        let mut labels = serde_json::Map::new();
        if let Some(run_id) = request
            .workflow_run_id
            .as_ref()
            .filter(|run_id| is_label_value(run_id))
        {
            labels.insert(RUN_ID_LABEL.to_owned(), Value::String(run_id.clone()));
        }

        let execution_name = execution_name(request.workflow_run_id.as_deref(), &version);
        let mut spec = json!({
            "launch_plan": {
                "resource_type": "LAUNCH_PLAN",
                "project": project,
                "domain": domain,
                "name": name,
                "version": version,
            },
            "metadata": { "mode": "MANUAL" },
        });
        if !labels.is_empty() {
            spec["labels"] = json!({ "values": labels });
        }

        let body = json!({
            "project": project,
            "domain": domain,
            "name": execution_name,
            "spec": spec,
            "inputs": literals,
        });
        let accepted = self.post("/api/v1/executions", body).await?;
        let created = field(&accepted, "id")
            .map(|id| text(id, "name"))
            .filter(|name| !name.is_empty())
            .unwrap_or(execution_name);

        tracing::info!(
            workflow = %reference,
            %version,
            execution = %created,
            workflow_run_id = request.workflow_run_id.as_deref().unwrap_or("-"),
            requested_by = %request.requested_by,
            "launched a Flyte execution"
        );

        Ok(LaunchAccepted {
            reference: format!("{project}:{domain}:{created}"),
            url: self.console_link(&project, &domain, &created),
            workflow_run_id: request.workflow_run_id,
        })
    }

    async fn execution(&self, reference: &str) -> PortResult<Option<EngineExecution>> {
        let (project, domain, name) = read_execution_ref(reference, &self.config)?;
        let path = format!("/api/v1/executions/{project}/{domain}/{name}");
        let found = match self.get(&path, Vec::new()).await {
            Ok(found) => found,
            Err(PortError::Rejected { message, .. }) if message.starts_with("404") => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let closure = field(&found, "closure");
        let phase = closure
            .map(|closure| text(closure, "phase"))
            .unwrap_or_default();
        let message = closure
            .and_then(|closure| field(closure, "error"))
            .map(|error| text(error, "message"))
            .or_else(|| {
                closure
                    .and_then(|closure| field(closure, "abort_metadata"))
                    .map(|abort| text(abort, "cause"))
            })
            .unwrap_or_default();
        Ok(Some(EngineExecution {
            reference: format!("{project}:{domain}:{name}"),
            phase: read_phase(&phase),
            message,
            workflow: field(&found, "spec")
                .and_then(|spec| field(spec, "launch_plan"))
                .map(|plan| {
                    EngineRef {
                        kind: EntityKind::LaunchPlan,
                        project: text(plan, "project"),
                        domain: text(plan, "domain"),
                        name: text(plan, "name"),
                        version: Some(text(plan, "version")).filter(|v| !v.is_empty()),
                    }
                    .render()
                }),
            started_at: closure.and_then(|closure| {
                time::OffsetDateTime::parse(
                    &text(closure, "started_at"),
                    &time::format_description::well_known::Rfc3339,
                )
                .ok()
            }),
            url: self.console_link(&project, &domain, &name),
            workflow_run_id: field(&found, "spec")
                .and_then(|spec| field(spec, "labels"))
                .and_then(|labels| field(labels, "values"))
                .and_then(|values| values.get(RUN_ID_LABEL))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        }))
    }
}

/// Rerunning a declared workflow through Flyte.
///
/// The same adapter serves both ports on purpose. `WorkflowRunner` is the
/// rerun button on a workflow aiwatcher *observed*; `WorkflowEngine` is the
/// picker over what Flyte *holds*. A deployment whose orchestrator is Flyte
/// should not have to configure two endpoints to get both, and the alternative
/// — an HTTP runner pointed at some shim that then talks to Flyte — is a
/// second thing to deploy for no new capability.
///
/// The workflow id on the log is a producer's name for its own graph, so it is
/// read as an engine reference when it looks like one and as a launch plan
/// name in the configured project otherwise.
#[async_trait]
impl WorkflowRunner for FlyteEngine {
    async fn rerun(&self, request: RerunRequest) -> PortResult<RerunAccepted> {
        let reference = EngineRef::from_str(&request.workflow_id).unwrap_or(EngineRef {
            kind: EntityKind::LaunchPlan,
            project: self.config.project.clone(),
            domain: self.config.domain.clone(),
            name: request.workflow_id.clone(),
            version: None,
        });
        if !reference.is_well_formed() {
            return Err(PortError::Rejected {
                target: TARGET,
                message: format!(
                    "{} is not a launch plan name Flyte could hold",
                    request.workflow_id
                ),
            });
        }
        if request.from_node.is_some() {
            // Flyte's own recover/relaunch decides where an execution resumes;
            // there is no "start at this node" in CreateExecution. Saying so
            // beats starting the whole graph and reporting it as a resume.
            tracing::warn!(
                workflow = %reference,
                "Flyte cannot start mid-graph; from_node was ignored"
            );
        }
        let inputs = match request.inputs {
            Value::Object(map) => map.into_iter().collect(),
            Value::Null => BTreeMap::new(),
            other => {
                return Err(PortError::Rejected {
                    target: TARGET,
                    message: format!("inputs must be an object, and this is {other}"),
                });
            }
        };
        let accepted = self
            .launch(LaunchRequest {
                workflow: reference.render(),
                inputs,
                workflow_run_id: request.workflow_run_id,
                requested_by: "aiwatcher".to_owned(),
            })
            .await?;
        Ok(RerunAccepted {
            reference: Some(accepted.reference),
            url: accepted.url,
        })
    }
}

/// `project:domain:name`, or a bare name in the configured project.
fn read_execution_ref(
    reference: &str,
    config: &FlyteConfig,
) -> PortResult<(String, String, String)> {
    let parts: Vec<&str> = reference.split(':').collect();
    let (project, domain, name) = match parts.as_slice() {
        [name] => (
            config.project.clone(),
            config.domain.clone(),
            (*name).to_owned(),
        ),
        [project, domain, name] => (
            (*project).to_owned(),
            (*domain).to_owned(),
            (*name).to_owned(),
        ),
        _ => {
            return Err(PortError::Rejected {
                target: TARGET,
                message: format!("{reference:?} is not an execution reference"),
            });
        }
    };
    // Interpolated into Flyte's own URL, so the same rule as `EngineRef`.
    for part in [&project, &domain, &name] {
        if part.is_empty()
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(PortError::Rejected {
                target: TARGET,
                message: format!("{reference:?} is not an execution reference"),
            });
        }
    }
    Ok((project, domain, name))
}

fn read_phase(phase: &str) -> EnginePhase {
    match phase {
        "QUEUED" | "UNDEFINED" => EnginePhase::Queued,
        "RUNNING" | "SUCCEEDING" | "FAILING" | "ABORTING" => EnginePhase::Running,
        "SUCCEEDED" => EnginePhase::Succeeded,
        "FAILED" | "TIMED_OUT" => EnginePhase::Failed,
        "ABORTED" => EnginePhase::Aborted,
        _ => EnginePhase::Unknown,
    }
}

/// Whether a value may be a Kubernetes label, which is what a Flyte execution
/// label becomes.
fn is_label_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

/// A name Flyte will accept for the execution.
///
/// Flyte wants a short DNS-ish label. Deriving it from the correlation id
/// rather than minting an unrelated one means the execution in Flyte's console
/// and the execution in aiwatcher's panel visibly share a prefix, which is the
/// difference between a join somebody can check and one they have to trust.
fn execution_name(seed: Option<&str>, version: &str) -> String {
    let mut name = String::with_capacity(20);
    name.push('a');
    let source = seed.unwrap_or(version);
    for byte in source.bytes() {
        if name.len() == 20 {
            break;
        }
        if byte.is_ascii_alphanumeric() {
            name.push(byte.to_ascii_lowercase() as char);
        }
    }
    // A seed that was short or entirely punctuation still has to produce a
    // usable name; padding beats refusing the launch.
    while name.len() < 20 {
        name.push('0');
    }
    name
}

/// 4xx means the request is wrong and will still be wrong next time; 5xx, 429
/// and a transport failure are worth a retry. The API branches on this to pick
/// between a 502 and a 503.
fn classify(status: reqwest::StatusCode, message: String) -> PortError {
    let message = format!("{status}: {message}");
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        PortError::Unavailable {
            target: TARGET,
            message,
        }
    } else {
        PortError::Rejected {
            target: TARGET,
            message,
        }
    }
}

/// An orchestrator's error page is not bounded by anything aiwatcher controls.
fn truncated(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn config() -> FlyteConfig {
        FlyteConfig {
            endpoint: "http://flyteadmin:80".to_owned(),
            project: "planner".to_owned(),
            domain: "production".to_owned(),
            console_url: Some("https://flyte.example".to_owned()),
            ..FlyteConfig::default()
        }
    }

    #[test]
    fn neither_secret_appears_in_a_debug_rendering() {
        // A secret reaches a log through a derived `Debug` three layers up on
        // something that happens to hold this, not through somebody printing
        // it on purpose.
        let rendered = format!(
            "{:?}",
            FlyteConfig {
                token: Some("static-s3cr3t".to_owned()),
                client_secret: Some("oauth-s3cr3t".to_owned()),
                client_id: Some("aiwatcher".to_owned()),
                ..config()
            }
        );
        assert!(!rendered.contains("s3cr3t"), "{rendered}");
        assert!(rendered.contains("token: true"), "{rendered}");
        assert!(rendered.contains("client_secret: true"), "{rendered}");

        let engine = FlyteEngine::new(config()).expect("builds");
        assert!(!format!("{engine:?}").contains("s3cr3t"));
    }

    #[test]
    fn an_execution_name_is_derived_from_the_correlation_id() {
        let name = execution_name(Some("018f3a2b-7c41-7b3e-9d55-2f6a1c0b8e77"), "v3");
        assert_eq!(name.len(), 20);
        assert!(name.starts_with("a018f3a2b7c417b3e9d5"), "{name}");
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
            "{name} has to be a DNS-ish label"
        );
    }

    #[test]
    fn a_seed_that_is_mostly_punctuation_still_produces_a_usable_name() {
        let name = execution_name(Some("--"), "v1");
        assert_eq!(name.len(), 20);
        assert!(name.starts_with('a'));
    }

    #[test]
    fn only_a_correlation_id_that_could_be_a_label_becomes_one() {
        assert!(is_label_value("018f3a2b7c417b3e9d552f6a1c0b8e77"));
        assert!(!is_label_value(&"x".repeat(64)), "63 bytes is the ceiling");
        assert!(!is_label_value("-leading"));
        assert!(!is_label_value("has space"));
        assert!(!is_label_value(""));
    }

    #[test]
    fn an_execution_reference_carrying_a_path_is_refused() {
        let config = config();
        assert!(read_execution_ref("../../admin", &config).is_err());
        assert!(read_execution_ref("p:d:../x", &config).is_err());
        assert!(read_execution_ref("p:d:e:extra", &config).is_err());
        assert_eq!(
            read_execution_ref("abc123", &config).expect("a bare name"),
            (
                "planner".to_owned(),
                "production".to_owned(),
                "abc123".to_owned()
            )
        );
    }

    #[test]
    fn a_phase_flyte_does_not_have_yet_is_unknown_rather_than_running() {
        // A spinner for a state nobody understood is a panel claiming to know
        // something it does not.
        assert_eq!(read_phase("RUNNING"), EnginePhase::Running);
        assert_eq!(read_phase("SUCCEEDED"), EnginePhase::Succeeded);
        assert_eq!(read_phase("TIMED_OUT"), EnginePhase::Failed);
        assert_eq!(read_phase("SOMETHING_NEW"), EnginePhase::Unknown);
    }

    #[test]
    fn a_5xx_is_retryable_and_a_4xx_is_not() {
        assert!(classify(reqwest::StatusCode::BAD_GATEWAY, String::new()).is_retryable());
        assert!(classify(reqwest::StatusCode::TOO_MANY_REQUESTS, String::new()).is_retryable());
        assert!(!classify(reqwest::StatusCode::NOT_FOUND, String::new()).is_retryable());
    }

    #[test]
    fn a_launch_plan_becomes_a_row_a_picker_can_render() {
        let plan = json!({
            "id": {
                "resource_type": "LAUNCH_PLAN",
                "project": "planner",
                "domain": "production",
                "name": "house_dataset_curation",
                "version": "v7"
            },
            "spec": { "entity_metadata": { "description": "Curate the house corpus" } },
            "closure": {
                "state": "ACTIVE",
                "updated_at": "2026-08-30T10:00:00Z",
                "expected_inputs": { "parameters": {
                    "since": { "var": { "type": { "simple": "DATETIME" } }, "required": true }
                } }
            }
        });
        let reference = EngineRef {
            kind: EntityKind::LaunchPlan,
            project: "planner".to_owned(),
            domain: "production".to_owned(),
            name: "house_dataset_curation".to_owned(),
            version: None,
        };
        let workflow = read_launch_plan(&plan, &reference, "", None);
        assert_eq!(
            workflow.id,
            "lp:planner:production:house_dataset_curation:v7"
        );
        assert!(workflow.active);
        assert_eq!(workflow.stage_hint, Some(PipelineStage::Curation));
        assert_eq!(workflow.parameters.len(), 1);
        assert!(workflow.parameters[0].required);
        assert_eq!(workflow.description, "Curate the house corpus");
        assert!(workflow.updated_at.is_some());
    }

    #[test]
    fn a_description_never_overrules_the_name_when_the_stage_is_guessed() {
        // "Fine-tune on a curated dataset" contains a curation keyword, and
        // filing that launch plan under curation would put a training job in
        // the picker Data Curation defaults to.
        let plan = json!({
            "id": { "project": "p", "domain": "d", "name": "llama_finetune", "version": "v1" },
            "closure": {}
        });
        let reference = EngineRef {
            kind: EntityKind::LaunchPlan,
            project: "p".to_owned(),
            domain: "d".to_owned(),
            name: "llama_finetune".to_owned(),
            version: None,
        };
        let workflow = read_launch_plan(&plan, &reference, "Fine-tune on a curated dataset", None);
        assert_eq!(workflow.stage_hint, Some(PipelineStage::Training));
    }

    #[test]
    fn a_launch_plan_whose_state_is_missing_is_not_reported_active() {
        let plan = json!({
            "id": { "project": "p", "domain": "d", "name": "n", "version": "v" },
            "closure": {}
        });
        let reference = EngineRef {
            kind: EntityKind::LaunchPlan,
            project: "p".to_owned(),
            domain: "d".to_owned(),
            name: "n".to_owned(),
            version: None,
        };
        assert!(!read_launch_plan(&plan, &reference, "", None).active);
    }
}
