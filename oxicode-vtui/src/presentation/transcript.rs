//! Pure transcript cell selection.
//!
//! The CLI controller appends logical lines; this module selects the cells
//! which a terminal viewport should paint. It contains no agent or terminal
//! lifecycle code, so it can be tested without a running TUI.

use crate::tui::core::{InlineMessageKind, InlineSegment};

/// One logical transcript line, retained in append order.
#[derive(Debug, Clone)]
pub struct TranscriptLine {
    pub kind: InlineMessageKind,
    pub segments: Vec<InlineSegment>,
    /// Consecutive lines of the same message class share a visual cell.
    pub block_id: usize,
}

/// Display policy for a transcript cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BlockDisplayMode {
    /// Show the cell heading only.
    Collapsed,
    /// Show the head and tail of long cells.
    #[default]
    Truncated,
    /// Show every logical line.
    Expanded,
}

/// A logical transcript item selected for painting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisibleItem {
    /// A source line. `folded` requests the collapsed-cell marker.
    Line { source_index: usize, folded: bool },
    /// A compact summary of hidden lines in a truncated cell.
    Gap {
        source_index: usize,
        hidden_lines: usize,
    },
}

/// Build the visual cell sequence for the current display policies.
///
/// Long cells retain one head and the final three lines. This preserves the
/// existing renderer's behavior while keeping grouping and elision outside
/// the terminal draw function.
pub fn visible_items(
    transcript: &[TranscriptLine],
    mode_for: impl Fn(usize) -> BlockDisplayMode,
) -> Vec<VisibleItem> {
    const TRUNC_TAIL: usize = 3;

    let mut visible = Vec::with_capacity(transcript.len());
    let mut start = 0;
    while start < transcript.len() {
        let block_id = transcript[start].block_id;
        let mut end = start + 1;
        while end < transcript.len() && transcript[end].block_id == block_id {
            end += 1;
        }

        let len = end - start;
        match mode_for(block_id) {
            BlockDisplayMode::Collapsed => visible.push(VisibleItem::Line {
                source_index: start,
                folded: true,
            }),
            BlockDisplayMode::Expanded => {
                visible.extend((start..end).map(|source_index| VisibleItem::Line {
                    source_index,
                    folded: false,
                }));
            }
            BlockDisplayMode::Truncated if len > TRUNC_TAIL + 1 => {
                visible.push(VisibleItem::Line {
                    source_index: start,
                    folded: false,
                });
                visible.push(VisibleItem::Gap {
                    source_index: start,
                    hidden_lines: len - 1 - TRUNC_TAIL,
                });
                visible.extend(
                    (end - TRUNC_TAIL..end).map(|source_index| VisibleItem::Line {
                        source_index,
                        folded: false,
                    }),
                );
            }
            BlockDisplayMode::Truncated => {
                visible.extend((start..end).map(|source_index| VisibleItem::Line {
                    source_index,
                    folded: false,
                }));
            }
        }
        start = end;
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(block_id: usize) -> TranscriptLine {
        TranscriptLine {
            kind: InlineMessageKind::Agent,
            segments: Vec::new(),
            block_id,
        }
    }

    #[test]
    fn truncated_cells_keep_head_gap_and_tail() {
        let transcript = vec![line(7), line(7), line(7), line(7), line(7), line(7)];
        assert_eq!(
            visible_items(&transcript, |_| BlockDisplayMode::Truncated),
            vec![
                VisibleItem::Line {
                    source_index: 0,
                    folded: false
                },
                VisibleItem::Gap {
                    source_index: 0,
                    hidden_lines: 2
                },
                VisibleItem::Line {
                    source_index: 3,
                    folded: false
                },
                VisibleItem::Line {
                    source_index: 4,
                    folded: false
                },
                VisibleItem::Line {
                    source_index: 5,
                    folded: false
                },
            ]
        );
    }

    #[test]
    fn collapsed_cells_keep_only_the_head() {
        let transcript = vec![line(1), line(1), line(2)];
        assert_eq!(
            visible_items(&transcript, |_| BlockDisplayMode::Collapsed),
            vec![
                VisibleItem::Line {
                    source_index: 0,
                    folded: true
                },
                VisibleItem::Line {
                    source_index: 2,
                    folded: true
                },
            ]
        );
    }
}
