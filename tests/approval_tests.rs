//! Tests for the pre-execute approval hook.
#![cfg(feature = "tools")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use orion_core::{
    Agent, AgentConfig, AgentEvent, ApprovalDecision, ApprovalHook, CoreResult, GenerationResult,
    InferenceParams, LlmBackend, Role, TokenCallback, Tool, ToolCall, ToolOutput,
    ToolUpdateCallback,
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

/// A tool that records how many times it actually executed.
struct CountingTool {
    name: String,
    runs: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn label(&self) -> &str {
        "Counting"
    }
    fn description(&self) -> &str {
        "Records that it ran"
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
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput {
            content: "ran".into(),
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

fn call_block(name: &str) -> String {
    format!("```tool_call\n{{\"name\": \"{name}\", \"arguments\": {{}}}}\n```")
}

/// Approves everything - behaviour must match having no hook at all.
struct ApproveAll;

#[async_trait]
impl ApprovalHook for ApproveAll {
    async fn review(&self, _call: &ToolCall) -> ApprovalDecision {
        ApprovalDecision::Approve
    }
}

/// Denies any call whose tool name is in the block list.
struct DenyNamed {
    blocked: Vec<String>,
}

#[async_trait]
impl ApprovalHook for DenyNamed {
    async fn review(&self, call: &ToolCall) -> ApprovalDecision {
        if self.blocked.iter().any(|n| n == &call.name) {
            ApprovalDecision::Deny {
                reason: format!("{} is not permitted", call.name),
            }
        } else {
            ApprovalDecision::Approve
        }
    }
}

#[tokio::test]
async fn approve_runs_the_tool() {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(CountingTool {
        name: "act".into(),
        runs: runs.clone(),
    })]);
    agent.set_approval_hook(Arc::new(ApproveAll));

    let backend: Arc<dyn LlmBackend> =
        Arc::new(ScriptedBackend::new(vec![&call_block("act"), "done"]));

    let events = run(&mut agent, backend, "go").await;

    assert_eq!(runs.load(Ordering::SeqCst), 1, "approved tool should run");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolExecEnd { .. })),
        "approved call emits ToolExecEnd"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolDenied { .. })),
        "approved call emits no ToolDenied"
    );
}

#[tokio::test]
async fn deny_skips_execution_and_feeds_the_model() {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(CountingTool {
        name: "act".into(),
        runs: runs.clone(),
    })]);
    agent.set_approval_hook(Arc::new(DenyNamed {
        blocked: vec!["act".into()],
    }));

    let backend: Arc<dyn LlmBackend> = Arc::new(ScriptedBackend::new(vec![
        &call_block("act"),
        "Understood, I will not do that.",
    ]));

    let events = run(&mut agent, backend, "go").await;

    // The tool never executed.
    assert_eq!(runs.load(Ordering::SeqCst), 0, "denied tool must not run");

    // A ToolDenied event fired, and no ToolExecStart/End for the call.
    let denied = events.iter().any(|e| {
        matches!(e, AgentEvent::ToolDenied { tool_name, reason, .. }
            if tool_name == "act" && reason.contains("not permitted"))
    });
    assert!(denied, "expected a ToolDenied event");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolExecStart { .. })),
        "denied call must not emit ToolExecStart"
    );

    // The denial is recorded as an error tool result the model can see.
    let refusal = agent.messages().iter().find(|m| m.role == Role::ToolResult);
    let refusal = refusal.expect("a tool-result message should be recorded for the denial");
    assert!(refusal.content.contains("not permitted"));
    assert!(refusal.tool_result.as_ref().unwrap().is_error);

    // The loop continued to a final answer rather than aborting.
    assert_eq!(
        agent.messages().last().unwrap().content,
        "Understood, I will not do that."
    );
}

#[tokio::test]
async fn mixed_decisions_in_one_turn() {
    let ok_runs = Arc::new(AtomicUsize::new(0));
    let no_runs = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![
        Box::new(CountingTool {
            name: "allow".into(),
            runs: ok_runs.clone(),
        }),
        Box::new(CountingTool {
            name: "block".into(),
            runs: no_runs.clone(),
        }),
    ]);
    agent.set_approval_hook(Arc::new(DenyNamed {
        blocked: vec!["block".into()],
    }));

    // One assistant turn requesting both tools, as a JSON array.
    let both = "```tool_call\n[{\"name\": \"allow\", \"arguments\": {}}, \
                {\"name\": \"block\", \"arguments\": {}}]\n```";
    let backend: Arc<dyn LlmBackend> = Arc::new(ScriptedBackend::new(vec![both, "all handled"]));

    let events = run(&mut agent, backend, "do both").await;

    assert_eq!(ok_runs.load(Ordering::SeqCst), 1, "allowed tool ran");
    assert_eq!(
        no_runs.load(Ordering::SeqCst),
        0,
        "blocked tool did not run"
    );

    // Exactly one execution and exactly one denial were reported.
    let exec_ends = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecEnd { tool_name, .. } if tool_name == "allow"))
        .count();
    let denials = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolDenied { tool_name, .. } if tool_name == "block"))
        .count();
    assert_eq!(exec_ends, 1);
    assert_eq!(denials, 1);

    // Both calls produced a tool-result message (one real, one refusal).
    let results = agent
        .messages()
        .iter()
        .filter(|m| m.role == Role::ToolResult)
        .count();
    assert_eq!(results, 2);
}

/// A hook with a genuine `.await` suspension point before it decides.
struct AwaitingHook {
    polled: Arc<AtomicUsize>,
}

#[async_trait]
impl ApprovalHook for AwaitingHook {
    async fn review(&self, _call: &ToolCall) -> ApprovalDecision {
        // Yield repeatedly so the future is suspended and resumed by the
        // executor before it returns a decision.
        for _ in 0..3 {
            self.polled.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
        }
        ApprovalDecision::Approve
    }
}

#[tokio::test]
async fn async_hook_that_awaits_is_driven_to_completion() {
    let runs = Arc::new(AtomicUsize::new(0));
    let polled = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(CountingTool {
        name: "act".into(),
        runs: runs.clone(),
    })]);
    agent.set_approval_hook(Arc::new(AwaitingHook {
        polled: polled.clone(),
    }));

    let backend: Arc<dyn LlmBackend> =
        Arc::new(ScriptedBackend::new(vec![&call_block("act"), "done"]));

    run(&mut agent, backend, "go").await;

    assert_eq!(
        polled.load(Ordering::SeqCst),
        3,
        "hook awaited before deciding"
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "approved tool ran after awaiting"
    );
}
