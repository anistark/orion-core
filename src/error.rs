use serde::Serialize;

/// Errors from the orion-core agent harness.
///
/// All variants are serializable (via [`serde::Serialize`]) for easy transport
/// over IPC.
///
/// ```
/// use orion_core::CoreError;
///
/// let err = CoreError::Backend("No model loaded".into());
/// assert_eq!(err.to_string(), "Backend error: No model loaded");
/// ```
///
/// This enum is `#[non_exhaustive]`: match it with a wildcard arm, as new
/// variants may be added in a minor release.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// The LLM backend failed (no model loaded, inference error, etc.).
    #[error("Backend error: {0}")]
    Backend(String),

    /// Context preparation failed (e.g. the prompt cannot fit the budget).
    #[error("Context error: {0}")]
    Context(String),

    /// A tool failed to execute or no tool matched the requested name.
    #[error("Tool error: {0}")]
    Tool(String),

    /// Agent-level logic error (e.g. an empty prompt).
    #[error("Agent error: {0}")]
    Agent(String),

    /// Generation was cancelled via the abort flag.
    #[error("Aborted")]
    Aborted,
}

impl Serialize for CoreError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Convenience alias for a `Result` whose error is [`CoreError`].
pub type CoreResult<T> = Result<T, CoreError>;
