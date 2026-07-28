//! Tool call block component — renders a tool invocation or result.
//!
//! Shows the tool name, arguments, and result in a formatted block.
//! The entire block is FINAL once the result arrives.

use super::super::component::{Component, LiveRegion, RenderResult};

/// A tool call block showing name, args, and optional result.
///
/// While the result is pending, the block has a live region (the "running..."
/// indicator). When the result arrives, the entire block becomes final.
pub struct ToolCallBlock {
    /// Tool name (e.g. "read", "bash").
    name: String,
    /// Short argument summary (e.g. file path, command).
    args_summary: String,
    /// Optional result text (None = still running).
    result: Option<String>,
    /// Pre-rendered lines.
    lines: Vec<String>,
    /// Cached hash.
    hash: u64,
}

impl ToolCallBlock {
    /// Create a tool call block that is still running.
    #[must_use]
    pub fn running(name: impl Into<String>, args_summary: impl Into<String>) -> Self {
        let name = name.into();
        let args_summary = args_summary.into();
        let lines = Self::build_lines(&name, &args_summary, None);
        let hash = RenderResult::new(lines.clone()).hash;
        Self {
            name,
            args_summary,
            result: None,
            lines,
            hash,
        }
    }

    /// Create a tool call block with a result.
    #[must_use]
    pub fn completed(
        name: impl Into<String>,
        args_summary: impl Into<String>,
        result: impl Into<String>,
    ) -> Self {
        let name = name.into();
        let args_summary = args_summary.into();
        let result = result.into();
        let lines = Self::build_lines(&name, &args_summary, Some(&result));
        let hash = RenderResult::new(lines.clone()).hash;
        Self {
            name,
            args_summary,
            result: Some(result),
            lines,
            hash,
        }
    }

    /// Set the result, making the block fully final.
    pub fn set_result(&mut self, result: impl Into<String>) {
        let result = result.into();
        self.result = Some(result);
        self.lines = Self::build_lines(&self.name, &self.args_summary, self.result.as_deref());
        self.hash = RenderResult::new(self.lines.clone()).hash;
    }

    /// Whether the tool call has a result.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        self.result.is_some()
    }

    fn build_lines(name: &str, args: &str, result: Option<&str>) -> Vec<String> {
        let mut lines = Vec::new();
        // Header line: ▸ tool_name(args)
        lines.push(format!("\x1b[36m▸ {name}\x1b[0m {args}"));
        match result {
            None => lines.push("\x1b[90m  ⠋ running...\x1b[0m".into()),
            Some(r) => {
                for line in r.lines() {
                    lines.push(format!("  {line}"));
                }
            }
        }
        lines
    }
}

impl Component for ToolCallBlock {
    fn render(&self, _width: u16) -> RenderResult {
        RenderResult {
            lines: self.lines.clone(),
            hash: self.hash,
        }
    }

    fn live_region(&self) -> LiveRegion {
        if self.result.is_some() {
            LiveRegion::None
        } else {
            // Header is final, "running..." is mutable
            LiveRegion::Mutable { start: 1 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_has_live_region() {
        let block = ToolCallBlock::running("read", "file.rs");
        assert_eq!(block.live_region(), LiveRegion::Mutable { start: 1 });
    }

    #[test]
    fn completed_has_no_live_region() {
        let block = ToolCallBlock::completed("bash", "ls", "file1\nfile2");
        assert_eq!(block.live_region(), LiveRegion::None);
    }

    #[test]
    fn running_renders_header_and_indicator() {
        let block = ToolCallBlock::running("read", "test.rs");
        let r = block.render(80);
        assert_eq!(r.lines.len(), 2);
        assert!(r.lines[0].contains("read"));
        assert!(r.lines[0].contains("test.rs"));
        assert!(r.lines[1].contains("running"));
    }

    #[test]
    fn completed_renders_result_lines() {
        let block = ToolCallBlock::completed("bash", "echo hi", "hi");
        let r = block.render(80);
        assert_eq!(r.lines.len(), 2); // header + result
        assert!(r.lines[1].contains("hi"));
    }

    #[test]
    fn set_result_promotes_to_final() {
        let mut block = ToolCallBlock::running("grep", "pattern");
        assert!(block.live_region() != LiveRegion::None);
        block.set_result("match found");
        assert_eq!(block.live_region(), LiveRegion::None);
        assert!(block.is_completed());
    }

    #[test]
    fn hash_changes_on_result() {
        let mut block = ToolCallBlock::running("ls", ".");
        let h1 = block.render(80).hash;
        block.set_result("file.txt");
        let h2 = block.render(80).hash;
        assert_ne!(h1, h2);
    }
}
