# Progress

## Status
In Progress

## Tasks
- [x] Fix #21: Add `#[non_exhaustive]` to key public enums

## Files Changed
- oxi-ai/src/types.rs — `Api`, `StopReason`, `ThinkingLevel`, `InputModality`
- oxi-ai/src/providers/event.rs — `ProviderEvent`
- oxi-agent/src/events.rs — `AgentEvent`

## Notes
- All enums compiled cleanly with `#[non_exhaustive]` — no wildcard arm fixes needed.
- `cargo check --workspace` passes with zero errors.
