use crate::error::{CoreError, CoreResult};
use crate::messages::{Message, Role};
use crate::template::ChatTemplate;
use crate::tools::ToolSchema;

/// Strategy for handling context overflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneStrategy {
    /// Drop oldest message pairs (keep system + most recent turns).
    SlidingWindow,
    /// Summarize the oldest turns into a single pinned summary message instead
    /// of dropping them outright. The summarization itself is performed by the
    /// agent (it needs the LLM backend); the context pipeline still prunes with
    /// a sliding window once the summary is in place.
    Summarize,
}

/// Configuration for context management.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Total context window size in tokens (prompt + reserved response).
    pub max_context_tokens: u32,
    /// Tokens reserved for the response; deducted from the prune budget.
    pub max_response_tokens: u32,
    /// How to handle a conversation that overflows the budget.
    pub prune_strategy: PruneStrategy,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 4096,
            max_response_tokens: 2048,
            prune_strategy: PruneStrategy::SlidingWindow,
        }
    }
}

/// Result of context preparation.
#[derive(Debug, Clone)]
pub struct PreparedContext {
    /// The fully formatted prompt string to feed the backend.
    pub prompt: String,
    /// Total token count of `prompt`.
    pub token_count: u32,
    /// Number of conversation messages kept in the prompt.
    pub messages_included: u32,
    /// Number of conversation messages dropped to fit the budget.
    pub messages_pruned: u32,
}

/// Which turns survive pruning and which are dropped, as index ranges into the
/// messages slice (in original order). Produced by [`plan_prune`]; the agent
/// uses `dropped` to decide what to summarize under [`PruneStrategy::Summarize`].
#[derive(Debug, Clone)]
pub struct PrunePlan {
    /// Turns that survive pruning, as index ranges into the messages slice.
    pub kept: Vec<std::ops::Range<usize>>,
    /// Turns that are dropped to fit the budget, as index ranges.
    pub dropped: Vec<std::ops::Range<usize>>,
}

/// Group conversation messages into turns for pair-wise pruning.
///
/// A turn starts with a User message and includes all subsequent non-User
/// messages (Assistant, ToolCall, ToolResult) until the next User message.
/// Returns index ranges into the messages slice.
fn group_into_turns(messages: &[Message]) -> Vec<std::ops::Range<usize>> {
    let mut turns = Vec::new();
    let mut turn_start: Option<usize> = None;

    for (i, msg) in messages.iter().enumerate() {
        if msg.role == Role::User {
            if let Some(start) = turn_start {
                turns.push(start..i);
            }
            turn_start = Some(i);
        }
    }
    if let Some(start) = turn_start {
        turns.push(start..messages.len());
    }

    turns
}

/// Plan which turns survive pruning to fit the token budget.
///
/// 1. Deducts system prompt + tools + assistant-prefix overhead from the budget
/// 2. Groups messages into turns (user + following non-user messages)
/// 3. Always keeps the most recent turn and every *pinned* turn
/// 4. Fills the remaining budget with the most-recent non-pinned turns backward
///
/// A turn is pinned if any of its messages is `pinned`. Returns
/// `CoreError::Context` if the system block, the latest turn, or the pinned
/// turns alone exceed the available budget.
pub fn plan_prune(
    template: &dyn ChatTemplate,
    system_prompt: &str,
    messages: &[Message],
    tools: &[ToolSchema],
    config: &ContextConfig,
    token_counter: &dyn Fn(&str) -> u32,
) -> CoreResult<PrunePlan> {
    let available = config
        .max_context_tokens
        .saturating_sub(config.max_response_tokens);

    // Fixed overhead: system block (system prompt + tools) + assistant prefix.
    let system_block = template.format_system(system_prompt, tools);
    let fixed_overhead = token_counter(&system_block) + token_counter(template.assistant_prefix());

    if fixed_overhead >= available {
        return Err(CoreError::Context(format!(
            "System prompt and tools ({fixed_overhead} tokens) exceed \
             available context budget ({available} tokens)"
        )));
    }
    let mut budget = available - fixed_overhead;

    let turns = group_into_turns(messages);
    if turns.is_empty() {
        return Ok(PrunePlan {
            kept: vec![],
            dropped: vec![],
        });
    }

    let turn_costs: Vec<u32> = turns
        .iter()
        .map(|range| {
            messages[range.clone()]
                .iter()
                .map(|msg| token_counter(&template.format_message(msg)))
                .sum()
        })
        .collect();
    let turn_pinned: Vec<bool> = turns
        .iter()
        .map(|range| messages[range.clone()].iter().any(|m| m.pinned))
        .collect();

    let last = turns.len() - 1;
    let mut keep = vec![false; turns.len()];

    // The latest turn must fit - otherwise context overflow.
    if turn_costs[last] > budget {
        return Err(CoreError::Context(format!(
            "Latest message ({} tokens) plus system prompt \
             ({fixed_overhead} tokens) exceeds context budget ({available} tokens). \
             Clear the conversation or increase context size.",
            turn_costs[last]
        )));
    }
    budget -= turn_costs[last];
    keep[last] = true;

    // Pinned turns always survive, regardless of recency.
    for i in 0..last {
        if turn_pinned[i] {
            if turn_costs[i] > budget {
                let pinned_total: u32 = (0..turns.len())
                    .filter(|&j| turn_pinned[j])
                    .map(|j| turn_costs[j])
                    .sum();
                return Err(CoreError::Context(format!(
                    "Pinned messages ({pinned_total} tokens) exceed the available \
                     context budget ({available} tokens). Unpin some messages or \
                     increase context size."
                )));
            }
            budget -= turn_costs[i];
            keep[i] = true;
        }
    }

    // Fill the remaining budget with the most-recent non-pinned turns, walking
    // backward. Stop at the first non-pinned turn that doesn't fit (sliding
    // window); already-pinned turns are skipped without stopping the walk.
    for i in (0..last).rev() {
        if keep[i] {
            continue;
        }
        if turn_costs[i] <= budget {
            budget -= turn_costs[i];
            keep[i] = true;
        } else {
            break;
        }
    }

    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for (i, range) in turns.iter().enumerate() {
        if keep[i] {
            kept.push(range.clone());
        } else {
            dropped.push(range.clone());
        }
    }
    Ok(PrunePlan { kept, dropped })
}

/// Prepare context: prune to fit the budget, apply the template, return the
/// formatted prompt. Thin wrapper over [`plan_prune`] that formats the kept
/// turns. Pinned messages always survive (see `plan_prune`).
///
/// The agent calls this automatically before each LLM call; call it directly
/// only when you want custom control.
///
/// ```
/// use orion_core::{ChatMLTemplate, Message};
/// use orion_core::context::{prepare_context, ContextConfig};
///
/// // A real backend tokenizes; here we approximate with a word count.
/// let token_counter = |text: &str| -> u32 { text.split_whitespace().count() as u32 };
/// let messages = vec![
///     Message::user("1", "Hello"),
///     Message::assistant("2", "Hi there!"),
/// ];
///
/// let prepared = prepare_context(
///     &ChatMLTemplate,           // any `ChatTemplate` impl
///     "You are helpful.",        // system prompt
///     &messages,                 // full conversation history
///     &[],                       // tool schemas to inject (may be empty)
///     &ContextConfig::default(),
///     &token_counter,
/// )?;
///
/// assert!(prepared.prompt.contains("Hi there!"));
/// assert_eq!(prepared.messages_included, 2);
/// assert_eq!(prepared.messages_pruned, 0);
/// # Ok::<(), orion_core::CoreError>(())
/// ```
pub fn prepare_context(
    template: &dyn ChatTemplate,
    system_prompt: &str,
    messages: &[Message],
    tools: &[ToolSchema],
    config: &ContextConfig,
    token_counter: &dyn Fn(&str) -> u32,
) -> CoreResult<PreparedContext> {
    let plan = plan_prune(
        template,
        system_prompt,
        messages,
        tools,
        config,
        token_counter,
    )?;

    // Collect kept messages in original order (kept ranges may be non-contiguous
    // when an old pinned turn survives alongside the recent window).
    let kept: Vec<Message> = plan
        .kept
        .iter()
        .flat_map(|range| messages[range.clone()].iter().cloned())
        .collect();
    let kept_count = kept.len() as u32;
    let pruned = messages.len() as u32 - kept_count;

    let prompt = template.format(system_prompt, &kept, tools);
    let token_count = token_counter(&prompt);

    Ok(PreparedContext {
        prompt,
        token_count,
        messages_included: kept_count,
        messages_pruned: pruned,
    })
}
