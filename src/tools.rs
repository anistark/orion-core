#[cfg(feature = "tools")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "tools")]
use std::sync::Arc;

#[cfg(feature = "tools")]
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
#[cfg(feature = "tools")]
use tokio::sync::mpsc;

#[cfg(feature = "tools")]
use crate::{
    agent::Agent,
    backend::Backend,
    error::{CoreError, CoreResult},
    events::AgentEvent,
    messages::Message,
};

/// A tool call parsed out of an assistant message.
///
/// Produced by [`parse_tool_calls`]. The agent assigns each call an id and
/// dispatches it to the matching registered `Tool`.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedToolCall {
    /// Tool name the model wants to invoke.
    pub name: String,
    /// Arguments object (defaults to `{}` when the model omits it).
    pub arguments: serde_json::Value,
}

/// Parse tool calls out of an assistant message.
///
/// Templates advertise the convention rendered by `render_tools`: a fenced
/// ```` ```tool_call ```` block holding `{"name": ..., "arguments": {...}}`
/// (or a JSON array of such objects). Parsing is deliberately lenient so
/// smaller local models still trigger tools when they drift from the exact
/// format:
///
/// 1. Every ```` ```tool_call ```` and ```` ```json ```` fenced block is parsed.
///    The tag is matched after trimming any punctuation around it, so a model
///    that writes ```` ```tool_call> ```` or ```` ```<json> ```` still triggers.
/// 2. If no fenced block yields a call, the *whole trimmed message* is tried as
///    a single JSON object - but only when it carries both `name` and
///    `arguments` keys.
///
/// Arbitrary mid-prose substrings are never scanned, so ordinary replies that
/// merely mention JSON don't produce false positives. Entries without a string
/// `name` are skipped; a missing `arguments` defaults to an empty object.
///
/// ```
/// use orion_core::parse_tool_calls;
///
/// let reply = "Sure.\n```tool_call\n\
///              {\"name\": \"read_file\", \"arguments\": {\"path\": \"Cargo.toml\"}}\n```";
/// let calls = parse_tool_calls(reply);
/// assert_eq!(calls.len(), 1);
/// assert_eq!(calls[0].name, "read_file");
/// assert_eq!(calls[0].arguments["path"], "Cargo.toml");
///
/// // A plain answer that merely mentions JSON yields nothing.
/// assert!(parse_tool_calls("Here is a name field somewhere.").is_empty());
/// ```
pub fn parse_tool_calls(text: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    for block in fenced_blocks(text) {
        if matches!(block.tag.as_str(), "tool_call" | "json") {
            collect_calls(block.body, &mut calls);
        }
    }

    // Fallback: a bare JSON object that is the entire message. Guarded on the
    // presence of both keys so plain JSON answers aren't mistaken for calls.
    if calls.is_empty() {
        let trimmed = text.trim();
        if trimmed.starts_with('{')
            && trimmed.contains("\"name\"")
            && trimmed.contains("\"arguments\"")
        {
            collect_calls(trimmed, &mut calls);
        }
    }

    calls
}

/// Parse one JSON snippet (object or array of objects) into tool calls,
/// appending any well-formed entries to `out`.
fn collect_calls(snippet: &str, out: &mut Vec<ParsedToolCall>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(snippet.trim()) else {
        return;
    };
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(call) = call_from_value(&item) {
                    out.push(call);
                }
            }
        }
        other => {
            if let Some(call) = call_from_value(&other) {
                out.push(call);
            }
        }
    }
}

/// Extract a single [`ParsedToolCall`] from a JSON value, if it names a tool.
fn call_from_value(value: &serde_json::Value) -> Option<ParsedToolCall> {
    let name = value.get("name")?.as_str()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let arguments = value
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Some(ParsedToolCall { name, arguments })
}

/// A fenced code block: its info-string tag (normalized, may be empty) and body.
struct FencedBlock<'a> {
    tag: String,
    body: &'a str,
}

/// Normalize a fence's info string for matching: lower-cased, with any
/// punctuation a model drifts into stripped from both ends, so `tool_call>`,
/// `<tool_call>` and `` `json` `` read as `tool_call` and `json`.
fn fence_tag(info: &str) -> String {
    info.trim()
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .to_ascii_lowercase()
}

/// Yield every ```` ``` ````-fenced code block in `text`. Tolerant of leading
/// indentation on the fence, of stray punctuation around the tag, and of a
/// missing closing fence at end of input.
fn fenced_blocks(text: &str) -> Vec<FencedBlock<'_>> {
    let mut blocks = Vec::new();
    let bytes = text.as_bytes();
    let mut search = 0;

    while let Some(rel) = text[search..].find("```") {
        let open = search + rel;
        // Tag runs from after the fence to the end of that line.
        let after_fence = open + 3;
        let line_end = text[after_fence..]
            .find('\n')
            .map(|i| after_fence + i)
            .unwrap_or(bytes.len());
        let tag = fence_tag(&text[after_fence..line_end]);
        let body_start = (line_end + 1).min(bytes.len());

        // Body runs to the next closing fence, or to EOF if unterminated.
        let (body_end, next) = match text[body_start..].find("```") {
            Some(i) => (body_start + i, body_start + i + 3),
            None => (bytes.len(), bytes.len()),
        };

        blocks.push(FencedBlock {
            tag,
            body: &text[body_start..body_end],
        });
        search = next;
    }

    blocks
}

/// Schema describing a tool's parameters (JSON Schema subset).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Tool name (must match what the model emits in a tool call).
    pub name: String,
    /// Human-readable description shown to the model.
    pub description: String,
    /// JSON Schema describing the tool's accepted arguments.
    pub parameters: serde_json::Value,
}

/// Result returned by a tool execution.
///
/// ```
/// use orion_core::ToolOutput;
///
/// let done = ToolOutput::new("42").with_details(serde_json::json!({ "rows": 1 }));
/// assert!(!done.pending);
///
/// let later = ToolOutput::pending("Started task 7. Its result will follow.");
/// assert!(later.pending);
/// ```
#[cfg(feature = "tools")]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Output content fed back to the model as the tool result.
    pub content: String,
    /// Optional structured details for the UI (not sent to the model).
    #[serde(default)]
    pub details: serde_json::Value,
    /// Whether the real result is still to come.
    ///
    /// A pending output is recorded as a placeholder result carrying `content`, so the
    /// conversation stays well formed and the model knows the work is under way. The
    /// call is then listed by [`Agent::pending_calls`] until its result is delivered
    /// through the agent's [`Inbox`](crate::Inbox).
    #[serde(default, skip_serializing_if = "crate::messages::is_false")]
    pub pending: bool,
}

#[cfg(feature = "tools")]
impl ToolOutput {
    /// A finished result.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ..Self::default()
        }
    }

    /// A placeholder for a result that will be delivered later.
    ///
    /// `content` is what the model reads in the meantime, so say what was started and
    /// that the result will follow. Deliver the result through the agent's
    /// [`Inbox`](crate::Inbox) and let the model act on it with
    /// [`Agent::resume`]:
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    /// # use async_trait::async_trait;
    /// # use orion_core::{Agent, AgentConfig, CoreResult, GenerationResult, InferenceParams,
    /// #     LlmBackend, TokenCallback, Tool, ToolContext, ToolOutput};
    /// # struct Scripted(AtomicUsize);
    /// # impl LlmBackend for Scripted {
    /// #     fn generate(&self, _p: &str, _x: &InferenceParams, _a: Arc<AtomicBool>,
    /// #         mut on_token: TokenCallback) -> CoreResult<GenerationResult> {
    /// #         let text = match self.0.fetch_add(1, Ordering::SeqCst) {
    /// #             0 => "```tool_call\n{\"name\": \"report\", \"arguments\": {}}\n```",
    /// #             1 => "Started the report.",
    /// #             _ => "The report is in out/nightly.csv.",
    /// #         };
    /// #         on_token(text, 1, 1.0);
    /// #         Ok(GenerationResult { text: text.into(), tokens_generated: 1,
    /// #             prompt_tokens: 0, tokens_per_sec: 1.0,
    /// #             time_to_first_token_ms: 1.0, generation_time_ms: 1.0 })
    /// #     }
    /// #     fn tokenize_count(&self, t: &str) -> CoreResult<u32> {
    /// #         Ok(t.split_whitespace().count() as u32) }
    /// #     fn is_ready(&self) -> bool { true }
    /// # }
    /// struct Report;
    ///
    /// #[async_trait]
    /// impl Tool for Report {
    ///     // name, label, description and parameters_schema as usual
    /// #   fn name(&self) -> &str { "report" }
    /// #   fn label(&self) -> &str { "Report" }
    /// #   fn description(&self) -> &str { "Start the nightly report" }
    /// #   fn parameters_schema(&self) -> serde_json::Value { serde_json::json!({}) }
    ///     async fn execute(&self, _args: serde_json::Value, _ctx: &ToolContext) -> CoreResult<ToolOutput> {
    ///         // Start the job on the host's own runtime, then answer straight away.
    ///         Ok(ToolOutput::pending("Started the report. Its result will follow."))
    ///     }
    /// }
    ///
    /// # fn main() -> CoreResult<()> {
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let mut agent = Agent::new(AgentConfig::default());
    /// # agent.set_tools(vec![Box::new(Report)]);
    /// # let backend: Arc<dyn LlmBackend> = Arc::new(Scripted(AtomicUsize::new(0)));
    /// # let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    /// # let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
    /// let inbox = agent.inbox();
    /// agent.prompt("Run the nightly report", backend.clone(), tx).await?;
    ///
    /// // Later, when the job the tool started finishes:
    /// for call in agent.pending_calls() {
    ///     inbox.resolve(&call.id, "Report written to out/nightly.csv");
    /// }
    /// agent.resume(backend, tx2).await?;
    /// # assert!(agent.pending_calls().is_empty());
    /// # assert_eq!(agent.messages().last().unwrap().content, "The report is in out/nightly.csv.");
    /// # Ok(())
    /// # })
    /// # }
    /// ```
    pub fn pending(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            pending: true,
            ..Self::default()
        }
    }

    /// Attach structured details for the UI. Builder-style.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }
}

/// What a tool knows about the call it is serving.
///
/// The agent builds one for every call and hands it to [`Tool::execute`]. It carries
/// the call's identity, the run's stop flag, and the way back into the run's event
/// stream, for progress and for the events of a child agent.
///
/// A host testing a tool on its own can build one with [`ToolContext::detached`].
#[cfg(feature = "tools")]
#[derive(Debug, Clone)]
pub struct ToolContext {
    call_id: String,
    tool_name: String,
    abort: Arc<AtomicBool>,
    events: mpsc::UnboundedSender<AgentEvent>,
}

#[cfg(feature = "tools")]
impl ToolContext {
    /// A context for a call, reporting into `events` and stopped by `abort`.
    pub fn new(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        abort: Arc<AtomicBool>,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            abort,
            events,
        }
    }

    /// A context whose events go nowhere and which is never stopped, for running a
    /// tool outside an agent.
    pub fn detached(call_id: impl Into<String>, tool_name: impl Into<String>) -> Self {
        let (events, _) = mpsc::unbounded_channel();
        Self::new(call_id, tool_name, Arc::default(), events)
    }

    /// Id of the call being served.
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Name the call was made under.
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Whether the run has been asked to stop. A long tool should check this and
    /// return early when it is set.
    pub fn is_aborted(&self) -> bool {
        self.abort.load(Ordering::Relaxed)
    }

    /// The run's stop flag, for handing to work the tool starts.
    pub fn abort_flag(&self) -> Arc<AtomicBool> {
        self.abort.clone()
    }

    /// Report progress, as an [`AgentEvent::ToolExecUpdate`].
    pub fn update(&self, partial: impl Into<String>) {
        self.events
            .send(AgentEvent::ToolExecUpdate {
                tool_call_id: self.call_id.clone(),
                tool_name: self.tool_name.clone(),
                partial: partial.into(),
            })
            .ok();
    }

    /// Pass an event from work the tool is running into the run, wrapped as an
    /// [`AgentEvent::Nested`] under this call.
    pub fn forward(&self, event: AgentEvent) {
        self.events
            .send(AgentEvent::Nested {
                tool_call_id: self.call_id.clone(),
                tool_name: self.tool_name.clone(),
                event: Box::new(event),
            })
            .ok();
    }

    /// Run a child agent on `text` as part of this call and answer with its reply.
    ///
    /// The child's events are forwarded live as [`AgentEvent::Nested`] under this call,
    /// so a host can show the child's work inside the parent's turn. For the length of
    /// the run the child answers to the parent's stop flag: stopping the parent stops
    /// the child, and a flag the host took from the child earlier with
    /// [`Agent::abort_flag`] does not reach this run.
    ///
    /// Answers `Err(CoreError::Aborted)` when the parent was stopped, and the child's
    /// error when its run failed. The child keeps its conversation, so a tool that
    /// holds on to it can send it a follow-up later.
    ///
    /// ```
    /// use async_trait::async_trait;
    /// use orion_core::{Agent, AgentConfig, Backend, CoreResult, Tool, ToolContext, ToolOutput};
    ///
    /// /// Hands a question to a researcher agent and answers with what it found.
    /// struct Delegate {
    ///     backend: Backend,
    /// }
    ///
    /// #[async_trait]
    /// impl Tool for Delegate {
    ///     fn name(&self) -> &str { "delegate" }
    ///     fn label(&self) -> &str { "Delegate" }
    ///     fn description(&self) -> &str { "Ask a researcher to look into something" }
    ///     fn parameters_schema(&self) -> serde_json::Value {
    ///         serde_json::json!({
    ///             "type": "object",
    ///             "properties": { "task": { "type": "string" } },
    ///             "required": ["task"]
    ///         })
    ///     }
    ///
    ///     async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> CoreResult<ToolOutput> {
    ///         let mut researcher = Agent::new(AgentConfig {
    ///             system_prompt: "You research one question and report back briefly.".into(),
    ///             ..AgentConfig::default()
    ///         });
    ///         let task = args["task"].as_str().unwrap_or_default();
    ///         let reply = ctx.run_child(&mut researcher, task, self.backend.clone()).await?;
    ///         Ok(ToolOutput::new(reply.content))
    ///     }
    /// }
    /// ```
    pub async fn run_child(
        &self,
        child: &mut Agent,
        text: impl Into<String>,
        backend: impl Into<Backend>,
    ) -> CoreResult<Message> {
        if self.is_aborted() {
            return Err(CoreError::Aborted);
        }

        let (tx, mut rx) = mpsc::unbounded_channel();
        let own = child.lend_abort(self.abort.clone());

        let run = child.prompt(text, backend, tx);
        let forward = async {
            let mut reply = None;
            let mut failed = None;
            while let Some(event) = rx.recv().await {
                match &event {
                    AgentEvent::TurnEnd { message, .. } => reply = Some(message.clone()),
                    AgentEvent::Error { message } => failed = Some(message.clone()),
                    _ => {}
                }
                self.forward(event);
            }
            (reply, failed)
        };
        let (ran, (reply, failed)) = tokio::join!(run, forward);

        child.restore_abort(own);
        ran?;

        if self.is_aborted() {
            return Err(CoreError::Aborted);
        }
        if let Some(message) = failed {
            return Err(CoreError::Agent(message));
        }
        reply.ok_or_else(|| CoreError::Agent("The child agent produced no reply".into()))
    }
}

/// A tool the agent can invoke.
///
/// Tools are defined by the host application and registered
/// with the agent. The agent loop calls `execute` when the
/// LLM emits a tool call.
///
/// ```no_run
/// use orion_core::{CoreError, CoreResult, Tool, ToolContext, ToolOutput};
/// use async_trait::async_trait;
///
/// struct ReadFileTool;
///
/// #[async_trait]
/// impl Tool for ReadFileTool {
///     fn name(&self) -> &str { "read_file" }
///     fn label(&self) -> &str { "Read File" }
///     fn description(&self) -> &str { "Read a file from disk" }
///
///     fn parameters_schema(&self) -> serde_json::Value {
///         serde_json::json!({
///             "type": "object",
///             "properties": { "path": { "type": "string" } },
///             "required": ["path"]
///         })
///     }
///
///     async fn execute(
///         &self,
///         args: serde_json::Value,
///         _ctx: &ToolContext,
///     ) -> CoreResult<ToolOutput> {
///         let path = args["path"].as_str().unwrap_or("");
///         let content = std::fs::read_to_string(path)
///             .map_err(|e| CoreError::Tool(e.to_string()))?;
///         Ok(ToolOutput::new(content).with_details(serde_json::json!({ "path": path })))
///     }
/// }
///
/// // Register with the agent: `agent.set_tools(vec![Box::new(ReadFileTool)]);`
/// ```
#[cfg(feature = "tools")]
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name (must match what the LLM outputs).
    fn name(&self) -> &str;

    /// Human-readable label for UI display.
    fn label(&self) -> &str;

    /// Description shown to the LLM in the system prompt.
    fn description(&self) -> &str;

    /// JSON Schema for the tool's parameters.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with the given arguments.
    ///
    /// `ctx` identifies the call and carries the run's stop flag and event stream. Return
    /// [`ToolOutput::pending`] to hand the model a placeholder now and deliver the
    /// result later through the agent's [`Inbox`](crate::Inbox).
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> CoreResult<ToolOutput>;

    /// Return the full schema for system prompt injection.
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fenced_tool_call() {
        let text =
            "Sure.\n```tool_call\n{\"name\": \"add\", \"arguments\": {\"a\": 2, \"b\": 3}}\n```";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "add");
        assert_eq!(calls[0].arguments, json!({"a": 2, "b": 3}));
    }

    #[test]
    fn parses_array_of_calls() {
        let text = "```tool_call\n[{\"name\": \"a\", \"arguments\": {}}, {\"name\": \"b\", \"arguments\": {\"x\": 1}}]\n```";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "a");
        assert_eq!(calls[1].name, "b");
        assert_eq!(calls[1].arguments, json!({"x": 1}));
    }

    #[test]
    fn parses_json_tagged_fence() {
        let text = "```json\n{\"name\": \"now\", \"arguments\": {}}\n```";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "now");
    }

    #[test]
    fn parses_bare_object_whole_message() {
        let text = "  {\"name\": \"now\", \"arguments\": {}}  ";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "now");
    }

    #[test]
    fn missing_arguments_defaults_to_empty_object() {
        let text = "```tool_call\n{\"name\": \"now\"}\n```";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, json!({}));
    }

    #[test]
    fn parses_a_tag_with_stray_punctuation() {
        let text = "```tool_call>\n{\"name\": \"a\", \"arguments\": {}}\n```\n\
                    ```<tool_call>\n{\"name\": \"b\", \"arguments\": {}}\n```\n\
                    ```JSON:\n{\"name\": \"c\", \"arguments\": {}}\n```";
        let calls = parse_tool_calls(text);
        let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn collects_multiple_fenced_blocks() {
        let text = "```tool_call\n{\"name\": \"a\", \"arguments\": {}}\n```\nthen\n```tool_call\n{\"name\": \"b\", \"arguments\": {}}\n```";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn skips_malformed_and_nameless() {
        let text = "```tool_call\nnot json at all\n```";
        assert!(parse_tool_calls(text).is_empty());
        let nameless = "```tool_call\n{\"arguments\": {}}\n```";
        assert!(parse_tool_calls(nameless).is_empty());
    }

    #[test]
    fn plain_prose_yields_no_calls() {
        let text = "Here is some JSON you might use: it has a name field somewhere.";
        assert!(parse_tool_calls(text).is_empty());
        // A normal JSON answer without `arguments` must not be treated as a call.
        let answer = "{\"name\": \"Ada\", \"age\": 36}";
        assert!(parse_tool_calls(answer).is_empty());
    }

    #[test]
    fn handles_unterminated_fence() {
        let text = "```tool_call\n{\"name\": \"now\", \"arguments\": {}}";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "now");
    }
}
