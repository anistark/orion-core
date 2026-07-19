//! Orion: agent harness for local LLM inference, published as the `orion-core` crate.
//!
//! Provides the agent loop, context pipeline, tool execution,
//! and event system for building AI chat interfaces on top of
//! local model backends (llama.cpp, MLX, etc.).
//!
//! # Architecture
//!
//! ```text
//! User prompt
//!   → Agent.prompt()
//!     → Context pipeline (prune pairs + template format)
//!       → LlmBackend.generate() (streaming tokens)
//!         → Tool execution loop (parse calls → run tools → feed results back)
//!           → AgentEvent stream → UI
//! ```
//!
//! The crate is backend-agnostic. Implement [`backend::LlmBackend`]
//! for your inference engine and the agent handles the rest.
//!
//! # Example
//!
//! Implement [`LlmBackend`] for your engine, then drive the agent. The mock
//! backend below streams a canned reply so the whole loop runs end to end (a
//! complete version lives in `examples/mock_backend.rs`):
//!
//! ```
//! use std::sync::Arc;
//! use std::sync::atomic::AtomicBool;
//! use orion_core::{
//!     Agent, AgentConfig, AgentEvent, CoreResult, GenerationResult,
//!     InferenceParams, LlmBackend, TokenCallback,
//! };
//! use tokio::sync::mpsc;
//!
//! struct MockBackend;
//! impl LlmBackend for MockBackend {
//!     fn generate(
//!         &self,
//!         _prompt: &str,
//!         _params: &InferenceParams,
//!         _abort: Arc<AtomicBool>,
//!         mut on_token: TokenCallback,
//!     ) -> CoreResult<GenerationResult> {
//!         on_token("Hi!", 1, 10.0);
//!         Ok(GenerationResult {
//!             text: "Hi!".into(),
//!             tokens_generated: 1,
//!             prompt_tokens: 0,
//!             tokens_per_sec: 10.0,
//!             time_to_first_token_ms: 1.0,
//!             generation_time_ms: 1.0,
//!         })
//!     }
//!     fn tokenize_count(&self, text: &str) -> CoreResult<u32> {
//!         Ok(text.split_whitespace().count() as u32)
//!     }
//!     fn is_ready(&self) -> bool { true }
//! }
//!
//! # fn main() {
//! let rt = tokio::runtime::Runtime::new().unwrap();
//! rt.block_on(async {
//!     let mut agent = Agent::new(AgentConfig::default());
//!     let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend);
//!     let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
//!
//!     // Consume events concurrently while the agent generates.
//!     let consumer = tokio::spawn(async move {
//!         let mut reply = String::new();
//!         while let Some(event) = rx.recv().await {
//!             if let AgentEvent::MessageDelta { delta, .. } = event {
//!                 reply.push_str(&delta);
//!             }
//!         }
//!         reply
//!     });
//!
//!     agent.prompt("Hello", backend, tx).await.unwrap();
//!     assert_eq!(consumer.await.unwrap(), "Hi!");
//! });
//! # }
//! ```

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

/// The [`Agent`] orchestrator and its configuration.
pub mod agent;
/// The [`LlmBackend`] trait and inference parameter/result types.
pub mod backend;
/// Ready-made backends (feature `http-backend`): the OpenAI-compatible HTTP client.
#[cfg(feature = "http-backend")]
pub mod backends;
/// Context-window management: pruning, token budgeting, and prompt formatting.
pub mod context;
/// Error and result types for the crate.
pub mod error;
/// The [`AgentEvent`] stream emitted while the agent runs.
pub mod events;
/// Conversation data types: [`Message`], [`Role`], and tool call/result records.
pub mod messages;
/// Chat prompt templates for the supported model families.
pub mod template;
/// The `Tool` trait (feature `tools`), tool schemas, and tool-call parsing.
pub mod tools;

pub use agent::{Agent, AgentConfig};
#[cfg(feature = "tools")]
pub use agent::{ApprovalDecision, ApprovalHook};
pub use backend::{GenerationResult, InferenceParams, LlmBackend, TokenCallback};
#[cfg(feature = "http-backend")]
pub use backends::{OpenAiConfig, OpenAiEndpoint, OpenAiHttpBackend};
pub use context::{plan_prune, ContextConfig, PreparedContext, PrunePlan, PruneStrategy};
pub use error::{CoreError, CoreResult};
pub use events::AgentEvent;
pub use messages::{Message, Role, ToolCall, ToolResult};
pub use template::{
    detect_template, template_from_name, AlpacaTemplate, ChatMLTemplate, ChatTemplate,
    CommandRTemplate, DeepSeekTemplate, GemmaTemplate, Llama2Template, Llama3Template,
    MistralTemplate, Phi3Template, VicunaTemplate,
};
pub use tools::{parse_tool_calls, ParsedToolCall, ToolSchema};
#[cfg(feature = "tools")]
pub use tools::{Tool, ToolOutput, ToolUpdateCallback};
