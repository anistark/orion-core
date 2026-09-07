//! The message-native half of the OpenAI-compatible backend.
//!
//! `http_backend_tests` covers the prompt path, which is synchronous and cannot run inside
//! a Tokio runtime. This one is async, and its subject is the thing the prompt path cannot
//! do: send a conversation as a conversation.
#![cfg(all(feature = "http-backend", feature = "chat-backend"))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;

use orion_core::backends::{OpenAiConfig, OpenAiHttpBackend};
use orion_core::{ChatBackend, InferenceParams, Message, Role};

/// A one-shot server that answers with a streamed reply and hands back the request body it
/// was sent, which is the half worth asserting on.
fn stand_in(sse: String) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");

    let handle = thread::spawn(move || -> String {
        let mut sent = String::new();
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 8192];
            let read = stream.read(&mut buf).unwrap_or(0);
            sent = String::from_utf8_lossy(&buf[..read]).into_owned();

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{sse}",
                sse.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
        sent
    });

    (format!("http://{addr}/v1"), handle)
}

/// Two content deltas, a usage block and the sentinel, which is what a real one sends.
fn streamed(head: &str, tail: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{head}\"}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{\"content\":\"{tail}\"}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}}}}],\"usage\":{{\"prompt_tokens\":11,\"completion_tokens\":2}}}}\n\n\
         data: [DONE]\n\n"
    )
}

fn backend(base_url: String) -> OpenAiHttpBackend {
    OpenAiHttpBackend::new(OpenAiConfig::new(base_url, "test-model")).expect("build backend")
}

/// The body of an HTTP request, which is whatever follows the blank line.
fn body(sent: &str) -> serde_json::Value {
    let at = sent.find("\r\n\r\n").expect("headers end");
    serde_json::from_str(&sent[at + 4..]).expect("a JSON body")
}

#[tokio::test]
async fn a_conversation_is_sent_as_turns_and_not_as_one_user_message() {
    let (url, server) = stand_in(streamed("Atl", "antis."));

    let answer = backend(url)
        .chat(
            "You are Odin.",
            &[
                Message::user("1", "Where is it?"),
                Message::assistant("2", "Under the sea."),
                Message::user("3", "And who lives there?"),
            ],
            &InferenceParams::default(),
            Arc::new(AtomicBool::new(false)),
            Box::new(|_, _, _| {}),
        )
        .await
        .expect("the turn");

    assert_eq!(answer.text, "Atlantis.");
    // The server's own counts win over what was streamed.
    assert_eq!(answer.tokens_generated, 2);
    assert_eq!(answer.prompt_tokens, 11);

    let sent = body(&server.join().unwrap());
    let turns: Vec<(&str, &str)> = sent["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|turn| {
            (
                turn["role"].as_str().unwrap(),
                turn["content"].as_str().unwrap(),
            )
        })
        .collect();

    assert_eq!(
        turns,
        vec![
            ("system", "You are Odin."),
            ("user", "Where is it?"),
            ("assistant", "Under the sea."),
            ("user", "And who lives there?"),
        ],
        "the conversation must arrive as roles, not collapsed into one turn",
    );

    // The failure this path exists to prevent: no chat template reaches the server.
    assert!(!sent["messages"].to_string().contains("<|im_start|>"));
    // Sent to the chat endpoint, whatever the config's endpoint says, since a message list
    // has only one endpoint it can mean.
    assert!(sent.get("prompt").is_none());
}

#[tokio::test]
async fn the_pieces_are_handed_over_as_they_arrive() {
    let (url, server) = stand_in(streamed("Atl", "antis."));
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

    let collected = seen.clone();
    backend(url)
        .chat(
            "",
            &[Message::user("1", "Where?")],
            &InferenceParams::default(),
            Arc::new(AtomicBool::new(false)),
            Box::new(move |piece, _, _| collected.lock().unwrap().push(piece.to_string())),
        )
        .await
        .expect("the turn");

    assert_eq!(*seen.lock().unwrap(), vec!["Atl", "antis."]);

    // An empty system prompt is not sent as an empty turn.
    let sent = body(&server.join().unwrap());
    assert_eq!(sent["messages"].as_array().unwrap().len(), 1);
    assert_eq!(sent["messages"][0]["role"], "user");
}

#[tokio::test]
async fn a_tool_result_is_carried_as_an_observation_rather_than_a_tool_role() {
    let (url, server) = stand_in(streamed("Th", "anks."));

    let mut result = Message::user("2", "42");
    result.role = Role::ToolResult;

    backend(url)
        .chat(
            "",
            &[Message::user("1", "Add them"), result],
            &InferenceParams::default(),
            Arc::new(AtomicBool::new(false)),
            Box::new(|_, _, _| {}),
        )
        .await
        .expect("the turn");

    let sent = body(&server.join().unwrap());
    let last = &sent["messages"][1];

    // The API's `tool` role wants a `tool_call_id` that this crate's text convention never
    // mints, so the observation goes over as something the model can simply read.
    assert_eq!(last["role"], "user");
    assert!(last["content"].as_str().unwrap().contains("[Tool result]"));
    assert!(last["content"].as_str().unwrap().contains("42"));
}
