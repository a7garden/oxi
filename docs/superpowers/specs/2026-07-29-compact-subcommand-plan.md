# Compaction: /compact snapcompact Subcommand — Implementation Plan

> **Tier 2 — Pattern-following.** P1.1
> 참조: `docs/REMAINING.md` §P1.1, `docs/designs/omp-adoption-2/09-compaction-modes.md` §4.

## Current State

- `CompactCommand` in `tools_commands.rs` — exists, accepts optional instructions, delegates to `session.compact(instructions)`
- `CompactionStrategy::Snapcompact` — exists in `oxicode-ai/src/compaction.rs:347`
- `SnapcompactCompactor` — exists in `oxicode-sdk/src/snapcompact_compactor.rs` (312 LOC, 5 tests)
- No subcommand routing — `/compact` always uses default strategy (soft)

## Plan

### Task: Add subcommand routing to CompactCommand

**Files:** Modify `oxicode-cli/src/tui/slash/builtin/tools_commands.rs`

**Change in `CompactCommand::execute()`:**

```rust
fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
    let trimmed = args.trim();
    let (strategy, instructions) = if trimmed.is_empty() {
        (None, None)  // default strategy
    } else if trimmed.starts_with("snapcompact") || trimmed.starts_with("snap") {
        let rest = trimmed.strip_prefix("snapcompact").or_else(|| trimmed.strip_prefix("snap")).unwrap_or("").trim();
        let instr = if rest.is_empty() { None } else { Some(rest.to_string()) };
        (Some(CompactionStrategy::Snapcompact), instr)
    } else {
        // "soft" keyword or anything else = soft with instructions
        let rest = trimmed.strip_prefix("soft").unwrap_or(trimmed).trim();
        let instr = if rest.is_empty() { None } else { Some(rest.to_string()) };
        (Some(CompactionStrategy::Soft), instr)
    };
    
    let sh = ctx.session.clone_handle();
    let tx = ctx.ui_tx.clone();
    tokio::spawn(async move {
        // Set strategy on the session's compaction manager before compacting
        if let Some(s) = strategy {
            sh.set_compaction_strategy(s).await;  // (needs method on AgentSession)
        }
        let result = sh.compact(instructions).await;
        let msg = match &result {
            Ok(r) => format!("Compacted ({}): {} tokens → {} tokens", 
                strategy.map(|s| format!("{:?}", s)).unwrap_or("default".into()),
                r.tokens_before, r.tokens_after),
            Err(e) => format!("Compaction failed: {e}"),
        };
        let _ = tx.send(UiEvent::SystemMessage(msg));
    });
    SlashOutcome::Handled
}
```

**Need to add to AgentSession:**
```rust
pub async fn set_compaction_strategy(&self, strategy: CompactionStrategy) {
    // Route to CompactionManager
    if let Some(mgr) = self.compaction_manager() {
        mgr.set_strategy(strategy).await;
    }
}
```

### Feature Gate

Check `settings.snapcompact_enabled` before allowing `snapcompact` strategy:
```rust
if strategy == Some(CompactionStrategy::Snapcompact) && !ctx.session.snapcompact_enabled() {
    return SlashOutcome::Display("snapcompact is not enabled (set snapcompact_enabled = true)".into());
}
```

### Files

| File | Action |
|---|---|
| `oxicode-cli/src/tui/slash/builtin/tools_commands.rs` | **Modify** — subcommand parsing + strategy routing |
| `oxicode-cli/src/app/agent_session.rs` | **Modify** — add `set_compaction_strategy()` and `snapcompact_enabled()` methods |

### Acceptance

- `/compact snapcompact` → PNG frame compaction, result shows frame count
- `/compact` (bare) → existing soft compaction behavior preserved
- `/compact soft <instructions>` → soft compaction with instructions
- snapcompact_enabled: false → error message on `/compact snapcompact`

### Test Strategy

- Unit test for subcommand parsing (`parse_compact_args`)
- Integration: `CompactCommand::execute()` with mock session
