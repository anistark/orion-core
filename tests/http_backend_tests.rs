//! Tests for the OpenAI-compatible HTTP backend against a mocked SSE server.
//!
//! `reqwest::blocking` cannot run inside a Tokio runtime, so these are plain
//! `#[test]`s calling the synchronous `generate` directly.
#![cfg(feature = "http-backend")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;

use orion_core::backends::{OpenAiConfig, OpenAiEndpoint, OpenAiHttpBackend};
use orion_core::{CoreError, InferenceParams, LlmBackend};

/// Spin up a one-shot HTTP server that replies with `status_line` and `body`,
/// then closes. Returns the base URL (`http://127.0.0.1:PORT/v1`) and a handle
/// that yields the raw request the client sent (request line, headers, body).
fn spawn_server(
    status_line: &'static str,
    content_type: &'static str,
    body: String,
) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");

    let handle = thread::spawn(move || -> String {
        let mut captured = String::new();
        if let Ok((mut stream, _)) = listener.accept() {
            // Read the request so the client's write completes. Localhost sends
            // this small request in a single segment.
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            captured = String::from_utf8_lossy(&buf[..n]).into_owned();

            let response = format!(
                "{status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
        captured
    });

    (format!("http://{addr}/v1"), handle)
}

fn backend(base_url: String) -> OpenAiHttpBackend {
    OpenAiHttpBackend::new(OpenAiConfig::new(base_url, "test-model")).expect("build backend")
}

fn backend_with(base_url: String, endpoint: OpenAiEndpoint) -> OpenAiHttpBackend {
    OpenAiHttpBackend::new(OpenAiConfig::new(base_url, "test-model").with_endpoint(endpoint))
        .expect("build backend")
}

fn no_abort() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

const SSE_STREAM: &str = "\
data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\
\n\
data: {\"choices\":[{\"delta\":{\"content\":\", \"}}]}\n\
\n\
data: {\"choices\":[{\"delta\":{\"content\":\"world!\"}}]}\n\
\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":3,\"total_tokens\":14}}\n\
\n\
data: [DONE]\n\
\n";

#[test]
fn streams_tokens_and_maps_usage() {
    let (url, srv) = spawn_server(
        "HTTP/1.1 200 OK",
        "text/event-stream",
        SSE_STREAM.to_string(),
    );
    let backend = backend(url);

    let collected = Arc::new(std::sync::Mutex::new(String::new()));
    let sink = collected.clone();
    let on_token = Box::new(move |tok: &str, _n: u32, _tps: f64| {
        sink.lock().unwrap().push_str(tok);
    });

    let result = backend
        .generate("hi", &InferenceParams::default(), no_abort(), on_token)
        .expect("generate");

    // Tokens were streamed through the callback in order.
    assert_eq!(*collected.lock().unwrap(), "Hello, world!");
    // The full text is assembled.
    assert_eq!(result.text, "Hello, world!");
    // Counts come from the server's usage block, not the streamed-chunk count.
    assert_eq!(result.prompt_tokens, 11);
    assert_eq!(result.tokens_generated, 3);

    // The default endpoint posts to chat/completions with a messages array.
    let request = srv.join().expect("server thread");
    assert!(request.contains("POST /v1/chat/completions"), "{request}");
    assert!(request.contains("\"messages\""), "{request}");
    assert!(!request.contains("\"prompt\""), "{request}");
}

#[test]
fn completions_endpoint_sends_raw_prompt() {
    // Completion-style stream: content arrives under `choices[0].text`.
    let stream = "\
data: {\"choices\":[{\"text\":\"raw \"}]}\n\
\n\
data: {\"choices\":[{\"text\":\"answer\"}]}\n\
\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n\
\n\
data: [DONE]\n\
\n";
    let (url, srv) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", stream.to_string());
    let backend = backend_with(url, OpenAiEndpoint::Completions);

    let result = backend
        .generate(
            "<|im_start|>user\nhi<|im_end|>",
            &InferenceParams::default(),
            no_abort(),
            Box::new(|_, _, _| {}),
        )
        .expect("generate");

    assert_eq!(result.text, "raw answer");
    assert_eq!(result.tokens_generated, 2);
    assert_eq!(result.prompt_tokens, 5);

    // Posts to completions with the already-templated prompt sent verbatim.
    let request = srv.join().expect("server thread");
    assert!(request.contains("POST /v1/completions"), "{request}");
    assert!(request.contains("\"prompt\""), "{request}");
    assert!(!request.contains("\"messages\""), "{request}");
    // The chat-template markup reached the body intact (not re-wrapped).
    assert!(request.contains("im_start"), "{request}");
}

#[test]
fn falls_back_to_streamed_count_without_usage() {
    // Same stream but no usage chunk: completion count falls back to the number
    // of streamed content chunks.
    let no_usage = "\
data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\
\n\
data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\
\n\
data: [DONE]\n\
\n";
    let (url, _srv) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", no_usage.to_string());
    let backend = backend(url);

    let result = backend
        .generate(
            "hi",
            &InferenceParams::default(),
            no_abort(),
            Box::new(|_, _, _| {}),
        )
        .expect("generate");

    assert_eq!(result.text, "ab");
    assert_eq!(result.tokens_generated, 2);
    assert_eq!(result.prompt_tokens, 0);
}

#[test]
fn aborts_mid_stream() {
    let (url, _srv) = spawn_server(
        "HTTP/1.1 200 OK",
        "text/event-stream",
        SSE_STREAM.to_string(),
    );
    let backend = backend(url);

    // Abort already requested: the loop must bail with `Aborted` rather than
    // finishing the stream.
    let abort = Arc::new(AtomicBool::new(true));
    let err = backend
        .generate(
            "hi",
            &InferenceParams::default(),
            abort,
            Box::new(|_, _, _| {}),
        )
        .expect_err("should abort");

    assert!(matches!(err, CoreError::Aborted), "got {err:?}");
}

#[test]
fn endpoint_error_maps_to_backend() {
    let (url, _srv) = spawn_server(
        "HTTP/1.1 500 Internal Server Error",
        "application/json",
        "{\"error\":\"boom\"}".to_string(),
    );
    let backend = backend(url);

    let err = backend
        .generate(
            "hi",
            &InferenceParams::default(),
            no_abort(),
            Box::new(|_, _, _| {}),
        )
        .expect_err("should error");

    match err {
        CoreError::Backend(msg) => {
            assert!(
                msg.contains("500"),
                "message should carry the status: {msg}"
            );
        }
        other => panic!("expected Backend, got {other:?}"),
    }
}

#[test]
fn unreachable_endpoint_maps_to_backend_unreachable() {
    // Bind a port, then drop the listener so nothing is listening there.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let url = format!("http://{addr}/v1");
    let backend = backend(url);

    let err = backend
        .generate(
            "hi",
            &InferenceParams::default(),
            no_abort(),
            Box::new(|_, _, _| {}),
        )
        .expect_err("should fail to connect");

    assert!(
        matches!(err, CoreError::BackendUnreachable(_)),
        "got {err:?}"
    );
}
