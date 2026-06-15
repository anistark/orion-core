use serde::{Deserialize, Serialize};

use crate::messages::{Message, ToolResult};

/// Events emitted by the agent loop.
/// Mirrors pi-agent-core's event system for UI reactivity.
///
/// This enum is `#[non_exhaustive]`: match it with a wildcard arm, as new
/// event variants may be added in a minor release.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentEvent {
    /// Agent begins processing a prompt.
    AgentStart,

    /// Agent finished all processing.
    AgentEnd {
        /// All messages produced during this `prompt()` call.
        messages: Vec<Message>,
    },

    /// A new turn begins (one LLM call + any tool executions).
    TurnStart,

    /// A turn completed.
    TurnEnd {
        /// The assistant message produced by the turn.
        message: Message,
        /// Results of any tools the turn executed.
        tool_results: Vec<ToolResult>,
    },

    /// A message was added (user, assistant, or tool_result).
    MessageStart {
        /// The message that was added.
        message: Message,
    },

    /// Streaming delta for the current assistant message.
    MessageDelta {
        /// The new token/chunk of text.
        delta: String,
        /// Tokens generated so far in this response.
        tokens_generated: u32,
        /// Current generation speed.
        tokens_per_sec: f64,
    },

    /// A message is complete.
    MessageEnd {
        /// The completed message.
        message: Message,
    },

    /// Final timing statistics for a completed generation. Emitted once per
    /// successful response so the UI / observability layer can record the real
    /// time-to-first-token and generation time (not approximated from tps).
    GenerationStats {
        /// Tokens generated in the response.
        tokens_generated: u32,
        /// Tokens in the formatted prompt.
        prompt_tokens: u32,
        /// Average generation speed in tokens per second.
        tokens_per_sec: f64,
        /// Time to the first emitted token, in milliseconds.
        time_to_first_token_ms: f64,
        /// Total generation time, in milliseconds.
        generation_time_ms: f64,
    },

    /// A tool execution started.
    ToolExecStart {
        /// Id of the tool call being executed.
        tool_call_id: String,
        /// Name of the tool being executed.
        tool_name: String,
        /// Arguments passed to the tool.
        args: serde_json::Value,
    },

    /// Streaming progress from a tool execution.
    ToolExecUpdate {
        /// Id of the tool call reporting progress.
        tool_call_id: String,
        /// Name of the tool reporting progress.
        tool_name: String,
        /// Partial output emitted so far.
        partial: String,
    },

    /// A tool execution completed.
    ToolExecEnd {
        /// Id of the completed tool call.
        tool_call_id: String,
        /// Name of the completed tool.
        tool_name: String,
        /// The tool's result.
        result: ToolResult,
    },

    /// Context budget info after formatting.
    ContextBudget {
        /// Tokens used by the prepared prompt.
        used_tokens: u32,
        /// Maximum context tokens available.
        max_tokens: u32,
        /// Number of messages kept in the prompt.
        messages_in_context: u32,
        /// Number of messages pruned to fit.
        messages_pruned: u32,
    },

    /// Non-fatal warning during processing.
    Warning {
        /// Human-readable warning text.
        message: String,
    },

    /// Fatal error that stopped processing.
    Error {
        /// Human-readable error text.
        message: String,
    },
}
