use serde::{Deserialize, Serialize};

/// Role in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// System / developer instruction that frames the conversation.
    System,
    /// A message from the end user.
    User,
    /// A message from the model.
    Assistant,
    /// An assistant turn that requested one or more tool calls.
    ToolCall,
    /// The result of executing a tool, fed back to the model.
    ToolResult,
}

/// A tool invocation requested by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique id linking this call to its [`ToolResult`].
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// Arguments to pass to the tool, as a JSON value.
    pub arguments: serde_json::Value,
}

/// Result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Id of the [`ToolCall`] this result answers.
    pub tool_call_id: String,
    /// Name of the tool that produced this result.
    pub tool_name: String,
    /// The tool's output (or the error message when `is_error`).
    pub content: String,
    /// Whether the tool failed.
    pub is_error: bool,
}

/// A single message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Stable identifier for the message (used to address pins and tool results).
    pub id: String,
    /// Who produced the message.
    pub role: Role,
    /// The message text.
    pub content: String,
    /// Creation time as a Unix timestamp in milliseconds.
    pub timestamp: i64,

    /// Tool calls made by the assistant (only when role = Assistant).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,

    /// Tool execution result (only when role = ToolResult).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResult>,

    /// Token count for this message (populated after tokenization).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_count: Option<u32>,

    /// Whether this message is pinned. Pinned messages always survive context
    /// pruning, regardless of the token budget or prune strategy.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Message {
    /// Build a system message.
    ///
    /// ```
    /// use orion_core::Message;
    /// let sys = Message::system("msg-1", "You are helpful.");
    /// assert_eq!(sys.content, "You are helpful.");
    /// ```
    pub fn system(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            role: Role::System,
            content: content.into(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            tool_calls: vec![],
            tool_result: None,
            token_count: None,
            pinned: false,
        }
    }

    /// Build a user message.
    pub fn user(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            role: Role::User,
            content: content.into(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            tool_calls: vec![],
            tool_result: None,
            token_count: None,
            pinned: false,
        }
    }

    /// Build an assistant message.
    pub fn assistant(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            role: Role::Assistant,
            content: content.into(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            tool_calls: vec![],
            tool_result: None,
            token_count: None,
            pinned: false,
        }
    }

    /// Build a tool-result message, linking it back to the assistant's
    /// [`ToolCall`] via `tool_call_id`.
    ///
    /// ```
    /// use orion_core::Message;
    /// let result = Message::tool_result(
    ///     "msg-4",            // message id
    ///     "call-1",           // tool_call_id (links to the assistant's request)
    ///     "read_file",        // tool name
    ///     "file contents...", // result content
    ///     false,              // is_error
    /// );
    /// assert!(result.tool_result.is_some());
    /// ```
    pub fn tool_result(
        id: impl Into<String>,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        let tool_call_id = tool_call_id.into();
        let tool_name = tool_name.into();
        let content = content.into();
        Self {
            id: id.into(),
            role: Role::ToolResult,
            content: content.clone(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            tool_calls: vec![],
            tool_result: Some(ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
            }),
            token_count: None,
            pinned: false,
        }
    }

    /// Mark this message as pinned (survives context pruning). Builder-style.
    pub fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }
}
