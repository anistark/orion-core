//! Pending tool results, the inbox, resuming a conversation and child agents.
#![cfg(feature = "tools")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use orion_core::{
    Agent, AgentConfig, AgentEvent, Backend, CoreError, CoreResult, GenerationResult, Inbox,
    InferenceParams, LlmBackend, Message, Role, TokenCallback, Tool, ToolCall, ToolContext,
    ToolOutput,
};
use tokio::sync::mpsc;

type OnGenerate = Box<dyn Fn(usize, &Arc<AtomicBool>) + Send + Sync>;

/// A backend that returns scripted replies, one per call, and keeps every prompt it
/// was given. The last reply repeats once the script runs out.
struct ScriptedBackend {
    replies: Vec<String>,
    call: AtomicUsize,
    prompts: Mutex<Vec<String>>,
    on_generate: Option<OnGenerate>,
}

impl ScriptedBackend {
    fn new(replies: Vec<&str>) -> Self {
        Self {
            replies: replies.into_iter().map(String::from).collect(),
            call: AtomicUsize::new(0),
            prompts: Mutex::new(Vec::new()),
            on_generate: None,
        }
    }

    /// Run `hook` at the start of every generation, with the call's index and the
    /// abort flag the generation was handed.
    fn on_generate(
        mut self,
        hook: impl Fn(usize, &Arc<AtomicBool>) + Send + Sync + 'static,
    ) -> Self {
        self.on_generate = Some(Box::new(hook));
        self
    }

    fn calls(&self) -> usize {
        self.call.load(Ordering::SeqCst)
    }

    fn last_prompt(&self) -> String {
        self.prompts
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }
}

impl LlmBackend for ScriptedBackend {
    fn generate(
        &self,
        prompt: &str,
        _params: &InferenceParams,
        abort: Arc<AtomicBool>,
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        let idx = self.call.fetch_add(1, Ordering::SeqCst);
        self.prompts.lock().unwrap().push(prompt.to_string());
        if let Some(hook) = &self.on_generate {
            hook(idx, &abort);
        }
        if abort.load(Ordering::Relaxed) {
            return Err(CoreError::Aborted);
        }

        let reply = self
            .replies
            .get(idx)
            .or_else(|| self.replies.last())
            .cloned()
            .unwrap_or_default();
        on_token(&reply, 1, 1.0);
        Ok(GenerationResult {
            text: reply.clone(),
            tokens_generated: 1,
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

/// Starts a job and answers with a placeholder, as a background task would.
struct StartJob;

#[async_trait]
impl Tool for StartJob {
    fn name(&self) -> &str {
        "start_job"
    }
    fn label(&self) -> &str {
        "Start job"
    }
    fn description(&self) -> &str {
        "Start a long job"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn execute(&self, _args: serde_json::Value, ctx: &ToolContext) -> CoreResult<ToolOutput> {
        Ok(ToolOutput::pending(format!(
            "Started job {}.",
            ctx.call_id()
        )))
    }
}

/// Pushes a message into the agent's inbox while it runs, as a user typing mid-run would.
struct Interject {
    inbox: Inbox,
}

#[async_trait]
impl Tool for Interject {
    fn name(&self) -> &str {
        "look"
    }
    fn label(&self) -> &str {
        "Look"
    }
    fn description(&self) -> &str {
        "Look around"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> CoreResult<ToolOutput> {
        self.inbox.push("Actually, use the blue one.");
        Ok(ToolOutput::new("A red one and a blue one."))
    }
}

/// Counts its runs. Used to prove an interrupted call is not run again.
struct Counted {
    runs: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for Counted {
    fn name(&self) -> &str {
        "write"
    }
    fn label(&self) -> &str {
        "Write"
    }
    fn description(&self) -> &str {
        "Write something"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> CoreResult<ToolOutput> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::new("written"))
    }
}

/// Hands its task to a child agent and answers with the child's reply.
struct Delegate {
    backend: Arc<dyn LlmBackend>,
    child: Arc<tokio::sync::Mutex<Agent>>,
}

#[async_trait]
impl Tool for Delegate {
    fn name(&self) -> &str {
        "delegate"
    }
    fn label(&self) -> &str {
        "Delegate"
    }
    fn description(&self) -> &str {
        "Hand a task to a helper"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": { "task": { "type": "string" } } })
    }
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> CoreResult<ToolOutput> {
        let task = args["task"].as_str().unwrap_or("help").to_string();
        let mut child = self.child.lock().await;
        let reply = ctx
            .run_child(&mut child, task, Backend::Prompt(self.backend.clone()))
            .await?;
        Ok(ToolOutput::new(reply.content))
    }
}

const CALL: &str = "```tool_call\n{\"name\": \"start_job\", \"arguments\": {}}\n```";

fn call(name: &str) -> String {
    format!("```tool_call\n{{\"name\": \"{name}\", \"arguments\": {{}}}}\n```")
}

async fn collect(
    run: impl std::future::Future<Output = CoreResult<()>>,
    mut rx: mpsc::UnboundedReceiver<AgentEvent>,
) -> (CoreResult<()>, Vec<AgentEvent>) {
    let (result, events) = tokio::join!(run, async move {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    });
    (result, events)
}

async fn prompt(agent: &mut Agent, backend: &Arc<ScriptedBackend>, text: &str) -> Vec<AgentEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    let backend: Arc<dyn LlmBackend> = backend.clone();
    let (result, events) = collect(agent.prompt(text, backend, tx), rx).await;
    result.unwrap();
    events
}

async fn resume(
    agent: &mut Agent,
    backend: &Arc<ScriptedBackend>,
) -> (CoreResult<()>, Vec<AgentEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let backend: Arc<dyn LlmBackend> = backend.clone();
    collect(agent.resume(backend, tx), rx).await
}

#[tokio::test]
async fn a_pending_result_is_a_placeholder_until_the_inbox_delivers_it() {
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(StartJob)]);
    let backend = Arc::new(ScriptedBackend::new(vec![
        CALL,
        "I've started it.",
        "The job found 3 files.",
    ]));

    let events = prompt(&mut agent, &backend, "Run the job").await;

    // The model read the placeholder and answered around it.
    assert_eq!(backend.calls(), 2);
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolExecEnd { result, .. } if result.pending && !result.is_error
    )));
    let pending = agent.pending_calls();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].name, "start_job");
    assert!(!agent.needs_reply());

    agent.inbox().resolve(&pending[0].id, "3 files");
    let (result, events) = resume(&mut agent, &backend).await;
    result.unwrap();

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolResolved { tool_call_id, result, .. }
            if *tool_call_id == pending[0].id && result.content == "3 files" && !result.pending
    )));
    assert!(agent.pending_calls().is_empty());
    assert!(backend
        .last_prompt()
        .contains("[Result of the earlier start_job call]\n3 files"));
    assert_eq!(
        agent.messages().last().unwrap().content,
        "The job found 3 files."
    );

    // The late result and the reply are what the run reports as its own.
    let Some(AgentEvent::AgentEnd { messages }) = events.last() else {
        panic!("expected AgentEnd last");
    };
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, Role::ToolResult);
}

#[tokio::test]
async fn pause_on_pending_ends_the_run_until_resumed() {
    let mut agent = Agent::new(AgentConfig {
        pause_on_pending: true,
        ..AgentConfig::default()
    });
    agent.set_tools(vec![Box::new(StartJob)]);
    let backend = Arc::new(ScriptedBackend::new(vec![CALL, "Done: 3 files."]));

    let events = prompt(&mut agent, &backend, "Run the job").await;

    assert_eq!(
        backend.calls(),
        1,
        "the model is not asked to answer around the job"
    );
    let paused = events.iter().find_map(|e| match e {
        AgentEvent::Paused { pending } => Some(pending.clone()),
        _ => None,
    });
    let pending = paused.expect("expected Paused");
    assert_eq!(pending.len(), 1);
    assert!(agent.needs_reply());

    agent.inbox().resolve(&pending[0].id, "3 files");
    let (result, _) = resume(&mut agent, &backend).await;
    result.unwrap();

    assert_eq!(backend.calls(), 2);
    assert_eq!(agent.messages().last().unwrap().content, "Done: 3 files.");
}

#[tokio::test]
async fn a_result_already_in_the_inbox_keeps_a_pausing_run_going() {
    let mut agent = Agent::new(AgentConfig {
        pause_on_pending: true,
        ..AgentConfig::default()
    });
    agent.set_tools(vec![Box::new(StartJob)]);
    let inbox = agent.inbox();
    let backend = Arc::new(
        ScriptedBackend::new(vec![CALL, "Done."]).on_generate(move |idx, _| {
            // The job finishes while the model is still writing its call.
            if idx == 0 {
                inbox.resolve("msg-2-call-1", "fast");
            }
        }),
    );

    let events = prompt(&mut agent, &backend, "Run the job").await;

    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::Paused { .. })));
    assert_eq!(backend.calls(), 2);
    assert!(agent.pending_calls().is_empty());
}

#[tokio::test]
async fn a_message_pushed_mid_run_is_read_before_the_next_generation() {
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(Interject {
        inbox: agent.inbox(),
    })]);
    let backend = Arc::new(ScriptedBackend::new(vec![&call("look"), "Blue it is."]));

    prompt(&mut agent, &backend, "Pick a colour").await;

    let roles: Vec<Role> = agent.messages().iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        roles,
        [
            Role::User,
            Role::Assistant,
            Role::ToolResult,
            Role::User,
            Role::Assistant
        ]
    );
    assert!(backend
        .last_prompt()
        .contains("Actually, use the blue one."));
}

#[tokio::test]
async fn a_message_that_lands_during_the_final_answer_is_answered_in_the_same_run() {
    let mut agent = Agent::new(AgentConfig::default());
    let inbox = agent.inbox();
    let backend = Arc::new(
        ScriptedBackend::new(vec!["Hello.", "Sure, one more."]).on_generate(move |idx, _| {
            if idx == 0 {
                inbox.push("And another thing.");
            }
        }),
    );

    prompt(&mut agent, &backend, "Hi").await;

    assert_eq!(backend.calls(), 2);
    assert_eq!(agent.messages().last().unwrap().content, "Sure, one more.");
}

#[tokio::test]
async fn a_result_for_a_call_that_is_not_pending_is_dropped_with_a_warning() {
    let mut agent = Agent::new(AgentConfig::default());
    let backend = Arc::new(ScriptedBackend::new(vec!["Hello."]));
    prompt(&mut agent, &backend, "Hi").await;

    agent.inbox().resolve("msg-99-call-1", "stray");
    let (result, events) = resume(&mut agent, &backend).await;
    result.unwrap();

    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::Warning { .. })));
    assert_eq!(backend.calls(), 1, "nothing new for the model to answer");
    assert!(agent.inbox().is_empty());
}

#[tokio::test]
async fn resume_with_nothing_to_do_is_an_error_and_emits_nothing() {
    let mut agent = Agent::new(AgentConfig::default());
    let backend = Arc::new(ScriptedBackend::new(vec!["Hello."]));
    prompt(&mut agent, &backend, "Hi").await;

    let (result, events) = resume(&mut agent, &backend).await;

    assert!(matches!(result, Err(CoreError::Agent(_))));
    assert!(events.is_empty());
}

#[tokio::test]
async fn a_restored_conversation_cut_off_mid_tools_settles_the_call_and_replies() {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(Counted { runs: runs.clone() })]);

    // Saved after the model asked for a tool and before the tool answered.
    let mut asked = Message::assistant("msg-2", call("write"));
    asked.tool_calls = vec![ToolCall {
        id: "msg-2-call-1".into(),
        name: "write".into(),
        arguments: serde_json::json!({}),
    }];
    agent.replace_messages(vec![Message::user("msg-1", "Write it"), asked]);
    assert!(agent.needs_reply());

    let backend = Arc::new(ScriptedBackend::new(vec!["It may not have been written."]));
    let (result, events) = resume(&mut agent, &backend).await;
    result.unwrap();

    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "an interrupted call is never run again"
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::Warning { .. })));
    let settled = agent.messages()[2].tool_result.as_ref().unwrap();
    assert_eq!(settled.tool_call_id, "msg-2-call-1");
    assert!(settled.is_error);
    assert_eq!(
        agent.messages().last().unwrap().content,
        "It may not have been written."
    );
    assert!(!agent.needs_reply());
}

#[tokio::test]
async fn pending_calls_survive_a_save_and_restore() {
    let mut agent = Agent::new(AgentConfig::default());
    agent.set_tools(vec![Box::new(StartJob)]);
    let backend = Arc::new(ScriptedBackend::new(vec![CALL, "Started."]));
    prompt(&mut agent, &backend, "Run the job").await;

    let saved = serde_json::to_string(agent.messages()).unwrap();
    let mut restored = Agent::new(AgentConfig::default());
    restored.replace_messages(serde_json::from_str(&saved).unwrap());

    assert_eq!(restored.pending_calls(), agent.pending_calls());
    assert_eq!(restored.pending_calls().len(), 1);
}

#[tokio::test]
async fn a_child_run_streams_nested_events_and_answers_with_its_reply() {
    let child_backend: Arc<dyn LlmBackend> =
        Arc::new(ScriptedBackend::new(vec!["Paris is the capital."]));
    let child = Arc::new(tokio::sync::Mutex::new(Agent::new(AgentConfig::default())));
    let mut parent = Agent::new(AgentConfig::default());
    parent.set_tools(vec![Box::new(Delegate {
        backend: child_backend,
        child: child.clone(),
    })]);
    let backend = Arc::new(ScriptedBackend::new(vec![
        "```tool_call\n{\"name\": \"delegate\", \"arguments\": {\"task\": \"Capital of France?\"}}\n```",
        "It's Paris.",
    ]));

    let events = prompt(&mut parent, &backend, "Ask the helper").await;

    let nested: Vec<&AgentEvent> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Nested {
                tool_name, event, ..
            } if tool_name == "delegate" => Some(event.as_ref()),
            _ => None,
        })
        .collect();
    assert!(matches!(nested.first(), Some(AgentEvent::AgentStart)));
    assert!(matches!(nested.last(), Some(AgentEvent::AgentEnd { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolExecEnd { result, .. } if result.content == "Paris is the capital."
    )));
    assert_eq!(parent.messages().last().unwrap().content, "It's Paris.");
    assert_eq!(
        child.lock().await.messages().len(),
        2,
        "the child keeps its thread"
    );
}

#[tokio::test]
async fn stopping_the_parent_stops_its_child() {
    let child = Arc::new(tokio::sync::Mutex::new(Agent::new(AgentConfig::default())));
    let own_flag = child.lock().await.abort_flag();

    let mut parent = Agent::new(AgentConfig::default());
    let parent_flag = parent.abort_flag();
    let child_saw_stop = Arc::new(AtomicBool::new(false));
    let saw = child_saw_stop.clone();
    let child_backend: Arc<dyn LlmBackend> = Arc::new(
        ScriptedBackend::new(vec!["never seen"]).on_generate(move |_, abort| {
            // The host presses stop while the child is generating.
            parent_flag.store(true, Ordering::Relaxed);
            saw.store(abort.load(Ordering::Relaxed), Ordering::Relaxed);
        }),
    );
    parent.set_tools(vec![Box::new(Delegate {
        backend: child_backend,
        child: child.clone(),
    })]);
    let backend = Arc::new(ScriptedBackend::new(vec![&call("delegate"), "unreachable"]));

    let events = prompt(&mut parent, &backend, "Ask the helper").await;

    assert!(
        child_saw_stop.load(Ordering::Relaxed),
        "the child answers to the parent's flag"
    );
    assert_eq!(backend.calls(), 1, "the parent stops after the tool round");
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolExecEnd { result, .. } if result.is_error && result.content == "Aborted"
    )));
    let child = child.lock().await;
    assert!(
        Arc::ptr_eq(&child.abort_flag(), &own_flag),
        "the child gets its own flag back"
    );
    assert!(!own_flag.load(Ordering::Relaxed));
}

#[tokio::test]
async fn a_detached_context_runs_a_tool_on_its_own() {
    let ctx = ToolContext::detached("call-1", "start_job");
    ctx.update("progress goes nowhere");
    let out = StartJob.execute(serde_json::json!({}), &ctx).await.unwrap();
    assert!(out.pending);
    assert_eq!(out.content, "Started job call-1.");
}
