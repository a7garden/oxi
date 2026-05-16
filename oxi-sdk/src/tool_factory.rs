//! Tool factories for common tool sets

use std::path::Path;
use std::sync::Arc;

use oxi_agent::{
    ToolRegistry,
    tools::{ReadTool, WriteTool, EditTool, LsTool},
};

/// Create the standard coding tools: read, write, edit, ls
pub fn coding_tools(cwd: &Path) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::with_cwd(cwd.to_path_buf()));
    registry.register(WriteTool::with_cwd(cwd.to_path_buf()));
    registry.register(EditTool::with_cwd(cwd.to_path_buf()));
    registry.register(LsTool::with_cwd(cwd.to_path_buf()));
    Arc::new(registry)
}

/// Create read-only tools: read, ls
pub fn readonly_tools(cwd: &Path) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::with_cwd(cwd.to_path_buf()));
    registry.register(LsTool::with_cwd(cwd.to_path_buf()));
    Arc::new(registry)
}
