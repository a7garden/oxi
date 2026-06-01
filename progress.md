# Progress

## Status
In Progress

## Tasks
- [x] #3: A2A message logging implementation
- [x] #2: oxios-web Chat API tool_calls 연결
- [x] #4: oxios-cli 모델/페르소나 전환 구현

## Files Changed

### #3: A2A message logging
- `crates/oxios-kernel/src/a2a/mod.rs` — Added `A2AMessageLogEntry` struct, `message_log` field to `A2AProtocol`, `append_log`/`get_message_log` methods, logging in `send_message` and `execute_delegation`
- `crates/oxios-kernel/src/kernel_handle/a2a_api.rs` — Added `get_message_log` facade method
- `surface/oxios-web/src/routes/a2a.rs` — Implemented `handle_a2a_messages` returning real log entries, updated `handle_a2a_topology` to derive edges from log

### #2: Chat API tool_calls 연결
- `crates/oxios-ouroboros/src/protocol.rs` — Added `ToolCallRecord` struct and `tool_calls: Vec<ToolCallRecord>` field to `ExecutionResult`
- `crates/oxios-ouroboros/src/lib.rs` — Exported `ToolCallRecord`
- `crates/oxios-ouroboros/src/ouroboros_engine.rs` — Added `tool_calls: vec![]` to placeholder `ExecutionResult`
- `crates/oxios-kernel/src/agent_runtime.rs` — Updated `run_agent` to return trajectory_steps; mapped trajectory_steps → `ToolCallRecord` in `ExecutionResult`
- `crates/oxios-kernel/src/supervisor.rs` — Added `tool_calls: vec![]` to all 3 `ExecutionResult` construction sites
- `crates/oxios-kernel/src/orchestrator.rs` — Added `tool_calls` field to `OrchestrationResult`; propagated `final_result.tool_calls` in main execution path
- `crates/oxios-gateway/src/gateway.rs` — Serialized `tool_calls` JSON into `OutgoingMessage.metadata["tool_calls"]` in `dispatch()`
- `surface/oxios-web/src/routes/chat.rs` — No changes needed (already reads `msg.metadata.get("tool_calls")`)

## Notes
- Full `cargo check --workspace` passes with no new errors
- chat.rs TODO at line ~570 still reads tool_calls from metadata (now populated by gateway)
- Session tool-calls endpoint (`GET /api/sessions/{id}/tool-calls`) still returns empty array — would need session persistence of tool_calls to populate

### #4: oxios-cli 모델/페르소나 전환 구현
- `crates/oxios-gateway/src/meta.rs` — Added `ACTION`, `MODEL_ID`, `PERSONA_ID` metadata key constants
- `crates/oxios-gateway/src/gateway.rs` — Added `engine_api` and `persona_api` Optional Arc fields to `Gateway`, added `with_apis()` constructor, added action-based routing in `dispatch()` (checks `metadata["action"]` before orchestrator routing), added `dispatch_switch_model()` and `dispatch_switch_persona()` handler methods
- `channels/oxios-cli/src/channel.rs` — Added `send_switch_model()` and `send_switch_persona()` methods to `CliChannelHandle` that send `IncomingMessage` with action metadata
- `channels/oxios-cli/src/interactive.rs` — Wired `MetaCommand::Model(Some(name))` and `MetaCommand::Persona(Some(name))` to call the new handle methods, removing the TODO comments
- `src/kernel.rs` — Updated `Kernel::new()` to use `Gateway::with_apis()` with Arc-wrapped EngineApi and PersonaApi
