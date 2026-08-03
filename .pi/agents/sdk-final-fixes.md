---
name: sdk-final-fixes
description: Fix remaining oxicode-sdk build errors
tools: read, edit, write, bash
model: anthropic/claude-sonnet-4-20250514
systemPromptMode: replace
inheritProjectContext: false
inheritSkills: false
defaultContext: fresh
---

Fix all remaining Rust compilation errors in oxicode-sdk.

Run: cargo build -p oxicode-sdk 2>&1

The errors reported are:
1. E0195 - lifetime mismatch: `async fn handle(&self, ctx: &MiddlewareContext) -> MiddlewareResult` in builtins.rs vs trait
   Fix: Make sure the #[async_trait] macro is applied and match trait signature exactly
2. E0599 - no method `agent_cost` on `Arc<CostTracker>` in builtins.rs
   Fix: Check what method CostTracker has for agent snapshot - likely `snapshot(agent_id)` returns Option<CostSnapshot> not agent_cost
3. E0277 - AgentToolResult: Clone not satisfied - likely MiddlewareData::AfterTool contains AgentToolResult
   Fix: Arc<AgentToolResult> or remove Clone requirement
4. E0277 - MiddlewareData doesn't implement Debug
   Fix: add derive(Debug) to MiddlewareData
5. E0502 - borrow checker in audit.rs (entries as immutable + mutable)

First run cargo build, read all errors, then read the specific files and fix each error.

Important: Read the existing files before editing. Do targeted fixes only.

Key files to check:
- oxicode-sdk/src/middleware/builtins.rs
- oxicode-sdk/src/middleware/mod.rs
- oxicode-sdk/src/observability/audit.rs
- oxicode-sdk/src/observability/cost.rs

After fixing all errors, run cargo build -p oxicode-sdk and then cargo test -p oxicode-sdk
