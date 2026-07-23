//! Benchmark: cursor-position dedup — the core of blink preservation.
//!
//! `CursorState::reconcile` is called every frame with the desired cursor
//! state. It emits escape bytes ONLY on a visibility transition or a
//! position change; the same position while visible emits zero bytes. That
//! is what stops the terminal cursor from flickering on every frame.
//!
//! ## Why a custom `CountingBackend` instead of `TestBackend`
//!
//! `TestBackend` is a pure cell grid with no observable side effects, so the
//! optimizer DCEs the emit path entirely (a 60-MoveTo loop benchmarks at
//! ~250 ps — meaningless). The cursor.rs unit tests hit the same trap and
//! work around it with a `RecordingBackend`; this bench does the same with
//! a `CountingBackend` whose cursor methods increment counters kept alive
//! by `black_box`. That forces the full `Terminal::set_cursor_position`
//! path to actually run.
//!
//! ## Benches (both prime one show+move, then loop 60 reconciles)
//!
//! - `cursor_reconcile_same_position`: 60 reconciles at an unchanged
//!   position while visible — emits zero MoveTo (the dedup path).
//! - `cursor_reconcile_changing_position`: 60 reconciles at 60 distinct
//!   positions while visible — emits a MoveTo each (the emit path).
//!
//! ## Success criterion (plan) + measured result
//!
//! `same_position` is cheaper than `changing_position`, but only modestly on
//! the CPU side: `reconcile` is itself sub-nanosecond, so the 60-frame loop
//! measures ~16 ns (dedup, 0 emits) vs ~26 ns (emit, 60 MoveTo) — the emit
//! path costs ~0.15 ns *per call* on top. That confirms the dedup overhead is
//! negligible, which is the real point: an idle cursor-parked 60 fps stream
//! spends ~zero CPU and ~zero bytes on cursor sync. The hard correctness proof
//! that the dedup path emits exactly zero bytes (and the emit path emits
//! exactly one MoveTo per change) lives in the `cursor.rs` unit tests, not
//! here — this bench exists to confirm the cost stays negligible.

use std::convert::Infallible;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use oxi_tui::pipeline::CursorState;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Position, Size};

const W: u16 = 80;
const H: u16 = 24;
/// Reconciles per iteration — ~1s idle at 60 fps.
const DEDUP_FRAMES: u16 = 60;
const P1: Position = Position { x: 5, y: 10 };

/// Backend that counts cursor operations so the emit path cannot be DCE'd.
#[derive(Default)]
struct CountingBackend {
    moveto: u64,
    show: u64,
    hide: u64,
    size: Size,
}

impl Backend for CountingBackend {
    type Error = Infallible;

    fn draw<'a, I>(&mut self, _content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.hide += 1;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.show += 1;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(Position { x: 0, y: 0 })
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, _p: P) -> Result<(), Self::Error> {
        self.moveto += 1;
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn clear_region(
        &mut self,
        _clear_type: ratatui::backend::ClearType,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(self.size)
    }

    fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Self::Error> {
        Ok(ratatui::backend::WindowSize {
            columns_rows: self.size,
            pixels: Size {
                width: 0,
                height: 0,
            },
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn make_terminal() -> Terminal<CountingBackend> {
    Terminal::new(CountingBackend {
        moveto: 0,
        show: 0,
        hide: 0,
        size: Size {
            width: W,
            height: H,
        },
    })
    .expect("terminal")
}

fn bench_same_position(c: &mut Criterion) {
    // Allocate once — the per-iteration terminal setup would otherwise dominate
    // and mask the sub-nanosecond reconcile-loop work we actually want to measure.
    let mut term = make_terminal();
    c.bench_function("cursor_reconcile_same_position", |b| {
        b.iter(|| {
            let mut cursor = CursorState::new();
            // Prime: one show + move (cursor visible at P1).
            black_box(cursor.reconcile(Some(P1), &mut term));
            // Steady state: same position, visible — emits zero bytes.
            for _ in 0..DEDUP_FRAMES {
                black_box(cursor.reconcile(Some(P1), &mut term));
            }
            // Keep the emit side effects alive (defeats DCE on the loop).
            let be = term.backend_mut();
            black_box(be.moveto + be.show + be.hide);
        });
    });
}

fn bench_changing_position(c: &mut Criterion) {
    let mut term = make_terminal();
    c.bench_function("cursor_reconcile_changing_position", |b| {
        b.iter(|| {
            let mut cursor = CursorState::new();
            // Prime identically, so only the loop work differs between benches.
            black_box(cursor.reconcile(Some(P1), &mut term));
            // Every frame the position differs → a MoveTo emit each time.
            for i in 0..DEDUP_FRAMES {
                // Distinct x each frame (all within the 80-wide row) → a MoveTo every call.
                let p = Position { x: i % W, y: 0 };
                black_box(cursor.reconcile(Some(p), &mut term));
            }
            let be = term.backend_mut();
            black_box(be.moveto + be.show + be.hide);
        });
    });
}

criterion_group!(benches, bench_same_position, bench_changing_position);
criterion_main!(benches);
