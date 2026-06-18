use orion_core::context::{prepare_context, ContextConfig, PruneStrategy};
use orion_core::messages::Message;
use orion_core::template::{ChatMLTemplate, ChatTemplate};
use orion_core::tools::ToolSchema;

/// Simple token counter: 1 char = 1 token. Predictable for tests.
fn char_counter(text: &str) -> u32 {
    text.len() as u32
}

fn make_config(max_ctx: u32, max_resp: u32) -> ContextConfig {
    ContextConfig {
        max_context_tokens: max_ctx,
        max_response_tokens: max_resp,
        prune_strategy: PruneStrategy::SlidingWindow,
    }
}

fn make_tool() -> ToolSchema {
    ToolSchema {
        name: "search".into(),
        description: "Search the web".into(),
        parameters: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
    }
}

// --- Pair-wise pruning ---

#[test]
fn pair_wise_pruning_keeps_pairs_together() {
    let template = ChatMLTemplate;
    let messages = vec![
        Message::user("1", "old question"),
        Message::assistant("2", "old answer"),
        Message::user("3", "mid question"),
        Message::assistant("4", "mid answer"),
        Message::user("5", "new question"),
    ];

    // Budget tight enough to force pruning of oldest pair but keep mid + new
    let system_block = template.format_system("sys", &[]);
    let prefix = template.assistant_prefix();
    let overhead = char_counter(&system_block) + char_counter(prefix);

    // Calculate cost of mid pair + new question
    let mid_user = char_counter(&template.format_message(&messages[2]));
    let mid_asst = char_counter(&template.format_message(&messages[3]));
    let new_user = char_counter(&template.format_message(&messages[4]));
    let needed = overhead + mid_user + mid_asst + new_user;

    // Old pair cost
    let old_user = char_counter(&template.format_message(&messages[0]));
    let old_asst = char_counter(&template.format_message(&messages[1]));

    // Budget fits mid+new but not old pair
    let config = make_config(needed + 5, 0);
    assert!(
        needed + 5 < needed + old_user + old_asst,
        "test setup: budget should exclude old pair"
    );

    let result = prepare_context(&template, "sys", &messages, &[], &config, &char_counter).unwrap();

    assert_eq!(
        result.messages_pruned, 2,
        "old user+assistant pair should be pruned together"
    );
    assert_eq!(result.messages_included, 3, "mid pair + new question kept");
    assert!(result.prompt.contains("mid question"));
    assert!(result.prompt.contains("mid answer"));
    assert!(result.prompt.contains("new question"));
    assert!(!result.prompt.contains("old question"));
    assert!(!result.prompt.contains("old answer"));
}

#[test]
fn no_orphaned_assistant_after_pruning() {
    let template = ChatMLTemplate;
    let messages = vec![
        Message::user("1", "first"),
        Message::assistant("2", "reply to first"),
        Message::user("3", "second"),
        Message::assistant("4", "reply to second"),
        Message::user("5", "third"),
    ];

    // Very tight budget: only fits the latest user message
    let system_block = template.format_system("hi", &[]);
    let prefix = template.assistant_prefix();
    let overhead = char_counter(&system_block) + char_counter(prefix);
    let last_msg_cost = char_counter(&template.format_message(&messages[4]));

    let config = make_config(overhead + last_msg_cost + 1, 0);

    let result = prepare_context(&template, "hi", &messages, &[], &config, &char_counter).unwrap();

    assert_eq!(
        result.messages_included, 1,
        "only the latest user message fits"
    );
    assert_eq!(result.messages_pruned, 4);
    assert!(result.prompt.contains("third"));
    // No orphaned assistant without its user message
    assert!(!result.prompt.contains("reply to first"));
    assert!(!result.prompt.contains("reply to second"));
}

// --- System prompt always present ---

#[test]
fn system_prompt_always_in_output() {
    let template = ChatMLTemplate;
    let messages = vec![Message::user("1", "hello")];
    let config = make_config(10000, 0);

    let result = prepare_context(
        &template,
        "You are helpful",
        &messages,
        &[],
        &config,
        &char_counter,
    )
    .unwrap();

    assert!(result.prompt.contains("You are helpful"));
    assert!(result.prompt.contains("<|im_start|>system"));
}

// --- Template overhead accounting ---

#[test]
fn template_overhead_counted_in_budget() {
    let template = ChatMLTemplate;
    let msg = Message::user("1", "test");

    // Raw content is 4 chars, but with template it's much larger
    let raw_cost = char_counter("test");
    let template_cost = char_counter(&template.format_message(&msg));
    assert!(
        template_cost > raw_cost,
        "template overhead should increase token count"
    );

    // If budget only accounts for raw content, this would fit but shouldn't
    let system_block = template.format_system("s", &[]);
    let prefix = template.assistant_prefix();
    let overhead = char_counter(&system_block) + char_counter(prefix);

    // Budget: fits raw content but not template-wrapped content
    let tight_budget = overhead + raw_cost + 1;
    let config = make_config(tight_budget, 0);

    let result = prepare_context(
        &template,
        "s",
        &messages_single(),
        &[],
        &config,
        &char_counter,
    );
    assert!(
        result.is_err(),
        "should fail because template overhead makes message too large"
    );
}

fn messages_single() -> Vec<Message> {
    vec![Message::user(
        "1",
        "test message that is fairly long to exceed the tight budget",
    )]
}

// --- System + tool budget ---

#[test]
fn tool_schemas_deducted_from_budget() {
    let template = ChatMLTemplate;
    let messages = vec![
        Message::user("1", "hello"),
        Message::assistant("2", "hi there"),
        Message::user("3", "bye"),
    ];
    let tool = make_tool();

    // Without tools: everything fits
    let config_generous = make_config(10000, 0);
    let without_tools = prepare_context(
        &template,
        "sys",
        &messages,
        &[],
        &config_generous,
        &char_counter,
    )
    .unwrap();

    // With tools: system block is larger
    let system_no_tools = char_counter(&template.format_system("sys", &[]));
    let system_with_tools =
        char_counter(&template.format_system("sys", std::slice::from_ref(&tool)));
    assert!(
        system_with_tools > system_no_tools,
        "tools increase system block size"
    );

    // Budget that fits without tools but is tight with tools
    let with_tools = prepare_context(
        &template,
        "sys",
        &messages,
        std::slice::from_ref(&tool),
        &config_generous,
        &char_counter,
    )
    .unwrap();
    assert!(
        with_tools.token_count > without_tools.token_count,
        "tools increase total prompt size"
    );

    // Very tight budget: fails with tools
    let tight = make_config(system_with_tools + 10, 0);
    let result = prepare_context(&template, "sys", &messages, &[tool], &tight, &char_counter);
    assert!(
        result.is_err() || result.unwrap().messages_pruned > 0,
        "tight budget should prune or fail with tools"
    );
}

// --- Context overflow errors ---

#[test]
fn overflow_when_system_prompt_exceeds_budget() {
    let template = ChatMLTemplate;
    let long_system = "a".repeat(5000);
    let messages = vec![Message::user("1", "hi")];
    let config = make_config(100, 0);

    let result = prepare_context(
        &template,
        &long_system,
        &messages,
        &[],
        &config,
        &char_counter,
    );
    assert!(result.is_err());

    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("System prompt and tools"),
        "error should mention system prompt: {err}"
    );
}

#[test]
fn overflow_when_latest_message_too_large() {
    let template = ChatMLTemplate;
    let huge_message = "x".repeat(5000);
    let messages = vec![Message::user("1", &huge_message)];

    // Budget fits system but not system + message
    let system_block = template.format_system("sys", &[]);
    let overhead = char_counter(&system_block) + char_counter(template.assistant_prefix());
    let config = make_config(overhead + 100, 0);

    let result = prepare_context(&template, "sys", &messages, &[], &config, &char_counter);
    assert!(result.is_err());

    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Latest message"),
        "error should mention latest message: {err}"
    );
}

// --- No pruning needed ---

#[test]
fn all_messages_fit_no_pruning() {
    let template = ChatMLTemplate;
    let messages = vec![
        Message::user("1", "hi"),
        Message::assistant("2", "hello"),
        Message::user("3", "how are you"),
        Message::assistant("4", "great"),
        Message::user("5", "bye"),
    ];
    let config = make_config(100_000, 0);

    let result = prepare_context(&template, "sys", &messages, &[], &config, &char_counter).unwrap();

    assert_eq!(result.messages_pruned, 0);
    assert_eq!(result.messages_included, 5);
    assert!(result.prompt.contains("hi"));
    assert!(result.prompt.contains("bye"));
}

// --- Empty messages ---

#[test]
fn empty_conversation() {
    let template = ChatMLTemplate;
    let config = make_config(10000, 0);

    let result = prepare_context(
        &template,
        "You are helpful",
        &[],
        &[],
        &config,
        &char_counter,
    )
    .unwrap();

    assert_eq!(result.messages_included, 0);
    assert_eq!(result.messages_pruned, 0);
    assert!(result.prompt.contains("You are helpful"));
    assert!(result.prompt.contains("<|im_start|>assistant"));
}

// --- ChatML formatting ---

#[test]
fn chatml_format_correct() {
    let template = ChatMLTemplate;
    let messages = vec![
        Message::user("1", "What is 2+2?"),
        Message::assistant("2", "4"),
        Message::user("3", "Thanks"),
    ];
    let config = make_config(100_000, 0);

    let result = prepare_context(
        &template,
        "You are a calculator",
        &messages,
        &[],
        &config,
        &char_counter,
    )
    .unwrap();

    let expected_parts = [
        "<|im_start|>system\nYou are a calculator<|im_end|>",
        "<|im_start|>user\nWhat is 2+2?<|im_end|>",
        "<|im_start|>assistant\n4<|im_end|>",
        "<|im_start|>user\nThanks<|im_end|>",
        "<|im_start|>assistant\n",
    ];
    for part in &expected_parts {
        assert!(
            result.prompt.contains(part),
            "prompt should contain: {part}"
        );
    }
}

#[test]
fn chatml_format_with_tools() {
    let template = ChatMLTemplate;
    let messages = vec![Message::user("1", "search for cats")];
    let tool = make_tool();
    let config = make_config(100_000, 0);

    let result =
        prepare_context(&template, "sys", &messages, &[tool], &config, &char_counter).unwrap();

    assert!(result.prompt.contains("### search"));
    assert!(result.prompt.contains("Search the web"));
    assert!(result.prompt.contains("tool_call"));
}

// --- max_response_tokens budget ---

#[test]
fn response_tokens_reserved_from_budget() {
    let template = ChatMLTemplate;
    let messages = vec![
        Message::user("1", "question one"),
        Message::assistant("2", "answer one"),
        Message::user("3", "question two"),
    ];

    // Without response reservation: all fits
    let config_no_reserve = make_config(10000, 0);
    let r1 = prepare_context(
        &template,
        "sys",
        &messages,
        &[],
        &config_no_reserve,
        &char_counter,
    )
    .unwrap();
    assert_eq!(r1.messages_pruned, 0);

    // With large response reservation: forces pruning
    let system_block = template.format_system("sys", &[]);
    let overhead = char_counter(&system_block) + char_counter(template.assistant_prefix());
    let last_cost = char_counter(&template.format_message(&messages[2]));
    // Total context fits everything, but response reservation leaves only enough for last message
    let total = overhead + last_cost + 50;
    let config_reserve = make_config(total + 5000, 5000);

    let r2 = prepare_context(
        &template,
        "sys",
        &messages,
        &[],
        &config_reserve,
        &char_counter,
    )
    .unwrap();
    assert!(
        r2.messages_pruned > 0,
        "response token reservation should force pruning"
    );
}

// --- detect_template ---

#[test]
fn detect_chatml_template() {
    let tmpl = orion_core::detect_template(Some("{% if messages[0] %}<|im_start|>system"));
    assert_eq!(tmpl.name(), "chatml");
}

#[test]
fn detect_unknown_falls_back_to_chatml() {
    let tmpl = orion_core::detect_template(Some("some unknown jinja template"));
    assert_eq!(tmpl.name(), "chatml");
}

#[test]
fn detect_none_falls_back_to_chatml() {
    let tmpl = orion_core::detect_template(None);
    assert_eq!(tmpl.name(), "chatml");
}

// --- Edge cases ---

#[test]
fn single_user_message() {
    let template = ChatMLTemplate;
    let messages = vec![Message::user("1", "hello world")];
    let config = make_config(10000, 0);

    let result = prepare_context(&template, "sys", &messages, &[], &config, &char_counter).unwrap();

    assert_eq!(result.messages_included, 1);
    assert_eq!(result.messages_pruned, 0);
    assert!(result.prompt.contains("hello world"));
}

#[test]
fn many_turns_progressive_pruning() {
    let template = ChatMLTemplate;
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(Message::user(format!("u{i}"), format!("question {i}")));
        if i < 19 {
            messages.push(Message::assistant(format!("a{i}"), format!("answer {i}")));
        }
    }

    // Very tight budget
    let system_block = template.format_system("s", &[]);
    let prefix = template.assistant_prefix();
    let overhead = char_counter(&system_block) + char_counter(prefix);

    // Enough for ~3 turns
    let per_turn: u32 = messages[0..2]
        .iter()
        .map(|m| char_counter(&template.format_message(m)))
        .sum();
    let last_msg_cost = char_counter(&template.format_message(messages.last().unwrap()));
    let budget = overhead + per_turn * 2 + last_msg_cost + 20;
    let config = make_config(budget, 0);

    let result = prepare_context(&template, "s", &messages, &[], &config, &char_counter).unwrap();

    assert!(result.messages_pruned > 0, "should prune some messages");
    assert!(
        result.messages_included > 1,
        "should keep more than just the last message"
    );
    // Latest question is always kept
    assert!(result.prompt.contains("question 19"));
}

// --- Pinned messages ---

#[test]
fn pinned_old_turn_survives_tight_budget() {
    let template = ChatMLTemplate;
    let messages = vec![
        Message::user("1", "remember my name is Ada").pinned(),
        Message::assistant("2", "Got it, Ada").pinned(),
        Message::user("3", "filler question one"),
        Message::assistant("4", "filler answer one"),
        Message::user("5", "latest question"),
    ];

    // Budget for system + the pinned first turn + the latest turn, but NOT the
    // middle filler turn — so without pinning the first turn would be dropped.
    let system_block = template.format_system("sys", &[]);
    let overhead = char_counter(&system_block) + char_counter(template.assistant_prefix());
    let pinned_cost: u32 = messages[0..2]
        .iter()
        .map(|m| char_counter(&template.format_message(m)))
        .sum();
    let last_cost = char_counter(&template.format_message(&messages[4]));
    let config = make_config(overhead + pinned_cost + last_cost + 5, 0);

    let result = prepare_context(&template, "sys", &messages, &[], &config, &char_counter).unwrap();

    // Pinned first turn + latest turn kept; middle filler turn pruned.
    assert!(result.prompt.contains("my name is Ada"));
    assert!(result.prompt.contains("Got it, Ada"));
    assert!(result.prompt.contains("latest question"));
    assert!(!result.prompt.contains("filler question one"));
    assert_eq!(result.messages_pruned, 2, "middle filler turn pruned");
    assert_eq!(result.messages_included, 3);
}

#[test]
fn pinned_messages_exceeding_budget_error() {
    let template = ChatMLTemplate;
    let huge = "p".repeat(2000);
    let messages = vec![
        Message::user("1", &huge).pinned(),
        Message::assistant("2", "ok"),
        Message::user("3", "latest"),
    ];

    let system_block = template.format_system("sys", &[]);
    let overhead = char_counter(&system_block) + char_counter(template.assistant_prefix());
    let last_cost = char_counter(&template.format_message(&messages[2]));
    // Fits system + latest, but not the giant pinned turn.
    let config = make_config(overhead + last_cost + 100, 0);

    let result = prepare_context(&template, "sys", &messages, &[], &config, &char_counter);
    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("Pinned messages"),
        "error should mention pinned messages"
    );
}

#[test]
fn pin_keeps_whole_turn_no_orphan() {
    let template = ChatMLTemplate;
    // Only the assistant message of the old turn is pinned; the whole turn
    // (user + assistant) must survive together.
    let messages = vec![
        Message::user("1", "old question"),
        Message::assistant("2", "pinned old answer").pinned(),
        Message::user("3", "filler"),
        Message::assistant("4", "filler reply"),
        Message::user("5", "latest"),
    ];

    let system_block = template.format_system("sys", &[]);
    let overhead = char_counter(&system_block) + char_counter(template.assistant_prefix());
    let pinned_turn: u32 = messages[0..2]
        .iter()
        .map(|m| char_counter(&template.format_message(m)))
        .sum();
    let last_cost = char_counter(&template.format_message(&messages[4]));
    let config = make_config(overhead + pinned_turn + last_cost + 5, 0);

    let result = prepare_context(&template, "sys", &messages, &[], &config, &char_counter).unwrap();

    // The pinned assistant's user message comes along (no orphan).
    assert!(result.prompt.contains("old question"));
    assert!(result.prompt.contains("pinned old answer"));
    assert!(!result.prompt.contains("filler"));
}
