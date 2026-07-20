//! Ready-made [`LlmBackend`](crate::LlmBackend) implementations.
//!
//! Currently this is the OpenAI-compatible HTTP backend, [`OpenAiHttpBackend`],
//! available behind the `http-backend` feature. It speaks the streaming
//! `/v1/chat/completions` protocol shared by OpenAI itself and every compatible
//! local server (llama.cpp `llama-server`, vLLM, Ollama, LM Studio, …), so a
//! single implementation targets any of them by changing the base URL.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::backend::{GenerationResult, InferenceParams, LlmBackend, TokenCallback};
use crate::error::{CoreError, CoreResult};

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
    client: reqwest::blocking::Client,
    config: OpenAiConfig,
}

impl OpenAiHttpBackend {
    /// Build a backend from `config`.
    ///
    /// Returns [`CoreError::Backend`] if the underlying HTTP client cannot be
    /// constructed (e.g. the platform TLS backend fails to initialize).
    pub fn new(config: OpenAiConfig) -> CoreResult<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| CoreError::Backend(format!("failed to build HTTP client: {e}")))?;
        Ok(Self { client, config })
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
        let mut req = self.client.post(&url).json(&body);
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
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };

            // Usage arrives in a trailing chunk (with `include_usage`).
            if let Some(usage) = chunk.get("usage").filter(|u| !u.is_null()) {
                usage_prompt = usage
                    .get("prompt_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32);
                usage_completion = usage
                    .get("completion_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32);
            }

            let piece = match self.config.endpoint {
                OpenAiEndpoint::Chat => chunk["choices"][0]["delta"]["content"].as_str(),
                OpenAiEndpoint::Completions => chunk["choices"][0]["text"].as_str(),
            };
            if let Some(piece) = piece {
                if piece.is_empty() {
                    continue;
                }
                if streamed == 0 {
                    ttft_ms = start.elapsed().as_secs_f64() * 1000.0;
                }
                streamed += 1;
                text.push_str(piece);
                let elapsed = start.elapsed().as_secs_f64().max(1e-6);
                on_token(piece, streamed, streamed as f64 / elapsed);
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
