# Progress

## Status
Completed

## Tasks
- [x] Fix print mode to only extract Text blocks (not Thinking) with GLM fallback
- [x] Improve TUI thinking rendering with tui-markdown + italic style
- [x] Add state-based border colors for ToolBox (yellow/green/red)
- [x] Update measurement for Thinking to use md_lines
- [x] Fix ZAI provider handling in openai.rs

## Files Changed
- `oxi-cli/src/print_mode.rs` — MessageUpdate handler + extract_text_from_message: Text-only with GLM fallback
- `oxi-tui/src/widgets/chat.rs` — Thinking: tui-markdown + italic; ToolBox: state-based border colors
- `oxi-ai/src/providers/openai.rs` — ZAI-specific params, dual-map tool call lookup, incremental JSON parsing

## Notes
- Print mode now matches pi behavior: Text blocks only, with GLM fallback when no Text blocks exist
- Thinking blocks in TUI now rendered via tui-markdown with italic style for distinct visual appearance
- ToolBox borders: yellow for pending/executing, green for success, red for error
- All 9 print_mode tests pass, all 11 chat widget tests pass
- Release build succeeds
- ZAI changes:
  - Added `is_zai()` detection (provider contains "zai" or base_url contains "api.z.ai")
  - When ZAI + reasoning: sends `enable_thinking: true/false` based on thinking_level presence
  - When ZAI + tools: sends `tool_stream: true`
  - Dual-map tool call lookup in scan() closure: index-based primary, ID-based fallback
  - Uses `parse_streaming_json` for incremental tool call argument parsing
  - All 78 openai tests pass, release build succeeds
