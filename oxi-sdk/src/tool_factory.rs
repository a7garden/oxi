//! Tool factories for common tool sets

use std::sync::Arc;

use oxi_agent::{
    ToolRegistry,
    tools::{ReadTool, WriteTool, EditTool, LsTool},
};

/// Create the standard coding tools: read, write, edit, ls
pub fn coding_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::new());
    registry.register(WriteTool::new());
    registry.register(EditTool::new());
    registry.register(LsTool::new());
    Arc::new(registry)
}

/// Create read-only tools: read, ls
pub fn readonly_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::new());
    registry.register(LsTool::new());
    Arc::new(registry)
}