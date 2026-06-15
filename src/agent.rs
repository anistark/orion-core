use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use log;
use tokio::sync::mpsc;

use crate::backend::{GenerationResult, InferenceParams, LlmBackend};
use crate::context::{plan_prune, prepare_context, ContextConfig, PruneStrategy};
use crate::error::{CoreError, CoreResult};
use crate::events::AgentEvent;
use crate::messages::{Message, Role, ToolCall};
use crate::template::{ChatMLTemplate, ChatTemplate};
use crate::tools::{parse_tool_calls, ToolSchema};
#[cfg(feature = "tools")]
use crate::{
    messages::ToolResult,
    tools::{Tool, ToolOutput, ToolUpdateCallback},
};

/// Prefix marking an agent-generated conversation summary (see the `Summarize`
/// prune strategy). Used to recognise and consolidate prior summaries.
const SUMMARY_MARKER: &str = "[Summary of earlier conversation]";

/// Token budget for a summarization pass.
const SUMMARY_MAX_TOKENS: u32 = 320;

/// Agent configuration.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// System prompt prepended to every formatted conversation.
    pub system_prompt: String,
    /// Sampling / inference parameters passed to the backend.
    pub inference_params: InferenceParams,
    /// Context-window management settings.
    pub context_config: ContextConfig,
    /// Maximum LLM↔tool round-trips in a single `prompt()` before the agent
    /// stops and emits a warning. Guards against tool loops that never produce
    /// a final answer.
    pub max_tool_iterations: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: "You are a helpful assistant.".to_string(),
            inference_params: InferenceParams::default(),
            context_config: ContextConfig::default(),
            max_tool_iterations: 8,
        }
    }
}

/// The agent: manages conversation state, context pipeline, and the
/// prompt → LLM → tool → LLM loop.
///
/// ```
/// use orion_core::{Agent, AgentConfig, ContextConfig, InferenceParams};
///
/// let mut agent = Agent::new(AgentConfig {
///     system_prompt: "You are a coding assistant.".into(),
///     inference_params: InferenceParams {
///         max_tokens: 4096,
///         temperature: 0.4,
///         context_size: 8192,
///         n_threads: 6,
///     },
///     context_config: ContextConfig {
///         max_context_tokens: 8192,
///         max_response_tokens: 4096,
///         ..Default::default()
///     },
///     ..Default::default()
/// });
///
/// // Change settings on the fly.
/// agent.set_system_prompt("You are a pirate.");
/// agent.set_inference_params(InferenceParams { temperature: 1.2, ..Default::default() });
/// agent.clear();
/// ```
pub struct Agent {
    config: AgentConfig,
    messages: Vec<Message>,
    #[cfg(feature = "tools")]
    tools: Vec<Box<dyn Tool>>,
    template: Arc<dyn ChatTemplate>,
    abort: Arc<AtomicBool>,
    msg_counter: u64,
}

impl Agent {
    /// Create an agent with the given config and the default ChatML template.
    pub fn new(config: AgentConfig) -> Self {
        Self::with_template(config, Arc::new(ChatMLTemplate))
    }

    /// Create an agent with an explicit chat template.
    pub fn with_template(config: AgentConfig, template: Arc<dyn ChatTemplate>) -> Self {
        log::debug!(
            "Agent created: system_prompt_len={}, max_ctx={}, max_resp={}, template={}",
            config.system_prompt.len(),
            config.context_config.max_context_tokens,
            config.context_config.max_response_tokens,
            template.name(),
        );
        Self {
            config,
            messages: Vec::new(),
            #[cfg(feature = "tools")]
            tools: Vec::new(),
            template,
            abort: Arc::new(AtomicBool::new(false)),
            msg_counter: 0,
        }
    }

    /// The current conversation messages, in order.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The agent's current configuration.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// The chat template currently in use.
    pub fn template(&self) -> &dyn ChatTemplate {
        self.template.as_ref()
    }

    /// Replace the system prompt used for subsequent prompts.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        let prompt = prompt.into();
        log::debug!("Agent system prompt updated: len={}", prompt.len());
        self.config.system_prompt = prompt;
    }

    /// Replace the inference parameters used for subsequent generations.
    pub fn set_inference_params(&mut self, params: InferenceParams) {
        log::debug!(
            "Agent inference params: max_tokens={}, temp={}, ctx={}, threads={}",
            params.max_tokens,
            params.temperature,
            params.context_size,
            params.n_threads,
        );
        self.config.inference_params = params;
    }

    /// Replace the context-management configuration.
    pub fn set_context_config(&mut self, config: ContextConfig) {
        log::debug!(
            "Agent context config: max_ctx={}, max_resp={}",
            config.max_context_tokens,
            config.max_response_tokens,
        );
        self.config.context_config = config;
    }

    /// Select the strategy used when the conversation overflows the budget.
    pub fn set_prune_strategy(&mut self, strategy: PruneStrategy) {
        log::debug!("Agent prune strategy: {strategy:?}");
        self.config.context_config.prune_strategy = strategy;
    }

    /// Pin or unpin a message by id. Pinned messages always survive context
    /// pruning. Returns whether a message with that id was found.
    pub fn set_pinned(&mut self, message_id: &str, pinned: bool) -> bool {
        match self.messages.iter_mut().find(|m| m.id == message_id) {
            Some(msg) => {
                msg.pinned = pinned;
                log::debug!("Agent message {message_id} pinned={pinned}");
                true
            }
            None => {
                log::warn!("set_pinned: message {message_id} not found");
                false
            }
        }
    }

    /// Swap the chat template at runtime (e.g. after detecting the model family).
    pub fn set_template(&mut self, template: Arc<dyn ChatTemplate>) {
        log::debug!("Agent template updated: {}", template.name());
        self.template = template;
    }

    /// Register the tools the agent may invoke during a prompt.
    ///
    /// Available only with the `tools` feature (enabled by default).
    #[cfg(feature = "tools")]
    pub fn set_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        log::debug!("Agent tools set: count={}", tools.len());
        self.tools = tools;
    }

    /// Clear the conversation history.
    pub fn clear(&mut self) {
        let count = self.messages.len();
        self.messages.clear();
        log::debug!("Agent conversation cleared: {count} messages removed");
    }

    /// Replace the entire conversation (e.g. when restoring a saved session).
    ///
    /// Advances the internal id counter past any restored `msg-N` ids so newly
    /// generated ids don't collide with restored ones.
    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        log::debug!("Agent messages replaced: count={}", messages.len());
        // Advance the id counter past any `msg-N` ids in the loaded set so
        // newly generated ids don't collide with restored ones (pins and tool
        // results are addressed by id).
        let max_loaded = messages
            .iter()
            .filter_map(|m| m.id.strip_prefix("msg-"))
            .filter_map(|n| n.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        self.msg_counter = self.msg_counter.max(max_loaded);
        self.messages = messages;
    }

    /// Request cancellation of an in-flight generation.
    pub fn abort(&self) {
        log::debug!("Agent abort requested");
        self.abort.store(true, Ordering::Relaxed);
    }

    /// Clone of the shared abort flag, for wiring cancellation into a backend.
    pub fn abort_flag(&self) -> Arc<AtomicBool> {
        self.abort.clone()
    }

    fn next_id(&mut self) -> String {
        self.msg_counter += 1;
        format!("msg-{}", self.msg_counter)
    }

    #[cfg(feature = "tools")]
    fn tool_schemas(&self) -> Vec<ToolSchema> {
        self.tools.iter().map(|t| t.schema()).collect()
    }

    /// Without the `tools` feature there are never any tools to advertise.
    #[cfg(not(feature = "tools"))]
    fn tool_schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }

    /// Run a prompt through the agent loop.
    ///
    /// Accepts an event sender so the caller can consume events
    /// concurrently while generation is in progress. This enables
    /// real-time token streaming to the UI.
    ///
    /// Flow:
    /// 1. Adds the user message.
    /// 2. Generates an assistant response (prune + template + LLM call),
    ///    streaming tokens via `tx`.
    /// 3. If tools are registered and the response contains tool calls, runs
    ///    each tool, appends a tool-result message, and loops back to the LLM.
    /// 4. Repeats until the model returns a tool-free answer or the
    ///    `max_tool_iterations` guard trips.
    /// 5. Emits lifecycle events for every step and updates conversation state.
    pub async fn prompt(
        &mut self,
        text: impl Into<String>,
        backend: Arc<dyn LlmBackend>,
        tx: mpsc::UnboundedSender<AgentEvent>,
    ) -> CoreResult<()> {
        let text = text.into().trim().to_string();
        if text.is_empty() {
            return Err(CoreError::Agent("Empty message".into()));
        }

        self.abort.store(false, Ordering::Relaxed);

        let user_msg = Message::user(self.next_id(), &text);
        self.messages.push(user_msg.clone());

        tx.send(AgentEvent::AgentStart).ok();
        tx.send(AgentEvent::MessageStart {
            message: user_msg.clone(),
        })
        .ok();
        tx.send(AgentEvent::MessageEnd { message: user_msg }).ok();

        // Under the Summarize strategy, fold overflowing older turns into a
        // pinned summary before generating (best-effort; no-op otherwise).
        self.compress_if_needed(&backend, &tx).await;

        // Messages produced this prompt (assistant turns + tool results),
        // reported in the final `AgentEnd`.
        let mut new_messages: Vec<Message> = Vec::new();
        #[cfg(feature = "tools")]
        let has_tools = !self.tools.is_empty();
        #[cfg(not(feature = "tools"))]
        let has_tools = false;

        for iteration in 0..self.config.max_tool_iterations {
            tx.send(AgentEvent::TurnStart).ok();

            let gen = match self.generate_once(backend.clone(), &tx).await {
                Ok(gen) => gen,
                Err(CoreError::Aborted) => {
                    // Aborted mid-generation: record an (empty) assistant turn
                    // so the conversation stays well-formed, then stop.
                    log::info!("Agent::prompt: generation aborted by user");
                    let assistant_msg = Message::assistant(self.next_id(), "");
                    self.messages.push(assistant_msg.clone());
                    new_messages.push(assistant_msg.clone());
                    tx.send(AgentEvent::MessageEnd {
                        message: assistant_msg.clone(),
                    })
                    .ok();
                    tx.send(AgentEvent::TurnEnd {
                        message: assistant_msg,
                        tool_results: vec![],
                    })
                    .ok();
                    tx.send(AgentEvent::AgentEnd {
                        messages: new_messages,
                    })
                    .ok();
                    return Ok(());
                }
                Err(e) => {
                    // Context overflow, backend not ready, generation failure.
                    log::error!("Agent::prompt: generation error: {e}");
                    // On the first turn, drop the just-added user message so a
                    // retry starts clean. On later turns the conversation
                    // already carries tool context; leave it for re-pruning.
                    if iteration == 0 {
                        self.messages.pop();
                    }
                    tx.send(AgentEvent::Error {
                        message: e.to_string(),
                    })
                    .ok();
                    tx.send(AgentEvent::AgentEnd { messages: vec![] }).ok();
                    return Ok(());
                }
            };

            log::debug!(
                "Agent::prompt: turn {} → {} tokens, {:.1} t/s, {:.1}ms ttft",
                iteration,
                gen.tokens_generated,
                gen.tokens_per_sec,
                gen.time_to_first_token_ms,
            );

            let mut assistant_msg = Message::assistant(self.next_id(), &gen.text);
            let parsed = if has_tools {
                parse_tool_calls(&gen.text)
            } else {
                Vec::new()
            };
            let tool_calls: Vec<ToolCall> = parsed
                .iter()
                .enumerate()
                .map(|(i, p)| ToolCall {
                    id: format!("{}-call-{}", assistant_msg.id, i + 1),
                    name: p.name.clone(),
                    arguments: p.arguments.clone(),
                })
                .collect();
            assistant_msg.tool_calls = tool_calls.clone();

            self.messages.push(assistant_msg.clone());
            new_messages.push(assistant_msg.clone());

            tx.send(AgentEvent::GenerationStats {
                tokens_generated: gen.tokens_generated,
                prompt_tokens: gen.prompt_tokens,
                tokens_per_sec: gen.tokens_per_sec,
                time_to_first_token_ms: gen.time_to_first_token_ms,
                generation_time_ms: gen.generation_time_ms,
            })
            .ok();
            tx.send(AgentEvent::MessageEnd {
                message: assistant_msg.clone(),
            })
            .ok();

            // No tool calls → this is the final answer.
            if tool_calls.is_empty() {
                tx.send(AgentEvent::TurnEnd {
                    message: assistant_msg,
                    tool_results: vec![],
                })
                .ok();
                tx.send(AgentEvent::AgentEnd {
                    messages: new_messages,
                })
                .ok();
                return Ok(());
            }

            // Tool calls are present — reachable only with the `tools` feature,
            // since without it `has_tools` is always false (so `tool_calls` is
            // always empty and we returned above).
            #[cfg(feature = "tools")]
            {
                let aborted = self
                    .run_tool_calls(&tool_calls, assistant_msg, &mut new_messages, &tx)
                    .await;
                if aborted {
                    tx.send(AgentEvent::AgentEnd {
                        messages: new_messages,
                    })
                    .ok();
                    return Ok(());
                }
                // Otherwise loop back: the LLM now sees the tool results.
            }
        }

        // Exhausted the iteration budget without a tool-free answer.
        log::warn!(
            "Agent::prompt: stopped after {} tool iterations",
            self.config.max_tool_iterations
        );
        tx.send(AgentEvent::Warning {
            message: format!(
                "Stopped after {} tool iterations without a final answer",
                self.config.max_tool_iterations
            ),
        })
        .ok();
        tx.send(AgentEvent::AgentEnd {
            messages: new_messages,
        })
        .ok();
        Ok(())
    }

    /// Execute every tool call from one assistant turn, appending a tool-result
    /// message per call and emitting the matching events, then emit `TurnEnd`.
    /// Returns `true` if an abort was requested while tools were running.
    #[cfg(feature = "tools")]
    async fn run_tool_calls(
        &mut self,
        tool_calls: &[ToolCall],
        assistant_msg: Message,
        new_messages: &mut Vec<Message>,
        tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> bool {
        let mut tool_results: Vec<ToolResult> = Vec::new();
        for call in tool_calls {
            tx.send(AgentEvent::ToolExecStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: call.arguments.clone(),
            })
            .ok();

            let (content, is_error) = match self.execute_tool(call, tx).await {
                Ok(out) => (out.content, false),
                Err(e) => {
                    log::warn!("Agent::prompt: tool '{}' failed: {e}", call.name);
                    (e.to_string(), true)
                }
            };

            let result = ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                content: content.clone(),
                is_error,
            };
            tx.send(AgentEvent::ToolExecEnd {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                result: result.clone(),
            })
            .ok();

            let result_msg =
                Message::tool_result(self.next_id(), &call.id, &call.name, content, is_error);
            self.messages.push(result_msg.clone());
            new_messages.push(result_msg.clone());
            tx.send(AgentEvent::MessageStart {
                message: result_msg.clone(),
            })
            .ok();
            tx.send(AgentEvent::MessageEnd {
                message: result_msg,
            })
            .ok();

            tool_results.push(result);
        }

        tx.send(AgentEvent::TurnEnd {
            message: assistant_msg,
            tool_results,
        })
        .ok();

        self.abort.load(Ordering::Relaxed)
    }

    /// Convenience wrapper over [`prompt`](Agent::prompt) that creates the event
    /// channel for you.
    ///
    /// Returns the event receiver plus a future that drives generation. Poll the
    /// future (e.g. with `tokio::join!`) while draining the receiver — the two
    /// run concurrently so tokens stream as they're produced:
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use std::sync::atomic::AtomicBool;
    /// # use orion_core::{Agent, AgentConfig, AgentEvent, CoreResult,
    /// #     GenerationResult, InferenceParams, LlmBackend, TokenCallback};
    /// # struct MockBackend;
    /// # impl LlmBackend for MockBackend {
    /// #     fn generate(&self, _p: &str, _x: &InferenceParams, _a: Arc<AtomicBool>,
    /// #         mut on_token: TokenCallback) -> CoreResult<GenerationResult> {
    /// #         on_token("Hi!", 1, 10.0);
    /// #         Ok(GenerationResult { text: "Hi!".into(), tokens_generated: 1,
    /// #             prompt_tokens: 0, tokens_per_sec: 10.0,
    /// #             time_to_first_token_ms: 1.0, generation_time_ms: 1.0 })
    /// #     }
    /// #     fn tokenize_count(&self, t: &str) -> CoreResult<u32> {
    /// #         Ok(t.split_whitespace().count() as u32) }
    /// #     fn is_ready(&self) -> bool { true }
    /// # }
    /// # fn main() {
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let mut agent = Agent::new(AgentConfig::default());
    /// let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend);
    ///
    /// let (mut rx, run) = agent.prompt_stream("Hello", backend);
    /// let (result, reply) = tokio::join!(run, async move {
    ///     let mut reply = String::new();
    ///     while let Some(event) = rx.recv().await {
    ///         if let AgentEvent::MessageDelta { delta, .. } = event {
    ///             reply.push_str(&delta);
    ///         }
    ///     }
    ///     reply
    /// });
    /// result.unwrap();
    /// assert_eq!(reply, "Hi!");
    /// # });
    /// # }
    /// ```
    pub fn prompt_stream(
        &mut self,
        text: impl Into<String>,
        backend: Arc<dyn LlmBackend>,
    ) -> (
        mpsc::UnboundedReceiver<AgentEvent>,
        impl std::future::Future<Output = CoreResult<()>> + '_,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let text = text.into();
        let fut = async move { self.prompt(text, backend, tx).await };
        (rx, fut)
    }

    /// Run a single LLM generation over the current conversation.
    ///
    /// Prepares context (prune + template) and calls the backend on a blocking
    /// thread, streaming `MessageDelta` tokens and emitting `ContextBudget`.
    /// Returns the completed [`GenerationResult`], or a `CoreError` (context
    /// overflow, backend-not-ready, `Aborted`, or a generation failure).
    async fn generate_once(
        &self,
        backend: Arc<dyn LlmBackend>,
        tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> CoreResult<GenerationResult> {
        let messages = self.messages.clone();
        let system_prompt = self.config.system_prompt.clone();
        let ctx_config = self.config.context_config.clone();
        let tool_schemas = self.tool_schemas();
        let params = self.config.inference_params.clone();
        let abort = self.abort.clone();
        let max_ctx = self.config.context_config.max_context_tokens;
        let template = self.template.clone();
        let token_tx = tx.clone();
        let budget_tx = tx.clone();

        log::debug!(
            "Agent::generate_once: spawning blocking (max_tokens={}, temp={}, ctx={}, threads={})",
            params.max_tokens,
            params.temperature,
            params.context_size,
            params.n_threads,
        );

        let handle = tokio::task::spawn_blocking(move || {
            if !backend.is_ready() {
                return Err(CoreError::Backend("No model loaded".into()));
            }

            let prepared = prepare_context(
                template.as_ref(),
                &system_prompt,
                &messages,
                &tool_schemas,
                &ctx_config,
                &|text| backend.tokenize_count(text).unwrap_or(0),
            )?;

            log::debug!(
                "Context prepared: tokens={}, kept={}, pruned={}",
                prepared.token_count,
                prepared.messages_included,
                prepared.messages_pruned,
            );

            budget_tx
                .send(AgentEvent::ContextBudget {
                    used_tokens: prepared.token_count,
                    max_tokens: max_ctx,
                    messages_in_context: prepared.messages_included,
                    messages_pruned: prepared.messages_pruned,
                })
                .ok();

            backend.generate(
                &prepared.prompt,
                &params,
                abort,
                Box::new(move |token, count, tps| {
                    token_tx
                        .send(AgentEvent::MessageDelta {
                            delta: token.to_string(),
                            tokens_generated: count,
                            tokens_per_sec: tps,
                        })
                        .ok();
                }),
            )
        });

        handle.await.map_err(|e| {
            log::error!("Agent::generate_once: blocking task panicked: {e}");
            CoreError::Agent(format!("Inference task failed: {e}"))
        })?
    }

    /// Dispatch one parsed tool call to its registered [`Tool`].
    ///
    /// Forwards the tool's streaming progress as `ToolExecUpdate` events.
    /// Returns `CoreError::Tool` when no tool matches the requested name.
    #[cfg(feature = "tools")]
    async fn execute_tool(
        &self,
        call: &ToolCall,
        tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> CoreResult<ToolOutput> {
        let Some(tool) = self.tools.iter().find(|t| t.name() == call.name) else {
            return Err(CoreError::Tool(format!("unknown tool: {}", call.name)));
        };

        let update_tx = tx.clone();
        let tool_call_id = call.id.clone();
        let tool_name = call.name.clone();
        let on_update: ToolUpdateCallback = Box::new(move |partial: &str| {
            update_tx
                .send(AgentEvent::ToolExecUpdate {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    partial: partial.to_string(),
                })
                .ok();
        });

        tool.execute(&call.id, call.arguments.clone(), Some(on_update))
            .await
    }

    /// Summarize-and-compress: when the conversation overflows under the
    /// `Summarize` strategy, replace the oldest droppable turns with a single
    /// pinned summary message so their gist survives instead of being dropped.
    ///
    /// Best-effort: any failure (backend not ready, summarizer error, abort)
    /// logs and returns, leaving the conversation untouched — the normal
    /// sliding-window pruning in `prepare_context` then applies.
    async fn compress_if_needed(
        &mut self,
        backend: &Arc<dyn LlmBackend>,
        tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        if self.config.context_config.prune_strategy != PruneStrategy::Summarize {
            return;
        }

        let messages = self.messages.clone();
        let system_prompt = self.config.system_prompt.clone();
        let tools = self.tool_schemas();
        let ctx_config = self.config.context_config.clone();
        let template = self.template.clone();
        let abort = self.abort.clone();
        let params = self.config.inference_params.clone();
        let backend = backend.clone();

        // Plan + summarize on a blocking thread (tokenize + generate block).
        let outcome = tokio::task::spawn_blocking(move || -> Option<(Vec<usize>, String)> {
            if !backend.is_ready() {
                return None;
            }
            let counter = |t: &str| backend.tokenize_count(t).unwrap_or(0);
            let plan = plan_prune(
                template.as_ref(),
                &system_prompt,
                &messages,
                &tools,
                &ctx_config,
                &counter,
            )
            .ok()?;
            if plan.dropped.is_empty() {
                return None; // everything fits — nothing to summarize
            }

            // Indices to fold away: the dropped turns plus any prior summary
            // (pinned, so it never lands in `dropped`) — consolidated into one.
            let mut remove: Vec<usize> = plan.dropped.iter().flat_map(|r| r.clone()).collect();
            let prior_summary = messages
                .iter()
                .position(|m| m.pinned && m.content.starts_with(SUMMARY_MARKER));
            let prior_body = prior_summary.map(|i| {
                remove.push(i);
                messages[i]
                    .content
                    .strip_prefix(SUMMARY_MARKER)
                    .unwrap_or(&messages[i].content)
                    .trim()
                    .to_string()
            });
            remove.sort_unstable();
            remove.dedup();

            let transcript = render_transcript(&messages, &remove);
            let mut body = String::new();
            if let Some(prev) = prior_body.filter(|s| !s.is_empty()) {
                body.push_str("Earlier summary:\n");
                body.push_str(&prev);
                body.push_str("\n\n");
            }
            body.push_str("Conversation excerpt:\n");
            body.push_str(&transcript);

            let instruction = "You compress conversation history. Summarize the \
                 material below into a concise note that preserves key facts, \
                 decisions, names, and unresolved questions. Reply with only the \
                 summary.";
            let req = Message::user("summary-req", format!("{instruction}\n\n{body}"));
            let prompt = template.format(
                "You summarize conversations faithfully and concisely.",
                std::slice::from_ref(&req),
                &[],
            );

            let sum_params = InferenceParams {
                max_tokens: SUMMARY_MAX_TOKENS,
                ..params
            };
            let gen = backend
                .generate(&prompt, &sum_params, abort, Box::new(|_, _, _| {}))
                .ok()?;
            let summary = gen.text.trim().to_string();
            if summary.is_empty() {
                return None;
            }
            Some((remove, summary))
        })
        .await;

        let Some((remove, summary)) = outcome.ok().flatten() else {
            return;
        };

        self.fold_into_summary(&remove, summary);
        tx.send(AgentEvent::Warning {
            message: format!(
                "Summarized {} earlier message(s) to fit the context window",
                remove.len()
            ),
        })
        .ok();
    }

    /// Remove the given message indices and splice a single pinned summary
    /// message in at the earliest removed position.
    fn fold_into_summary(&mut self, remove: &[usize], summary: String) {
        if remove.is_empty() {
            return;
        }
        let insert_at = *remove.iter().min().unwrap();
        let mut sorted = remove.to_vec();
        sorted.sort_unstable();
        for &i in sorted.iter().rev() {
            if i < self.messages.len() {
                self.messages.remove(i);
            }
        }
        let summary_msg =
            Message::user(self.next_id(), format!("{SUMMARY_MARKER}\n{summary}")).pinned();
        let at = insert_at.min(self.messages.len());
        self.messages.insert(at, summary_msg);
        log::info!(
            "Folded {} messages into a pinned summary at index {at}",
            remove.len()
        );
    }
}

/// Render selected messages as a plain-text transcript for summarization.
fn render_transcript(messages: &[Message], indices: &[usize]) -> String {
    indices
        .iter()
        .filter_map(|&i| messages.get(i))
        .map(|m| {
            let role = match m.role {
                Role::User => "User",
                Role::Assistant | Role::ToolCall => "Assistant",
                Role::ToolResult => "Tool",
                Role::System => "System",
            };
            format!("{role}: {}", m.content)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
