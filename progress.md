# Dead Code Cleanup Progress

## Status: Completed (Round 1 + Round 2)

## Round 2: oxi-agent & oxi-ai Cleanup (2026-05-16)

### Result
- **0 warnings** for `cargo check -p oxi-agent -p oxi-ai --lib`
- **232 tests pass** in oxi-agent
- **346 tests pass** in oxi-ai (2 pre-existing failures unrelated to cleanup)

### Removed Files (untracked, ~3,029 lines total)

| File | Reason |
|------|--------|
| oxi-ai/src/providers/cloudflare.rs | Routes through OpenAiProvider via built-in registry |
| oxi-ai/src/providers/copilot.rs | Routes through OpenAiProvider |
| oxi-ai/src/providers/codex.rs | Routes through OpenAiProvider |
| oxi-ai/src/providers/deepseek.rs | Routes through OpenAiProvider |
| oxi-ai/src/providers/openai_completions.rs | Routes through OpenAiProvider |

### Key Deletions in oxi-agent
- `Agent::DEFAULT_FALLBACK_MODEL`, `MpscRetryCallback`, `ToolBatchResult`
- 12 dead Agent methods (run_compaction_check, execute_tool_batch, stream_with_retry, etc.)
- `gh_json()` function in github.rs
- Dead constants from agent_loop/config.rs

### Key Deletions in oxi-ai
- `GoogleThinkingLevel` enum, 3 dead functions from google_shared
- `with_api_key()` from anthropic, google providers
- `with_region()` from bedrock
- ~30 unused serde struct fields prefixed with `_` across 8 provider files

### See Also
- Detailed report: `deadcode_cleanup_2.md`

---

## Round 1: oxi-cli Cleanup (earlier)

### Removed Files/Modules

| File | Reason |
|------|--------|
| oxi-cli/src/media/image_convert.rs | Dead - no callers |
| oxi-cli/src/media/image_resize.rs | Dead - no callers |
| oxi-cli/src/media/exif_orientation.rs | Dead - no callers |
| oxi-cli/src/media/clipboard_image.rs | Dead - no callers |
| oxi-cli/src/media/mime_detect.rs | Dead - no callers |
| oxi-cli/src/media/file_processor.rs | Dead - no callers |
| oxi-cli/src/prompt/frontmatter.rs | Dead - no callers |
| oxi-cli/src/prompt/templates.rs | Dead - no callers |
| oxi-cli/src/storage/resource_loader_compat.rs | Kept but cleaned |
| oxi-cli/src/util/git_utils.rs | Replaced with minimal version |
| oxi-cli/src/infra/error_recovery.rs | Dead - no callers |
| oxi-agent/src/agent.rs (partial) | Removed unused constants/structs |

### Removed Code
- ~2000 lines of dead code removed
- 11 entire modules deleted

### Bugs Fixed
1. Fixed typo in oxi-ai/src/providers/openai_responses.rs (response.id → response._id)
2. Restored accidentally removed oxi-agent/src/agent.rs code
3. Restored accidentally removed oxi-ai provider files

### Remaining Warnings (in other crates)
78 warnings in oxi-cli - mostly documentation-related, not dead code

### Next Steps (for future cleanup)
1. Remove remaining unused fields from AgentSessionRuntimeDiagnostic, AgentSessionServices
2. Remove unused enum variants (Threshold, Automatic, Overflow, Iteration from CompactionReason)
3. Remove unused SourceInfo, SlashCommandSource, SlashCommandInfo structs
4. Clean up unused imports throughout
