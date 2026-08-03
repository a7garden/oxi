# Commit: /commit Slash + CLI — Implementation Plan

> **Tier 2 — Pattern-following.** P1.2 (/commit slash) + P1.3 (oxicode commit CLI).
> 참조: `docs/REMAINING.md` §P1.2, §P1.3.
>
> P0.2 (CommitTool LLM injection)는 `bootstrap.rs:419-427`에서 **이미 완료**됨.

## Current State

- `handle_commit()` in `misc.rs:90` — already wired as `oxicode commit [--push] [--dry-run] [-c]`. Checks staged changes, falls back to `git commit`. TODO at line 138: wire into CommitTool.
- No `/commit` slash command registered.
- `CommitTool` (1,798 LOC) available but only callable through agent loop.
- `commit_tool_enabled` settings flag needs addition (see Tier 3).

## Plan

### Task 1: Implement `/commit` slash command

**Files:** Create `oxicode-cli/src/tui/slash/builtin/commit.rs`

**Pattern:** Follow `agents.rs` exactly — SlashCommand trait, execute() opens overlay.

**execute() logic:**
1. Get session handle
2. Run `git diff --cached HEAD` to get staged diff
3. Spawn async: call `CommitTool` directly with the diff (synchronous, deterministic-only for now)
4. Display proposed commit messages in `TextViewerOverlay`

Or simpler: just delegate to the agent's commit tool via the session's tool registry.

Since the CommitTool needs the agent loop (it's an AgentTool), the simpler approach for the slash command: run `CommitTool::unconfigured()` directly — deterministic scope extraction only. Display results. For LLM-based, user should use the agent's `/tool commit` or set `commit_tool_enabled: true` (the agent will call it automatically).

**Registration:** Add to `oxicode-cli/src/tui/slash/builtin/mod.rs::register_all()`.

### Task 2: Upgrade oxicode commit CLI with LLM analysis

**Files:** Modify `oxicode-cli/src/cli/commands/misc.rs:90-175`

Replace the TODO at line 138 with:
1. If `commit_tool_enabled` in settings AND model role "commit" is configured:
   - Use `CommitTool` for LLM-driven message generation
2. Otherwise: keep existing behavior (deterministic + editor fallback)
3. `--dry-run`: run analysis, print proposed messages, don't commit

### Files

| File | Action |
|---|---|
| `oxicode-cli/src/tui/slash/builtin/commit.rs` | **Create** — SlashCommand impl, 40 LOC |
| `oxicode-cli/src/tui/slash/builtin/mod.rs` | **Modify** — register CommitCommand |
| `oxicode-cli/src/cli/commands/misc.rs` | **Modify** — line 138 TODO → real CommitTool call |

### Acceptance

- `/commit` slash shows commit preview in overlay
- `oxicode commit --dry-run` prints analysis to stdout without committing
- Settings flag gates LLM path correctly

### Test Strategy

- Unit: slash command handler with mock session
- Manual: `/commit` in TUI, `oxicode commit --dry-run` on CLI
