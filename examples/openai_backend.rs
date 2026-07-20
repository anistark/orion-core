//! Drive the agent against an OpenAI-compatible HTTP endpoint using the
//! supported [`OpenAiHttpBackend`](orion_core::backends::OpenAiHttpBackend).
//!
//! The same `Agent` loop that drives an in-process model drives a remote server
//! here - only the backend changes. Point it at any OpenAI-compatible endpoint
//! (OpenAI, llama.cpp `llama-server`, vLLM, LM Studio, Ollama's OpenAI shim, …).
//! Set `OPENAI_ENDPOINT=completions` to target `/v1/completions` (sending
//! Orion's already-templated prompt verbatim) instead of the default chat endpoint:
//!
//! ```sh
//! # Local server (no key needed):
//! OPENAI_BASE_URL=http://localhost:8080/v1 OPENAI_MODEL=local \
//!     cargo run --example openai_backend --features http-backend
//!
//! # Local instruct model behind a completion endpoint:
//! OPENAI_BASE_URL=http://localhost:8080/v1 OPENAI_MODEL=local OPENAI_ENDPOINT=completions \
//!     cargo run --example openai_backend --features http-backend
//!
//! # OpenAI:
//! OPENAI_API_KEY=sk-... OPENAI_MODEL=gpt-4o-mini \
//!     cargo run --example openai_backend --features http-backend
//! ```

use std::env;
use std::sync::Arc;

use orion_core::backends::{OpenAiConfig, OpenAiEndpoint, OpenAiHttpBackend};
use orion_core::{Agent, AgentConfig, AgentEvent, CoreResult, LlmBackend};

#[tokio::main]
async fn main() -> CoreResult<()> {
    let base_url =
        env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let endpoint = match env::var("OPENAI_ENDPOINT").as_deref() {
        Ok("completions") => OpenAiEndpoint::Completions,
        _ => OpenAiEndpoint::Chat,
    };

    let mut config = OpenAiConfig::new(base_url, model).with_endpoint(endpoint);
    if let Ok(key) = env::var("OPENAI_API_KEY") {
        config = config.with_api_key(key);
    }

    let backend: Arc<dyn LlmBackend> = Arc::new(OpenAiHttpBackend::new(config)?);

    let mut agent = Agent::new(AgentConfig {
        system_prompt: "You are a helpful assistant.".into(),
        ..Default::default()
    });

    let (mut rx, run) = agent.prompt_stream("What is Rust, in one sentence?", backend);
    let consumer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::MessageDelta { delta, .. } => print!("{delta}"),
                AgentEvent::GenerationStats {
                    tokens_generated,
                    prompt_tokens,
                    tokens_per_sec,
                    ..
                } => println!(
                    "\n[{prompt_tokens} prompt + {tokens_generated} completion tokens, \
                     {tokens_per_sec:.1} tok/s]"
                ),
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
