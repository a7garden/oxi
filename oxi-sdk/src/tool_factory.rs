//! Tool factories for common tool sets

// This will use the pattern from pi's createCodingTools({cwd})
// but adapted for Rust's ownership model.

use std::path::Path;
use std::sync::Arc;

use oxi_agent::{
    ToolRegistry, AgentTool,
    tools::{ReadTool, WriteTool, EditTool, LsTool},
};

/// Create the standard coding tools: read, write, edit, ls
pub fn coding_tools(cwd: &Path) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::new(cwd.to_path_buf()));
    registry.register(WriteTool::new(cwd.to_path_buf()));
    registry.register(EditTool::new(cwd.to_path_buf()));
    registry.register(LsTool::new(cwd.to_path_buf()));
    Arc::new(registry)
}

/// Create read-only tools: read, ls
pub fn readonly_tools(cwd: &Path) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::new(cwd.to_path_buf()));
    registry.register(LsTool::new(cwd.to_path_buf()));
    Arc::new(registry)
}