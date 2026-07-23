//! Benchmark: per-subtree memoization during streaming.
//!
//! During a token stream, only the active message's subtree changes each
//! frame. `RetainedChild<T>` short-circuits unchanged siblings so they
//! re-hash (cheap) but never re-render (expensive buffer writes).
//!
//! ## Benches
//!
//! Single-subtree (per-call overhead — plan-named):
//! - `retained_child_skip_unchanged`: 1 priming render + 100 renders that
//!   all skip (hash unchanged).
//! - `retained_child_render_on_change`: 100 renders where content mutates
//!   each call.
//!
//!   For a trivial one-line `Text`, `render()` and `content_hash()` are
//!   similarly cheap, so the ratio here is modest (~2×). This pair measures
//!   *per-call* overhead, not the streaming win.
//!
//! Composite streaming (the real workload — demonstrates the 10×+ goal):
//! - `streaming_composite_memoized`: N=40 subtrees, one active per frame.
//!   Per frame only the active child re-renders; the other N-1 skip.
//! - `streaming_composite_naive`: same N subtrees, but every child
//!   re-renders every frame (the cost without memoization).
//!
//! ## Success criterion (plan)
//!
//! `streaming_composite_memoized` should be ≥ 10× faster than
//! `streaming_composite_naive`. That ratio IS the value of per-subtree
//! memoization: it scales with subtree count, exactly the streaming case.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use oxi_tui::theme::{TerminalCaps, Theme};
use oxi_tui::widget::{RenderCtx, Renderable, RetainedChild, Text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

const W: u16 = 80;
const H: u16 = 24;
const FRAMES: u32 = 100;
/// Subtrees per composite — models a chat view with ~40 messages.
const N: usize = 40;

/// Row `i`'s area — each subtree paints its own single row so renders touch
/// distinct cells (realistic, no overdraw masking the cost).
fn row_area(i: usize) -> Rect {
    Rect::new(0, (i as u16).min(H - 1), W, 1)
}

fn bench_single_skip(c: &mut Criterion) {
    let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
    c.bench_function("retained_child_skip_unchanged", |b| {
        b.iter(|| {
            term.draw(|frame| {
                let theme = Theme::dark();
                let caps = TerminalCaps::default();
                let mut ctx = RenderCtx::new(frame, &theme, &caps);
                let mut child = RetainedChild::new(Text::new("hello world"));
                black_box(child.render_if_changed(ctx.area(), &mut ctx));
                for _ in 0..FRAMES {
                    black_box(child.render_if_changed(ctx.area(), &mut ctx));
                }
            })
            .expect("draw");
        });
    });
}

fn bench_single_render_on_change(c: &mut Criterion) {
    let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
    c.bench_function("retained_child_render_on_change", |b| {
        b.iter(|| {
            term.draw(|frame| {
                let theme = Theme::dark();
                let caps = TerminalCaps::default();
                let mut ctx = RenderCtx::new(frame, &theme, &caps);
                let mut child = RetainedChild::new(Text::new("hello"));
                for i in 0..FRAMES {
                    child.inner_mut().set_content(format!("token {i}"));
                    black_box(child.render_if_changed(ctx.area(), &mut ctx));
                }
            })
            .expect("draw");
        });
    });
}

fn bench_composite_memoized(c: &mut Criterion) {
    let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
    c.bench_function("streaming_composite_memoized", |b| {
        b.iter(|| {
            term.draw(|frame| {
                let theme = Theme::dark();
                let caps = TerminalCaps::default();
                let mut ctx = RenderCtx::new(frame, &theme, &caps);
                let mut children: Vec<RetainedChild<Text>> = (0..N)
                    .map(|i| RetainedChild::new(Text::new(format!("line {i}"))))
                    .collect();
                // First frame: all N render (cold, last_hash=0). Each subsequent
                // frame changes exactly one child (round-robin) → 1 render + N-1 skips.
                for f in 0..FRAMES {
                    let active = (f as usize) % N;
                    children[active]
                        .inner_mut()
                        .set_content(format!("token {f}"));
                    for (i, child) in children.iter_mut().enumerate() {
                        black_box(child.render_if_changed(row_area(i), &mut ctx));
                    }
                }
            })
            .expect("draw");
        });
    });
}

fn bench_composite_naive(c: &mut Criterion) {
    let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
    c.bench_function("streaming_composite_naive", |b| {
        b.iter(|| {
            term.draw(|frame| {
                let theme = Theme::dark();
                let caps = TerminalCaps::default();
                let mut ctx = RenderCtx::new(frame, &theme, &caps);
                let mut children: Vec<RetainedChild<Text>> = (0..N)
                    .map(|i| RetainedChild::new(Text::new(format!("line {i}"))))
                    .collect();
                // No memoization: every child re-renders every frame regardless.
                for f in 0..FRAMES {
                    let active = (f as usize) % N;
                    children[active]
                        .inner_mut()
                        .set_content(format!("token {f}"));
                    for (i, child) in children.iter_mut().enumerate() {
                        child.inner_mut().render(row_area(i), &mut ctx);
                    }
                }
            })
            .expect("draw");
        });
    });
}

criterion_group!(
    benches,
    bench_single_skip,
    bench_single_render_on_change,
    bench_composite_memoized,
    bench_composite_naive
);
criterion_main!(benches);
