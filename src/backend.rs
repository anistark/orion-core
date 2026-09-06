use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::error::CoreResult;
#[cfg(feature = "chat-backend")]
use crate::messages::Message;

/// Token callback invoked for each generated token.
/// Receives the token text, tokens generated so far, and current tokens/sec.
pub type TokenCallback = Box<dyn FnMut(&str, u32, f64) + Send>;

/// Inference parameters for a single generation request.
#[derive(Debug, Clone)]
pub struct InferenceParams {
    /// Maximum number of tokens to generate in the response.
    pub max_tokens: u32,
    /// Sampling temperature (0.0 = deterministic, higher = more random).
    pub temperature: f32,
    /// Context window size in tokens to allocate for this request.
    pub context_size: u32,
    /// Number of CPU threads to use for inference.
    pub n_threads: u32,
}

impl Default for InferenceParams {
    fn default() -> Self {
        let default_threads = std::thread::available_parallelism()
            .map(|n| (n.get() as u32).saturating_sub(2).max(1))
            .unwrap_or(4);
        Self {
            max_tokens: 2048,
            temperature: 0.7,
            context_size: 4096,
            n_threads: default_threads,
        }
    }
}

/// Result of a completed generation.
#[derive(Debug, Clone)]
pub struct GenerationResult {
    /// The full generated text.
    pub text: String,
    /// Number of tokens generated in the response.
    pub tokens_generated: u32,
    /// Number of tokens in the (formatted) prompt that was fed in.
    pub prompt_tokens: u32,
    /// Average generation speed in tokens per second.
    pub tokens_per_sec: f64,
    /// Time from request start to the first emitted token, in milliseconds.
    pub time_to_first_token_ms: f64,
    /// Total generation time, in milliseconds.
    pub generation_time_ms: f64,
}

/// Trait for LLM backends (llama.cpp, MLX, cloud APIs, etc.).
///
/// The agent loop is backend-agnostic. OrionPod implements this
/// with llama.cpp; other backends can be swapped in freely.
///
/// `generate` runs synchronously on a blocking thread. The agent
/// loop handles the async orchestration around it.
///
/// ```no_run
/// use orion_core::{LlmBackend, InferenceParams, GenerationResult, TokenCallback, CoreResult};
/// use std::sync::atomic::AtomicBool;
/// use std::sync::Arc;
///
/// struct MyBackend; // your engine state
///
/// impl LlmBackend for MyBackend {
///     fn generate(
///         &self,
///         prompt: &str,             // fully formatted (chat template applied)
///         params: &InferenceParams, // max_tokens, temperature, context_size, n_threads
///         abort: Arc<AtomicBool>,   // check each token to support cancellation
///         on_token: TokenCallback,  // call with (token_text, count, tokens_per_sec)
///     ) -> CoreResult<GenerationResult> {
///         // Feed prompt, sample tokens, call on_token per token, return stats.
///         todo!()
///     }
///
///     fn tokenize_count(&self, text: &str) -> CoreResult<u32> {
///         // Count tokens without running inference (used for budgeting).
///         todo!()
///     }
///
///     fn is_ready(&self) -> bool {
///         // Whether a model is loaded and ready.
///         todo!()
///     }
/// }
/// ```
pub trait LlmBackend: Send + Sync {
    /// Run inference on a formatted prompt string.
    ///
    /// The prompt is already fully formatted (chat template applied).
    /// The backend just needs to feed it and generate tokens.
    fn generate(
        &self,
        prompt: &str,
        params: &InferenceParams,
        abort: Arc<AtomicBool>,
        on_token: TokenCallback,
    ) -> CoreResult<GenerationResult>;

    /// Count tokens in a string without running inference.
    ///
    /// Defaults to the usual four-characters-a-token approximation, which is what a
    /// backend with no local tokenizer can honestly offer. Override it wherever the real
    /// count is cheap: context budgeting is only as good as this answer.
    fn tokenize_count(&self, text: &str) -> CoreResult<u32> {
        Ok(estimate_tokens(text))
    }

    /// Whether a model is currently loaded and ready.
    ///
    /// Defaults to true, which is the right answer for a backend with nothing to load.
    fn is_ready(&self) -> bool {
        true
    }
}

/// Tokens in a string, approximated at four characters each.
///
/// A remote endpoint exposes no cheap tokenizer, so this is what stands in for one when
/// the budget has to be decided before the request goes out. Real counts come back with
/// the response and are reported in [`GenerationResult`].
pub fn estimate_tokens(text: &str) -> u32 {
    (text.chars().count() as u32 / 4).max(1)
}

/// A backend that takes the conversation as messages rather than as a formatted prompt.
///
/// [`LlmBackend`] hands over a string that has already had a [`ChatTemplate`] applied,
/// which is what a local engine wants and what a hosted chat API does not: those take a
/// structured message list and apply the model's own template themselves. Sending a
/// formatted prompt to one means either templating twice or collapsing every turn into a
/// single user message, and both change what the model sees.
///
/// So a hosted provider implements this instead, and the agent skips its own templating
/// for it. Everything else - pruning, the tool loop, the event stream - is unchanged.
///
/// Unlike `generate` this is async, because the work is I/O rather than compute and there
/// is no reason to hold a blocking thread for it.
///
/// [`ChatTemplate`]: crate::ChatTemplate
#[cfg(feature = "chat-backend")]
#[async_trait::async_trait]
pub trait ChatBackend: Send + Sync {
    /// Run one turn against a conversation.
    ///
    /// `system` is the system prompt with any tool instructions already folded in, kept
    /// apart from `messages` because that is how every chat API takes it. `messages` are
    /// the turns that survived pruning, in order, and carry no system message of their own.
    async fn chat(
        &self,
        system: &str,
        messages: &[Message],
        params: &InferenceParams,
        abort: Arc<AtomicBool>,
        on_token: TokenCallback,
    ) -> CoreResult<GenerationResult>;

    /// Count tokens in a string, for context budgeting. See
    /// [`LlmBackend::tokenize_count`]; the same approximation applies.
    fn tokenize_count(&self, text: &str) -> u32 {
        estimate_tokens(text)
    }

    /// Whether the backend can be asked to run. Defaults to true.
    fn is_ready(&self) -> bool {
        true
    }
}

/// Whichever kind of backend a turn is being run against.
///
/// Callers do not name this: `Arc<dyn LlmBackend>` and `Arc<dyn ChatBackend>` both convert
/// into it, so [`Agent::prompt`](crate::Agent::prompt) takes either and every existing
/// caller keeps compiling.
#[derive(Clone)]
pub enum Backend {
    /// A backend fed a formatted prompt string.
    Prompt(Arc<dyn LlmBackend>),
    /// A backend fed structured messages.
    #[cfg(feature = "chat-backend")]
    Chat(Arc<dyn ChatBackend>),
}

impl Backend {
    /// Whether the backend can be asked to run.
    pub fn is_ready(&self) -> bool {
        match self {
            Self::Prompt(backend) => backend.is_ready(),
            #[cfg(feature = "chat-backend")]
            Self::Chat(backend) => backend.is_ready(),
        }
    }

    /// Tokens in a string, however this backend counts them.
    pub fn tokenize_count(&self, text: &str) -> u32 {
        match self {
            Self::Prompt(backend) => backend.tokenize_count(text).unwrap_or(0),
            #[cfg(feature = "chat-backend")]
            Self::Chat(backend) => backend.tokenize_count(text),
        }
    }
}

impl From<Arc<dyn LlmBackend>> for Backend {
    fn from(backend: Arc<dyn LlmBackend>) -> Self {
        Self::Prompt(backend)
    }
}

#[cfg(feature = "chat-backend")]
impl From<Arc<dyn ChatBackend>> for Backend {
    fn from(backend: Arc<dyn ChatBackend>) -> Self {
        Self::Chat(backend)
    }
}
