//! Ready-made [`LlmBackend`](crate::LlmBackend) implementations.
//!
//! Currently this is the OpenAI-compatible HTTP backend, [`OpenAiHttpBackend`],
//! available behind the `http-backend` feature. It speaks the streaming
//! `/v1/chat/completions` protocol shared by OpenAI itself and every compatible
//! local server (llama.cpp `llama-server`, vLLM, Ollama, LM Studio, …), so a
//! single implementation targets any of them by changing the base URL.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[cfg(feature = "chat-backend")]
use crate::backend::ChatBackend;
use crate::backend::{GenerationResult, InferenceParams, LlmBackend, TokenCallback};
use crate::error::{CoreError, CoreResult};
#[cfg(feature = "chat-backend")]
use crate::messages::{Message, Role};

/// Default request timeout when none is set.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Which OpenAI-compatible endpoint the backend targets.
///
/// Orion always hands the backend a prompt that has already had its
/// [`ChatTemplate`](crate::ChatTemplate) applied, so the two endpoints differ in
/// how that string is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenAiEndpoint {
    /// `/v1/chat/completions`. Orion's formatted prompt is sent as the content
    /// of a single user message, so the server applies its own chat template on
    /// top. Use this for hosted APIs and chat-only servers.
    #[default]
    Chat,
    /// `/v1/completions`. Orion's formatted prompt is sent verbatim as the raw
    /// `prompt`, avoiding a second layer of templating. Prefer this against a
    /// local instruct model served with a completion endpoint, where
    /// double-templating would corrupt the prompt.
    Completions,
}

/// Configuration for an [`OpenAiHttpBackend`].
///
/// ```
/// use std::time::Duration;
/// use orion_core::backends::{OpenAiConfig, OpenAiEndpoint};
///
/// let config = OpenAiConfig::new("http://localhost:8080/v1", "local-model")
///     .with_api_key("sk-...")
///     .with_endpoint(OpenAiEndpoint::Completions)
///     .with_timeout(Duration::from_secs(30));
/// assert_eq!(config.base_url, "http://localhost:8080/v1");
/// ```
#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Base URL up to and including `/v1` (e.g. `https://api.openai.com/v1`).
    /// The endpoint path (`/chat/completions` or `/completions`) is appended.
    pub base_url: String,
    /// Model identifier sent in each request.
    pub model: String,
    /// Bearer token. Omit for local servers that need no auth.
    pub api_key: Option<String>,
    /// Which endpoint to target. Defaults to [`OpenAiEndpoint::Chat`].
    pub endpoint: OpenAiEndpoint,
    /// Per-request timeout.
    pub timeout: Duration,
}

impl OpenAiConfig {
    /// Config for `base_url` and `model` with no API key, the `Chat` endpoint,
    /// and the default timeout.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key: None,
            endpoint: OpenAiEndpoint::default(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Set the bearer token used for authorization.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Select which endpoint to target.
    pub fn with_endpoint(mut self, endpoint: OpenAiEndpoint) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// Set the per-request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// A [`LlmBackend`] that streams from an OpenAI-compatible endpoint.
///
/// It targets either `/v1/chat/completions` or `/v1/completions`, selected via
/// [`OpenAiConfig::with_endpoint`] (see [`OpenAiEndpoint`] for how the formatted
/// prompt is delivered in each case). Tokens stream back through the `on_token`
/// callback as they arrive, and the returned [`GenerationResult`] carries the
/// real token counts from the response's `usage` block when the server provides
/// one (requested via `stream_options.include_usage`).
///
/// Because [`LlmBackend::generate`] is synchronous, this backend blocks the
/// calling thread on I/O - drive it from a blocking context (e.g. the agent
/// loop's `spawn_blocking`), not directly on an async executor.
///
/// ```no_run
/// use std::sync::Arc;
/// use orion_core::backends::{OpenAiConfig, OpenAiHttpBackend};
/// use orion_core::LlmBackend;
///
/// let backend: Arc<dyn LlmBackend> = Arc::new(
///     OpenAiHttpBackend::new(OpenAiConfig::new("http://localhost:8080/v1", "local"))
///         .expect("client build"),
/// );
/// // agent.prompt("Hello", backend, tx).await?;
/// ```
pub struct OpenAiHttpBackend {
    /// For the [`LlmBackend`] path, which the trait makes synchronous.
    ///
    /// Built on first use and not before. A blocking client carries a runtime of its own,
    /// and building or dropping one inside an async context panics, so a backend driven
    /// only through [`ChatBackend`] must never make one.
    blocking: OnceLock<reqwest::blocking::Client>,
    /// For the [`ChatBackend`] path, which it does not.
    #[cfg(feature = "chat-backend")]
    streaming: reqwest::Client,
    config: OpenAiConfig,
}

impl OpenAiHttpBackend {
    /// Build a backend from `config`.
    ///
    /// Returns [`CoreError::Backend`] if the underlying HTTP client cannot be
    /// constructed (e.g. the platform TLS backend fails to initialize).
    pub fn new(config: OpenAiConfig) -> CoreResult<Self> {
        Ok(Self {
            blocking: OnceLock::new(),
            #[cfg(feature = "chat-backend")]
            streaming: reqwest::Client::builder()
                .timeout(config.timeout)
                .build()
                .map_err(|e| CoreError::Backend(format!("failed to build HTTP client: {e}")))?,
            config,
        })
    }

    /// The blocking client, built the first time one is wanted.
    ///
    /// A loser in a race gets the winner's client rather than its own, which is what
    /// `OnceLock` is for: two would mean two runtimes for one backend.
    fn blocking(&self) -> CoreResult<&reqwest::blocking::Client> {
        if let Some(client) = self.blocking.get() {
            return Ok(client);
        }

        let built = reqwest::blocking::Client::builder()
            .timeout(self.config.timeout)
            .build()
            .map_err(|e| CoreError::Backend(format!("failed to build HTTP client: {e}")))?;
        let _ = self.blocking.set(built);

        self.blocking
            .get()
            .ok_or_else(|| CoreError::Backend("HTTP client went missing".into()))
    }
}

impl LlmBackend for OpenAiHttpBackend {
    fn generate(
        &self,
        prompt: &str,
        params: &InferenceParams,
        abort: Arc<AtomicBool>,
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        let mut body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": params.max_tokens,
            "temperature": params.temperature,
            "stream": true,
            // Ask the server to include the token usage block in the final chunk.
            "stream_options": { "include_usage": true },
        });
        let path = match self.config.endpoint {
            OpenAiEndpoint::Chat => {
                // Deliver the already-formatted prompt as a single user message.
                body["messages"] = serde_json::json!([{ "role": "user", "content": prompt }]);
                "chat/completions"
            }
            OpenAiEndpoint::Completions => {
                // Send the already-templated prompt verbatim.
                body["prompt"] = serde_json::json!(prompt);
                "completions"
            }
        };

        let url = format!("{}/{path}", self.config.base_url);
        let mut req = self.blocking()?.post(&url).json(&body);
        if let Some(key) = &self.config.api_key {
            req = req.bearer_auth(key);
        }

        // No response at all → the endpoint is unreachable (retryable).
        let resp = req
            .send()
            .map_err(|e| CoreError::BackendUnreachable(format!("request to {url} failed: {e}")))?;

        // A response with a non-success status → the endpoint answered with an
        // error (not retryable without changing the request).
        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().unwrap_or_default();
            return Err(CoreError::Backend(format!(
                "endpoint returned HTTP {}: {}",
                status.as_u16(),
                detail.trim()
            )));
        }

        let start = Instant::now();
        let mut ttft_ms = 0.0;
        let mut text = String::new();
        let mut streamed: u32 = 0;
        let mut usage_prompt: Option<u32> = None;
        let mut usage_completion: Option<u32> = None;

        // Server-Sent Events: `data: {json}` lines, ending with `data: [DONE]`.
        let reader = BufReader::new(resp);
        for line in reader.lines() {
            if abort.load(Ordering::Relaxed) {
                return Err(CoreError::Aborted);
            }
            let line = line
                .map_err(|e| CoreError::BackendUnreachable(format!("stream read failed: {e}")))?;
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };

            // The same envelope the async path reads, so the two cannot drift on what a
            // usage block or a `[DONE]` means.
            let piece = read_event(data, self.config.endpoint);
            usage_prompt = piece.prompt_tokens.or(usage_prompt);
            usage_completion = piece.completion_tokens.or(usage_completion);

            if piece.done {
                break;
            }
            if let Some(said) = piece.text {
                if streamed == 0 {
                    ttft_ms = start.elapsed().as_secs_f64() * 1000.0;
                }
                streamed += 1;
                text.push_str(&said);
                let elapsed = start.elapsed().as_secs_f64().max(1e-6);
                on_token(&said, streamed, streamed as f64 / elapsed);
            }
        }

        let gen_ms = start.elapsed().as_secs_f64() * 1000.0;
        // Prefer the server's real counts; fall back to what we streamed.
        let tokens_generated = usage_completion.unwrap_or(streamed);
        let prompt_tokens = usage_prompt.unwrap_or(0);
        Ok(GenerationResult {
            text,
            tokens_generated,
            prompt_tokens,
            tokens_per_sec: tokens_generated as f64 / (gen_ms / 1000.0).max(1e-6),
            time_to_first_token_ms: ttft_ms,
            generation_time_ms: gen_ms,
        })
    }

    fn tokenize_count(&self, text: &str) -> CoreResult<u32> {
        // The HTTP API exposes no cheap tokenizer, so approximate (~4 chars per
        // token) for context budgeting. Real generation counts come from the
        // response `usage` block, not this estimate.
        Ok((text.chars().count() as u32 / 4).max(1))
    }

    fn is_ready(&self) -> bool {
        true
    }
}

/// What one server-sent event carries, whichever way its bytes arrived.
///
/// The two paths differ in transport and not in envelope, so the reading of one is shared:
/// a blocking reader hands over lines, an async one hands over chunks that have to be cut
/// into lines first, and both end up here.
#[derive(Debug, Default)]
struct Piece {
    text: Option<String>,
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    /// The `[DONE]` sentinel, after which nothing else is coming.
    done: bool,
}

/// Read one `data:` payload. A payload that will not parse is skipped rather than fatal:
/// servers vary in what they put between events, and one unreadable line is a worse reason
/// to fail a turn than it is to ignore.
fn read_event(data: &str, endpoint: OpenAiEndpoint) -> Piece {
    let data = data.trim();
    if data == "[DONE]" {
        return Piece {
            done: true,
            ..Piece::default()
        };
    }

    let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data) else {
        return Piece::default();
    };

    let usage = chunk.get("usage").filter(|usage| !usage.is_null());
    let count = |name: &str| {
        usage
            .and_then(|usage| usage.get(name))
            .and_then(serde_json::Value::as_u64)
            .map(|n| n as u32)
    };

    let text = match endpoint {
        OpenAiEndpoint::Chat => chunk["choices"][0]["delta"]["content"].as_str(),
        OpenAiEndpoint::Completions => chunk["choices"][0]["text"].as_str(),
    };

    Piece {
        text: text.filter(|piece| !piece.is_empty()).map(str::to_string),
        prompt_tokens: count("prompt_tokens"),
        completion_tokens: count("completion_tokens"),
        done: false,
    }
}

/// The messages as `/v1/chat/completions` takes them.
///
/// The system prompt arrives beside the turns and goes on the front as one of them, which
/// is how every chat API takes it. A tool result becomes a user turn carrying the
/// observation rather than a `tool` message, because the API's `tool` role requires a
/// `tool_call_id` that this crate's text-based tool convention never mints; an assistant
/// turn that asked for a tool keeps its text, which is where the call is written.
#[cfg(feature = "chat-backend")]
fn wire(system: &str, messages: &[Message]) -> Vec<serde_json::Value> {
    let mut turns = Vec::with_capacity(messages.len() + 1);
    if !system.trim().is_empty() {
        turns.push(serde_json::json!({ "role": "system", "content": system }));
    }

    for message in messages {
        let (role, content) = match message.role {
            Role::System => ("system", message.content.clone()),
            Role::User => ("user", message.content.clone()),
            Role::Assistant | Role::ToolCall => ("assistant", message.content.clone()),
            Role::ToolResult => ("user", format!("[Tool result]\n{}", message.content)),
        };
        turns.push(serde_json::json!({ "role": role, "content": content }));
    }

    turns
}

/// The message-native half.
///
/// This is the one to use against a hosted chat API. [`LlmBackend`] hands a backend a
/// prompt that has already had a template applied, and delivering that to
/// `/v1/chat/completions` means stuffing an entire conversation into a single user turn,
/// markup and all. Here the turns arrive as turns and the server applies the model's own
/// template, which is what it is for.
///
/// [`OpenAiConfig::endpoint`] is not consulted: `/v1/completions` takes a prompt and not a
/// conversation, so a message-native path has only one endpoint it can mean. Use the
/// [`LlmBackend`] impl for that one.
#[cfg(feature = "chat-backend")]
#[async_trait::async_trait]
impl ChatBackend for OpenAiHttpBackend {
    async fn chat(
        &self,
        system: &str,
        messages: &[Message],
        params: &InferenceParams,
        abort: Arc<AtomicBool>,
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        let body = serde_json::json!({
            "model": self.config.model,
            "messages": wire(system, messages),
            "max_tokens": params.max_tokens,
            "temperature": params.temperature,
            "stream": true,
            "stream_options": { "include_usage": true },
        });

        let url = format!("{}/chat/completions", self.config.base_url);
        let mut request = self.streaming.post(&url).json(&body);
        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key);
        }

        // No response at all is the endpoint being unreachable, which is retryable. A
        // response carrying an error is the endpoint answering, which is not.
        let mut response = request
            .send()
            .await
            .map_err(|e| CoreError::BackendUnreachable(format!("request to {url} failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(CoreError::Backend(format!(
                "endpoint returned HTTP {}: {}",
                status.as_u16(),
                detail.trim()
            )));
        }

        let start = Instant::now();
        let mut ttft_ms = 0.0;
        let mut text = String::new();
        let mut streamed: u32 = 0;
        let mut usage_prompt: Option<u32> = None;
        let mut usage_completion: Option<u32> = None;
        // A read off the wire is a length of bytes, not a line: one event can arrive split
        // across two and two can arrive in one, so the remainder waits for its newline.
        let mut pending = String::new();
        let mut finished = false;

        while !finished {
            if abort.load(Ordering::Relaxed) {
                return Err(CoreError::Aborted);
            }

            let Some(bytes) = response
                .chunk()
                .await
                .map_err(|e| CoreError::BackendUnreachable(format!("stream read failed: {e}")))?
            else {
                break;
            };
            pending.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(at) = pending.find('\n') {
                let line: String = pending.drain(..=at).collect();
                let Some(data) = line.trim_end().strip_prefix("data: ") else {
                    continue;
                };

                let piece = read_event(data, OpenAiEndpoint::Chat);
                usage_prompt = piece.prompt_tokens.or(usage_prompt);
                usage_completion = piece.completion_tokens.or(usage_completion);

                if piece.done {
                    finished = true;
                    break;
                }
                if let Some(said) = piece.text {
                    if streamed == 0 {
                        ttft_ms = start.elapsed().as_secs_f64() * 1000.0;
                    }
                    streamed += 1;
                    text.push_str(&said);
                    let elapsed = start.elapsed().as_secs_f64().max(1e-6);
                    on_token(&said, streamed, streamed as f64 / elapsed);
                }
            }
        }

        let gen_ms = start.elapsed().as_secs_f64() * 1000.0;
        // The server's real counts when it gave them, and what was streamed when it did not.
        let tokens_generated = usage_completion.unwrap_or(streamed);
        Ok(GenerationResult {
            text,
            tokens_generated,
            prompt_tokens: usage_prompt.unwrap_or(0),
            tokens_per_sec: tokens_generated as f64 / (gen_ms / 1000.0).max(1e-6),
            time_to_first_token_ms: ttft_ms,
            generation_time_ms: gen_ms,
        })
    }
}
