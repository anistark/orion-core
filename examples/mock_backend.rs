//! Minimal runnable example: a mock backend that streams a canned reply.
//!
//! ```sh
//! cargo run --example mock_backend
//! ```
//!
//! It implements [`LlmBackend`] with no real model — just enough to show the
//! agent loop, event streaming, and context budget working end to end.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orion_core::{
    Agent, AgentConfig, AgentEvent, CoreError, CoreResult, GenerationResult, InferenceParams,
    LlmBackend, TokenCallback,
};
use tokio::sync::mpsc;

/// A fake backend that "generates" a fixed reply, one word at a time.
struct MockBackend;

impl LlmBackend for MockBackend {
    fn generate(
        &self,
        _prompt: &str,
        _params: &InferenceParams,
        abort: Arc<AtomicBool>,
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        let reply = "Rust is a systems programming language focused on safety, \
                     speed, and concurrency.";
        let start = Instant::now();
        let mut text = String::new();
        let mut count: u32 = 0;

        for word in reply.split_inclusive(' ') {
            if abort.load(Ordering::Relaxed) {
                return Err(CoreError::Aborted);
            }
            // Simulate decode latency so the streaming is visible.
            std::thread::sleep(Duration::from_millis(40));
            count += 1;
            text.push_str(word);
            let elapsed = start.elapsed().as_secs_f64().max(1e-6);
            on_token(word, count, count as f64 / elapsed);
        }

        let secs = start.elapsed().as_secs_f64().max(1e-6);
        Ok(GenerationResult {
            text,
            tokens_generated: count,
            prompt_tokens: 0,
            tokens_per_sec: count as f64 / secs,
            time_to_first_token_ms: 40.0,
            generation_time_ms: secs * 1000.0,
        })
    }

    fn tokenize_count(&self, text: &str) -> CoreResult<u32> {
        // A real backend tokenizes; here we approximate with a word count.
        Ok(text.split_whitespace().count() as u32)
    }

    fn is_ready(&self) -> bool {
        true
    }
}

#[tokio::main]
async fn main() -> CoreResult<()> {
    let mut agent = Agent::new(AgentConfig {
        system_prompt: "You are a helpful assistant.".into(),
        ..Default::default()
    });

    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend);
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();

    // Consume events concurrently while the agent generates.
    let consumer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::ContextBudget {
                    used_tokens,
                    max_tokens,
                    ..
                } => println!("[context: {used_tokens}/{max_tokens} tokens]"),
                AgentEvent::MessageDelta { delta, .. } => {
                    print!("{delta}");
                    let _ = std::io::stdout().flush();
                }
                AgentEvent::GenerationStats { tokens_per_sec, .. } => {
                    println!("\n[{tokens_per_sec:.1} tok/s]")
                }
                AgentEvent::Error { message } => eprintln!("error: {message}"),
                _ => {}
            }
        }
    });

    println!("> What is Rust?");
    agent.prompt("What is Rust?", backend, tx).await?;
    consumer.await.expect("consumer task panicked");

    Ok(())
}
