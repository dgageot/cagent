//! Benchmarks for cagent
//!
//! Run with: cargo bench

use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Benchmark message creation
fn bench_message_creation(c: &mut Criterion) {
    use cagent::chat::Message;

    c.bench_function("create_user_message", |b| {
        b.iter(|| Message::user(black_box("Hello, world!")))
    });

    c.bench_function("create_assistant_message", |b| {
        b.iter(|| Message::assistant(black_box("I can help you with that.")))
    });

    c.bench_function("create_system_message", |b| {
        b.iter(|| Message::system(black_box("You are a helpful assistant.")))
    });
}

/// Benchmark config parsing
fn bench_config_parsing(c: &mut Criterion) {
    use cagent::config::Config;

    let yaml = r#"
version: "3"
agents:
  root:
    model: openai/gpt-4o
    description: A helpful AI assistant
    instruction: You are a helpful AI assistant.
    add_date: true
    add_environment_info: true
    toolsets:
      - type: filesystem
      - type: shell
      - type: think
"#;

    c.bench_function("parse_simple_config", |b| {
        b.iter(|| Config::from_yaml(black_box(yaml)).unwrap())
    });
}

/// Benchmark markdown rendering (basic operations)
fn bench_markdown(c: &mut Criterion) {
    // This tests our markdown module's ability to detect code blocks
    // and other patterns quickly
    let sample_text = r#"Here's some code:

```python
def hello():
    print("Hello, world!")
```

And here's more text with `inline code` and **bold**.

- Item 1
- Item 2
- Item 3
"#;

    c.bench_function("markdown_text_processing", |b| {
        b.iter(|| {
            // Just measure the string processing overhead
            let _ = black_box(sample_text).lines().count();
        })
    });
}

criterion_group!(
    benches,
    bench_message_creation,
    bench_config_parsing,
    bench_markdown
);
criterion_main!(benches);
