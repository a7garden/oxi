# Progress

## Status
In Progress

## Tasks

### Fix 1: app.rs에서 ToolExecutionStart/End을 UiEvent로 매핑 ✅ DONE
- **File**: `oxi-cli/src/tui/app.rs` (lines ~593-604)
- **What**: Added two new match arms to the event forwarder for `AgentEvent::ToolExecutionStart` → `UiEvent::ToolCall` and `AgentEvent::ToolExecutionEnd` → `UiEvent::ToolResult`
- **Verification**: `cargo check -p oxi-cli` passes (no new errors/warnings)

## Files Changed
- `oxi-cli/src/tui/app.rs` — Added ToolExecutionStart/End mapping in event forwarder match

## Notes
- Existing `AgentEvent::ToolStart`, `AgentEvent::ToolComplete`, `AgentEvent::ToolCall`, `AgentEvent::ToolError` mappings preserved
- ToolExecutionEnd result content truncated to 500 chars (consistent with ToolComplete mapping)
