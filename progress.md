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

### Port miscellaneous interactive components from pi-mono to Rust
All 5 interactive components have been ported and added to `oxi-cli/src/tui_components.rs`.

1. **ArminComponent** - XBM art rendering with half-block characters (█▀▄). Renders a 31×36 pixel XBM image as 18 rows of Unicode block characters with accent color styling and "ARMIN SAYS HI" message. Includes caching support.
2. **DaxnutsComponent** - 32×32 RGB image rendering with true-color (24-bit) half-block characters. Parses hex-encoded pixel data and renders using `▄` with per-pixel foreground/background colors. Includes centered text with "Powered by daxnuts" tribute.
3. **DynamicBorder** - Dynamic width-adjustable horizontal border using `─` box-drawing characters with configurable color styles (Accent, Muted, Success, Error, Custom).
4. **EarendilAnnouncement** - Framed announcement/notification display with title, body lines, optional link, and styled borders. Auto-detects URLs for link styling.
5. **ToolExecutionDisplay** - Enhanced tool execution rendering with timing (elapsed time tracking), progress indicators, collapsible arguments/output, partial/streaming args support, expand/collapse toggle, and rich status display with ANSI colors.

### Additional changes:
- Added `center_ansi()` utility for centering strings containing ANSI escape codes.
- Added `BorderStyle` enum for configurable border styling.
- Added 22 new tests covering all 5 new components.
- Fixed `strip_ansi` usage in tests to use `.chars().count()` instead of `.len()` for proper Unicode character counting.

### Port extension-related interactive components from pi-mono to Rust
All 8 extension-related UI components have been ported and added to `oxi-cli/src/tui_components.rs`.

1. **ExtensionEditor** - Multi-line editor for extension-provided input with title, prefill support, external editor detection ($VISUAL/$EDITOR), cursor-aware input/backspace, and bordered rendering with keybinding hints.
2. **ExtensionInput** - Single-line text input with title, optional countdown timeout (tick-based), cursor-aware input/backspace, and bordered rendering.
3. **ExtensionSelector** - Generic option selector with up/down navigation, selected option tracking, optional countdown timeout, and bordered rendering with navigation hints.
4. **CustomEditor** - Custom editor buffer for extension-registered shortcuts with action registration (deduplication), autocomplete state tracking, empty detection (for Ctrl+D), and simple text rendering.
5. **CustomMessageComponent** - Custom message type rendering with styled type label (`[customType]`), expand/collapse toggle, purple background styling, line preview in collapsed mode, and multi-line display in expanded mode.
6. **ProviderLoginDialog** - Provider login dialog with state machine phases (Init → ShowAuth → ManualInput → Prompt → Info → Waiting → Completed), cursor-aware input for ManualInput/Prompt phases, progress message appending, success/failure completion, and styled rendering per phase.
7. **OAuthSelector** - OAuth provider selection list with login/logout modes, fuzzy search filtering, scroll offset management, config status indicators (Configured/PartiallyConfigured/Unconfigured), and empty state messages.
8. **BorderedLoader** - Loading indicator with border, spinner animation (⠋⠙⠹⠸ cycle), cancellable mode with cancel hint, and non-cancellable mode.

### Supporting types added:
- `LoginDialogPhase` enum with 7 variants for login flow state machine
- `AuthProviderInfo` struct with id, name, and `AuthType` (OAuth/ApiKey)
- `ProviderConfigStatus` enum (Unconfigured/Configured/PartiallyConfigured)
- `OAuthSelectorMode` enum (Login/Logout)

### Tests added:
- 38 new tests covering all 8 extension components.

## Files Changed
- `oxi-cli/src/tui_components.rs` — Added ~700 lines of interactive component implementations plus ~250 lines of tests (selectors task). Added ~500 lines of extension component implementations plus ~350 lines of tests (extension task).

## Notes
- All 8 extension components follow the same pattern: struct with state fields and `render() -> Vec<String>`.
- `cargo check -p oxi-cli` passes cleanly.
- All 38 new extension component tests pass.
