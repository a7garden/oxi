// oxicode-cli/src/tui_vt/issues_panel/input.rs
//! Key handling for the `/issue` panel — checked before all other key
//! handlers whenever `state.issues_panel.is_some()`, mirroring
//! `handle_overlay_key` / `handle_confirmation_key` / `handle_file_search_key`.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;

use super::{FormField, IssueActionRequest, IssueFormState, IssuesPanelMode};
use crate::tui_vt::main_loop::RenderState;

/// Returns `true` if the key was consumed by the panel (caller must not fall
/// through to composer/global key handling).
pub(crate) fn handle_issues_panel_key(
    state: &Arc<Mutex<RenderState>>,
    issue_action_tx: &UnboundedSender<IssueActionRequest>,
    key: KeyEvent,
) -> bool {
    // Destructure so the existing arms can stay keyed on `code`; the
    // Ctrl+U guard in FilterInput needs the original `key` for modifiers.
    let code = key.code;
    let mut s = state.lock();
    let Some(panel) = s.issues_panel.as_mut() else {
        return false;
    };
    match &mut panel.mode {
        IssuesPanelMode::List => match code {
            KeyCode::Char('j') | KeyCode::Down => {
                if !panel.rows.is_empty() {
                    panel.selected = (panel.selected + 1).min(panel.rows.len() - 1);
                }
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                panel.selected = panel.selected.saturating_sub(1);
                true
            }
            KeyCode::Char('f') => {
                panel.status_filter = if panel.status_filter.is_some() {
                    None
                } else {
                    Some(oxicode_sdk::Status::Open)
                };
                let store = s.issue_store.clone();
                if let (Some(store), Some(panel)) = (store, s.issues_panel.as_mut()) {
                    panel.refresh(&store, &store.issues_dir());
                }
                true
            }
            KeyCode::Char('/') => {
                panel.mode = IssuesPanelMode::FilterInput(String::new());
                true
            }
            KeyCode::Enter => {
                if let Some(row) = panel.rows.get(panel.selected) {
                    let id = row.id;
                    let store = s.issue_store.clone();
                    if let Some(store) = store
                        && let Ok((issue, _hash)) = store.read(id)
                        && let Some(panel) = s.issues_panel.as_mut()
                    {
                        panel.detail_body_cache = Some(issue.body);
                        panel.mode = IssuesPanelMode::Detail { id, scroll: 0 };
                    }
                }
                true
            }
            KeyCode::Esc => {
                s.issues_panel = None;
                true
            }
            // F3 fix: input lock — while an async mutation is in flight
            // (`panel.pending == true`), the mutating keys are still
            // consumed (so they don't leak to the composer) but they do
            // NOT dispatch. Visible feedback comes from the "(busy…)"
            // title marker in render_list / render_detail.
            KeyCode::Char('c') if !panel.pending => {
                // List-mode close: opens the same y/n confirmation as Detail;
                // dispatch lives in handle_confirmation_key (not here).
                if let Some(row) = panel.rows.get(panel.selected) {
                    let id = row.id;
                    s.confirmation = Some(crate::tui_vt::main_loop::ModalConfirmation {
                        title: "Close issue".into(),
                        message: format!("  y \u{2014} close #{id}     n / x \u{2014} cancel"),
                        action: crate::tui_vt::main_loop::ConfirmationAction::CloseIssue(id),
                    });
                }
                true
            }
            KeyCode::Char('c') => true,
            KeyCode::Char('r') if !panel.pending => {
                // List-mode reopen: reads fresh hash from store and sends the
                // request immediately (no confirmation gate — reopen is cheap
                // to reverse and the same action is live in Detail mode).
                if let Some(row) = panel.rows.get(panel.selected) {
                    let id = row.id;
                    let hash = s
                        .issue_store
                        .as_ref()
                        .and_then(|store| store.read(id).ok())
                        .map(|(_, h)| h);
                    let _ = issue_action_tx.send(IssueActionRequest::Reopen { id, hash });
                    if let Some(panel) = s.issues_panel.as_mut() {
                        panel.pending = true;
                    }
                }
                true
            }
            KeyCode::Char('r') => true,
            KeyCode::Char('n') => {
                // New issue — drop into a fresh Form. The user's title/priority/
                // labels/body start at defaults; `editing_id = None` makes the
                // submit path take the Create branch (sync `store.create`).
                panel.mode = IssuesPanelMode::Form(Box::default());
                true
            }
            KeyCode::Char('e') => {
                // Edit the selected row — same logic as Detail's 'e'
                // (live-claim gate + pre-filled form) via `start_edit`.
                if let Some(row) = panel.rows.get(panel.selected) {
                    let id = row.id;
                    start_edit(&mut s, id);
                }
                true
            }
            _ => true,
        },
        IssuesPanelMode::FilterInput(buf) => match code {
            // Order-sensitive: this guarded arm MUST come before the plain
            // `KeyCode::Char(c)` arm below, or the plain arm would swallow
            // `'u'` first and the clear-buffer shortcut would be unreachable.
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                buf.clear();
                true
            }
            KeyCode::Char(c) => {
                buf.push(c);
                true
            }
            KeyCode::Backspace => {
                buf.pop();
                true
            }
            KeyCode::Enter => {
                let buf = buf.clone();
                let extra = super::parse_issue_filter(&buf, panel.status_filter);
                panel.extra_filter = extra;
                panel.mode = IssuesPanelMode::List;
                let store = s.issue_store.clone();
                if let (Some(store), Some(panel)) = (store, s.issues_panel.as_mut()) {
                    panel.refresh(&store, &store.issues_dir());
                }
                true
            }
            KeyCode::Esc => {
                panel.mode = IssuesPanelMode::List;
                true
            }
            _ => true, // consume everything else in FilterInput mode (e.g. arrows)
        },
        IssuesPanelMode::Detail { id, scroll } => {
            let id = *id;
            match code {
                KeyCode::Esc => {
                    panel.mode = IssuesPanelMode::List;
                    true
                }
                KeyCode::Char('j') | KeyCode::Down | KeyCode::PageDown => {
                    // PageDown jumps a screen-height-equivalent chunk (10 lines)
                    // so a long body can be scrolled through without hammering
                    // `j`. Matches design §5 Detail scroll contract.
                    let step = if matches!(code, KeyCode::PageDown) {
                        10usize
                    } else {
                        1usize
                    };
                    *scroll = scroll.saturating_add(step);
                    true
                }
                KeyCode::Char('k') | KeyCode::Up | KeyCode::PageUp => {
                    // PageUp mirrors PageDown's 10-line step. `saturating_sub`
                    // keeps `scroll` at 0 when the user pages above the top.
                    let step = if matches!(code, KeyCode::PageUp) {
                        10usize
                    } else {
                        1usize
                    };
                    *scroll = scroll.saturating_sub(step);
                    true
                }
                // F3 fix: input lock (mirror of List). While a pending
                // async mutation is in flight, `c`/`r` still consume the
                // key but never dispatch — the "(busy…)" title marker
                // gives the user visible feedback.
                KeyCode::Char('c') if !panel.pending => {
                    s.confirmation = Some(crate::tui_vt::main_loop::ModalConfirmation {
                        title: "Close issue".into(),
                        message: format!("  y \u{2014} close #{id}     n / x \u{2014} cancel"),
                        action: crate::tui_vt::main_loop::ConfirmationAction::CloseIssue(id),
                    });
                    true
                }
                KeyCode::Char('c') => true,
                KeyCode::Char('r') if !panel.pending => {
                    let hash = s
                        .issue_store
                        .as_ref()
                        .and_then(|store| store.read(id).ok())
                        .map(|(_, h)| h);
                    let _ = issue_action_tx.send(IssueActionRequest::Reopen { id, hash });
                    if let Some(panel) = s.issues_panel.as_mut() {
                        panel.pending = true;
                    }
                    true
                }
                KeyCode::Char('r') => true,
                KeyCode::Char('e') => {
                    // F4 fix: design §5 — Detail's 'e' behaves like List's,
                    // targeting the currently-open issue. Shared logic lives
                    // in `start_edit` (live-claim gate + pre-filled form).
                    start_edit(&mut s, id);
                    true
                }
                _ => true,
            }
        }
        IssuesPanelMode::Form(form) => match code {
            // Esc cancels and returns to the List. We discard the form data
            // (unsubmitted edits are dropped — design §5: "cancel discards").
            KeyCode::Esc => {
                panel.mode = IssuesPanelMode::List;
                true
            }
            // Tab cycles forward through Title → Priority → Labels → Body →
            // Title. Shift+Tab (BackTab in crossterm) cycles backward. The
            // Shift+Tab guard MUST come before the plain Backspace arm, and
            // the Backspace arm MUST come before the Body fall-through
            // (Backspace edits the body, not the form field).
            KeyCode::BackTab => {
                form.focus = match form.focus {
                    FormField::Title => FormField::Body,
                    FormField::Priority => FormField::Title,
                    FormField::Labels => FormField::Priority,
                    FormField::Body => FormField::Labels,
                };
                true
            }
            KeyCode::Tab => {
                form.focus = match form.focus {
                    FormField::Title => FormField::Priority,
                    FormField::Priority => FormField::Labels,
                    FormField::Labels => FormField::Body,
                    FormField::Body => FormField::Title,
                };
                true
            }
            // Priority is the only field with discrete left/right semantics
            // — Left/Right cycle the priority when focus is on this field
            // (per brief). The fallback Body catch-all below would forward
            // arrows to the textarea, so the priority-field guard is what
            // actually catches Left/Right here.
            KeyCode::Left if form.focus == FormField::Priority => {
                form.priority = super::cycle_priority(form.priority, false);
                true
            }
            KeyCode::Right if form.focus == FormField::Priority => {
                form.priority = super::cycle_priority(form.priority, true);
                true
            }
            KeyCode::Char(c) if form.focus == FormField::Title => {
                form.title.push(c);
                true
            }
            KeyCode::Backspace if form.focus == FormField::Title => {
                form.title.pop();
                true
            }
            KeyCode::Char(c) if form.focus == FormField::Labels => {
                form.labels_input.push(c);
                true
            }
            KeyCode::Backspace if form.focus == FormField::Labels => {
                form.labels_input.pop();
                true
            }
            // F4 fix (design §5): Ctrl+Enter submits from ANY focus
            // (Title/Priority/Labels/Body) — the design specifies
            // `Ctrl+Enter: 제출` unconditionally. The plain `KeyCode::Enter`
            // arm and the Body fall-through below are NOT reached when
            // Ctrl+Enter arrives because this guarded arm matches first
            // (crossterm evaluates match arms top-to-bottom).
            //
            // Per carry-forward: global Ctrl+Enter is gated out by Task 5
            // when the panel is open, so a Ctrl+Enter here always means
            // "submit the form".
            //
            // F3 fix: while an async edit is in flight (`pending == true`)
            // the submit is consumed but does NOT dispatch — input lock
            // mirrors List/Detail's `c`/`r` gate.
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) && !panel.pending => {
                submit_form(&mut s, issue_action_tx);
                true
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => true,
            // Body fall-through: hand the full KeyEvent to the textarea so
            // every editing primitive (arrows, paste, undo, etc.) keeps
            // working. Only reached when focus == Body AND the key is not
            // one of the guarded forms above.
            _ if form.focus == FormField::Body => {
                form.body.input(key);
                true
            }
            // Consume any other key — the form has a closed set of inputs,
            // and unhandled keys should not leak to the composer or global
            // hotkeys below us.
            _ => true,
        },
    }
}

/// Open the Create/Edit form pre-filled from issue `id`.
///
/// Shared by List's and Detail's `'e'` handlers (design §5: Detail behaves
/// like List, targeting the open issue). Blocks entry — setting
/// `panel.error`, rendered as the red footer — when a live OTHER-session
/// claim holds the issue (design §6 gate at entry; the write itself is
/// CAS-guarded later). No-ops when the store is missing or the read fails;
/// the caller consumes the key either way.
fn start_edit(s: &mut parking_lot::MutexGuard<'_, crate::tui_vt::main_loop::RenderState>, id: u32) {
    let this_session = s.ownership_session_id.clone();
    let Some(store) = s.issue_store.clone() else {
        return;
    };
    let Ok((issue, hash)) = store.read(id) else {
        return;
    };
    let Some(panel) = s.issues_panel.as_mut() else {
        return;
    };
    if let Some(a) = &issue.meta.assigned_to
        && a.session != this_session
        && oxicode_sdk::liveness::is_session_alive(&store.issues_dir(), &a.session)
    {
        panel.error = Some(format!(
            "issue #{id} is being worked on by session {} (since {})",
            a.session,
            a.acquired_at.format("%m-%d %H:%M")
        ));
        return;
    }
    let mut form = IssueFormState {
        editing_id: Some(id),
        content_hash: Some(hash),
        title: issue.meta.title.clone(),
        priority: issue.meta.priority,
        labels_input: issue.meta.labels.join(", "),
        ..IssueFormState::default()
    };
    form.body.set_text(&issue.body);
    panel.mode = IssuesPanelMode::Form(Box::new(form));
}

/// Submit the Create/Edit form currently held in `s.issues_panel`.
///
/// Per carry-forward the panel/mode/form types are **not** `Clone` — the
/// brief's `s.issues_panel.as_ref().map(|p| p.mode.clone())` would not
/// compile. We borrow `&mut s.issues_panel` and CLONE the plain-data fields
/// the dispatch needs (strings/`Vec` are `Clone`; only the enclosing structs
/// aren't), then drop the form (`panel.mode = List`) only after a successful
/// dispatch. Nothing is ever `mem::take`n out of the mounted form, so a
/// failed create leaves every field the user typed intact for retry — the
/// failure is surfaced via `panel.error` (rendered as the red footer).
/// The Create branch writes synchronously via `FileIssueStore::create` and
/// then refreshes; the Edit branch queues an async `ApplyPatch` action and
/// flips `panel.pending = true`.
fn submit_form(
    s: &mut parking_lot::MutexGuard<'_, crate::tui_vt::main_loop::RenderState>,
    issue_action_tx: &tokio::sync::mpsc::UnboundedSender<IssueActionRequest>,
) {
    // Clone the submission data out while the form is still mounted; the
    // borrow ends at this block, so the dispatch below can re-take
    // `s.issues_panel` to flip modes.
    let (editing_id, content_hash, title, priority, labels, body) = {
        let Some(panel) = s.issues_panel.as_mut() else {
            return;
        };
        let IssuesPanelMode::Form(form) = &panel.mode else {
            return;
        };
        (
            form.editing_id,
            form.content_hash.clone(),
            form.title.clone(),
            form.priority,
            super::parse_labels(&form.labels_input),
            form.body.text().to_string(),
        )
    };

    match editing_id {
        None => {
            // Create path — synchronous; `FileIssueStore::create` is not
            // CAS-guarded so we don't need to round-trip through the
            // `issue_action_tx` channel.
            let Some(store) = s.issue_store.clone() else {
                // Keep the form mounted (and intact) and say why.
                if let Some(panel) = s.issues_panel.as_mut() {
                    panel.error = Some("issue store unavailable".into());
                }
                return;
            };
            match store.create(
                title,
                body,
                priority,
                labels,
                Some(s.ownership_session_id.as_str()),
            ) {
                Ok(_issue) => {
                    if let Some(panel) = s.issues_panel.as_mut() {
                        panel.mode = IssuesPanelMode::List;
                        panel.error = None;
                        panel.refresh(&store, &store.issues_dir());
                    }
                }
                // Failure keeps the form mounted — and, because the dispatch
                // above worked on clones, fully intact — so the user can
                // fix the input and retry (error shown via the footer).
                Err(e) => {
                    if let Some(panel) = s.issues_panel.as_mut() {
                        panel.error = Some(e.to_string());
                    }
                }
            }
        }
        Some(id) => {
            // Edit path — async (CAS-guarded by `apply_patch`); queue an
            // action and let the event loop dispatch it (Task 9 replaces
            // the no-op `dispatch_action` with a real store call).
            let patch = oxicode_sdk::IssuePatch {
                title: Some(title),
                body: Some(body),
                priority: Some(priority),
                labels: Some(labels),
                ..Default::default()
            };
            let _ = issue_action_tx.send(IssueActionRequest::ApplyPatch {
                id,
                patch,
                caller: (!s.ownership_session_id.is_empty())
                    .then(|| s.ownership_session_id.clone()),
                hash: content_hash,
            });
            if let Some(panel) = s.issues_panel.as_mut() {
                panel.pending = true;
                panel.mode = IssuesPanelMode::List;
            }
        }
    }
}
