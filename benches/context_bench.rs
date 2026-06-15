//! Benchmarks for the context pipeline: pruning + template formatting.
//!
//! ```sh
//! cargo bench
//! ```

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use orion_core::context::{prepare_context, ContextConfig};
use orion_core::messages::Message;
use orion_core::template::{ChatMLTemplate, ChatTemplate, Llama3Template, MistralTemplate};

/// Build a `turns`-long user/assistant conversation with realistic-ish text.
fn make_convo(turns: usize) -> Vec<Message> {
    let mut msgs = Vec::with_capacity(turns * 2);
    for i in 0..turns {
        msgs.push(Message::user(
            format!("msg-{}", i * 2 + 1),
            format!("User question number {i} with a fair number of words to tokenize and format."),
        ));
        msgs.push(Message::assistant(
            format!("msg-{}", i * 2 + 2),
            format!(
                "Assistant answer number {i} with a fair number of words to tokenize and format."
            ),
        ));
    }
    msgs
}

/// Word-count tokenizer stand-in (a real backend would tokenize properly).
fn word_count(text: &str) -> u32 {
    text.split_whitespace().count() as u32
}

fn bench_prepare_context(c: &mut Criterion) {
    let config = ContextConfig {
        max_context_tokens: 4096,
        max_response_tokens: 1024,
        ..Default::default()
    };
    let mut group = c.benchmark_group("prepare_context");
    for turns in [10usize, 50, 200] {
        let convo = make_convo(turns);
        group.bench_function(format!("{turns}_turns"), |b| {
            b.iter(|| {
                prepare_context(
                    &ChatMLTemplate,
                    black_box(SYSTEM),
                    black_box(&convo),
                    &[],
                    &config,
                    &word_count,
                )
                .unwrap()
            });
        });
    }
    group.finish();
}

fn bench_format(c: &mut Criterion) {
    let convo = make_convo(50);
    let templates: [(&str, &dyn ChatTemplate); 3] = [
        ("chatml", &ChatMLTemplate),
        ("llama3", &Llama3Template),
        ("mistral", &MistralTemplate),
    ];
    let mut group = c.benchmark_group("format_50_turns");
    for (name, template) in templates {
        group.bench_function(name, |b| {
            b.iter(|| template.format(black_box(SYSTEM), black_box(&convo), &[]));
        });
    }
    group.finish();
}

const SYSTEM: &str = "You are a helpful assistant.";

criterion_group!(benches, bench_prepare_context, bench_format);
criterion_main!(benches);
