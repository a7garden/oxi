# Progress

## Status
Completed

## Tasks

### Port interactive UI selector components from pi-mono to Rust
All 8 selector components have been ported and added to `oxi-cli/src/tui_components.rs`.

1. **ConfigSelector** - Config file/resource selection with hierarchical display (groups → subgroups → items), fuzzy search filtering, enable/disable toggle support, and scroll indicator.
2. **ModelSelectorEnhanced** - Enhanced model selection with provider badges, scope switching (all/scoped), fuzzy search filtering, current model highlighting, and detail display.
3. **SessionSelectorEnhanced** - Session list with scope (current folder/all), sort mode (recent/threaded/fuzzy), name filter (all/named), relative timestamps, working directory display, branch indicators, and labels.
4. **SettingsSelector** - Settings key-value selector with descriptions, fuzzy search over labels/descriptions/IDs, value cycling, and scroll support. Includes `SettingsConfig` struct for building from app settings.
5. **ThemeSelector** - Theme name selection with fuzzy search, current theme indicator, color preview, and selection highlighting.
6. **ThinkingSelector** - Thinking level selection (off/minimal/low/medium/high/xhigh) with descriptions for each level and current level indicator.
7. **TreeSelector** - Session tree navigation with ASCII tree connectors (├─/└─), filter modes (default/no-tools/user-only/labeled-only/all), active path highlighting, search filtering, and node type icons (user/tool/assistant).
8. **ScopedModelsSelector** - Scoped models cycling display with enable/disable toggle, provider toggle, reorder (up/down), enable all/clear all, fuzzy search, unsaved change tracking, and detail display.

### Additional changes (selector components):
- Added `ThinkingLevel` enum with `Default` derive, `as_str()`, `description()`, `all()`, and `from_str()` methods.
- Added `fuzzy_score()` and `fuzzy_filter_indices()` utility functions for fuzzy matching across all selectors.
- Enhanced existing `truncate_str()` function to handle ANSI escape codes (was previously breaking on styled text).
- Added helper types: `ModelScope`, `SessionScope`, `SessionSortMode`, `SessionNameFilter`, `TreeFilterMode`, `ResourceType`, etc.

### Port interactive message components from pi-mono to Rust
All 10 message components have been ported and added to `oxi-cli/src/tui_components.rs`.

1. **UserMessageRenderer** - Renders user messages with optional image indicators and OSC 133 prompt markers. Supports background styling and text wrapping.
2. **UserMessageSelector** - Select/edit previous user messages with scrolling, wrapping navigation, and visual selection cursor. Includes header with "Fork from Message" label.
3. **SkillInvocationMessage** - Renders skill invocation blocks with collapsed/expanded state. Collapsed shows single line with skill name; expanded shows full content with markdown rendering.
4. **DiffRenderer** - Renders unified diffs with color (red/green) and optional intra-line word-level change highlighting. Supports context lines, hunk headers, and tab normalization.
5. **KeybindingHints** - Renders keyboard shortcut lists with dim keys and muted descriptions. Supports both line-per-hint and inline layout.
6. **FooterComponentData** - Enhanced footer with model name, provider, thinking level, session name, git branch, pwd, token stats (↑↓RW), cost with subscription indicator, context window with color coding (green/yellow/red), and extension statuses.
7. **VisualTruncate** - Truncates text to terminal width with line wrapping. Returns visual lines from the end and a skipped count for "...N lines hidden" display.
8. **ShowImagesSelector** - Toggle image display with Yes/No options, navigation, and selection rendering.
9. **CountdownTimer** - Countdown display for timeouts with tick-based progression, expiry detection, and optional progress bar rendering.
10. **ansi module** - Internal ANSI color helper module with 20+ color functions (red, green, blue, yellow, cyan, magenta, bold, italic, dim, inverse, diff colors, user message colors, skill colors, thinking colors, etc.).

### Supporting additions:
- `ParsedSkillBlock` struct for skill block data.
- `UserMessageItem` struct for message selector entries.
- `KeyHint` struct for keybinding hint display.
- `ShowImagesOption` struct for image toggle options.
- `VisualTruncateResult` struct for truncation results.
- `FooterComponentData` struct with all status fields from pi-mono footer.
- `visible_len()` function that correctly measures string width ignoring ANSI escape codes and OSC sequences.
- `is_wide_char()` and `truncate_str()` functions for CJK/emoji-aware truncation.
- `split_words()`, `common_prefix_len()`, `common_suffix_len()` for word-level diff computation.
- `render_intra_line_diff()` for inverse-highlighted intra-line changes.
- Comprehensive test suite (~30 tests) covering all new components.

## Files Changed
- `oxi-cli/src/tui_components.rs` — Added ~2000 lines of message component implementations with data models, ANSI styling, and rendering logic.

## Notes
- All components follow the same pattern: struct with state fields and `render()` / `render(width)` returning `String` or `Vec<String>`.
- No errors introduced in `tui_components.rs` — `cargo check -p oxi-cli` shows only pre-existing errors in `interactive.rs` (missing `imode`, `libc`, `KeyCode::Escape`, `KeyModifiers::CTRL`, etc.).
- Full TUI integration (event handling, component lifecycle) is a separate step — these components focus on data model and rendering logic as specified.
