//! Chat Completions over reqwest, with backoff. Stateless: every call sends the whole input, so the run
//! log on disk is the only state. See agent/AGENTS.md, LLM.

use std::time::Duration;

use serde_json::{Value, json};

use crate::connect::http_client;

/// The first wait after a failed call, doubled after each further failure up to `backoff_max`.
const FIRST_BACKOFF: Duration = Duration::from_secs(1);

/// How long one call may take before it counts as failed.
const CALL_TIME: Duration = Duration::from_secs(600);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The input is larger than the model takes: compact, do not retry.
    #[error("the input is over the model's context length")]
    ContextLength,
    /// Any other answer than 2xx, with what `Retry-After` asked.
    #[error("HTTP {status}")]
    Status {
        status: u16,
        retry_after: Option<Duration>,
    },
    #[error("the network: {0}")]
    Network(String),
    /// A 2xx answer that is not Chat Completions' shape.
    #[error("the answer could not be read: {0}")]
    Answer(String),
}

impl Error {
    /// The short detail the presence text carries, such as `HTTP 429`.
    pub fn detail(&self) -> String {
        match self {
            Self::ContextLength => "context length".to_string(),
            Self::Status { status, .. } => format!("HTTP {status}"),
            Self::Network(_) => "network".to_string(),
            Self::Answer(_) => "bad answer".to_string(),
        }
    }
}

/// One tool call the model made.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Its arguments as the model wrote them, JSON text.
    pub arguments: String,
}

/// The model's answer to one call.
#[derive(Debug, Clone)]
pub struct Reply {
    /// The assistant message as the provider sent it, to go back into the input as it is.
    pub message: Value,
    pub content: Option<String>,
    pub tool_call_list: Vec<ToolCall>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
}

impl Reply {
    /// Reads an assistant message back, as a restart finds it saved.
    pub fn of(message: Value, tokens_in: Option<u64>, tokens_out: Option<u64>) -> Self {
        let content = match message.get("content") {
            Some(Value::String(text)) => Some(text.clone()),
            _ => None,
        };
        let mut tool_call_list = Vec::new();
        if let Some(Value::Array(calls)) = message.get("tool_calls") {
            for call in calls {
                let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
                let function = call.get("function");
                let name = function
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = match function.and_then(|function| function.get("arguments")) {
                    Some(Value::String(text)) => text.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                tool_call_list.push(ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments,
                });
            }
        }
        Self {
            message,
            content,
            tool_call_list,
            tokens_in,
            tokens_out,
        }
    }
}

/// One provider, as `[llm]` names it.
pub struct Llm {
    http: reqwest::Client,
    url: String,
    key: String,
    model: String,
    backoff_max: Duration,
}

impl Llm {
    pub fn new(url: &str, key: &str, model: &str, backoff_max: Duration) -> Self {
        Self {
            http: http_client(false, CALL_TIME),
            url: url.trim_end_matches('/').to_string(),
            key: key.to_string(),
            model: model.to_string(),
            backoff_max,
        }
    }

    /// One call: the messages so far and the tools the model may call, none for a call that must
    /// answer in text.
    pub async fn call(&self, message_list: &[Value], tool_list: &[Value]) -> Result<Reply, Error> {
        let mut body = json!({
            "model": self.model,
            "messages": message_list,
        });
        if !tool_list.is_empty() {
            body["tools"] = json!(tool_list);
        }
        let sent = self
            .http
            .post(format!("{}/chat/completions", self.url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await;
        let response = match sent {
            Ok(response) => response,
            Err(error) => return Err(Error::Network(error.without_url().to_string())),
        };
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let text = match response.text().await {
            Ok(text) => text,
            Err(error) => return Err(Error::Network(error.without_url().to_string())),
        };
        if !(200..300).contains(&status) {
            if context_length(status, &text) {
                return Err(Error::ContextLength);
            }
            return Err(Error::Status {
                status,
                retry_after,
            });
        }
        let answer: Value = match serde_json::from_str(&text) {
            Ok(answer) => answer,
            Err(error) => return Err(Error::Answer(error.to_string())),
        };
        let Some(message) = answer
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
        else {
            return Err(Error::Answer("no choices[0].message".to_string()));
        };
        let usage = answer.get("usage");
        let tokens_in = usage
            .and_then(|usage| usage.get("prompt_tokens"))
            .and_then(Value::as_u64);
        let tokens_out = usage
            .and_then(|usage| usage.get("completion_tokens"))
            .and_then(Value::as_u64);
        let mut message = message.clone();
        // Some providers write `content: null` beside tool calls, and some leave out `role`.
        if let Some(object) = message.as_object_mut() {
            object.insert("role".to_string(), json!("assistant"));
        }
        Ok(Reply::of(message, tokens_in, tokens_out))
    }

    /// Calls until one succeeds, backing off forever, as the author chose: from 1 s, doubling, up to
    /// `backoff_max`, and waiting what `Retry-After` asks within that cap. Only a context-length error
    /// comes back, as the caller compacts. `failing` is told each failure, its count and how long
    /// the next wait is, and `recovered` once a call succeeds after a failure.
    pub async fn call_until(
        &self,
        message_list: &[Value],
        tool_list: &[Value],
        mut failing: impl FnMut(&Error, u32, Duration),
        recovered: impl FnOnce(),
    ) -> Result<Reply, Error> {
        let mut backoff = FIRST_BACKOFF;
        let mut failures = 0;
        loop {
            match self.call(message_list, tool_list).await {
                Ok(reply) => {
                    if failures > 0 {
                        recovered();
                    }
                    return Ok(reply);
                }
                Err(Error::ContextLength) => {
                    if failures > 0 {
                        recovered();
                    }
                    return Err(Error::ContextLength);
                }
                Err(error) => {
                    failures += 1;
                    let wait = match &error {
                        Error::Status {
                            retry_after: Some(asked),
                            ..
                        } => (*asked).max(backoff),
                        _ => backoff,
                    }
                    .min(self.backoff_max);
                    tracing::warn!(
                        error = %error.detail(),
                        failures,
                        wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                        "an LLM call failed"
                    );
                    failing(&error, failures, wait);
                    tokio::time::sleep(wait).await;
                    backoff = (backoff * 2).min(self.backoff_max);
                }
            }
        }
    }
}

/// Whether a refusal says the input is over the model's context: OpenAI's code
/// `context_length_exceeded`, or the words other providers use for it, on a 400 or 413.
fn context_length(status: u16, text: &str) -> bool {
    if status != 400 && status != 413 {
        return false;
    }
    if status == 413 {
        return true;
    }
    let lower = text.to_lowercase();
    if lower.contains("context_length_exceeded") {
        return true;
    }
    let about_context = lower.contains("context") || lower.contains("too many tokens");
    let about_size = lower.contains("length")
        || lower.contains("window")
        || lower.contains("too long")
        || lower.contains("maximum")
        || lower.contains("too many tokens");
    about_context && about_size
}
