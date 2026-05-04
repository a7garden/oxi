# Progress

## Status
In Progress

## Tasks

### Port pi-mono's agent-session.ts core to Rust — DONE

Created `oxi-cli/src/agent_session.rs` — the missing link between `oxi_agent::Agent` and `interactive.rs`.

#### What was ported (from pi-mono `agent-session.ts`, 3108 lines):

1. **AgentSession struct** — session wrapper around Agent
   - Holds: agent, settings, session_manager, scoped_models, event listeners
   - Methods: `prompt()`, `steer()`, `follow_up()`, `abort()`
   - Properties: `model_id()`, `thinking_level()`, `is_streaming()`, `messages()`, `session_id()`

2. **Model management**
   - `set_model()` — switch model mid-conversation, persists to session + settings
   - `cycle_model()` — cycle through scoped models or default list, forward/backward
   - `ScopedModel` struct for --models flag cycling

3. **Thinking level management**
   - `set_thinking_level()` — change level, persist to session
   - `cycle_thinking_level()` — cycle through None/Minimal/Standard/Thorough

4. **Auto-compaction** (integrated)
   - `compact()` — manual compaction trigger with event emission
   - `check_auto_compaction()` — automatic threshold-based check after responses
   - `run_compaction()` — uses agent's CompactionManager
   - `abort_compaction()` — cancel in-progress compaction
   - CompactionReason enum: Manual, Threshold, Overflow
   - CompactionResult with summary, tokens_before, details

5. **Auto-retry** (integrated)
   - `check_auto_retry()` — detect retryable errors (429, 500-504, overloaded, etc.)
   - Exponential backoff with configurable settings
   - `abort_retry()`, `wait_for_retry()`
   - SessionEvent::AutoRetryStart/AutoRetryEnd events

6. **Session persistence**
   - `persist_session()` — sync agent state to SessionManager on each event
   - Auto-save on prompt completion via `process_events()`
   - Handles User, Assistant, ToolResult message types

7. **Event system**
   - `SessionEvent` enum — extends AgentEvent with session-level events
   - `subscribe()` — listener registration with RAII guard (SessionListenerGuard)
   - `subscribe_channel()` — convenience for async event consumption
   - QueueUpdate, CompactionStart/End, AutoRetryStart/End, SessionInfoChanged, ThinkingLevelChanged

8. **Steering/follow-up queues**
   - `steer()` / `follow_up()` — queue messages during streaming
   - `clear_queue()` — drain and return queued messages
   - Automatic follow-up processing after agent completion

9. **Streaming support**
   - `prompt_streaming()` — returns event channel, uses LocalSet for !Send agent

10. **Extension integration hooks**
    - `forward_event_to_extensions()` — stub for ExtensionRunner wiring
    - `has_extension_handlers()` — check for registered handlers

11. **Utility types**
    - `AgentSessionHandle` — cheaply-clonable Arc handle
    - `PromptOptions`, `StreamingBehavior`, `InputSource`
    - `SessionStats`, `TokenStats`, `SessionRetrySettings`
    - `CycleDirection`, `ModelCycleResult`

#### Tests included:
- `test_is_retryable_error` — verifies retryable/non-retryable error patterns
- `test_default_model_list` — default model cycling list
- `test_session_retry_settings_default`
- `test_cycle_direction_default`
- `test_thinking_level_ordering` — cycle wraps correctly
- `test_scoped_model`
- `test_compaction_reason`
- `test_model_cycle_result`
- `test_session_stats_default`
- `test_streaming_behavior`
- `test_input_source_default`
- `test_prompt_options_default`

## Files Changed
- `oxi-cli/src/agent_session.rs` — NEW: ~1100 lines, core session abstraction
- `oxi-cli/src/lib.rs` — added `pub mod agent_session;`

## Notes
- `cargo check -p oxi-cli` passes for agent_session.rs (no errors from this module)
- Pre-existing errors in other modules (export.rs, compaction_utils.rs, session.rs) are unrelated
- The Agent struct's internal RwLock is !Send, so `prompt_streaming()` uses `spawn_blocking` + `LocalSet`
- `is_streaming()` returns `false` as Agent doesn't yet expose streaming state; TODO for future
- Extension integration is stubbed (`has_extension_handlers` returns false) pending full ExtensionRunner wiring
