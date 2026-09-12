//! The judge a scorecard may ask, over one OpenAI-compatible endpoint.
//!
//! In the work role, because asking a model is a socket and a credential — the
//! query engine's reason, for a different service. The address is
//! `AIWATCHER_JUDGE_URL` and the profile `AIWATCHER_JUDGE_PROVIDER`; without
//! both this registers no executor, so a `judge_evaluation` attempt is never
//! claimed here and the start route refuses a judged run naming them.
//!
//! **A profile is declared, never detected.** `llamacpp` turns a thinking
//! model's thinking off, because a reply that spends its tokens reasoning never
//! reaches the JSON object it was asked for; `openai` sends the request as
//! written. Which one a result was measured under is pinned in its context.
//!
//! **Nothing the endpoint says about an answer is logged.** A refusal carries
//! the status and the provider's own error message, clipped, and never the
//! body of a reply: a reply can repeat the answer it was shown.

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_evaluation::{
    EvaluationError, JudgeCall, JudgeFailure, JudgeModel, JudgeReply, Registry as Evaluations,
    Served,
};
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::config::Config;

/// One judge endpoint.
#[derive(Debug)]
pub struct OpenAiJudge {
    client: reqwest::Client,
    url: String,
    token: Option<String>,
    provider: String,
}

impl OpenAiJudge {
    /// The judge this configuration names, or `None` when it names none.
    ///
    /// # Errors
    ///
    /// When the HTTP client cannot be built.
    pub fn from_config(config: &Config) -> Result<Option<Self>, reqwest::Error> {
        let (Some(url), Some(provider)) = (&config.judge_url, &config.judge_provider) else {
            return Ok(None);
        };
        Ok(Some(Self::new(
            url,
            provider,
            config.judge_token.clone(),
            config.judge_timeout,
        )?))
    }

    /// # Errors
    ///
    /// When the HTTP client cannot be built.
    pub fn new(
        url: &str,
        provider: &str,
        token: Option<String>,
        timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder().timeout(timeout).build()?,
            url: format!("{}/chat/completions", url.trim_end_matches('/')),
            token,
            provider: provider.to_owned(),
        })
    }

    fn body(&self, call: &JudgeCall) -> Value {
        let mut body = json!({
            "model": call.model,
            "messages": call.messages,
            "temperature": call.temperature,
            "max_tokens": call.max_tokens,
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "judgement", "strict": true, "schema": call.schema},
            },
        });
        if let Some(seed) = call.seed {
            body["seed"] = json!(seed);
        }
        if self.provider == "llamacpp" {
            body["chat_template_kwargs"] = json!({"enable_thinking": false});
        }
        body
    }
}

#[async_trait]
impl JudgeModel for OpenAiJudge {
    fn provider(&self) -> &str {
        &self.provider
    }

    async fn ask(&self, call: &JudgeCall) -> Result<JudgeReply, JudgeFailure> {
        let mut request = self.client.post(&self.url).json(&self.body(call));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| JudgeFailure::Unavailable(error.without_url().to_string()))?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.as_u16() == 429 || status.is_server_error() {
            return Err(JudgeFailure::Unavailable(refusal(status, &body)));
        }
        if !status.is_success() {
            return Err(JudgeFailure::Refused(refusal(status, &body)));
        }
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                JudgeFailure::Refused("the judge answered with no message content".to_owned())
            })?;
        Ok(JudgeReply {
            content: content.to_owned(),
            served: Served {
                model: named(&body["model"]),
                fingerprint: named(&body["system_fingerprint"]),
            },
        })
    }
}

/// A provider's name for something, as one bounded line, or nothing.
fn named(value: &Value) -> Option<String> {
    let name: String = value
        .as_str()?
        .chars()
        .filter(|letter| !letter.is_control())
        .take(200)
        .collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// A status and the provider's own words about it, bounded.
fn refusal(status: reqwest::StatusCode, body: &Value) -> String {
    let said = body["error"]["message"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .unwrap_or_default();
    let said: String = said.chars().take(200).collect();
    if said.is_empty() {
        format!("{status}")
    } else {
        format!("{status}: {said}")
    }
}

/// A judge that answers from what one declared run was already told.
///
/// Every reply is kept before it is used, under the run and the question, so
/// an attempt after a failure asks only what nobody answered yet — and an
/// attempt after a publication whose settlement was lost folds the same bytes
/// and lands on the result that is already there, rather than on a conflict.
#[derive(Debug)]
pub struct Remembering {
    judge: Arc<dyn JudgeModel>,
    evaluations: Arc<Evaluations>,
    declaration: String,
}

impl Remembering {
    #[must_use]
    pub fn new(
        judge: Arc<dyn JudgeModel>,
        evaluations: Arc<Evaluations>,
        declaration: impl Into<String>,
    ) -> Self {
        Self {
            judge,
            evaluations,
            declaration: declaration.into(),
        }
    }
}

#[async_trait]
impl JudgeModel for Remembering {
    fn provider(&self) -> &str {
        self.judge.provider()
    }

    async fn ask(&self, call: &JudgeCall) -> Result<JudgeReply, JudgeFailure> {
        if let Some(reply) = self
            .evaluations
            .remembered_reply(&self.declaration, call)
            .await
            .map_err(forgetting)?
        {
            return Ok(reply);
        }
        let reply = self.judge.ask(call).await?;
        self.evaluations
            .remember_reply(&self.declaration, call, reply)
            .await
            .map_err(forgetting)
    }
}

/// A store that could not keep or give back a reply. Worth another attempt
/// only when the store said so: a kept reply that does not read will not read
/// next time either.
fn forgetting(error: EvaluationError) -> JudgeFailure {
    match &error {
        EvaluationError::Storage(port) if port.is_retryable() => {
            JudgeFailure::Unavailable(format!("the replies kept for this run: {error}"))
        }
        _ => JudgeFailure::Refused(format!("the replies kept for this run: {error}")),
    }
}

/// Put every question to the judge, a bounded number at a time.
///
/// All of them or none: a provider that stops answering halfway through is an
/// outage rather than half a measurement, so the first failure ends the step.
/// Handed a [`Remembering`] judge, the attempt after it asks only what was not
/// answered. The replies come back in the order the questions were asked.
///
/// A stop ends it too, and sooner: the questions in flight are dropped, which
/// closes their connections, and the ones not yet asked are never asked. What
/// already came back was kept by the [`Remembering`] judge as it arrived.
///
/// # Errors
///
/// The first [`JudgeFailure`] any question met, or `Unavailable` once `stop`
/// was requested — the caller reads the stop's own reason from the signal.
pub async fn ask_all(
    judge: &Arc<dyn JudgeModel>,
    calls: Vec<JudgeCall>,
    concurrency: usize,
    stop: &aiwatcher_execution::StopSignal,
) -> Result<Vec<JudgeReply>, JudgeFailure> {
    let permits = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
    let mut asking = tokio::task::JoinSet::new();
    let total = calls.len();
    for (index, call) in calls.into_iter().enumerate() {
        let judge = Arc::clone(judge);
        let permits = Arc::clone(&permits);
        asking.spawn(async move {
            let _permit = permits
                .acquire_owned()
                .await
                .map_err(|error| JudgeFailure::Unavailable(error.to_string()))?;
            judge.ask(&call).await.map(|reply| (index, reply))
        });
    }
    let mut replies: Vec<Option<JudgeReply>> = vec![None; total];
    loop {
        let joined = tokio::select! {
            biased;
            _ = stop.stopped() => {
                asking.abort_all();
                return Err(JudgeFailure::Unavailable(
                    "stopped before every question was answered".into(),
                ));
            }
            joined = asking.join_next() => joined,
        };
        let Some(joined) = joined else { break };
        let (index, reply) =
            joined.map_err(|error| JudgeFailure::Unavailable(error.to_string()))??;
        replies[index] = Some(reply);
    }
    replies
        .into_iter()
        .map(|reply| {
            reply.ok_or_else(|| JudgeFailure::Unavailable("a question went unanswered".into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct Counting {
        asked: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl JudgeModel for Counting {
        fn provider(&self) -> &str {
            "llamacpp"
        }

        async fn ask(&self, call: &JudgeCall) -> Result<JudgeReply, JudgeFailure> {
            let asked = call.messages[1].content.clone();
            if asked.contains("outage") {
                return Err(JudgeFailure::Unavailable("503".into()));
            }
            self.asked.lock().expect("a lock").push(asked.clone());
            Ok(JudgeReply {
                content: asked,
                served: Served::default(),
            })
        }
    }

    fn call(answer: &str) -> JudgeCall {
        JudgeCall {
            model: "gemma".into(),
            messages: vec![
                aiwatcher_evaluation::JudgeMessage {
                    role: "system",
                    content: "rubric".into(),
                },
                aiwatcher_evaluation::JudgeMessage {
                    role: "user",
                    content: answer.into(),
                },
            ],
            temperature: 0.0,
            seed: Some(1),
            max_tokens: 64,
            schema: aiwatcher_evaluation::reply_schema(&aiwatcher_evaluation::Scale::Flag),
        }
    }

    #[tokio::test]
    async fn replies_come_back_in_the_order_the_questions_were_asked() {
        let judge: Arc<dyn JudgeModel> = Arc::new(Counting::default());
        let calls = (0..20).map(|n| call(&format!("answer {n}"))).collect();
        let replies = ask_all(&judge, calls, 3, &aiwatcher_execution::StopSignal::new())
            .await
            .expect("every question answers");
        for (n, reply) in replies.iter().enumerate() {
            assert_eq!(reply.content, format!("answer {n}"));
        }
    }

    #[tokio::test]
    async fn one_outage_ends_the_measurement_rather_than_half_of_it() {
        let judge: Arc<dyn JudgeModel> = Arc::new(Counting::default());
        let calls = vec![call("fine"), call("outage"), call("fine too")];
        assert!(matches!(
            ask_all(&judge, calls, 1, &aiwatcher_execution::StopSignal::new()).await,
            Err(JudgeFailure::Unavailable(_))
        ));
    }

    /// A judge that never answers, the way a stalled provider does not.
    #[derive(Debug)]
    struct Silent;

    #[async_trait]
    impl JudgeModel for Silent {
        fn provider(&self) -> &str {
            "llamacpp"
        }

        async fn ask(&self, _call: &JudgeCall) -> Result<JudgeReply, JudgeFailure> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn a_stop_ends_the_asking_without_waiting_for_the_questions_in_flight() {
        let judge: Arc<dyn JudgeModel> = Arc::new(Silent);
        let stop = aiwatcher_execution::StopSignal::new();
        let calls = (0..5).map(|n| call(&format!("answer {n}"))).collect();
        let (asked, ()) = tokio::join!(ask_all(&judge, calls, 2, &stop), async {
            tokio::task::yield_now().await;
            stop.stop(aiwatcher_execution::StopReason::RunStopping);
        });
        assert!(matches!(asked, Err(JudgeFailure::Unavailable(_))));
    }

    #[test]
    fn a_providers_name_for_what_served_is_one_bounded_line_or_nothing() {
        assert_eq!(
            named(&json!("gpt-4o-2024-08-06")).as_deref(),
            Some("gpt-4o-2024-08-06")
        );
        assert_eq!(named(&json!("b6500-\nabc")).as_deref(), Some("b6500-abc"));
        assert_eq!(
            named(&json!("x".repeat(500))).map(|name| name.len()),
            Some(200)
        );
        assert_eq!(named(&json!("  ")), None);
        assert_eq!(named(&json!(7)), None);
        assert_eq!(named(&Value::Null), None);
    }

    #[test]
    fn a_llamacpp_judge_is_asked_not_to_think_and_an_openai_one_is_asked_as_written() {
        let llama = OpenAiJudge::new(
            "http://127.0.0.1:1/v1/",
            "llamacpp",
            None,
            Duration::from_secs(1),
        )
        .expect("a client");
        assert_eq!(llama.url, "http://127.0.0.1:1/v1/chat/completions");
        let body = llama.body(&call("x"));
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["required"],
            json!(["value"])
        );
        assert_eq!(body["seed"], 1);
        let openai = OpenAiJudge::new(
            "http://127.0.0.1:1/v1",
            "openai",
            None,
            Duration::from_secs(1),
        )
        .expect("a client");
        assert!(
            openai
                .body(&call("x"))
                .get("chat_template_kwargs")
                .is_none()
        );
    }
}
