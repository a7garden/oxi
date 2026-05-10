# Progress

## Status
In Progress

## Tasks

### Fix 1: app.rs에서 ToolExecutionStart/End을 UiEvent로 매핑 ✅ DONE
- **File**: `oxi-cli/src/tui/app.rs` (lines ~593-604)
- **What**: Added two new match arms to the event forwarder for `AgentEvent::ToolExecutionStart` → `UiEvent::ToolCall` and `AgentEvent::ToolExecutionEnd` → `UiEvent::ToolResult`
- **Verification**: `cargo check -p oxi-cli` passes (no new errors/warnings)

### Fix 2: push_blocks에 area.width 전달, box_width 하드코딩 50 제거 ✅ DONE
- **File**: `oxi-tui/src/widgets/chat.rs`
- **What**:
  1. Added `area_width: u16` parameter to `push_blocks` function signature
  2. Added `let box_width = area_width;` inside push_blocks
  3. Replaced all 17 hardcoded `50` with `box_width` in block_header_line/block_body_line/block_footer_line/block_divider_line/block_truncate_line calls
  4. Updated both call sites (completed messages loop + streaming message) to pass `area.width`
- **Verification**: `cargo check -p oxi-tui` passes (no new errors)

## Files Changed
- `oxi-cli/src/tui/app.rs` — Added ToolExecutionStart/End mapping in event forwarder match
- `oxi-tui/src/widgets/chat.rs` — Added area_width param to push_blocks, replaced hardcoded 50 with dynamic box_width

## Notes
- Existing `AgentEvent::ToolStart`, `AgentEvent::ToolComplete`, `AgentEvent::ToolCall`, `AgentEvent::ToolError` mappings preserved
- ToolExecutionEnd result content truncated to 500 chars (consistent with ToolComplete mapping)
- All 17 occurrences of hardcoded 50 replaced with box_width (area.width from caller)
