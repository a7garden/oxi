//! Table renderer ported from pi's markdown.ts
//! Supports cell wrapping, width-aware column sizing, and proper alignment.

use pulldown_cmark::{
    Event, Options, Parser, Tag, TagEnd,
};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Wrap text to fit within max_width, breaking at word boundaries.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    use std::fmt::Write;
    
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
            if word_width > max_width {
                // Word too long even for empty line, truncate
                let mut truncated = String::new();
                let mut w = 0usize;
                for ch in word.chars() {
                    let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1);
                    if w + ch_width > max_width {
                        break;
                    }
                    let _ = write!(truncated, "{}", ch);
                    w += ch_width;
                }
                if !truncated.is_empty() {
                    lines.push(truncated);
                }
            } else {
                current_line = word.to_string();
                current_width = word_width;
            }
        } else if current_width + 1 + word_width <= max_width {
            // Can fit with space
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_width;
        } else {
            // Start new line
            lines.push(current_line.clone());
            current_line = word.to_string();
            current_width = word_width;
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

    let parser = Parser::new_ext(input, options);
    
    let mut table_state = TableState::default();
    let mut in_table = false;
    
    for event in parser {
        match event {
            Event::Start(Tag::Table(_)) | Event::Start(Tag::TableHead) => {
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
                }
            }
            Event::End(TagEnd::TableCell) => {
                if in_table {
                    table_state.current_row.push(table_state.current_cell.clone());
                }
            }
            Event::End(TagEnd::TableRow) => {
                if in_table {
                    if table_state.in_head {
                        table_state.header = table_state.current_row.clone();
                    } else {
                        table_state.rows.push(table_state.current_row.clone());
                    }
                    table_state.current_row = Vec::new();
                }
            }
            Event::End(TagEnd::TableHead) => {
                // Store header row before switching to body rows
                if table_state.current_row.len() > 0 {
                    table_state.header = table_state.current_row.clone();
                    table_state.current_row = Vec::new();
                }
                table_state.in_head = false;
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                // Render the table
                let rendered = render_table_data(&table_state, available_width);
                return rendered;
            }
            _ => {}
        }
    }
    
    // No table found, return empty
    Vec::new()
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
            min_word_widths[i] = min_word_widths[i].max(longest_word_width(cell, max_unbroken_word_width));
        }
    }
    
    // Calculate column widths
    let mut column_widths = calculate_column_widths(
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
        let parts: Vec<String> = header_lines.iter()
            .map(|cell_lines| {
                cell_lines.get(line_idx).map(|s| s.as_str()).unwrap_or("")
            })
            .enumerate()
            .map(|(col_idx, text)| {
                let padded = pad_to_width(text, column_widths[col_idx]);
                format!("\x1b[1m{}\x1b[0m", padded) // Bold for header
            })
            .collect();
        lines.push(Line::from(Span::raw(format!("│ {} │", parts.join(" │ ")))));
    }
    
    // Separator
    lines.push(make_separator_line(&column_widths));
    
    // Body rows
    for row in &state.rows {
        let cell_lines = wrap_cell_rows(row, &column_widths);
        let row_line_count = cell_lines.iter().map(|l| l.len()).max().unwrap_or(1);
        
        for line_idx in 0..row_line_count {
            let parts: Vec<String> = cell_lines.iter()
                .map(|cell_lines| {
                    cell_lines.get(line_idx).map(|s| s.as_str()).unwrap_or("")
                })
                .enumerate()
                .map(|(col_idx, text)| pad_to_width(text, column_widths[col_idx]))
                .collect();
            lines.push(Line::from(Span::raw(format!("│ {} │", parts.join(" │ ")))));
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
            let total_weight: usize = min_word_widths.iter()
                .map(|w| w.saturating_sub(1))
                .sum();
            
            let growth: Vec<usize> = min_word_widths.iter()
                .map(|&width| {
                    let weight = width.saturating_sub(1);
                    if total_weight > 0 {
                        (weight * remaining) / total_weight
                    } else {
                        0
                    }
                })
                .collect();
            
            for i in 0..num_cols {
                min_column_widths[i] += growth.get(i).copied().unwrap_or(0);
            }
            
            let allocated: usize = growth.iter().sum();
            let mut leftover = remaining.saturating_sub(allocated);
            
            for i in 0..num_cols {
                if leftover > 0 {
                    min_column_widths[i] += 1;
                    leftover -= 1;
                }
            }
        }
    }
    
    // Check if natural widths fit
    let total_natural: usize = natural_widths.iter().sum();
    let total_natural_with_border = total_natural + num_cols - 1;
    
    if total_natural_with_border <= available_for_cells {
        // Everything fits naturally
        return natural_widths.iter()
            .enumerate()
            .map(|(i, &w)| w.max(min_column_widths[i]))
            .collect();
    }
    
    // Need to shrink
    let extra_width = available_for_cells.saturating_sub(min_column_widths.iter().sum::<usize>() + num_cols - 1);
    let total_grow_potential: usize = natural_widths.iter()
        .enumerate()
        .map(|(i, &w)| w.saturating_sub(min_column_widths[i]))
        .sum();
    
    let mut column_widths: Vec<usize> = min_column_widths.iter()
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
    let mut remaining = available_for_cells - num_cols - allocated;
    
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
    cells.iter()
        .enumerate()
        .map(|(i, text)| wrap_text(text, widths[i]))
        .collect()
}

fn pad_to_width(text: &str, width: usize) -> String {
    let text_width = UnicodeWidthStr::width(text);
    if text_width >= width {
        // Truncate
        let mut result = String::new();
        let mut current_width = 0usize;
        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1);
            if current_width + ch_width > width {
                break;
            }
            result.push(ch);
            current_width += ch_width;
        }
        result
    } else {
        format!("{:<width$}", text, width = width)
    }
}

fn make_border_line(widths: &[usize], left: char, mid: char, right: char) -> Line<'static> {
    // +2 accounts for the spaces we render around each cell: `│ {cell} │`
    let parts: Vec<String> = widths
        .iter()
        .map(|w| "─".repeat(w.saturating_add(2)))
        .collect();

    let mut s = String::new();
    s.push(left);
    for (i, part) in parts.iter().enumerate() {
        s.push_str(part);
        if i + 1 < parts.len() {
            s.push(mid);
        }
    }
    s.push(right);
    Line::from(Span::raw(s))
}

fn make_separator_line(widths: &[usize]) -> Line<'static> {
    let parts: Vec<String> = widths
        .iter()
        .map(|w| "─".repeat(w.saturating_add(2)))
        .collect();

    let mut s = String::new();
    s.push('├');
    for (i, part) in parts.iter().enumerate() {
        s.push_str(part);
        if i + 1 < parts.len() {
            s.push('┼');
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
        lines.push(Line::from(Span::raw(format!("\x1b[1m{}\x1b[0m", padded))));
    }
    
    for row in rows {
        let parts: Vec<String> = row.iter()
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