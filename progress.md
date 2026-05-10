# oxi Project Progress

## 2026-05-10 — Markdown Module Cleanup Analysis

**Status:** ✅ Complete

**Task:** Analyze `markdown.rs` for dead code and usage across the codebase.

**Findings written to:** `/tmp/markdown-cleanup.md`

### Key Results:
- **markdown.rs** (~300 lines) has **13 public items**
- **10 items ALIVE** — actively used by `chat.rs` (Segment enum, parse_inline, 7 style helpers)
- **3 items DEAD** — `LineType` enum, `detect_line_type()`, `heading_text()` (~65 lines + ~40 lines of tests)
- **Only consumer:** `chat.rs` — no other module references markdown
- **Recommendation:** KEEP the module, delete the 3 dead items + associated tests (~108 lines total reduction)

### Next Steps:
- [ ] Delete `LineType` enum, `detect_line_type()`, `heading_text()` from markdown.rs
- [ ] Remove 6 associated test functions
- [ ] Verify `cargo test` still passes
