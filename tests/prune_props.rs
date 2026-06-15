//! Property tests for the context-pruning invariants:
//! - turns are kept or dropped whole (a user/assistant pair is never orphaned),
//! - the latest turn always survives,
//! - pinned turns always survive,
//! - the prepared prompt never exceeds the context window.

use orion_core::context::{plan_prune, prepare_context, ContextConfig, PruneStrategy};
use orion_core::messages::{Message, Role};
use orion_core::template::ChatMLTemplate;
use proptest::prelude::*;

const SYSTEM: &str = "You are a helpful assistant.";

/// Byte-length token counter. It is additive under concatenation
/// (`len(a + b) == len(a) + len(b)`), so the budget arithmetic is exact and the
/// "never exceed budget" invariant can be checked precisely.
fn byte_counter(text: &str) -> u32 {
    text.len() as u32
}

/// One conversation turn: user text, optional assistant reply, and whether the
/// turn is pinned (the pin is applied to the user message).
type Turn = (String, Option<String>, bool);

fn turn_strategy() -> impl Strategy<Value = Turn> {
    (
        "[a-z ]{0,30}",
        proptest::option::of("[a-z ]{0,30}"),
        any::<bool>(),
    )
}

fn convo_strategy() -> impl Strategy<Value = Vec<Turn>> {
    prop::collection::vec(turn_strategy(), 1..=12)
}

/// `(available_budget, max_response_tokens)` → a config whose
/// `max_context_tokens - max_response_tokens == available_budget`.
fn config_strategy() -> impl Strategy<Value = ContextConfig> {
    (100u32..2000u32, 0u32..200u32).prop_map(|(available, resp)| ContextConfig {
        max_context_tokens: available + resp,
        max_response_tokens: resp,
        prune_strategy: PruneStrategy::SlidingWindow,
    })
}

fn build(turns: &[Turn]) -> Vec<Message> {
    let mut msgs = Vec::new();
    let mut id = 0u32;
    for (user, assistant, pinned) in turns {
        id += 1;
        let mut u = Message::user(format!("msg-{id}"), user.clone());
        if *pinned {
            u = u.pinned();
        }
        msgs.push(u);
        if let Some(a) = assistant {
            id += 1;
            msgs.push(Message::assistant(format!("msg-{id}"), a.clone()));
        }
    }
    msgs
}

proptest! {
    #[test]
    fn pruning_invariants(turns in convo_strategy(), config in config_strategy()) {
        let messages = build(&turns);
        let template = ChatMLTemplate;

        // Overflow (latest turn / pinned turns / system block can't fit) is a
        // valid outcome; the invariants only constrain the successful case.
        let Ok(plan) = plan_prune(&template, SYSTEM, &messages, &[], &config, &byte_counter) else {
            return Ok(());
        };

        // 1. Partition: kept ∪ dropped covers every message index exactly once.
        let mut covered: Vec<usize> = plan
            .kept
            .iter()
            .chain(plan.dropped.iter())
            .flat_map(|r| r.clone())
            .collect();
        covered.sort_unstable();
        prop_assert_eq!(covered, (0..messages.len()).collect::<Vec<_>>());

        // 2. Turn integrity: each range starts with a User message and contains
        //    no other User — i.e. it is a whole turn, never a split pair.
        for range in plan.kept.iter().chain(plan.dropped.iter()) {
            prop_assert_eq!(&messages[range.start].role, &Role::User);
            for msg in &messages[range.start + 1..range.end] {
                prop_assert_ne!(&msg.role, &Role::User);
            }
        }

        // 3. The latest turn always survives (some kept range ends at the tail).
        prop_assert!(
            plan.kept.iter().any(|r| r.end == messages.len()),
            "latest turn must survive"
        );

        // 4. Pinned turns are never dropped.
        for range in &plan.dropped {
            prop_assert!(
                !messages[range.clone()].iter().any(|m| m.pinned),
                "a pinned turn was dropped"
            );
        }

        // 5. Never exceed budget: the prepared prompt fits the context window.
        let prepared =
            prepare_context(&template, SYSTEM, &messages, &[], &config, &byte_counter)
                .expect("a valid plan implies a valid prepared context");
        prop_assert!(
            prepared.token_count <= config.max_context_tokens,
            "prompt of {} tokens exceeds context window of {}",
            prepared.token_count,
            config.max_context_tokens
        );
    }
}
