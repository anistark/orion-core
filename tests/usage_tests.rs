//! Pins the `AgentEvent::GenerationStats` emission guarantee: exactly one per
//! completed LLM iteration, always before the closing `AgentEnd`, with no gaps
//! or duplicates. A refactor that skips or double-emits fails these tests.
#![cfg(feature = "tools")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use orion_core::{
    Agent, AgentConfig, AgentEvent, CoreError, CoreResult, GenerationResult, InferenceParams,
    LlmBackend, TokenCallback, Tool, ToolOutput, ToolUpdateCallback,
};
use tokio::sync::mpsc;

/// A backend that returns scripted replies, advancing one entry per call.
/// The last entry repeats once the script is exhausted.
struct ScriptedBackend {
    replies: Vec<String>,
    call: AtomicUsize,
}

impl ScriptedBackend {
    fn new(replies: Vec<&str>) -> Self {
        Self {
            replies: replies.into_iter().map(String::from).collect(),
            call: AtomicUsize::new(0),
        }
    }
}

impl LlmBackend for ScriptedBackend {
    fn generate(
        &self,
        _prompt: &str,
        _params: &InferenceParams,
        _abort: Arc<AtomicBool>,
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        let idx = self.call.fetch_add(1, Ordering::SeqCst);
        let reply = self
            .replies
            .get(idx)
            .or_else(|| self.replies.last())
            .cloned()
            .unwrap_or_default();
        on_token(&reply, 1, 1.0);
        Ok(GenerationResult {
            text: reply.clone(),
            tokens_generated: reply.split_whitespace().count() as u32,
            prompt_tokens: 7,
            tokens_per_sec: 10.0,
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

/// A backend that completes its first `ok_calls` generations normally, then
/// returns `Aborted` - simulating the abort flag tripping mid-generation. An
/// aborted generation produces no `GenerationResult`, so it must emit no
/// `GenerationStats`.
struct AbortAfterBackend {
    ok_calls: usize,
    reply: String,
    call: AtomicUsize,
}

impl AbortAfterBackend {
    fn new(ok_calls: usize, reply: &str) -> Self {
        Self {
            ok_calls,
            reply: reply.into(),
            call: AtomicUsize::new(0),
        }
    }
}

impl LlmBackend for AbortAfterBackend {
    fn generate(
        &self,
        _prompt: &str,
        _params: &InferenceParams,
        _abort: Arc<AtomicBool>,
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        let idx = self.call.fetch_add(1, Ordering::SeqCst);
        if idx >= self.ok_calls {
            return Err(CoreError::Aborted);
        }
        on_token(&self.reply, 1, 1.0);
        Ok(GenerationResult {
            text: self.reply.clone(),
            tokens_generated: self.reply.split_whitespace().count() as u32,
            prompt_tokens: 7,
            tokens_per_sec: 10.0,
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

/// A no-op tool that always succeeds.
struct NoopTool;

#[async_trait]
impl Tool for NoopTool {
    fn name(&self) -> &str {
        "noop"
    }
    fn label(&self) -> &str {
        "Noop"
    }
    fn description(&self) -> &str {
        "Does nothing"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        _args: serde_json::Value,
        _on_update: Option<ToolUpdateCallback>,
    ) -> CoreResult<ToolOutput> {
        Ok(ToolOutput {
            content: "ok".into(),
            details: serde_json::Value::Null,
        })
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

/// Assert the guarantee: `expected` GenerationStats events, all strictly before
/// a single trailing `AgentEnd`.
fn assert_stats_contract(events: &[AgentEvent], expected: usize) {
    let stats: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, AgentEvent::GenerationStats { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        stats.len(),
        expected,
        "expected {expected} GenerationStats, got {}",
        stats.len()
    );

    let ends: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, AgentEvent::AgentEnd { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(ends.len(), 1, "exactly one AgentEnd per run");
    let end = ends[0];
    assert_eq!(end, events.len() - 1, "AgentEnd must be the final event");
    assert!(
        stats.iter().all(|&i| i < end),
        "every GenerationStats must precede AgentEnd"
    );
}

#[tokio::test]
async fn one_stats_for_a_tool_free_answer() {
    let mut agent = Agent::new(AgentConfig::default());
    let backend: Arc<dyn LlmBackend> = Arc::new(ScriptedBackend::new(vec!["Just an answer."]));

    let events = run(&mut agent, backend, "hello").await;
    assert_stats_contract(&events, 1);
}

#[tokio::test]
async fn one_stats_per_iteration_across_a_tool_loop() {
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(NoopTool)]);

    // Two iterations: a tool call, then a final answer.
    let backend: Arc<dyn LlmBackend> = Arc::new(ScriptedBackend::new(vec![
        "```tool_call\n{\"name\": \"noop\", \"arguments\": {}}\n```",
        "All done.",
    ]));

    let events = run(&mut agent, backend, "run the tool").await;
    assert_stats_contract(&events, 2);
}

#[tokio::test]
async fn one_stats_per_iteration_when_max_iterations_trips() {
    let mut agent = Agent::new(AgentConfig {
        max_tool_iterations: 3,
        ..AgentConfig::default()
    });
    agent.set_tools(vec![Box::new(NoopTool)]);

    // Never a final answer - the loop runs the full budget of iterations.
    let backend: Arc<dyn LlmBackend> = Arc::new(ScriptedBackend::new(vec![
        "```tool_call\n{\"name\": \"noop\", \"arguments\": {}}\n```",
    ]));

    let events = run(&mut agent, backend, "loop").await;
    assert_stats_contract(&events, 3);
}

#[tokio::test]
async fn no_stats_when_generation_aborts() {
    let mut agent = Agent::new(AgentConfig::default());
    // Aborts on the very first generation: no iteration completes.
    let backend: Arc<dyn LlmBackend> = Arc::new(AbortAfterBackend::new(0, "unused"));

    let events = run(&mut agent, backend, "hello").await;
    // The aborted iteration emits no GenerationStats, yet the run still closes
    // with a single trailing AgentEnd.
    assert_stats_contract(&events, 0);
}

#[tokio::test]
async fn only_completed_iterations_emit_stats_before_abort() {
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(NoopTool)]);

    // First generation completes (a tool call → 1 stat); the second aborts
    // mid-generation (0 stats). Summing yields exactly the completed iteration.
    let backend: Arc<dyn LlmBackend> = Arc::new(AbortAfterBackend::new(
        1,
        "```tool_call\n{\"name\": \"noop\", \"arguments\": {}}\n```",
    ));

    let events = run(&mut agent, backend, "run then abort").await;
    assert_stats_contract(&events, 1);
}
