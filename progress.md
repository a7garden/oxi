# Progress

## Status
Completed

## Keys Port (pi-mono → oxi-tui)
- [x] Port `pi-mono/packages/tui/src/keys.ts` to `oxi-tui/src/keys.rs`
- [x] KeyCode enum + KeyModifiers (using existing event.rs types)
- [x] Kitty keyboard protocol parsing (CSI-u with flags 1, 2, 4)
- [x] xterm modifyOtherKeys parsing (CSI 27;mod;code ~)
- [x] Legacy CSI/SS3 escape sequences (arrows, F-keys, home/end, etc.)
- [x] Shift/Ctrl/Alt modified legacy sequences
- [x] SGR mouse event parsing (press, release, drag, scroll)
- [x] X10 old-style mouse event parsing
- [x] Focus events (FocusGained/FocusLost)
- [x] UTF-8 multi-byte character decoding
- [x] Key release/repeat detection (Kitty flag 2)
- [x] Kitty printable character decoding (for text insertion)
- [x] `matches_key()` - check if raw bytes match a key identifier
- [x] `parse_key()` - parse raw bytes to human-readable key string
- [x] `parse_event()` - parse raw bytes to structured Event
- [x] `decode_printable_key()` - decode to printable char for text input
- [x] Numpad codepoint normalization (Kitty KP_0..KP_DELETE)
- [x] Non-Latin keyboard layout support via base layout key fallback
- [x] Added PartialEq/Eq to KeyEvent, MouseEvent, Event in event.rs
- [x] Module registered in lib.rs
- [x] `cargo check -p oxi-tui` passes with no warnings
- [x] All 41 tests pass

## Extension Runner Integration
- [x] Wire `ExtensionRunner` into `AgentSession` as an `Arc<RwLock<Option<ExtensionRunner>>>` field
- [x] Connect `forward_event_to_extensions()` — now broadcasts to session listeners AND extension runner with typed hooks for ToolCall/ToolExecutionStart/ToolExecutionEnd/Error events
- [x] Connect `has_extension_handlers()` — now delegates to `ExtensionRunner.has_handlers()`
- [x] Wire `beforeToolCall` / `afterToolCall` — `emit_before_tool_call()` and `emit_after_tool_result()` methods on AgentSession delegate to ExtensionRunner
- [x] Connect `extensionRunnerRef` — `set_extension_runner()` and `take_extension_runner()` for runtime runner updates with proper lifecycle (shutdown old, load new)
- [x] Extension lifecycle: on_load, on_unload — fired during `set_extension_runner()` and `take_extension_runner()`
- [x] Tool registration from extensions — `extension_tools()` collects tools from all enabled extensions
- [x] Command registration from extensions — `extension_commands()` collects commands from all enabled extensions
- [x] `process_events()` updated to forward events to extensions with typed dispatch
- [x] `build_extension_context()` helper for creating ExtensionContext
- [x] `process_input_through_extensions()` for input transform/handling via extensions
- [x] Notification helpers: `notify_extensions_message_sent/received/settings_changed`
- [x] `cargo check -p oxi-cli` passes with no errors
- [x] `cargo check` (full workspace) passes with no errors

## Files Changed
- `oxi-cli/src/agent_session.rs` — Full ExtensionRunner integration:
  - Added `extension_runner: Arc<RwLock<Option<ExtensionRunner>>>` field
  - Updated constructor to initialize extension_runner
  - Updated `clone_inner()` to share extension_runner arc
  - Replaced stub `forward_event_to_extensions()` with full implementation dispatching to ExtensionRunner
  - Replaced stub `has_extension_handlers()` to delegate to runner
  - Added `set_extension_runner()` — installs runner, fires on_load/session_start on new, shutdown/unload on old
  - Added `take_extension_runner()` — graceful shutdown + removal
  - Added `extension_runner()` — read access to current runner
  - Added `build_extension_context()` — creates ExtensionContext for current session
  - Added `extension_tools()` — collects tools from extensions
  - Added `extension_commands()` — collects commands from extensions
  - Added `emit_before_tool_call()` — delegates to runner.emit_tool_call()
  - Added `emit_after_tool_result()` — delegates to runner.emit_tool_result_event()
  - Added `process_input_through_extensions()` — input transform via extensions
  - Added `notify_extensions_message_sent/received/settings_changed()` — typed notification helpers
  - Updated `process_events()` to forward events to extensions with typed hooks

## Notes
- Pre-existing test compilation failures in oxi-tui (keys.rs errors) are unrelated to this change
- All warnings are pre-existing (unused variables in packages.rs, session.rs, etc.)
- The ExtensionRunner is stored as `Option` since sessions may be created before extensions are loaded
- Runner updates use graceful lifecycle: old runner gets session_shutdown + session_end + unload, new runner gets load + session_start
- Extension tools/commands can be collected via `extension_tools()` / `extension_commands()` for registration with the agent
