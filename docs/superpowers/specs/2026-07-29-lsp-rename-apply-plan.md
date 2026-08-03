# LSP: Rename.apply + willRenameFiles — Implementation Plan

> **Tier 2 — Pattern-following.** P3.1 + P3.3.
> 참조: `docs/REMAINING.md` §P3.1, §P3.3.

## Current State

- `CliLspProvider` in `oxicode-cli/src/lsp/provider.rs` — `action_rename()` exists, sends `textDocument/rename` but returns **preview only** (accepts `_apply: bool` parameter but ignores it)
- `action_file_rename()` — sends `WillRenameFiles` request, also preview-only
- `oxicode-agent/src/tools/lsp.rs::LspAction` — has `Rename`, `FileRename` variants with `apply: bool` field already defined
- No standalone willRenameFiles operation exposed to the agent tool

## Plan

### Task 1: Implement rename apply in CliLspProvider

**Files:** Modify `oxicode-cli/src/lsp/provider.rs`

**Current `action_rename()` (preview-only):**
```rust
async fn action_rename(&self, file: &str, line: u32, new_name: &str, _apply: bool) -> Result<String, ToolError> {
    // ... send Rename request, format WorkspaceEdit as text preview
}
```

**Change: when `apply: true`:**
1. Send `textDocument/rename` request (same as now)
2. Receive `WorkspaceEdit` response
3. Apply edits to disk:
   - For `TextDocumentEdit`: sort by URI, then per-file apply changes bottom-to-top
   - For text edits: apply in reverse order (bottom-to-top) to preserve positions
   - Use `std::fs::write` to write modified content
4. Return summary: "Renamed symbol in N files"

**Key design:**
```rust
fn apply_workspace_edit(edit: &WorkspaceEdit) -> Result<(), ToolError> {
    // WorkspaceEdit.document_changes takes precedence over .changes
    if let Some(changes) = &edit.document_changes {
        match changes {
            DocumentChanges::Edits(edits) => {
                for doc_edit in edits {
                    apply_text_document_edit(doc_edit)?;
                }
            }
            DocumentChanges::Operations(ops) => {
                for op in ops {
                    match op {
                        ResourceOp::Create(_) | ResourceOp::Delete(_) => {
                            // Skip create/delete ops for now (rare)
                        }
                        ResourceOp::Rename(rename) => {
                            std::fs::rename(&rename.old_uri, &rename.new_uri)?;
                        }
                    }
                }
            }
        }
    } else if let Some(changes) = &edit.changes {
        for (uri, text_edits) in changes {
            apply_text_edits_to_file(uri, text_edits)?;
        }
    }
    Ok(())
}
```

### Task 2: Add willRenameFiles as standalone LspAction

**Files:** Modify `oxicode-agent/src/tools/lsp.rs` + `oxicode-cli/src/lsp/provider.rs`

- Add `LspAction::WillRenameFiles` variant
- Add `action_will_rename_files()` to `LspProvider` trait
- Implement in CliLspProvider (currently only called internally by `action_file_rename`)

### Files

| File | Action |
|---|---|
| `oxicode-cli/src/lsp/provider.rs` | **Modify** — `action_rename()` apply path, `action_file_rename()` apply path, `action_will_rename_files()` |
| `oxicode-agent/src/tools/lsp.rs` | **Modify** — add `LspAction::WillRenameFiles`, add trait method |
| `oxicode-agent/src/tools.rs` | **Modify** — update `LspProvider` trait if needed |

### Acceptance

- `LspTool` with `action: "rename", apply: true` actually renames symbols in files
- Result message shows how many files were changed
- `action: "will_rename_files"` returns preview of pending renames
- Rollback on error: changes are applied, but if a write fails, previous files are not rolled back (LSP semantics: partial application acceptable)

### Test Strategy

- Unit test for `apply_workspace_edit()` with mock WorkspaceEdit
- Round-trip: create temp file, rename symbol, verify file changed
- Existing LSP tests continue to pass
