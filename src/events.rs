use serde::{Deserialize, Serialize};

use crate::messages::{Message, ToolCall, ToolResult};

/// Events emitted by the agent loop.
/// Mirrors pi-agent-core's event system for UI reactivity.
///
/// This enum is `#[non_exhaustive]`: match it with a wildcard arm, as new
/// event variants may be added in a minor release.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentEvent {
    /// Agent begins a run (`prompt` or `resume`).
    AgentStart,

    /// Agent finished all processing.
    AgentEnd {
        /// Every message the run added to the conversation, in order, other than the
        /// prompt's own user message: assistant turns, tool results, messages read
        /// from the inbox and results settled for interrupted calls.
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

    /// A user or tool-result message was added. An assistant message streams as
    /// `MessageDelta` and is announced only by its `MessageEnd`.
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

    /// Timing and token statistics for one completed LLM generation.
    ///
    /// **Emission guarantee.** Exactly one `GenerationStats` is emitted for each
    /// LLM iteration that runs to completion within a single run (`prompt()` or
    /// `resume()`) - no more, no less - and always before that turn's
    /// `MessageEnd`/`TurnEnd` and before the run's closing `AgentEnd`. When tools
    /// fire, a run spans several iterations; summing the `tokens_generated` / `prompt_tokens`
    /// of every `GenerationStats` in the run therefore yields the exact per-run
    /// totals, with no gaps and no double counting. A generation that is aborted
    /// or errors before completing produces no result and so emits no
    /// `GenerationStats` (the internal summarization pass likewise does not emit
    /// one). Consumers metering usage can rely on this contract; it is pinned by
    /// tests.
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

    /// A tool call was refused by the approval hook and never executed.
    ///
    /// Distinct from a `ToolExecEnd` carrying an error result: that signals a
    /// tool that ran and failed, whereas this signals a call that was blocked
    /// before execution. The same `reason` is also appended to the conversation
    /// as an error tool result so the model can adapt.
    ToolDenied {
        /// Id of the denied tool call.
        tool_call_id: String,
        /// Name of the denied tool.
        tool_name: String,
        /// Human-readable reason the call was refused.
        reason: String,
    },

    /// A pending tool call got its result through the inbox.
    ///
    /// The result is also added to the conversation as a tool-result message for the
    /// same call, so the model reads it on its next generation.
    ToolResolved {
        /// Id of the call that was pending.
        tool_call_id: String,
        /// Name of the tool the call was for.
        tool_name: String,
        /// The late result.
        result: ToolResult,
    },

    /// The run stopped to wait on pending tool calls.
    ///
    /// Emitted just before `AgentEnd` when `AgentConfig::pause_on_pending` is set and a
    /// turn left calls pending with nothing in the inbox to go on. Deliver the results
    /// through the agent's inbox and call `Agent::resume` to carry on.
    Paused {
        /// The calls still waiting on a result.
        pending: Vec<ToolCall>,
    },

    /// An event from a child agent run inside a tool call.
    ///
    /// A tool that runs another agent through `ToolContext::run_child` forwards the
    /// child's events wrapped in this, so a host can draw the child's work inside the
    /// call that started it. A grandchild's events arrive nested twice.
    Nested {
        /// Id of the tool call running the child.
        tool_call_id: String,
        /// Name of the tool running the child.
        tool_name: String,
        /// The child's event.
        event: Box<AgentEvent>,
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
