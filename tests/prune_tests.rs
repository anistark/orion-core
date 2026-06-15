//! Agent-level test for the `Summarize` prune strategy.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use orion_core::{
    Agent, AgentConfig, AgentEvent, ContextConfig, CoreResult, GenerationResult, InferenceParams,
    LlmBackend, Message, PruneStrategy, TokenCallback,
};
use tokio::sync::mpsc;

const SUMMARY_MARKER: &str = "[Summary of earlier conversation]";

/// Backend that returns a fixed reply (word-count tokenizer). Used both for the
/// summarization pass and the real generation.
struct CannedBackend;

impl LlmBackend for CannedBackend {
    fn generate(
        &self,
        _prompt: &str,
        _params: &InferenceParams,
        _abort: Arc<AtomicBool>,
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        let text = "CANNED REPLY";
        on_token(text, 1, 1.0);
        Ok(GenerationResult {
            text: text.to_string(),
            tokens_generated: 2,
            prompt_tokens: 0,
            tokens_per_sec: 1.0,
            time_to_first_token_ms: 1.0,
            generation_time_ms: 1.0,
        })
    }

    fn tokenize_count(&self, text: &str) -> CoreResult<u32> {
        Ok(text.split_whitespace().count() as u32)
    }

    fn is_ready(&self) -> bool {
        true
    }
}

async fn run(agent: &mut Agent, backend: Arc<dyn LlmBackend>, prompt: &str) -> Vec<AgentEvent> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        events
    });
    agent.prompt(prompt, backend, tx).await.unwrap();
    collector.await.unwrap()
}

#[tokio::test]
async fn summarize_folds_old_turns_into_pinned_summary() {
    let mut agent = Agent::new(AgentConfig {
        context_config: ContextConfig {
            max_context_tokens: 90,
            max_response_tokens: 20,
            prune_strategy: PruneStrategy::Summarize,
        },
        ..Default::default()
    });

    // Preload a long history that won't fit the tiny budget.
    let mut history = Vec::new();
    for i in 0..10 {
        history.push(Message::user(
            format!("msg-{}", i * 2 + 1),
            format!("user message number {i} carrying several extra words here"),
        ));
        history.push(Message::assistant(
            format!("msg-{}", i * 2 + 2),
            format!("assistant reply number {i} carrying several extra words here"),
        ));
    }
    agent.replace_messages(history);

    let backend: Arc<dyn LlmBackend> = Arc::new(CannedBackend);
    let events = run(&mut agent, backend, "the brand new latest question").await;

    // A summarization warning was emitted.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Warning { .. })),
        "expected a summarization Warning"
    );

    let msgs = agent.messages();

    // Exactly one pinned summary message, carrying the marker + canned summary.
    let summaries: Vec<_> = msgs
        .iter()
        .filter(|m| m.pinned && m.content.starts_with(SUMMARY_MARKER))
        .collect();
    assert_eq!(summaries.len(), 1, "exactly one pinned summary expected");
    assert!(summaries[0].content.contains("CANNED REPLY"));

    // The oldest raw turns were folded away (no longer present verbatim).
    assert!(
        !msgs
            .iter()
            .any(|m| m.content.contains("user message number 0")),
        "oldest turn should have been summarized away"
    );

    // The latest question and its answer survive.
    assert!(msgs
        .iter()
        .any(|m| m.content.contains("the brand new latest question")));
    assert_eq!(msgs.last().unwrap().content, "CANNED REPLY");
}

#[tokio::test]
async fn sliding_window_does_not_summarize() {
    let mut agent = Agent::new(AgentConfig {
        context_config: ContextConfig {
            max_context_tokens: 90,
            max_response_tokens: 20,
            prune_strategy: PruneStrategy::SlidingWindow,
        },
        ..Default::default()
    });

    let mut history = Vec::new();
    for i in 0..10 {
        history.push(Message::user(
            format!("msg-{}", i * 2 + 1),
            format!("user message number {i} carrying several extra words here"),
        ));
        history.push(Message::assistant(
            format!("msg-{}", i * 2 + 2),
            format!("assistant reply number {i} carrying several extra words here"),
        ));
    }
    agent.replace_messages(history);

    let backend: Arc<dyn LlmBackend> = Arc::new(CannedBackend);
    let _ = run(&mut agent, backend, "latest question").await;

    // No summary message under the sliding-window strategy.
    assert!(
        !agent
            .messages()
            .iter()
            .any(|m| m.content.starts_with(SUMMARY_MARKER)),
        "sliding window must not create summaries"
    );
}
