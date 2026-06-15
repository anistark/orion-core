//! End-to-end tests for the agent tool-execution loop.
#![cfg(feature = "tools")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use orion_core::{
    Agent, AgentConfig, AgentEvent, CoreResult, GenerationResult, InferenceParams, LlmBackend,
    Role, TokenCallback, Tool, ToolOutput, ToolUpdateCallback,
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
            prompt_tokens: 0,
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

/// Adds two integers from `{ "a": .., "b": .. }`.
struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &str {
        "add"
    }
    fn label(&self) -> &str {
        "Add"
    }
    fn description(&self) -> &str {
        "Add two numbers a and b"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}}
        })
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        args: serde_json::Value,
        _on_update: Option<ToolUpdateCallback>,
    ) -> CoreResult<ToolOutput> {
        let a = args.get("a").and_then(|v| v.as_i64()).unwrap_or(0);
        let b = args.get("b").and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(ToolOutput {
            content: (a + b).to_string(),
            details: serde_json::Value::Null,
        })
    }
}

/// Drive a prompt to completion, returning all emitted events.
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
async fn runs_tool_then_returns_final_answer() {
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(AddTool)]);

    let backend: Arc<dyn LlmBackend> = Arc::new(ScriptedBackend::new(vec![
        "```tool_call\n{\"name\": \"add\", \"arguments\": {\"a\": 2, \"b\": 3}}\n```",
        "The sum is 5.",
    ]));

    let events = run(&mut agent, backend, "What is 2 + 3?").await;

    // The tool ran with start/end events.
    let started = events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolExecStart { tool_name, .. } if tool_name == "add"));
    assert!(started, "expected ToolExecStart for add");

    let ended_ok = events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::ToolExecEnd { result, .. } if result.content == "5" && !result.is_error
        )
    });
    assert!(ended_ok, "expected ToolExecEnd carrying the result 5");

    // The conversation ends with the model's tool-free final answer.
    let msgs = agent.messages();
    let last = msgs.last().unwrap();
    assert_eq!(last.role, Role::Assistant);
    assert_eq!(last.content, "The sum is 5.");
    assert!(last.tool_calls.is_empty());

    // A tool-result message is recorded between the two assistant turns.
    let has_tool_result = msgs
        .iter()
        .any(|m| m.role == Role::ToolResult && m.content == "5");
    assert!(has_tool_result, "expected a tool-result message");

    // Final AgentEnd reports every new message (2 assistant turns + 1 result).
    let agent_end = events
        .iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::AgentEnd { messages } => Some(messages.len()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        agent_end, 3,
        "AgentEnd should report assistant+result+assistant"
    );
}

#[tokio::test]
async fn no_tools_registered_skips_parsing() {
    // With no tools, even a tool_call-looking reply is treated as the answer.
    let mut agent = Agent::new(AgentConfig::default());
    let backend: Arc<dyn LlmBackend> = Arc::new(ScriptedBackend::new(vec![
        "```tool_call\n{\"name\": \"add\", \"arguments\": {}}\n```",
    ]));

    let events = run(&mut agent, backend, "hi").await;

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolExecStart { .. })),
        "no tools registered → no tool execution"
    );
    assert_eq!(agent.messages().last().unwrap().role, Role::Assistant);
}

#[tokio::test]
async fn unknown_tool_yields_error_result_then_continues() {
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(AddTool)]);

    let backend: Arc<dyn LlmBackend> = Arc::new(ScriptedBackend::new(vec![
        "```tool_call\n{\"name\": \"unknown\", \"arguments\": {}}\n```",
        "Sorry, I could not do that.",
    ]));

    let events = run(&mut agent, backend, "do something").await;

    let error_result = events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::ToolExecEnd { result, .. } if result.is_error
        )
    });
    assert!(error_result, "unknown tool should produce an error result");
    // Loop still reaches a final tool-free answer.
    assert_eq!(
        agent.messages().last().unwrap().content,
        "Sorry, I could not do that."
    );
}

#[tokio::test]
async fn max_tool_iterations_guard_trips() {
    let mut agent = Agent::new(AgentConfig {
        max_tool_iterations: 3,
        ..AgentConfig::default()
    });
    agent.set_tools(vec![Box::new(AddTool)]);

    // Backend always asks for the tool — never a final answer.
    let backend: Arc<dyn LlmBackend> = Arc::new(ScriptedBackend::new(vec![
        "```tool_call\n{\"name\": \"add\", \"arguments\": {\"a\": 1, \"b\": 1}}\n```",
    ]));

    let events = run(&mut agent, backend, "loop forever").await;

    let warned = events
        .iter()
        .any(|e| matches!(e, AgentEvent::Warning { .. }));
    assert!(
        warned,
        "guard should emit a Warning when iterations are exhausted"
    );

    // Exactly max_tool_iterations turns happened.
    let turn_starts = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart))
        .count();
    assert_eq!(
        turn_starts, 3,
        "should stop after max_tool_iterations turns"
    );
}
