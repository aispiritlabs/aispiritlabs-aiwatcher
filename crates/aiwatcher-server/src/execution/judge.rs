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

use aiwatcher_evaluation::{JudgeCall, JudgeFailure, JudgeModel, JudgeReply};
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
        })
    }
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

/// Put every question to the judge, a bounded number at a time.
///
/// All of them or none: a provider that stops answering halfway through is an
/// outage rather than half a measurement, so the first failure ends the step
/// and the attempt's retry asks again. The replies come back in the order the
/// questions were asked.
///
/// # Errors
///
/// The first [`JudgeFailure`] any question met.
pub async fn ask_all(
    judge: &Arc<dyn JudgeModel>,
    calls: Vec<JudgeCall>,
    concurrency: usize,
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
    while let Some(joined) = asking.join_next().await {
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
            Ok(JudgeReply { content: asked })
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
        let replies = ask_all(&judge, calls, 3)
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
            ask_all(&judge, calls, 1).await,
            Err(JudgeFailure::Unavailable(_))
        ));
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
