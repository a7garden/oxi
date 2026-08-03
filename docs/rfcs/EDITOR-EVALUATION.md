# Editor Evaluation: ratatui-textarea vs pi Custom Editor

**Date**: 2026-05-27
**Decision**: **Enhance textarea** — no custom editor replacement needed

## 1. pi Custom Editor (Reference)

pi-tui uses a ~76KB custom editor built for the specific needs of an AI chat input:

| Feature | pi | Notes |
|---------|-----|-------|
| Undo/Redo | ✅ | Full undo stack |
| Kill-ring | ✅ | Alt+Kill/Yank with cyclic buffer |
| History | ✅ | Up/Down with saved input |
| Jump | ✅ | Ctrl+Arrow word-skip |
| Yank/Yank-pop | ✅ | Kill-ring cycling |
| CJK input | ✅ | Wide character handling |
| Bracketed paste | ✅ | Multi-line paste support |
| Selection | ✅ | Shift+movement |
| Multi-line | ✅ | Full multi-line editing |

## 2. oxicode Current Editor (ratatui-textarea)

`ratatui-textarea` (used via `oxicode-tui/src/widgets/input.rs`, ~350 LOC wrapper):

| Feature | Status | Implementation |
|---------|--------|---------------|
| Undo/Redo | ✅ | `textarea.undo()`, `textarea.redo()` |
| Word movement | ✅ | `move_word_left()`, `move_word_right()` |
| Line movement | ✅ | `move_home()`, `move_end()` |
| Delete word | ✅ | `delete_word_backward()`, `delete_word_forward()` |
| Delete line | ✅ | `delete_line_by_head()`, `delete_line_by_end()` |
| Multi-line | ✅ | Native textarea support |
| CJK input | ✅ | Built into ratatui-textarea |
| Bracketed paste | ✅ | Built into ratatui-textarea |
| History | ✅ | Manual via `input_history` in AppState |
| Selection | ✅ | Via shift+movement in textarea |

## 3. Gap Analysis

| Feature | pi | oxicode | Gap | Impact |
|---------|-----|-----|-----|--------|
| Kill-ring | ✅ | ❌ | **Missing** | Low — most users don't use kill-ring; standard clipboard (Ctrl+C/V) is sufficient |
| Yank-pop | ✅ | ❌ | **Missing** | Low — depends on kill-ring |
| Jump forward/backward | ✅ | ✅ | **Covered** by word movement | None |
| Grapheme cluster navigation | ✅ | ⚠️ | **Partial** — textarea handles basic CJK | Low |
| Custom key rebinding | ✅ | ✅ | **Covered** by KeybindingsManager | None |
| Search in input | ❌ | ❌ | N/A | N/A |

## 4. Decision

**Enhance the existing `ratatui-textarea` wrapper rather than building a custom editor.**

### Rationale

1. **Feature coverage is ~90%**: The only missing features are kill-ring and yank-pop, which are niche (most users never learn them in terminal editors).

2. **Cost/benefit is poor**: Building a custom editor at ~76KB LOC would take 2-3 weeks and introduce a large maintenance burden for features most users don't need.

3. **The textarea already works**: It handles the critical features (CJK, undo, multi-line, selection) that are hard to get right.

4. **Keybinding system decouples**: With the new `KeybindingsManager`, key mapping is already independent of the editor implementation. Swapping editors later would only require updating the dispatch layer.

### Recommended Enhancements (If Needed Later)

- Kill-ring: 200-300 LOC addition to `InputState`
- Yank-pop: 50 LOC addition building on kill-ring
- Grapheme-aware navigation: Use `unicode-segmentation` for precise cursor movement

These can be added incrementally without replacing the editor.

## 5. Conclusion

The current `ratatui-textarea` approach is sound. No custom editor replacement is warranted at this time. The RFC's Phase 6 research concludes with: **Proceed with enhancement, not replacement.**
