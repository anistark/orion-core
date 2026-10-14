use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Something waiting in an [`Inbox`] for the agent to read.
#[derive(Debug, Clone)]
pub(crate) enum Inbound {
    /// A user message that arrived while the agent was busy or idle.
    Message(String),
    /// The late result of a tool call that answered with a pending output.
    Result {
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
}

/// A handle for reaching an agent from outside its run.
///
/// [`Agent::prompt`](crate::Agent::prompt) borrows the agent for the whole run, so
/// nothing else can touch it until the run ends. An inbox is the way in meanwhile:
/// it is cheap to clone, can be moved to another task or thread, and is read by the
/// agent at fixed points.
///
/// - Before every model call after the first, so a message pushed mid-run is seen on
///   the next step.
/// - When the model gives a final answer, so anything that landed during that
///   generation is answered in the same run rather than left waiting.
/// - At the start of [`prompt`](crate::Agent::prompt), ahead of the prompt's own text,
///   and at the start of [`resume`](crate::Agent::resume).
///
/// A message becomes a user turn. A result answers a call whose tool returned
/// `ToolOutput::pending` (feature `tools`). Nothing in an inbox is lost when a run
/// ends: whatever arrives after the last read waits for the next run, which a host
/// starts with [`resume`](crate::Agent::resume) when there is no user text.
///
/// ```
/// use orion_core::{Agent, AgentConfig};
///
/// let agent = Agent::new(AgentConfig::default());
/// let inbox = agent.inbox();
///
/// inbox.push("Also, check the README.");
/// assert_eq!(inbox.len(), 1);
/// ```
#[derive(Debug, Clone, Default)]
pub struct Inbox {
    queue: Arc<Mutex<VecDeque<Inbound>>>,
}

impl Inbox {
    /// Queue a user message for the agent's next read.
    pub fn push(&self, text: impl Into<String>) {
        self.queue().push_back(Inbound::Message(text.into()));
    }

    /// Deliver the result of a pending tool call.
    ///
    /// `tool_call_id` is the id of the call whose tool returned `ToolOutput::pending`,
    /// as it appears in [`Agent::pending_calls`](crate::Agent::pending_calls) and on the
    /// tool's events.
    /// A result for a call that is not pending is dropped with a
    /// [`Warning`](crate::AgentEvent::Warning) when the agent reads it.
    pub fn resolve(&self, tool_call_id: impl Into<String>, content: impl Into<String>) {
        self.queue().push_back(Inbound::Result {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: false,
        });
    }

    /// Fail a pending tool call, with the reason the model is told.
    pub fn reject(&self, tool_call_id: impl Into<String>, reason: impl Into<String>) {
        self.queue().push_back(Inbound::Result {
            tool_call_id: tool_call_id.into(),
            content: reason.into(),
            is_error: true,
        });
    }

    /// How many items are waiting to be read.
    pub fn len(&self) -> usize {
        self.queue().len()
    }

    /// Whether nothing is waiting to be read.
    pub fn is_empty(&self) -> bool {
        self.queue().is_empty()
    }

    /// Take everything waiting, oldest first.
    pub(crate) fn take(&self) -> Vec<Inbound> {
        self.queue().drain(..).collect()
    }

    fn queue(&self) -> MutexGuard<'_, VecDeque<Inbound>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
