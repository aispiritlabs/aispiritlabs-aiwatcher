//! The scorer service a scorecard's external metrics are measured by.
//!
//! In the work role, for the judge's reason: the service is a socket, and a
//! graded metric's model behind it is a credential. The address is
//! `AIWATCHER_SCORER_URL`; without it this registers no executor, so an
//! `external_evaluation` attempt is never claimed here and the start route
//! refuses such a run naming it.
//!
//! The service speaks the contract in `services/scorers` and nothing here names
//! a framework. Two routes: the catalog it describes itself with, which this
//! role records in the evaluation registry for the serve role to pin cards
//! against, and one question per case.
//!
//! **Nothing the service says about an answer is logged or kept.** A reply is
//! a number or the adapter's own sentence, a refusal is its status and its own
//! message, clipped, and a kept reply holds the same.

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_evaluation::{
    EvaluationError, ExternalCall, ExternalReply, ExternalScorers, Registry as Evaluations,
    ScorerCatalog, ScorerFailure,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// How often the catalog is read again after the read at start-up.
///
/// Five minutes: a catalog changes when somebody installs another framework
/// release or points the service at another model, which is a deployment, and
/// a card published in the minutes between is refused naming what it lacks.
pub const CATALOG_EVERY: Duration = Duration::from_secs(5 * 60);

/// One scorer service.
#[derive(Debug)]
pub struct ScorerService {
    client: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl ScorerService {
    /// The service this configuration names, or `None` when it names none.
    ///
    /// # Errors
    ///
    /// When the HTTP client cannot be built.
    pub fn from_config(config: &Config) -> Result<Option<Self>, reqwest::Error> {
        let Some(url) = &config.scorer_url else {
            return Ok(None);
        };
        Ok(Some(Self::new(
            url,
            config.scorer_token.clone(),
            config.scorer_timeout,
        )?))
    }

    /// # Errors
    ///
    /// When the HTTP client cannot be built.
    pub fn new(
        url: &str,
        token: Option<String>,
        timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder().timeout(timeout).build()?,
            base: url.trim_end_matches('/').to_owned(),
            token,
        })
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, ScorerFailure> {
        let request = match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        let response = request
            .send()
            .await
            .map_err(|error| ScorerFailure::Unavailable(error.without_url().to_string()))?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.as_u16() == 429 || status.is_server_error() {
            return Err(ScorerFailure::Unavailable(refusal(status, &body)));
        }
        if !status.is_success() {
            return Err(ScorerFailure::Refused(refusal(status, &body)));
        }
        Ok(body)
    }
}

#[async_trait]
impl ExternalScorers for ScorerService {
    async fn catalog(&self) -> Result<ScorerCatalog, ScorerFailure> {
        let body = self
            .send(self.client.get(format!("{}/scorers/catalog", self.base)))
            .await?;
        serde_json::from_value(body).map_err(|error| {
            ScorerFailure::Refused(format!(
                "the catalog does not read as the contract: {error}"
            ))
        })
    }

    async fn score(&self, call: &ExternalCall) -> Result<ExternalReply, ScorerFailure> {
        let body = self
            .send(
                self.client
                    .post(format!("{}/scorers/score", self.base))
                    .json(&request_body(call)),
            )
            .await?;
        let reply = body["replies"]
            .as_array()
            .filter(|replies| replies.len() == 1)
            .and_then(|replies| replies.first())
            .ok_or_else(|| {
                ScorerFailure::Refused("the service answered with no reply for the case".into())
            })?;
        serde_json::from_value(reply.clone()).map_err(|error| {
            ScorerFailure::Refused(format!("the reply does not read as the contract: {error}"))
        })
    }
}

/// One case, as the contract's `POST /scorers/score` takes it.
fn request_body(call: &ExternalCall) -> Value {
    let mut body = json!({
        "adapter": call.adapter,
        "metric": call.metric,
        "declared": call.declared,
        "cases": [call.case],
    });
    if !call.parameters.is_empty() {
        body["parameters"] = json!(call.parameters);
    }
    body
}

/// A status and the service's own words about it, bounded.
fn refusal(status: reqwest::StatusCode, body: &Value) -> String {
    let said: String = body["message"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .unwrap_or_default()
        .chars()
        .filter(|letter| !letter.is_control())
        .take(200)
        .collect();
    if said.is_empty() {
        format!("{status}")
    } else {
        format!("{status}: {said}")
    }
}

/// A scorer service that answers from what one declared run was already told.
///
/// The judge's [`super::judge::Remembering`], for the same reasons: an attempt
/// after a failure asks only what nobody answered, and an attempt after a lost
/// settlement folds the same bytes as the publication already there.
#[derive(Debug)]
pub struct Remembering {
    scorers: Arc<dyn ExternalScorers>,
    evaluations: Arc<Evaluations>,
    declaration: String,
}

impl Remembering {
    #[must_use]
    pub fn new(
        scorers: Arc<dyn ExternalScorers>,
        evaluations: Arc<Evaluations>,
        declaration: impl Into<String>,
    ) -> Self {
        Self {
            scorers,
            evaluations,
            declaration: declaration.into(),
        }
    }
}

#[async_trait]
impl ExternalScorers for Remembering {
    async fn catalog(&self) -> Result<ScorerCatalog, ScorerFailure> {
        self.scorers.catalog().await
    }

    async fn score(&self, call: &ExternalCall) -> Result<ExternalReply, ScorerFailure> {
        if let Some(reply) = self
            .evaluations
            .remembered_score(&self.declaration, call)
            .await
            .map_err(forgetting)?
        {
            return Ok(reply);
        }
        let reply = self.scorers.score(call).await?;
        self.evaluations
            .remember_score(&self.declaration, call, &reply)
            .await
            .map_err(forgetting)
    }
}

fn forgetting(error: EvaluationError) -> ScorerFailure {
    match &error {
        EvaluationError::Storage(port) if port.is_retryable() => {
            ScorerFailure::Unavailable(format!("the replies kept for this run: {error}"))
        }
        _ => ScorerFailure::Refused(format!("the replies kept for this run: {error}")),
    }
}

/// Put every case to the scorer service, a bounded number at a time.
///
/// All of them or none, and a stop ends it sooner — [`super::judge::ask_all`]'s
/// rules, for the other kind of question. The replies come back in the order
/// the calls were made.
///
/// # Errors
///
/// The first [`ScorerFailure`] any call met, or `Unavailable` once `stop` was
/// requested.
pub async fn score_all(
    scorers: &Arc<dyn ExternalScorers>,
    calls: Vec<ExternalCall>,
    concurrency: usize,
    stop: &aiwatcher_execution::StopSignal,
) -> Result<Vec<ExternalReply>, ScorerFailure> {
    let permits = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
    let mut asking = tokio::task::JoinSet::new();
    let total = calls.len();
    for (index, call) in calls.into_iter().enumerate() {
        let scorers = Arc::clone(scorers);
        let permits = Arc::clone(&permits);
        asking.spawn(async move {
            let _permit = permits
                .acquire_owned()
                .await
                .map_err(|error| ScorerFailure::Unavailable(error.to_string()))?;
            scorers.score(&call).await.map(|reply| (index, reply))
        });
    }
    let mut replies: Vec<Option<ExternalReply>> = vec![None; total];
    loop {
        let joined = tokio::select! {
            biased;
            _ = stop.stopped() => {
                asking.abort_all();
                return Err(ScorerFailure::Unavailable(
                    "stopped before every case was scored".into(),
                ));
            }
            joined = asking.join_next() => joined,
        };
        let Some(joined) = joined else { break };
        let (index, reply) =
            joined.map_err(|error| ScorerFailure::Unavailable(error.to_string()))??;
        replies[index] = Some(reply);
    }
    replies
        .into_iter()
        .map(|reply| reply.ok_or_else(|| ScorerFailure::Unavailable("a case went unscored".into())))
        .collect()
}

/// Read the service's catalog at start-up and every [`CATALOG_EVERY`], and
/// record it where the serve role pins cards against it.
///
/// A read or a record that fails is a warning, and the last catalog recorded
/// stands: a card is pinned against what the service said, and a service that
/// is briefly down has not stopped implementing anything.
#[must_use]
pub fn spawn_catalog(
    scorers: Arc<dyn ExternalScorers>,
    evaluations: Arc<Evaluations>,
    owner: String,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(CATALOG_EVERY);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            match scorers.catalog().await {
                Ok(catalog) => {
                    let now = time::OffsetDateTime::now_utc().unix_timestamp();
                    match evaluations
                        .record_scorer_catalog(&catalog, &owner, now)
                        .await
                    {
                        Ok(recorded) => tracing::debug!(
                            adapters = recorded.catalog.adapters.len(),
                            "recorded the scorer service's catalog"
                        ),
                        Err(error) => tracing::warn!(
                            %error,
                            "the scorer service's catalog could not be recorded"
                        ),
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "the scorer service's catalog could not be read");
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aiwatcher_evaluation::{Aggregation, ExternalCase, ExternalDeclaration, MetricDirection};

    #[derive(Debug)]
    struct Silent;

    #[async_trait]
    impl ExternalScorers for Silent {
        async fn catalog(&self) -> Result<ScorerCatalog, ScorerFailure> {
            Err(ScorerFailure::Unavailable("silent".into()))
        }

        async fn score(&self, _call: &ExternalCall) -> Result<ExternalReply, ScorerFailure> {
            std::future::pending().await
        }
    }

    fn call() -> ExternalCall {
        ExternalCall {
            adapter: "opik".into(),
            metric: "equals".into(),
            declared: ExternalDeclaration {
                version: "2.2.59".into(),
                model: None,
                unit: "ratio".into(),
                direction: MetricDirection::Higher,
                aggregation: Aggregation::Rate,
                reads: vec![aiwatcher_evaluation::CaseSide::Answer],
                range: None,
            },
            parameters: serde_json::Map::new(),
            case: ExternalCase {
                input: None,
                answer: json!("Warsaw"),
                expected: None,
            },
        }
    }

    #[tokio::test]
    async fn a_stop_ends_the_scoring_without_waiting_for_the_cases_in_flight() {
        let scorers: Arc<dyn ExternalScorers> = Arc::new(Silent);
        let stop = aiwatcher_execution::StopSignal::new();
        let (scored, ()) =
            tokio::join!(score_all(&scorers, vec![call(), call()], 2, &stop), async {
                tokio::task::yield_now().await;
                stop.stop(aiwatcher_execution::StopReason::RunStopping);
            });
        assert!(matches!(scored, Err(ScorerFailure::Unavailable(_))));
    }

    #[test]
    fn what_this_sends_and_reads_is_the_contract_the_service_is_tested_against() {
        // The same fixtures `services/scorers` reads, so a change on either
        // side of the wire fails a test on that side.
        let catalog: ScorerCatalog = serde_json::from_str(include_str!(
            "../../../../contracts/fixtures/scorers-v1/catalog.json"
        ))
        .expect("the catalog fixture reads");
        catalog
            .validate()
            .expect("and is one a card can be pinned against");
        let (adapter, metric) = catalog
            .find("deepeval", "answer_relevancy")
            .expect("the fixture's graded metric");
        let call = ExternalCall {
            adapter: adapter.name.clone(),
            metric: metric.metric.clone(),
            declared: ExternalDeclaration::of(adapter, metric),
            parameters: serde_json::Map::new(),
            case: ExternalCase {
                input: Some(json!("What is the capital of Poland?")),
                answer: json!("Warsaw"),
                expected: None,
            },
        };
        let request: Value = serde_json::from_str(include_str!(
            "../../../../contracts/fixtures/scorers-v1/score-request.json"
        ))
        .expect("the request fixture reads");
        assert_eq!(request_body(&call), request);
        let replies: Value = serde_json::from_str(include_str!(
            "../../../../contracts/fixtures/scorers-v1/score-replies.json"
        ))
        .expect("the replies fixture reads");
        let replies: Vec<ExternalReply> =
            serde_json::from_value(replies["replies"].clone()).expect("replies read");
        assert_eq!(replies[0].value, Some(0.75));
        assert!(
            replies[1]
                .failed
                .as_deref()
                .is_some_and(|reason| reason.starts_with("MetricError"))
        );
    }

    #[test]
    fn a_refusal_carries_the_services_own_sentence_bounded_to_one_line() {
        let said = refusal(
            reqwest::StatusCode::CONFLICT,
            &json!({"message": format!("declared elsewhere\n{}", "x".repeat(400))}),
        );
        assert!(said.starts_with("409 Conflict: declared elsewhere"));
        assert!(!said.contains('\n'));
        assert!(said.len() < 260);
    }
}
