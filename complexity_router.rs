

## Summary

### Completed

Created `oxi-ai/src/complexity_router.rs` implementing the `ComplexityRouter` trait and `DefaultRouter` as specified.

### Files Changed

- **`oxi-ai/src/complexity_router.rs`** — New file with complete implementation
- **`oxi-ai/src/lib.rs`** — Added module declaration and re-exports
- **`oxi-ai/src/types.rs`** — Added `Complexity` enum with `cost_tier()` method

### Implementation Details

**`ComplexityRouter` trait:**
- `classify(&self, context: &Context) -> Complexity` — Classifies task complexity
- `route(&self, complexity: Complexity, prefer_cost_efficient: bool) -> Vec<&'static ModelEntry>` — Returns sorted model candidates

**`DefaultRouter` implementation:**
- **Classification** (`classify`):
  - Extracts last user message text from context
  - Counts tokens using `crate::high_level::tokens::estimate`
  - Keyword analysis with priority hierarchy (Research > Complex > Moderate > Simple > Trivial)
  - System prompt analysis for additional hints
  - Token count adjustment: long inputs (>500 tokens) bump complexity
  - Tool presence bumps complexity by 1 (capped at Research)

- **Routing** (`route`):
  - Searches model DB by patterns per complexity tier
  - Filters models by tier suitability criteria
  - Sorts by cost (if `prefer_cost_efficient`) or capability (reasoning > context > output > cost)
  - Returns top 3 candidates

**Model mapping:**
| Complexity | Patterns |
|------------|----------|
| Trivial | haiku, gpt-4o-mini, mini |
| Simple | haiku, sonnet, gpt-4o-mini, mini |
| Moderate | sonnet, opus, gpt-4o, gpt-4.1 |
| Complex | opus, gemini-2.5-pro, gpt-4.1, claude-sonnet |
| Research | opus-4.5, opus-4.6, gemini-3-pro, gemini-2.5-pro, claude-opus |

### Notes
- All 15 complexity_router tests pass
- All 411 library tests pass
- Clippy warnings resolved
- `Complexity` enum moved from root `complexity.rs` to `types.rs` for proper integration
- Removed orphan rule violation by removing external `Arc` impls