# Task 4 Report

## Status

Complete.

## Commit

Pending creation at report-write time; see final returned SHA.

## Test summary

Bridge tests: 3/3 passed; workspace build, both required clippy invocations, and rustfmt check passed.

## Constructor / wiring decision

`AgentSession::new` is the single production constructor used by both runtime branches and test scaffolding, so the registry is initialized there rather than through a TUI-only `wire_hub` helper. The session directory is derived from the existing `SessionManager` file before `Self` construction, and `clone_inner` shares the same `Arc<HubRegistry>`.

Advisor registration remains in `set_advisor_enabled`, immediately after `build_advisor` succeeds, so both startup auto-enable and later slash-command enable follow the same path. `AdvisorRuntime` now carries an optional host-supplied transcript path with a getter as required; `build_advisor` derives the path from the same session file and reserved filename used by `AdvisorTranscriptRecorder`.

## Concerns

The v1 `is_main_session_stem` contract deliberately returns false. Correct exclusion therefore depends on scanning before the current main session file is first written, as specified by the plan.

## Fix round 1

### What changed

The reviewer flagged a P1 defect: `register_persisted_subagents` misclassified every main session file in the shared per-CWD `sessions/--{cwd}--/` directory as a parked subagent because `is_main_session_stem` returned `false` unconditionally. The spec called for caller-side unregister as the "least-invasive" fix, but a closer read of the test plan revealed that the bridge unit test calls `register_persisted_subagents` directly (no `AgentSession::new` involvement), so the unregister alone cannot make the test pass — the heuristic is the load-bearing fix.

Two changes:

1. **`is_main_session_stem` now matches the real production format.** Added a `LazyLock<Regex>` (`^[0-9]{4}.*_[0-9a-f]{8}$`) keyed off the actual `SessionManager` naming convention (`oxi-cli/src/store/session.rs:2166-2170`): `file_timestamp` is RFC3339 with `:` `.` `T` `-` `+` replaced by `-`; `short_id` is the first 8 hex chars of the session UUID. The full stem is `{file_timestamp}_{short_id}`. This excludes the current session file AND every prior session file in the directory — addressing the full "10+ main files" defect, not just the current one.

2. **Caller-side unregister retained as defense-in-depth.** `AgentSession::new` calls `hub.unregister(own_stem)` after the scan, using `SessionManager::get_session_file().file_stem()`. The unregister is idempotent (no-op when the file hasn't been flushed yet) and protects against any future heuristic regression. Kept per the spec's preference for the least-invasive caller-side fix.

### Test fixture swap

The plan's illustrative `01HXY.jsonl` is a ULID (base-32) and does NOT match the real `{rfc3339}_{8hex}` production format. The test fixture was upgraded to a realistic stem:
`2026-07-29-14-30-00-00-00_a1b2c3d4.jsonl`. This better reflects production data and would have caught the P1 bug had it been present in the original test.

### Test count

The bridge test module now has **4 tests** (was 3):
- `registers_subagent_jsonl_excluding_main_and_advisor` — restored plan fixture with realistic main file
- `main_session_stem_heuristic_matches_real_pattern` — NEW: explicit unit test for the heuristic (positive + negative cases)
- `empty_dir_registers_nothing`
- `missing_dir_is_noop`

### Covering tests run

```
$ cargo nextest run -p oxi-cli app::agent_hub_bridge::tests
   PASS [   0.023s] (1/4) oxi-cli app::agent_hub_bridge::tests::empty_dir_registers_nothing
   PASS [   0.024s] (2/4) oxi-cli app::agent_hub_bridge::tests::main_session_stem_heuristic_matches_real_pattern
   PASS [   0.025s] (3/4) oxi-cli app::agent_hub_bridge::tests::missing_dir_is_noop
   PASS [   0.032s] (4/4) oxi-cli app::agent_hub_bridge::tests::registers_subagent_jsonl_excluding_main_and_advisor
 Summary [   0.034s] 4 tests run: 4 passed, 742 skipped
```

Full suite: **746/746 passing** (no regressions).

### Gates

- `cargo build --workspace` — clean
- `cargo nextest run -p oxi-cli app::agent_hub_bridge::tests` — **4/4 PASS** (added heuristic unit test)
- `cargo nextest run -p oxi-cli` — **746/746 PASS** (no regressions)
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `cargo clippy -p oxi-sdk --features native-browser -- -D warnings` — clean
- `cargo fmt --all -- --check` — clean

### Commit

`a669770510d17aec4c1875d868bd44da58f0973b` — `fix(cli): exclude own session file from subagent scan in HubRegistry`
