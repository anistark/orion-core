//! An OpenAI-compatible HTTP backend, to show that Orion is genuinely
//! backend-agnostic — the same `Agent` loop drives a remote server just as it
//! drives an in-process model.
//!
//! It targets the streaming **`/v1/completions`** (text-completion) endpoint, so
//! the prompt Orion formats with its [`ChatTemplate`](orion_core::ChatTemplate)
//! is sent verbatim. That works against OpenAI itself and any compatible local
//! server (llama.cpp `server`, vLLM, LM Studio, Ollama's OpenAI shim, …).
//!
//! ```sh
//! # Local llama.cpp server (no key needed):
//! OPENAI_BASE_URL=http://localhost:8080/v1 OPENAI_MODEL=local \
//!     cargo run --example openai_backend --features openai-example
//!
//! # OpenAI (a completion-capable model):
//! OPENAI_API_KEY=sk-... OPENAI_MODEL=gpt-3.5-turbo-instruct \
//!     cargo run --example openai_backend --features openai-example
//! ```

use std::env;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use orion_core::{
    Agent, AgentConfig, AgentEvent, CoreError, CoreResult, GenerationResult, InferenceParams,
    LlmBackend, TokenCallback,
};

/// A backend that streams from an OpenAI-compatible `/v1/completions` endpoint.
struct OpenAiBackend {
    client: reqwest::blocking::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl LlmBackend for OpenAiBackend {
    fn generate(
        &self,
        prompt: &str,
        params: &InferenceParams,
        abort: Arc<AtomicBool>,
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "max_tokens": params.max_tokens,
            "temperature": params.temperature,
            "stream": true,
        });

        let mut req = self
            .client
            .post(format!("{}/completions", self.base_url))
            .json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .map_err(|e| CoreError::Backend(format!("request failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(CoreError::Backend(format!("HTTP {}", resp.status())));
        }

        let start = Instant::now();
        let mut ttft_ms = 0.0;
        let mut text = String::new();
        let mut count: u32 = 0;

        // The endpoint streams Server-Sent Events: `data: {json}` lines, ending
        // with a `data: [DONE]` sentinel.
        let reader = BufReader::new(resp);
        for line in reader.lines() {
            if abort.load(Ordering::Relaxed) {
                break;
            }
            let line = line.map_err(|e| CoreError::Backend(format!("read failed: {e}")))?;
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data.trim() == "[DONE]" {
                break;
            }
            let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            if let Some(piece) = chunk["choices"][0]["text"].as_str() {
                if piece.is_empty() {
                    continue;
                }
                if count == 0 {
                    ttft_ms = start.elapsed().as_secs_f64() * 1000.0;
                }
                count += 1;
                text.push_str(piece);
                let elapsed = start.elapsed().as_secs_f64().max(1e-6);
                on_token(piece, count, count as f64 / elapsed);
            }
        }

        let gen_ms = start.elapsed().as_secs_f64() * 1000.0;
        Ok(GenerationResult {
            text,
            tokens_generated: count,
            prompt_tokens: 0,
            tokens_per_sec: count as f64 / (gen_ms / 1000.0).max(1e-6),
            time_to_first_token_ms: ttft_ms,
            generation_time_ms: gen_ms,
        })
    }

    fn tokenize_count(&self, text: &str) -> CoreResult<u32> {
        // The API doesn't expose a tokenizer cheaply, so approximate (~4 chars
        // per token) for context budgeting. A real backend would tokenize.
        Ok((text.chars().count() as u32 / 4).max(1))
    }

    fn is_ready(&self) -> bool {
        true
    }
}

#[tokio::main]
async fn main() -> CoreResult<()> {
    let base_url =
        env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-3.5-turbo-instruct".to_string());
    let api_key = env::var("OPENAI_API_KEY").ok();

    let backend: Arc<dyn LlmBackend> = Arc::new(OpenAiBackend {
        client: reqwest::blocking::Client::new(),
        base_url,
        model,
        api_key,
    });

    let mut agent = Agent::new(AgentConfig {
        system_prompt: "You are a helpful assistant.".into(),
        ..Default::default()
    });

    let (mut rx, run) = agent.prompt_stream("What is Rust, in one sentence?", backend);
    let consumer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::MessageDelta { delta, .. } => print!("{delta}"),
                AgentEvent::GenerationStats { tokens_per_sec, .. } => {
                    println!("\n[{tokens_per_sec:.1} tok/s]")
                }
                AgentEvent::Error { message } => eprintln!("error: {message}"),
                _ => {}
            }
        }
    });

    println!("> What is Rust, in one sentence?");
    run.await?;
    consumer.await.expect("consumer task panicked");

    Ok(())
}
