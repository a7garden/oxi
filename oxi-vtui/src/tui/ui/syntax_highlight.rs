use ratatui::text::Line;

pub struct SyntaxHighlighter;
impl SyntaxHighlighter {
    pub fn highlight_lines(&self, _code: &str, _language: &str) -> Vec<Line<'static>> {
        vec![]
    }
}

pub fn highlight_line_to_anstyle_segments(_line: &str, _lang: &str) -> Vec<(anstyle::Style, String)> {
    vec![]
}

pub fn get_active_syntax_theme() -> anstyle::Style {
    anstyle::Style::default()
}
pub fn find_syntax_by_token(_token: &str) -> Option<&'static syntect::parsing::SyntaxReference> { None }
