//! Session listing, tree, fork, and delete handlers.

use crate::store::session::{AgentMessage, SessionManager};
use anyhow::Result;
use uuid::Uuid;

/// `oxicode sessions` — list sessions for the current project.
pub async fn handle_sessions() -> Result<()> {
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .to_string();
    let sessions = crate::store::session::SessionManager::list(&cwd, None).await?;
    if sessions.is_empty() {
        println!("No sessions found.");
    } else {
        println!("Sessions:");
        println!(
            "{:<20} {:>6} {:<30} {:>12}",
            "NAME", "MSG", "PREVIEW", "TIME"
        );
        println!("{:-<20} {:-<6} {:-<30} {:-<12}", "", "", "", "");
        for session in &sessions {
            let name = session.name.as_deref().unwrap_or("-");
            let preview = if session.first_message.len() > 28 {
                format!("{}...", &session.first_message[..28])
            } else {
                session.first_message.clone()
            };
            let time = chrono::DateTime::<chrono::Local>::from(session.modified)
                .format("%m-%d %H:%M")
                .to_string();
            println!(
                "{:<20} {:>6} {:<30} {:>12}",
                &name[..name.len().min(20)],
                session.message_count,
                preview,
                time
            );
        }
    }
    Ok(())
}

/// `oxicode tree <session_id>` — show the conversation tree for a session.
pub async fn handle_tree(session_id: &str) -> Result<()> {
    let manager = SessionManager::new().await?;
    show_tree(&manager, session_id).await
}

/// `oxicode fork <parent_id> <entry_id>` — branch a new session from an entry.
pub async fn handle_fork(parent_id: &str, entry_id: &str) -> Result<()> {
    let manager = SessionManager::new().await?;
    fork_session(&manager, parent_id, entry_id).await
}

/// `oxicode delete <session_id>` — delete a session file.
pub async fn handle_delete(session_id: &str) -> Result<()> {
    let manager = SessionManager::new().await?;
    delete_session(&manager, session_id).await
}

/// List all sessions via the manager API (legacy fallback).
#[allow(dead_code)]
pub async fn list_sessions(manager: &SessionManager) -> Result<()> {
    let sessions = manager.list_sessions().await?;

    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("Sessions:");
    println!("{:<36} {:<20} UPDATED", "ID", "BRANCH");
    println!("{:-<36} {:-<20} {:-<20}", "", "", "");

    for meta in sessions {
        let branch_str = if let Some(ref pid) = meta.parent_id {
            format!("forked from {}", &pid.to_string()[..8])
        } else {
            "root".to_string()
        };
        let updated = chrono::DateTime::from_timestamp_millis(meta.updated_at)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!("{:<36} {:<20} {}", meta.id, branch_str, updated);
    }

    Ok(())
}

/// Render the tree representation of a session.
pub async fn show_tree(manager: &SessionManager, session_id: &str) -> Result<()> {
    let id = if session_id.is_empty() {
        // Get most recent session
        let sessions = manager.list_sessions().await?;
        match sessions.first() {
            Some(s) => s.id,
            None => {
                println!("No sessions found.");
                return Ok(());
            }
        }
    } else {
        Uuid::parse_str(session_id)?
    };

    let tree = manager.get_tree(id)?;
    let branch_info = manager.get_branch_info(id).await?;

    if let Some(info) = branch_info
        && let Some(ref pid) = info.parent_session_id
    {
        println!("Session: {} (branched from {})", id, pid);
    } else {
        println!("Session: {} (root)", id);
    }
    println!();

    // Show tree structure
    for node in &tree {
        let role_marker = match &node.entry.message {
            AgentMessage::User { .. } => "U",
            AgentMessage::Assistant { .. } => "A",
            AgentMessage::System { .. } => "S",
            _ => "-",
        };

        let content_preview = truncate(&node.entry.content(), 60);
        let prefix = if node.entry.parent_id.is_some() {
            "├─"
        } else {
            "└─"
        };

        println!(
            "  {}{} [{:.8}] {}",
            prefix, role_marker, node.entry.id, content_preview
        );
    }

    Ok(())
}

/// Fork a new session from a specific entry point.
pub async fn fork_session(
    manager: &SessionManager,
    parent_id_str: &str,
    entry_id_str: &str,
) -> Result<()> {
    let sessions = manager.list_sessions().await?;
    let info = sessions
        .iter()
        .find(|s| s.id.to_string().starts_with(parent_id_str))
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", parent_id_str))?;
    let entry_id = Uuid::parse_str(entry_id_str)
        .map_err(|_| anyhow::anyhow!("Invalid entry ID: {}", entry_id_str))?;
    let (new_session_id, _) = manager.branch_from(info.id, entry_id).await?;
    println!("Created forked session: {}", new_session_id);
    println!("File: {}", manager.session_path(&new_session_id).display());
    Ok(())
}

/// Delete a session by id prefix.
pub async fn delete_session(manager: &SessionManager, session_id: &str) -> Result<()> {
    let sessions = manager.list_sessions().await?;
    let info = sessions
        .iter()
        .find(|s| s.id.to_string().starts_with(session_id))
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;
    let path = manager.session_path(&info.id);
    manager.delete(info.id).await?;
    println!("Deleted session: {}", path.display());
    Ok(())
}

/// Truncate a string to a maximum number of bytes, preserving a UTF-8 boundary.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let boundary = s
        .char_indices()
        .take_while(|(i, _)| *i <= max_len.saturating_sub(3))
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}...", &s[..boundary])
}
