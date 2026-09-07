//! Tests for the message-native backend path.
#![cfg(feature = "chat-backend")]

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use orion_core::{
    Agent, AgentConfig, ChatBackend, ContextConfig, CoreResult, GenerationResult, InferenceParams,
    Message, PruneStrategy, Role, TokenCallback,
};

/// A backend that answers with a fixed reply and keeps what it was handed, so a test can
/// assert on the shape of the conversation rather than on the text that came back.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<(String, Vec<Message>)>>,
}

#[async_trait]
impl ChatBackend for Recorder {
    async fn chat(
        &self,
        system: &str,
        messages: &[Message],
        _params: &InferenceParams,
        _abort: Arc<AtomicBool>,
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        self.seen
            .lock()
            .unwrap()
            .push((system.to_string(), messages.to_vec()));

        on_token("Atlantis.", 1, 10.0);
        Ok(GenerationResult {
            text: "Atlantis.".into(),
            tokens_generated: 1,
            prompt_tokens: 0,
            tokens_per_sec: 10.0,
            time_to_first_token_ms: 1.0,
            generation_time_ms: 1.0,
        })
    }
}

fn agent() -> Agent {
    Agent::new(AgentConfig {
        system_prompt: "You are Odin, a watcher.".into(),
        ..Default::default()
    })
}

#[tokio::test]
async fn a_chat_backend_is_handed_messages_and_never_a_formatted_prompt() {
    let backend = Arc::new(Recorder::default());
    let mut agent = agent();

    let reply = agent
        .send("What is here?", backend.clone() as Arc<dyn ChatBackend>)
        .await
        .unwrap();

    assert_eq!(reply.content, "Atlantis.");

    let seen = backend.seen.lock().unwrap();
    let (system, messages) = &seen[0];

    assert_eq!(system, "You are Odin, a watcher.");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages[0].content, "What is here?");

    // The whole point of the path: nothing carries template markup, and the conversation
    // has not been collapsed into one turn.
    for message in messages {
        assert!(
            !message.content.contains("<|im_start|>"),
            "a chat backend must not receive a templated prompt",
        );
    }
    assert!(!system.contains("<|im_start|>"));
}

#[tokio::test]
async fn a_thread_arrives_as_separate_turns_in_order() {
    let backend = Arc::new(Recorder::default());
    let mut agent = agent();

    agent.replace_messages(vec![
        Message::user("1", "Where is it?"),
        Message::assistant("2", "Under the sea."),
    ]);
    agent
        .send(
            "And who lives there?",
            backend.clone() as Arc<dyn ChatBackend>,
        )
        .await
        .unwrap();

    let seen = backend.seen.lock().unwrap();
    let (_system, messages) = &seen[0];

    let turns: Vec<(&Role, &str)> = messages
        .iter()
        .map(|m| (&m.role, m.content.as_str()))
        .collect();
    assert_eq!(
        turns,
        vec![
            (&Role::User, "Where is it?"),
            (&Role::Assistant, "Under the sea."),
            (&Role::User, "And who lives there?"),
        ],
    );
}

#[tokio::test]
async fn the_reply_lands_in_the_conversation() {
    let backend = Arc::new(Recorder::default());
    let mut agent = agent();

    agent
        .send("What is here?", backend as Arc<dyn ChatBackend>)
        .await
        .unwrap();

    let kept: Vec<&str> = agent
        .messages()
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(kept, vec!["What is here?", "Atlantis."]);
}

/// The summarize strategy folds the oldest turns into a pinned note rather than dropping
/// them. It is planned the same way for either kind of backend and run differently, and
/// only the prompt path had a test saying so.
#[tokio::test]
async fn an_overflowing_conversation_is_summarized_through_a_chat_backend() {
    let backend = Arc::new(Recorder::default());
    let mut agent = Agent::new(AgentConfig {
        system_prompt: "You are Odin.".into(),
        context_config: ContextConfig {
            max_context_tokens: 200,
            max_response_tokens: 100,
            prune_strategy: PruneStrategy::Summarize,
        },
        ..Default::default()
    });

    // Six turns of about ten tokens each against a budget of roughly ninety, so the oldest
    // cannot fit and the newest comfortably can.
    let said = "a".repeat(40);
    agent.replace_messages(
        (0..6)
            .flat_map(|n| {
                [
                    Message::user(format!("u{n}"), &said),
                    Message::assistant(format!("a{n}"), &said),
                ]
            })
            .collect(),
    );

    agent
        .send("And now?", backend.clone() as Arc<dyn ChatBackend>)
        .await
        .unwrap();

    let seen = backend.seen.lock().unwrap();
    assert!(
        seen.len() >= 2,
        "summarizing is a turn of its own, taken before the one that was asked for",
    );

    let (system, asked) = &seen[0];
    assert!(
        system.contains("summarize"),
        "the summarizer frames itself: {system:?}"
    );
    assert!(
        asked[0].content.contains("Conversation excerpt:"),
        "the pass is handed the turns it is folding away",
    );

    // What it answered with is kept as a pinned note, which is what survives the next prune.
    let folded = agent
        .messages()
        .iter()
        .find(|message| message.pinned)
        .expect("a summary should have been folded in");
    assert!(folded
        .content
        .starts_with("[Summary of earlier conversation]"));
    assert!(folded.content.contains("Atlantis."));
}
