//! CLI-side `LspProvider` implementation — bridges the agent's `lsp`
//! tool to [`super::LspManager`].
//!
//! Implements every method of [`oxi_agent::tools::LspProvider`] by
//! routing to the manager's lazy-spawned [`oxi_lsp::LspClient`]s.
//! Errors are returned as plain `String` (the `ToolError` alias).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use oxi_agent::tools::{
    DiagnosticsSummary, FileDiagnosticEntry, LspAction, LspProvider, ToolError,
};

use lsp_types::request::{
    CodeActionRequest, DocumentSymbolRequest, GotoDefinition, GotoImplementation,
    GotoTypeDefinition, HoverRequest, References, Rename, WillRenameFiles, WorkspaceSymbolRequest,
};
use lsp_types::{
    CodeActionContext, CodeActionParams, DocumentSymbolParams, FileRename, GotoDefinitionParams,
    HoverParams, Position, Range, ReferenceParams, RenameParams, TextDocumentIdentifier,
    TextDocumentPositionParams, WorkDoneProgressParams, WorkspaceSymbolParams,
};

use crate::lsp::manager::LspManager;

/// CLI-side `LspProvider` wrapping an [`LspManager`].
#[derive(Clone)]
pub struct CliLspProvider {
    manager: Arc<LspManager>,
}

impl std::fmt::Debug for CliLspProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliLspProvider")
            .field("workspace_root", &self.manager.workspace_root())
            .field("server_count", &self.manager.server_count())
            .finish_non_exhaustive()
    }
}

/// Per-RPC timeout for LSP requests (definition, references, hover, …).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

impl CliLspProvider {
    /// Construct a provider that shares the given manager.
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }

    /// Construct a provider owning a fresh manager configured from
    /// [`crate::lsp::manager::default_servers()`].
    pub fn with_defaults(workspace_root: PathBuf) -> Self {
        let manager = Arc::new(LspManager::with_defaults(workspace_root));
        Self::new(manager)
    }

    /// Borrow the wrapped manager.
    pub fn manager(&self) -> &LspManager {
        &self.manager
    }

    /// Best-effort shutdown of every spawned server.
    pub async fn shutdown_all(&self) {
        self.manager.shutdown_all().await;
    }

    /// Resolve a file argument to an absolute path inside the
    /// workspace root. Returns a `ToolError` when the file does not
    /// exist.
    fn resolve_file(&self, file: &str) -> Result<PathBuf, ToolError> {
        let p = PathBuf::from(file);
        let abs = if p.is_absolute() {
            p
        } else {
            self.manager.workspace_root().join(&p)
        };
        if !abs.exists() {
            return Err(format!("LSP file does not exist: {}", abs.display()));
        }
        Ok(abs)
    }

    fn work_done() -> WorkDoneProgressParams {
        WorkDoneProgressParams {
            work_done_token: None,
        }
    }

    fn position_params(uri: lsp_types::Url, line: u32) -> TextDocumentPositionParams {
        TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position: Position {
                line: line.saturating_sub(1),
                character: 0,
            },
        }
    }

    fn goto_params(uri: lsp_types::Url, line: u32) -> GotoDefinitionParams {
        GotoDefinitionParams {
            text_document_position_params: Self::position_params(uri, line),
            work_done_progress_params: Self::work_done(),
            partial_result_params: lsp_types::PartialResultParams {
                partial_result_token: None,
            },
        }
    }

    /// Spawn-or-fetch the LSP client owning `abs`'s extension.
    async fn client_for(
        &self,
        abs: &std::path::Path,
    ) -> Result<Arc<oxi_lsp::LspClient>, ToolError> {
        self.manager
            .client_for_path(abs)
            .await
            .map_err(|e| format!("LSP spawn failed: {e}"))?
            .ok_or_else(|| {
                format!(
                    "no LSP server configured for extension {:?}",
                    abs.extension().and_then(|e| e.to_str()).unwrap_or("")
                )
            })
    }

    fn uri(abs: &std::path::Path) -> Result<lsp_types::Url, ToolError> {
        oxi_lsp::uri_for(abs).ok_or_else(|| "invalid path for URI".to_string())
    }

    // ── Action handlers ──────────────────────────────────────────

    async fn action_diagnostics(&self, file: &str) -> Result<String, ToolError> {
        let abs = self.resolve_file(file)?;
        let client = self.client_for(&abs).await?;
        let uri_string = oxi_lsp::uri_for(&abs)
            .map(|u| u.to_string())
            .unwrap_or_default();
        let entries = client.read_diagnostics(&[uri_string]);
        Ok(summarize_entries(&abs, &entries))
    }

    async fn action_definition(&self, file: &str, line: u32) -> Result<String, ToolError> {
        let abs = self.resolve_file(file)?;
        let client = self.client_for(&abs).await?;
        let uri = Self::uri(&abs)?;
        let params = Self::goto_params(uri, line);
        let resp: lsp_types::GotoDefinitionResponse = client
            .request::<GotoDefinition>(params, REQUEST_TIMEOUT)
            .await
            .map_err(lsp_err)?
            .unwrap_or(lsp_types::GotoDefinitionResponse::Array(vec![]));
        Ok(format_locations(&resp))
    }

    async fn action_references(&self, file: &str, line: u32) -> Result<String, ToolError> {
        let abs = self.resolve_file(file)?;
        let client = self.client_for(&abs).await?;
        let uri = Self::uri(&abs)?;
        let params = ReferenceParams {
            text_document_position: Self::position_params(uri, line),
            work_done_progress_params: Self::work_done(),
            partial_result_params: lsp_types::PartialResultParams {
                partial_result_token: None,
            },
            context: lsp_types::ReferenceContext {
                include_declaration: true,
            },
        };
        let locs: Vec<lsp_types::Location> = client
            .request::<References>(params, REQUEST_TIMEOUT)
            .await
            .map_err(lsp_err)?
            .unwrap_or_default();
        let resp = lsp_types::GotoDefinitionResponse::Array(locs);
        Ok(format_locations(&resp))
    }

    async fn action_hover(&self, file: &str, line: u32) -> Result<String, ToolError> {
        let abs = self.resolve_file(file)?;
        let client = self.client_for(&abs).await?;
        let uri = Self::uri(&abs)?;
        let params = HoverParams {
            text_document_position_params: Self::position_params(uri, line),
            work_done_progress_params: Self::work_done(),
        };
        let hover: Option<lsp_types::Hover> = client
            .request::<HoverRequest>(params, REQUEST_TIMEOUT)
            .await
            .map_err(lsp_err)?;
        Ok(hover
            .map(|h| match h.contents {
                lsp_types::HoverContents::Scalar(s) => marked_string_to_string(&s),
                lsp_types::HoverContents::Array(arr) => arr
                    .into_iter()
                    .map(|m| marked_string_to_string(&m))
                    .collect::<Vec<_>>()
                    .join("\n"),
                lsp_types::HoverContents::Markup(m) => m.value,
            })
            .unwrap_or_else(|| "(no hover info)".into()))
    }

    async fn action_rename(
        &self,
        file: &str,
        line: u32,
        new_name: String,
        apply: bool,
    ) -> Result<String, ToolError> {
        let abs = self.resolve_file(file)?;
        let client = self.client_for(&abs).await?;
        let uri = Self::uri(&abs)?;
        let params = RenameParams {
            text_document_position: Self::position_params(uri, line),
            new_name,
            work_done_progress_params: Self::work_done(),
        };
        let resp: Option<lsp_types::WorkspaceEdit> = client
            .request::<Rename>(params, REQUEST_TIMEOUT)
            .await
            .map_err(lsp_err)?;
        Ok(match resp {
            None => "(no edits)".into(),
            Some(edit) => {
                if apply {
                    let summary = summarize_workspace_edit(&edit);
                    apply_workspace_edit(&edit)?;
                    format!("Rename applied: {summary}")
                } else {
                    format!("Rename preview:\n{}", summarize_workspace_edit(&edit))
                }
            }
        })
    }

    async fn action_symbols(&self, file: &str, query: Option<String>) -> Result<String, ToolError> {
        let abs = self.resolve_file(file)?;
        let client = self.client_for(&abs).await?;

        // Workspace symbol search when a non-empty query is given.
        if let Some(q) = query
            && !q.is_empty()
        {
            let params = WorkspaceSymbolParams {
                query: q,
                work_done_progress_params: Self::work_done(),
                partial_result_params: lsp_types::PartialResultParams {
                    partial_result_token: None,
                },
            };
            let resp: Option<lsp_types::WorkspaceSymbolResponse> = client
                .request::<WorkspaceSymbolRequest>(params, REQUEST_TIMEOUT)
                .await
                .map_err(lsp_err)?;
            let symbols: Vec<lsp_types::SymbolInformation> = match resp {
                Some(lsp_types::WorkspaceSymbolResponse::Flat(arr)) => arr,
                Some(lsp_types::WorkspaceSymbolResponse::Nested(_)) => {
                    return Ok("(nested workspace symbols not supported)".into());
                }
                None => Vec::new(),
            };
            return Ok(format_symbols_flat(&symbols));
        }

        // Document symbols otherwise.
        let uri = Self::uri(&abs)?;
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: Self::work_done(),
            partial_result_params: lsp_types::PartialResultParams {
                partial_result_token: None,
            },
        };
        let resp: Option<lsp_types::DocumentSymbolResponse> = client
            .request::<DocumentSymbolRequest>(params, REQUEST_TIMEOUT)
            .await
            .map_err(lsp_err)?;
        Ok(match resp {
            Some(lsp_types::DocumentSymbolResponse::Flat(flat)) => format_symbols_flat(&flat),
            Some(lsp_types::DocumentSymbolResponse::Nested(_)) => {
                "(nested document symbols — showing top-level only)".into()
            }
            None => "(no symbols)".into(),
        })
    }

    async fn action_code_actions(&self, file: &str, line: u32) -> Result<String, ToolError> {
        let abs = self.resolve_file(file)?;
        let client = self.client_for(&abs).await?;
        let uri = Self::uri(&abs)?;
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range {
                start: Position {
                    line: line.saturating_sub(1),
                    character: 0,
                },
                end: Position {
                    line: line.saturating_sub(1),
                    character: u32::MAX,
                },
            },
            context: CodeActionContext::default(),
            work_done_progress_params: Self::work_done(),
            partial_result_params: lsp_types::PartialResultParams {
                partial_result_token: None,
            },
        };
        let actions: Vec<lsp_types::CodeActionOrCommand> = client
            .request::<CodeActionRequest>(params, REQUEST_TIMEOUT)
            .await
            .map_err(lsp_err)?
            .unwrap_or_default();
        Ok(format_code_actions(&actions))
    }

    async fn action_type_definition(&self, file: &str, line: u32) -> Result<String, ToolError> {
        let abs = self.resolve_file(file)?;
        let client = self.client_for(&abs).await?;
        let uri = Self::uri(&abs)?;
        let params = Self::goto_params(uri, line);
        let resp: lsp_types::GotoDefinitionResponse = client
            .request::<GotoTypeDefinition>(params, REQUEST_TIMEOUT)
            .await
            .map_err(lsp_err)?
            .unwrap_or(lsp_types::GotoDefinitionResponse::Array(vec![]));
        Ok(format_locations(&resp))
    }

    async fn action_implementation(&self, file: &str, line: u32) -> Result<String, ToolError> {
        let abs = self.resolve_file(file)?;
        let client = self.client_for(&abs).await?;
        let uri = Self::uri(&abs)?;
        let params = Self::goto_params(uri, line);
        let resp = client
            .request::<GotoImplementation>(params, REQUEST_TIMEOUT)
            .await
            .map_err(lsp_err)?
            .unwrap_or(lsp_types::GotoDefinitionResponse::Array(vec![]));
        Ok(format_locations(&resp))
    }

    async fn action_file_rename(
        &self,
        old_path: &str,
        new_path: &str,
        apply: bool,
    ) -> Result<String, ToolError> {
        let old_abs = self.resolve_file(old_path)?;
        let client = self.client_for(&old_abs).await?;
        let old_uri = Self::uri(&old_abs)?;
        let new_uri = oxi_lsp::uri_for(&PathBuf::from(new_path))
            .ok_or_else(|| "invalid new_path for URI".to_string())?;
        let params = lsp_types::RenameFilesParams {
            files: vec![FileRename {
                old_uri: old_uri.to_string(),
                new_uri: new_uri.to_string(),
            }],
        };
        let edit: Option<lsp_types::WorkspaceEdit> = client
            .request::<WillRenameFiles>(params, REQUEST_TIMEOUT)
            .await
            .map_err(lsp_err)?;
        Ok(match edit {
            None => "(no willRenameFiles response)".into(),
            Some(e) => {
                if apply {
                    let summary = summarize_workspace_edit(&e);
                    apply_workspace_edit(&e)?;
                    format!("File rename applied: {summary}")
                } else {
                    format!("File rename preview:\n{}", summarize_workspace_edit(&e))
                }
            }
        })
    }
}

#[async_trait]
impl LspProvider for CliLspProvider {
    fn ensure_started_background<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // Lazy spawn happens on first request. Eager pre-warm is a
        // documented follow-up — the hook exists so future work can
        // pre-warm configured servers without changing call sites.
        Box::pin(async {})
    }

    fn ensure_ready<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        // Lazy initialization happens on first request, so there's
        // nothing to "wait for" until at least one server has been
        // spawned. Once spawned, `LspClient::start` already awaits
        // `initialize` before returning, so we're effectively ready
        // as soon as a client exists.
        Box::pin(async { Ok(()) })
    }

    fn drain_diagnostics<'a>(
        &'a self,
        timeout: Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<DiagnosticsSummary>> + Send + 'a>>
    {
        let live = self.manager.live_clients();
        Box::pin(async move {
            let mut merged = DiagnosticsSummary::default();
            for (_name, client) in live {
                if let Some(entries) = client.drain_diagnostics(timeout).await {
                    for entry in entries {
                        let counts = count_diagnostics(&entry.diagnostics);
                        merged.count += counts.total;
                        merged.errors += counts.errors;
                        merged.warnings += counts.warnings;
                        merged.entries.push(FileDiagnosticEntry {
                            uri: entry.uri,
                            path: String::new(),
                            diagnostics: entry.diagnostics,
                        });
                    }
                }
            }
            if merged.count == 0 {
                None
            } else {
                Some(merged)
            }
        })
    }

    fn read_diagnostics<'a>(
        &'a self,
        paths: &'a [PathBuf],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<FileDiagnosticEntry>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut out = Vec::new();
            for path in paths {
                let Ok(Some(client)) = self.manager.client_for_path(path).await else {
                    continue;
                };
                let Some(uri) = oxi_lsp::uri_for(path) else {
                    continue;
                };
                let uri_string = uri.to_string();
                for entry in client.read_diagnostics(&[uri_string]) {
                    out.push(FileDiagnosticEntry {
                        uri: entry.uri,
                        path: path.to_string_lossy().into_owned(),
                        diagnostics: entry.diagnostics,
                    });
                }
            }
            out
        })
    }

    fn notify_file_changed<'a>(
        &'a self,
        path: &'a std::path::Path,
        content: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let Ok(Some(client)) = self.manager.client_for_path(path).await else {
                return;
            };
            let Some(uri) = oxi_lsp::uri_for(path) else {
                return;
            };
            // Best-effort didOpen with the new content. We send
            // didOpen rather than didChange because the agent's
            // write/edit tools have already flushed the new content
            // to disk; the server sees the file as freshly opened
            // with the latest text.
            let _ = client.notify::<lsp_types::notification::DidOpenTextDocument>(
                lsp_types::DidOpenTextDocumentParams {
                    text_document: lsp_types::TextDocumentItem {
                        uri,
                        language_id: path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("plaintext")
                            .into(),
                        version: 1,
                        text: content.into(),
                    },
                },
            );
        })
    }

    fn execute_action<'a>(
        &'a self,
        action: &'a LspAction,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, ToolError>> + Send + 'a>>
    {
        Box::pin(async move {
            match action {
                LspAction::Status => {
                    let live = self.manager.live_clients();
                    let mut s = format!(
                        "LSP status: {} configured, {} live\n",
                        self.manager.server_count(),
                        live.len()
                    );
                    for (name, _) in &live {
                        s.push_str(&format!("  - {name}\n"));
                    }
                    Ok(s)
                }
                LspAction::Diagnostics { file } => self.action_diagnostics(file).await,
                LspAction::Definition { file, line, .. } => {
                    self.action_definition(file, *line).await
                }
                LspAction::References { file, line, .. } => {
                    self.action_references(file, *line).await
                }
                LspAction::Hover { file, line, .. } => self.action_hover(file, *line).await,
                LspAction::Rename {
                    file,
                    line,
                    new_name,
                    apply,
                    ..
                } => {
                    self.action_rename(file, *line, new_name.clone(), *apply)
                        .await
                }
                LspAction::Symbols { file, query, .. } => {
                    self.action_symbols(file, query.clone()).await
                }
                LspAction::CodeActions { file, line, .. } => {
                    self.action_code_actions(file, *line).await
                }
                LspAction::TypeDefinition { file, line, .. } => {
                    self.action_type_definition(file, *line).await
                }
                LspAction::Implementation { file, line, .. } => {
                    self.action_implementation(file, *line).await
                }
                LspAction::FileRename {
                    old_path,
                    new_path,
                    apply,
                } => self.action_file_rename(old_path, new_path, *apply).await,
            }
        })
    }
}

// ── formatting helpers ───────────────────────────────────────────

fn lsp_err(e: oxi_lsp::LspError) -> ToolError {
    format!("LSP request failed: {e}")
}

fn marked_string_to_string(s: &lsp_types::MarkedString) -> String {
    match s {
        lsp_types::MarkedString::String(s) => s.clone(),
        lsp_types::MarkedString::LanguageString(ls) => ls.value.clone(),
    }
}

fn format_locations(resp: &lsp_types::GotoDefinitionResponse) -> String {
    use lsp_types::GotoDefinitionResponse::*;
    let locs: Vec<lsp_types::Location> = match resp {
        Scalar(l) => vec![l.clone()],
        Array(arr) => arr.clone(),
        Link(links) => links
            .iter()
            .map(|l| lsp_types::Location {
                uri: l.target_uri.clone(),
                range: l.target_selection_range,
            })
            .collect(),
    };
    if locs.is_empty() {
        return "(no locations)".into();
    }
    locs.iter()
        .map(|l| {
            format!(
                "{}:{}:{}",
                l.uri.as_str(),
                l.range.start.line + 1,
                l.range.start.character + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_symbols_flat(symbols: &[lsp_types::SymbolInformation]) -> String {
    if symbols.is_empty() {
        return "(no symbols)".into();
    }
    symbols
        .iter()
        .map(|s| {
            let kind = format!("{:?}", s.kind).to_lowercase();
            format!(
                "{} ({}) — {}:{}",
                s.name,
                kind,
                s.location.uri.as_str(),
                s.location.range.start.line + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_code_actions(actions: &[lsp_types::CodeActionOrCommand]) -> String {
    if actions.is_empty() {
        return "(no code actions)".into();
    }
    actions
        .iter()
        .map(|a| match a {
            lsp_types::CodeActionOrCommand::CodeAction(ca) => {
                format!("CodeAction: {} ({:?})", ca.title, ca.kind)
            }
            lsp_types::CodeActionOrCommand::Command(cmd) => {
                format!("Command: {} ({})", cmd.title, cmd.command)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarize_workspace_edit(edit: &lsp_types::WorkspaceEdit) -> String {
    let changes = edit.changes.as_ref().map(|c| c.len()).unwrap_or(0);
    let doc_changes = edit
        .document_changes
        .as_ref()
        .map(|c| match c {
            lsp_types::DocumentChanges::Edits(e) => e.len(),
            lsp_types::DocumentChanges::Operations(o) => o.len(),
        })
        .unwrap_or(0);
    format!("WorkspaceEdit: {changes} uri-changes, {doc_changes} document-changes")
}

/// Apply a `WorkspaceEdit` to disk.
///
/// Supports `documentChanges` (modern) and `changes` (legacy) formats.
/// Edits are applied bottom-to-top per file to preserve positions.
/// Uses atomic temp+rename writes (crash-safe).
fn apply_workspace_edit(edit: &lsp_types::WorkspaceEdit) -> Result<(), ToolError> {
    if let Some(changes) = &edit.document_changes {
        match changes {
            lsp_types::DocumentChanges::Edits(edits) => {
                for doc_edit in edits {
                    apply_text_document_edit(doc_edit)?;
                }
            }
            lsp_types::DocumentChanges::Operations(_ops) => {
                // Resource operations (create/delete/rename) are rare;
                // skip for now to keep the implementation focused.
            }
        }
    } else if let Some(changes) = &edit.changes {
        for (_uri, text_edits) in changes {
            let path = _uri
                .to_file_path()
                .map_err(|_| format!("invalid URI in changes: {_uri}"))?;
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
            let has_trailing_newline = content.ends_with('\n');
            let mut lines: Vec<String> = content.lines().map(String::from).collect();

            let mut sorted = text_edits.clone();
            sorted.sort_by_key(|b| std::cmp::Reverse(b.range.start.line));

            for text_edit in &sorted {
                apply_text_edit_to_lines(&mut lines, text_edit);
            }

            let mut output = lines.join("\n");
            if has_trailing_newline {
                output.push('\n');
            }
            atomic_write(&path, &output)?;
        }
    }
    Ok(())
}

/// Atomic file write: write to a temp file in the same directory, then
/// rename. Crash-safe: a crash mid-write leaves the original intact.
fn atomic_write(path: &std::path::Path, content: &str) -> Result<(), ToolError> {
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let mut tmp = dir.to_path_buf();
    tmp.push(format!(
        ".tmp.{}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, content)
        .map_err(|e| format!("failed to write temp {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        format!(
            "failed to rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

/// Apply a single `TextDocumentEdit` (modern format).
fn apply_text_document_edit(doc_edit: &lsp_types::TextDocumentEdit) -> Result<(), ToolError> {
    let uri = &doc_edit.text_document.uri;
    let path = uri
        .to_file_path()
        .map_err(|_| format!("invalid URI: {uri}"))?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let has_trailing_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    let mut edits: Vec<&lsp_types::TextEdit> = Vec::with_capacity(doc_edit.edits.len());
    for edit in &doc_edit.edits {
        match edit {
            lsp_types::OneOf::Left(text_edit) => edits.push(text_edit),
            lsp_types::OneOf::Right(annotated) => edits.push(&annotated.text_edit),
        }
    }

    edits.sort_by_key(|b| std::cmp::Reverse(b.range.start.line));

    for text_edit in edits {
        apply_text_edit_to_lines(&mut lines, text_edit);
    }

    let mut output = lines.join("\n");
    if has_trailing_newline {
        output.push('\n');
    }
    atomic_write(&path, &output)?;
    Ok(())
}

/// Convert an LSP UTF-16 code-unit offset to a Rust byte offset.
///
/// LSP uses UTF-16 code units for character positions (per the spec),
/// while Rust uses byte indices. Direct byte slicing with an LSP
/// offset would panic on non-ASCII characters.
fn u16_to_byte_offset(line: &str, u16_offset: usize) -> usize {
    let mut u16_pos = 0;
    for (byte_off, c) in line.char_indices() {
        if u16_pos >= u16_offset {
            return byte_off;
        }
        u16_pos += c.len_utf16();
    }
    line.len()
}

/// Apply a single `TextEdit` to a mutable line buffer (bottom-to-top safe).
///
/// Converts LSP UTF-16 code-unit offsets to byte offsets before slicing
/// to avoid panicking on non-ASCII characters.
fn apply_text_edit_to_lines(lines: &mut Vec<String>, edit: &lsp_types::TextEdit) {
    let start_line = edit.range.start.line as usize;
    let end_line = edit.range.end.line as usize;

    if start_line >= lines.len() {
        lines.push(edit.new_text.clone());
        return;
    }

    if start_line == end_line {
        let line = &lines[start_line];
        let start_col = u16_to_byte_offset(line, edit.range.start.character as usize);
        let end_col = u16_to_byte_offset(line, edit.range.end.character as usize);
        let end_col = end_col.min(line.len());
        let before = &line[..start_col.min(line.len())];
        let after = &line[end_col..];
        let mut new_line = String::with_capacity(before.len() + edit.new_text.len() + after.len());
        new_line.push_str(before);
        new_line.push_str(&edit.new_text);
        new_line.push_str(after);
        lines[start_line] = new_line;
    } else if end_line < lines.len() {
        let start_col = u16_to_byte_offset(&lines[start_line], edit.range.start.character as usize);
        let end_col = u16_to_byte_offset(&lines[end_line], edit.range.end.character as usize);
        let before = lines[start_line][..start_col.min(lines[start_line].len())].to_string();
        let after = lines[end_line][end_col.min(lines[end_line].len())..].to_string();
        let mut new_line = String::with_capacity(before.len() + edit.new_text.len() + after.len());
        new_line.push_str(&before);
        new_line.push_str(&edit.new_text);
        new_line.push_str(&after);
        let _ = lines.splice(start_line..=end_line, std::iter::once(new_line));
    }
}

fn summarize_entries(path: &std::path::Path, entries: &[oxi_lsp::PublishedDiagnostics]) -> String {
    if entries.is_empty() {
        return format!("{}: no diagnostics", path.display());
    }
    let mut out = String::new();
    for entry in entries {
        let counts = count_diagnostics(&entry.diagnostics);
        out.push_str(&format!(
            "{}: {} diagnostics ({} errors, {} warnings)\n",
            entry.uri, counts.total, counts.errors, counts.warnings
        ));
    }
    out
}

#[derive(Default)]
struct DiagnosticCounts {
    total: usize,
    errors: usize,
    warnings: usize,
}

fn count_diagnostics(value: &serde_json::Value) -> DiagnosticCounts {
    let mut c = DiagnosticCounts::default();
    if let Some(arr) = value.as_array() {
        c.total = arr.len();
        for d in arr {
            if let Some(sev) = d.get("severity").and_then(|v| v.as_u64()) {
                match sev {
                    1 => c.errors += 1,
                    2 => c.warnings += 1,
                    _ => {}
                }
            }
        }
    }
    c
}
