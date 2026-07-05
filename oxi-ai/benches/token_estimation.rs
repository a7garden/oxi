//! Benchmarks for token estimation.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use oxi_ai::{context_usage, estimate, estimate_words};
use std::hint::black_box;

/// Generate a realistic English prose sample of approximately `n` bytes.
fn prose(n: usize) -> String {
    let base = "The quick brown fox jumps over the lazy dog. \
                In a world of endless possibilities, each token represents \
                a fragment of meaning. Language models process these tokens \
                to understand and generate human-like text. ";
    let mut s = String::new();
    while s.len() < n {
        s.push_str(base);
    }
    s.truncate(n);
    s
}

/// Generate a realistic source code sample of approximately `n` bytes.
fn code(n: usize) -> String {
    let base = "fn process_tokens(text: &str) -> usize {\n  \
                let mut count = 0;\n  \
                for ch in text.chars() {\n    \
                  if ch.is_alphanumeric() {\n      \
                    count += 1;\n    \
                  }\n  \
                }\n  \
                count\n\
                }\n";
    let mut s = String::new();
    while s.len() < n {
        s.push_str(base);
    }
    s.truncate(n);
    s
}

/// Generate mixed CJK + English text of approximately `n` chars.
/// Uses chars-based truncation to avoid splitting multi-byte sequences.
fn mixed_cjk(n: usize) -> String {
    let base = "这是一段中文测试文本，mixed with English words. \
                日本語テキストも含まれています. And some more CJK text too. \
                The multilingual tokenizer must handle all of these. ";
    let mut s = String::new();
    while s.chars().count() < n {
        s.push_str(base);
    }
    // Truncate by char boundary
    let truncated: String = s.chars().take(n).collect();
    truncated
}

/// Generate punctuation-heavy JSON-like text of approximately `n` bytes.
fn json_like(n: usize) -> String {
    let base = r#"{"key": "value", "nested": {"a": 1, "b": [true, null, "str"]}, "count": 42}"#;
    let mut s = String::new();
    while s.len() < n {
        s.push_str(base);
        s.push(',');
    }
    s.truncate(n);
    s
}

fn bench_estimate(c: &mut Criterion) {
    let mut group = c.benchmark_group("estimate");

    for (name, input) in [
        ("prose_1k", prose(1_000)),
        ("prose_10k", prose(10_000)),
        ("prose_100k", prose(100_000)),
        ("code_1k", code(1_000)),
        ("code_10k", code(10_000)),
        ("code_100k", code(100_000)),
        ("cjk_mixed_1k", mixed_cjk(1_000)),
        ("cjk_mixed_10k", mixed_cjk(10_000)),
        ("json_1k", json_like(1_000)),
        ("json_10k", json_like(10_000)),
    ] {
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_with_input(BenchmarkId::new("estimate", name), &input, |b, text| {
            b.iter(|| estimate(black_box(text)));
        });
    }

    group.finish();
}

fn bench_estimate_words(c: &mut Criterion) {
    let mut group = c.benchmark_group("estimate_words");

    let prose_10k = prose(10_000);
    let code_10k = code(10_000);
    let cjk_10k = mixed_cjk(10_000);

    group.throughput(Throughput::Bytes(prose_10k.len() as u64));
    group.bench_function("prose_10k", |b| {
        b.iter(|| estimate_words(black_box(&prose_10k)));
    });

    group.throughput(Throughput::Bytes(code_10k.len() as u64));
    group.bench_function("code_10k", |b| {
        b.iter(|| estimate_words(black_box(&code_10k)));
    });

    group.throughput(Throughput::Bytes(cjk_10k.len() as u64));
    group.bench_function("cjk_mixed_10k", |b| {
        b.iter(|| estimate_words(black_box(&cjk_10k)));
    });

    group.finish();
}

fn bench_context_usage(c: &mut Criterion) {
    let text = prose(50_000);
    let mut group = c.benchmark_group("context_usage");

    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("prose_50k_ctx_128k", |b| {
        b.iter(|| context_usage(black_box(&text), 128_000));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_estimate,
    bench_estimate_words,
    bench_context_usage
);
criterion_main!(benches);
