use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::error::CoreResult;

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
    fn tokenize_count(&self, text: &str) -> CoreResult<u32>;

    /// Whether a model is currently loaded and ready.
    fn is_ready(&self) -> bool;
}
