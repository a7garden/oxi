//! Frame-based spinner animation.
//!
//! Picks a frame from `frames` based on the elapsed time since the first
//! `render()` call. The cached hash reflects the currently-displayed
//! frame so the pipeline re-renders the cell when the frame rotates.
//!
//! Used for "loading…" indicators while the assistant streams.

use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use crate::widget::{RenderCtx, Renderable, hash_combine};

const FRAME_INTERVAL: Duration = Duration::from_millis(120);

/// Classic ASCII spinner: `| / - \` rotated once every 120 ms.
const ASCII_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// Frame-based spinner.
#[derive(Debug, Clone)]
pub struct Spinner {
    frames: &'static [&'static str],
    /// First `ctx.time` we observed; the spinner rotates relative to this.
    anchor: Option<Instant>,
    /// Last computed hash (frame index + frame count).
    cached_hash: u64,
}

impl Spinner {
    #[must_use]
    pub fn new() -> Self {
        Self::with_frames(ASCII_FRAMES)
    }

    #[must_use]
    pub fn with_frames(frames: &'static [&'static str]) -> Self {
        let frames = if frames.is_empty() {
            ASCII_FRAMES
        } else {
            frames
        };
        Self {
            frames,
            anchor: None,
            cached_hash: hash_for_frame_index(0, frames.len()),
        }
    }

    #[must_use]
    pub fn frame_at(&self, time: Instant) -> &'static str {
        let idx = self.index_at(time);
        self.frames[idx]
    }

    fn index_at(&self, time: Instant) -> usize {
        let len = self.frames.len();
        if len == 0 {
            return 0;
        }
        let Some(anchor) = self.anchor else {
            return 0;
        };
        let elapsed = time.saturating_duration_since(anchor);
        let step = (elapsed.as_millis() / FRAME_INTERVAL.as_millis()) as usize;
        step % len
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderable for Spinner {
    fn content_hash(&self) -> u64 {
        self.cached_hash
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        // A spinner is a single cell.
        1
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let anchor = self.anchor.get_or_insert(ctx.time);
        let idx = if self.frames.is_empty() {
            0
        } else {
            let elapsed = ctx.time.saturating_duration_since(*anchor);
            let step = (elapsed.as_millis() / FRAME_INTERVAL.as_millis()) as usize;
            step % self.frames.len()
        };
        // Recompute hash so the next pipeline pass picks up frame rotation.
        self.cached_hash = hash_for_frame_index(idx, self.frames.len());
        let buf = ctx.buffer_mut();
        let cell = &mut buf[(area.x, area.y)];
        cell.set_symbol(self.frames[idx]);
    }
}

fn hash_for_frame_index(index: usize, len: usize) -> u64 {
    // Distinguish frame index and total frames so two different framesets
    // at the same time don't accidentally collide.
    hash_combine(index as u64, len as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{TerminalCaps, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn render_spinner(spinner: &mut Spinner, anchor: Instant, time: Instant) -> Buffer {
        let backend = TestBackend::new(1, 1);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            ctx.time = time;
            // Seed the anchor manually so we don't depend on render order.
            spinner.anchor = Some(anchor);
            spinner.render(
                Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                &mut ctx,
            );
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    #[test]
    fn spinner_animates_on_time_change() {
        let mut spinner = Spinner::new();
        let anchor = Instant::now();
        let buf0 = render_spinner(&mut spinner, anchor, anchor);
        // Step well past the frame interval.
        let later = anchor
            .checked_add(Duration::from_secs(10))
            .expect("anchor should be far enough in the past");
        let buf1 = render_spinner(&mut spinner, anchor, later);
        assert_ne!(
            buf0[(0, 0)].symbol(),
            buf1[(0, 0)].symbol(),
            "spinner should rotate over time"
        );
    }

    #[test]
    fn spinner_hash_changes_on_frame_rotation() {
        let mut a = Spinner::new();
        let mut b = Spinner::new();
        let anchor = Instant::now();
        render_spinner(&mut a, anchor, anchor);
        let h_now = a.content_hash();
        let later = anchor
            .checked_add(FRAME_INTERVAL)
            .expect("anchor should be far enough in the past");
        render_spinner(&mut b, anchor, later);
        let h_later = b.content_hash();
        assert_ne!(h_now, h_later);
    }

    #[test]
    fn spinner_with_empty_frames_falls_back_to_ascii() {
        let spinner = Spinner::with_frames(&[]);
        assert_eq!(spinner.frames, ASCII_FRAMES);
        let now = Instant::now();
        assert!(!spinner.frame_at(now).is_empty());
    }
}
