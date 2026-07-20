#[cfg(feature = "tools")]
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[cfg(feature = "tools")]
use crate::error::CoreResult;

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

/// A fenced code block: its info-string tag (lower-cased, may be empty) and body.
struct FencedBlock<'a> {
    tag: String,
    body: &'a str,
}

/// Yield every ```` ``` ````-fenced code block in `text`. Tolerant of leading
/// indentation on the fence and of a missing closing fence at end of input.
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
        let tag = text[after_fence..line_end].trim().to_ascii_lowercase();
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
#[cfg(feature = "tools")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Output content fed back to the model as the tool result.
    pub content: String,
    /// Optional structured details for the UI (not sent to the model).
    #[serde(default)]
    pub details: serde_json::Value,
}

/// Callback for streaming tool progress.
#[cfg(feature = "tools")]
pub type ToolUpdateCallback = Box<dyn FnMut(&str) + Send>;

/// A tool the agent can invoke.
///
/// Tools are defined by the host application and registered
/// with the agent. The agent loop calls `execute` when the
/// LLM emits a tool call.
///
/// ```no_run
/// use orion_core::{Tool, ToolOutput, CoreError, CoreResult};
/// use orion_core::tools::ToolUpdateCallback;
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
///         _tool_call_id: &str,
///         args: serde_json::Value,
///         _on_update: Option<ToolUpdateCallback>,
///     ) -> CoreResult<ToolOutput> {
///         let path = args["path"].as_str().unwrap_or("");
///         let content = std::fs::read_to_string(path)
///             .map_err(|e| CoreError::Tool(e.to_string()))?;
///         Ok(ToolOutput { content, details: serde_json::json!({ "path": path }) })
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
    async fn execute(
        &self,
        tool_call_id: &str,
        args: serde_json::Value,
        on_update: Option<ToolUpdateCallback>,
    ) -> CoreResult<ToolOutput>;

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
