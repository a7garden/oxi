//! Conversion from ratatui styled lines to ANSI tape rows.

use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

use crate::render::terminal::TerminalCapabilities;

fn color_sgr(color: Color, background: bool, true_color: bool) -> Option<String> {
    let prefix = if background { 48 } else { 38 };
    match color {
        Color::Reset => None,
        Color::Black => Some(if background { "40" } else { "30" }.into()),
        Color::Red => Some(if background { "41" } else { "31" }.into()),
        Color::Green => Some(if background { "42" } else { "32" }.into()),
        Color::Yellow => Some(if background { "43" } else { "33" }.into()),
        Color::Blue => Some(if background { "44" } else { "34" }.into()),
        Color::Magenta => Some(if background { "45" } else { "35" }.into()),
        Color::Cyan => Some(if background { "46" } else { "36" }.into()),
        Color::Gray => Some(if background { "47" } else { "37" }.into()),
        Color::DarkGray => Some(if background { "100" } else { "90" }.into()),
        Color::LightRed => Some(if background { "101" } else { "91" }.into()),
        Color::LightGreen => Some(if background { "102" } else { "92" }.into()),
        Color::LightYellow => Some(if background { "103" } else { "93" }.into()),
        Color::LightBlue => Some(if background { "104" } else { "94" }.into()),
        Color::LightMagenta => Some(if background { "105" } else { "95" }.into()),
        Color::LightCyan => Some(if background { "106" } else { "96" }.into()),
        Color::White => Some(if background { "107" } else { "97" }.into()),
        Color::Indexed(index) => Some(format!("{prefix};5;{index}")),
        Color::Rgb(r, g, b) if true_color => Some(format!("{prefix};2;{r};{g};{b}")),
        Color::Rgb(r, g, b) => {
            let index = crate::render::color_level::rgb_to_ansi256(r, g, b);
            Some(format!("{prefix};5;{index}"))
        }
    }
}

fn style_sgr(style: Style, caps: &TerminalCapabilities) -> String {
    let mut codes = Vec::new();
    let modifiers = style.add_modifier;
    if modifiers.contains(Modifier::BOLD) {
        codes.push("1".into());
    }
    if modifiers.contains(Modifier::DIM) {
        codes.push("2".into());
    }
    if modifiers.contains(Modifier::ITALIC) {
        codes.push("3".into());
    }
    if modifiers.contains(Modifier::UNDERLINED) {
        codes.push("4".into());
    }
    if modifiers.contains(Modifier::SLOW_BLINK) {
        codes.push("5".into());
    }
    if modifiers.contains(Modifier::RAPID_BLINK) {
        codes.push("6".into());
    }
    if modifiers.contains(Modifier::REVERSED) {
        codes.push("7".into());
    }
    if modifiers.contains(Modifier::HIDDEN) {
        codes.push("8".into());
    }
    if modifiers.contains(Modifier::CROSSED_OUT) {
        codes.push("9".into());
    }
    if let Some(fg) = style.fg.and_then(|c| color_sgr(c, false, caps.true_color)) {
        codes.push(fg);
    }
    if let Some(bg) = style.bg.and_then(|c| color_sgr(c, true, caps.true_color)) {
        codes.push(bg);
    }
    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

/// Serialize one styled ratatui line into an ANSI tape row.
#[must_use]
pub fn styled_line_to_ansi(line: &Line<'_>, caps: &TerminalCapabilities) -> String {
    let mut out = String::new();
    let mut current = Style::default();
    for span in &line.spans {
        let next = line.style.patch(span.style);
        if next != current {
            out.push_str("\x1b[0m");
            out.push_str(&style_sgr(next, caps));
            current = next;
        }
        out.push_str(span.content.as_ref());
    }
    if current != Style::default() {
        out.push_str("\x1b[0m");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    #[test]
    fn equal_adjacent_styles_do_not_repeat_sgr() {
        let line = Line::from(vec![
            Span::styled("a", Style::default().fg(Color::Red)),
            Span::styled("b", Style::default().fg(Color::Red)),
        ]);
        let ansi = styled_line_to_ansi(&line, &TerminalCapabilities::default());
        assert_eq!(ansi.matches("\x1b[31m").count(), 1);
        assert!(ansi.contains("ab"));
        assert!(ansi.ends_with("\x1b[0m"));
    }

    #[test]
    fn rgb_downgrades_without_true_color() {
        let line = Line::from(Span::styled(
            "x",
            Style::default().fg(Color::Rgb(255, 0, 0)),
        ));
        let ansi = styled_line_to_ansi(&line, &TerminalCapabilities::default());
        assert!(ansi.contains("38;5;196"));
    }

    #[test]
    fn true_color_is_preserved() {
        let mut caps = TerminalCapabilities::default();
        caps.true_color = true;
        let line = Line::from(Span::styled("한", Style::default().fg(Color::Rgb(1, 2, 3))));
        let ansi = styled_line_to_ansi(&line, &caps);
        assert!(ansi.contains("38;2;1;2;3"));
        assert!(ansi.contains('한'));
    }
}
