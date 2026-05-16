//! Table renderer ported from pi's markdown.ts
//! Supports cell wrapping, width-aware column sizing, and proper alignment.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Break a long word at character boundaries (CJK-safe).
/// Returns (completed_lines, remaining_chunk, remaining_width).
fn break_long_word(word: &str, max_width: usize) -> (Vec<String>, String, usize) {
    let mut completed = Vec::new();
    let mut chunk = String::new();
    let mut chunk_width = 0usize;
    for ch in word.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if chunk_width + cw > max_width && !chunk.is_empty() {
            completed.push(chunk);
            chunk = String::new();
            chunk_width = 0;
        }
        chunk.push(ch);
        chunk_width += cw;
    }
    (completed, chunk, chunk_width)
}

/// Wrap text to fit within max_width, breaking at word boundaries,
/// with CJK character-level fallback for long words.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![];
    }

    let text_width = UnicodeWidthStr::width(text);
    if text_width <= max_width {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);

        if current_line.is_empty() {
            if word_width <= max_width {
                current_line = word.to_string();
                current_width = word_width;
            } else {
                // Word too long — break at character boundaries (CJK support)
                let (completed, remainder, rem_w) = break_long_word(word, max_width);
                lines.extend(completed);
                current_line = remainder;
                current_width = rem_w;
            }
        } else if current_width + 1 + word_width <= max_width {
            // Can fit with space
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_width;
        } else {
            // Doesn't fit — start new line
            lines.push(current_line);
            if word_width <= max_width {
                current_line = word.to_string();
                current_width = word_width;
            } else {
                let (completed, remainder, rem_w) = break_long_word(word, max_width);
                lines.extend(completed);
                current_line = remainder;
                current_width = rem_w;
            }
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(text.to_string());
    }

    lines
}

/// Get the longest unbroken word width in text.
fn longest_word_width(text: &str, max_width: usize) -> usize {
    let mut longest = 0usize;
    for word in text.split_whitespace() {
        let w = UnicodeWidthStr::width(word).min(max_width);
        longest = longest.max(w);
    }
    longest.max(1)
}

/// Render a markdown table as styled Lines.
/// Uses pulldown-cmark for parsing and implements pi's column width algorithm.
///
/// If the content contains a table, renders the *entire* content (text before/after
/// the table is preserved). Returns empty Vec if no table is found (so the caller
/// can fall back to tui-markdown).
pub fn render_markdown_table(content: &str, available_width: u16) -> Vec<Line<'static>> {
    // In practice pulldown-cmark's table parsing can be sensitive to option
    // combinations; enable all extensions so tables are reliably recognized.
    let options = Options::all();

    // pulldown-cmark's table parser can fail to recognize a trailing row if the
    // input doesn't end with a newline. Normalize to a newline-terminated string.
    let content_owned;
    let input = if content.ends_with('\n') {
        content
    } else {
        content_owned = format!("{}\n", content);
        &content_owned
    };

    // Parse once: collect events into a Vec, then check and render.
    let events: Vec<Event> = Parser::new_ext(input, options).collect();
    let has_table = events.iter().any(|e| {
        matches!(
            e,
            Event::Start(Tag::Table(_)) | Event::Start(Tag::TableHead)
        )
    });
    if !has_table {
        return Vec::new();
    }

    // Render using the collected events.
    let mut table_state = TableState::default();
    let mut in_table = false;
    let mut pending_text = String::new();
    let mut lines: Vec<Line<'static>> = Vec::new();

    for event in events {
        match event {
            Event::Start(Tag::Table(_)) | Event::Start(Tag::TableHead) => {
                // Flush accumulated text before table
                flush_text(&mut pending_text, &mut lines);
                in_table = true;
                table_state = TableState::default();
                if matches!(event, Event::Start(Tag::TableHead)) {
                    table_state.in_head = true;
                }
            }
            Event::Start(Tag::TableRow) => {
                table_state.current_row = Vec::new();
            }
            Event::Start(Tag::TableCell) => {
                table_state.current_cell = String::new();
            }
            Event::Text(text) => {
                if in_table {
                    table_state.current_cell.push_str(&text);
                } else {
                    pending_text.push_str(&text);
                }
            }
            Event::End(TagEnd::TableCell) => {
                if in_table {
                    table_state
                        .current_row
                        .push(table_state.current_cell.clone());
                }
            }
            Event::End(TagEnd::TableRow) => {
                if in_table {
                    if !table_state.in_head {
                        table_state.rows.push(table_state.current_row.clone());
                    }
                    table_state.current_row = Vec::new();
                }
            }
            Event::End(TagEnd::TableHead) => {
                table_state.header = table_state.current_row.clone();
                table_state.current_row = Vec::new();
                table_state.in_head = false;
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                let rendered = render_table_data(&table_state, available_width);
                lines.extend(rendered);
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_table {
                    table_state.current_cell.push(' ');
                } else {
                    pending_text.push('\n');
                }
            }
            // All other structural events: flush text separator
            _ => {
                if !in_table {
                    pending_text.push('\n');
                }
            }
        }
    }

    // Flush remaining text after table
    flush_text(&mut pending_text, &mut lines);

    lines
}

#[derive(Default)]
struct TableState {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
}

fn render_table_data(state: &TableState, available_width: u16) -> Vec<Line<'static>> {
    let num_cols = state.header.len();
    if num_cols == 0 {
        return Vec::new();
    }

    let width = available_width as usize;

    // Border overhead: "│ " + (n-1) * " │ " + " │" = 3n + 1
    let border_overhead = 3 * num_cols + 1;
    let available_for_cells = width.saturating_sub(border_overhead);

    // Too narrow - return raw markdown as fallback
    if available_for_cells < num_cols {
        return fallback_render(&state.header, &state.rows, width);
    }

    let max_unbroken_word_width = 30;

    // Calculate natural column widths
    let mut natural_widths = vec![0usize; num_cols];
    let mut min_word_widths = vec![1usize; num_cols];

    for (i, cell) in state.header.iter().enumerate() {
        natural_widths[i] = UnicodeWidthStr::width(cell.as_str());
        min_word_widths[i] = longest_word_width(cell, max_unbroken_word_width);
    }

    for row in &state.rows {
        for (i, cell) in row.iter().enumerate().take(num_cols) {
            let w = UnicodeWidthStr::width(cell.as_str());
            natural_widths[i] = natural_widths[i].max(w);
            min_word_widths[i] =
                min_word_widths[i].max(longest_word_width(cell, max_unbroken_word_width));
        }
    }

    // Calculate column widths
    let column_widths = calculate_column_widths(
        &natural_widths,
        &min_word_widths,
        available_for_cells,
        num_cols,
    );

    let mut lines = Vec::new();

    // Top border
    lines.push(make_border_line(&column_widths, '┌', '┬', '┐'));

    // Header with wrapping
    let header_lines = wrap_cell_rows(&state.header, &column_widths);
    let header_line_count = header_lines.iter().map(|l| l.len()).max().unwrap_or(1);

    for line_idx in 0..header_line_count {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::raw("│ ".to_string()));
        for (col_idx, cell_lines) in header_lines.iter().enumerate() {
            if col_idx > 0 {
                spans.push(Span::raw(" │ ".to_string()));
            }
            let text = cell_lines.get(line_idx).map(|s| s.as_str()).unwrap_or("");
            let padded = pad_to_width(text, column_widths[col_idx]);
            spans.push(Span::styled(
                padded,
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
        spans.push(Span::raw(" │".to_string()));
        lines.push(Line::from(spans));
    }

    // Separator
    lines.push(make_separator_line(&column_widths));

    // Body rows
    let separator = make_separator_line(&column_widths);
    for (row_index, row) in state.rows.iter().enumerate() {
        let cell_lines = wrap_cell_rows(row, &column_widths);
        let row_line_count = cell_lines.iter().map(|l| l.len()).max().unwrap_or(1);

        for line_idx in 0..row_line_count {
            let parts: Vec<String> = cell_lines
                .iter()
                .map(|cell_lines| cell_lines.get(line_idx).map(|s| s.as_str()).unwrap_or(""))
                .enumerate()
                .map(|(col_idx, text)| pad_to_width(text, column_widths[col_idx]))
                .collect();
            lines.push(Line::from(Span::raw(format!("│ {} │", parts.join(" │ ")))));
        }

        if row_index < state.rows.len() - 1 {
            lines.push(separator.clone());
        }
    }

    // Bottom border
    lines.push(make_border_line(&column_widths, '└', '┴', '┘'));

    lines
}

fn calculate_column_widths(
    natural_widths: &[usize],
    min_word_widths: &[usize],
    available_for_cells: usize,
    num_cols: usize,
) -> Vec<usize> {
    let mut min_column_widths = min_word_widths.to_vec();
    let min_cells_width: usize = min_column_widths.iter().sum();

    if min_cells_width > available_for_cells {
        // Need to shrink to minimum
        min_column_widths = vec![1usize; num_cols];
        let remaining = available_for_cells.saturating_sub(num_cols);

        if remaining > 0 {
            let total_weight: usize = min_word_widths.iter().map(|w| w.saturating_sub(1)).sum();

            let growth: Vec<usize> = min_word_widths
                .iter()
                .map(|&width| {
                    let weight = width.saturating_sub(1);
                    if total_weight > 0 {
                        (weight * remaining) / total_weight
                    } else {
                        0
                    }
                })
                .collect();

            for (i, width) in min_column_widths.iter_mut().enumerate() {
                *width += growth.get(i).copied().unwrap_or(0);
            }

            let allocated: usize = growth.iter().sum();
            let mut leftover = remaining.saturating_sub(allocated);

            for width in &mut min_column_widths {
                if leftover > 0 {
                    *width += 1;
                    leftover -= 1;
                }
            }
        }
    }

    // Check if natural widths fit
    let total_natural: usize = natural_widths.iter().sum();

    if total_natural <= available_for_cells {
        // Everything fits naturally
        return natural_widths
            .iter()
            .enumerate()
            .map(|(i, &w)| w.max(min_column_widths[i]))
            .collect();
    }

    // Need to shrink
    let min_cells_width: usize = min_column_widths.iter().sum();
    let extra_width = available_for_cells.saturating_sub(min_cells_width);
    let total_grow_potential: usize = natural_widths
        .iter()
        .enumerate()
        .map(|(i, &w)| w.saturating_sub(min_column_widths[i]))
        .sum();

    let mut column_widths: Vec<usize> = min_column_widths
        .iter()
        .enumerate()
        .map(|(i, &min_w)| {
            let natural_w = natural_widths[i];
            let grow_potential = natural_w.saturating_sub(min_w);
            let grow = if total_grow_potential > 0 && extra_width > 0 {
                (grow_potential * extra_width) / total_grow_potential
            } else {
                0
            };
            min_w + grow
        })
        .collect();

    // Adjust for rounding errors
    let allocated: usize = column_widths.iter().sum();
    let mut remaining = available_for_cells.saturating_sub(allocated);

    while remaining > 0 {
        let mut grew = false;
        for i in 0..num_cols {
            if remaining > 0 && column_widths[i] < natural_widths[i] {
                column_widths[i] += 1;
                remaining -= 1;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    column_widths
}

fn wrap_cell_rows(cells: &[String], widths: &[usize]) -> Vec<Vec<String>> {
    cells
        .iter()
        .enumerate()
        .map(|(i, text)| wrap_text(text, widths[i]))
        .collect()
}

fn pad_to_width(text: &str, width: usize) -> String {
    let text_width = UnicodeWidthStr::width(text);
    if text_width >= width {
        // Truncate to fit
        let mut result = String::new();
        let mut current_width = 0usize;
        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if current_width + ch_width > width {
                break;
            }
            result.push(ch);
            current_width += ch_width;
        }
        result
    } else {
        // Pad with spaces on the right, accounting for display width
        let padding = width - text_width;
        let mut result = text.to_string();
        for _ in 0..padding {
            result.push(' ');
        }
        result
    }
}

fn make_border_line(widths: &[usize], left: char, mid: char, right: char) -> Line<'static> {
    let mut s = String::new();
    s.push(left);
    s.push('─');
    for (i, w) in widths.iter().enumerate() {
        s.push_str(&"─".repeat(*w));
        if i + 1 < widths.len() {
            s.push('─');
            s.push(mid);
            s.push('─');
        } else {
            s.push('─');
        }
    }
    s.push(right);
    Line::from(Span::raw(s))
}

fn make_separator_line(widths: &[usize]) -> Line<'static> {
    let mut s = String::new();
    s.push('├');
    s.push('─');
    for (i, w) in widths.iter().enumerate() {
        s.push_str(&"─".repeat(*w));
        if i + 1 < widths.len() {
            s.push('─');
            s.push('┼');
            s.push('─');
        } else {
            s.push('─');
        }
    }
    s.push('┤');
    Line::from(Span::raw(s))
}

fn fallback_render(header: &[String], rows: &[Vec<String>], width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let num_cols = header.len();

    if num_cols == 0 {
        return lines;
    }

    // Simple fallback: just show header and data as-is
    let col_width = (width - 2).max(1) / num_cols;

    for cell in header {
        let padded = pad_to_width(cell, col_width);
        lines.push(Line::from(Span::styled(
            padded,
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }

    for row in rows {
        let parts: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                if i < num_cols {
                    pad_to_width(cell, col_width)
                } else {
                    String::new()
                }
            })
            .collect();
        lines.push(Line::from(Span::raw(parts.join(" "))));
    }

    lines
}

/// Flush accumulated non-table text through tui-markdown.
fn flush_text(pending: &mut String, lines: &mut Vec<Line<'static>>) {
    if pending.is_empty() || pending.trim().is_empty() {
        pending.clear();
        return;
    }
    // Use tui-markdown for non-table text
    let preprocessed = pending.replace("\t", "   ");
    let text: ratatui::text::Text<'_> = tui_markdown::from_str(&preprocessed);
    for l in text.lines {
        let line_style = l.style;
        let spans: Vec<Span<'static>> = l
            .spans
            .into_iter()
            .map(|s| Span::styled(s.content.into_owned(), line_style.patch(s.style)))
            .collect();
        lines.push(Line::from(spans));
    }
    pending.clear();
}
