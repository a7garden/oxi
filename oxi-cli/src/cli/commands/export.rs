//! Export / Import / Share subcommand handlers.

use anyhow::Result;
use std::path::Path;

/// Handle `oxi export [SESSION_ID] [--output PATH]`
pub fn handle_export(session_id: Option<&str>, output_path: Option<&Path>) -> Result<()> {
    use crate::store::session::SessionManager;

    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .to_string();

    // Resolve session file
    let session_path = if let Some(sid) = session_id {
        // Try as a direct path first
        let direct = std::path::Path::new(sid);
        if direct.exists() {
            direct.to_path_buf()
        } else {
            anyhow::bail!("Session not found: {}", sid);
        }
    } else {
        // Find the most recent session for this CWD
        // SessionManager::list is async but only does std::fs I/O internally.
        let sessions = std::thread::scope(|s| {
            s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(SessionManager::list(&cwd, None))
            })
            .join()
            .map_err(|e| anyhow::anyhow!("thread panicked: {:?}", e))?
        })?;
        let most_recent = sessions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No sessions found for this project"))?;
        // Reconstruct the path from session_dir + id
        let session_dir: std::path::PathBuf =
            crate::store::session::get_default_session_dir(&cwd).into();
        session_dir.join(format!("{}.jsonl", most_recent.id))
    };

    if !session_path.exists() {
        anyhow::bail!("Session file not found: {}", session_path.display());
    }

    // Load session entries
    let sm = SessionManager::open(&session_path.to_string_lossy(), None, Some(&cwd));
    let branch = sm.get_branch(None);

    // Build metadata
    let meta = crate::storage::export::ExportMeta {
        model: None,
        provider: None,
        exported_at: chrono::Utc::now().timestamp_millis(),
        total_user_tokens: None,
        total_assistant_tokens: None,
    };

    let entries: Vec<crate::store::session::SessionEntry> = branch.into_iter().collect();
    let html = crate::storage::export::export_to_html(
        &entries,
        &meta,
        &crate::storage::export::HtmlExportOptions::default(),
    )?;

    // Determine output path
    let out = if let Some(p) = output_path {
        p.to_path_buf()
    } else {
        let sid_short = session_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session");
        let short = &sid_short[..8.min(sid_short.len())];
        std::path::PathBuf::from(format!("oxi-export-{}.html", short))
    };

    std::fs::write(&out, &html)?;
    println!(
        "Exported {} entries to {} ({} bytes)",
        entries.len(),
        out.display(),
        html.len()
    );
    Ok(())
}

/// Handle `oxi import <PATH>`
pub fn handle_import(path: &Path) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("File not found: {}", path.display());
    }

    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .to_string();

    let resolved = crate::store::session::resolve_session_path(&path.to_string_lossy(), &cwd)
        .map_err(|e| anyhow::anyhow!("Error resolving path: {}", e))?;

    if !std::path::Path::new(&resolved).exists() {
        anyhow::bail!("File not found: {}", resolved);
    }

    // Copy the session file into the sessions directory
    let sessions_dir: std::path::PathBuf =
        crate::store::session::get_default_session_dir(&cwd).into();
    std::fs::create_dir_all(&sessions_dir)?;

    let filename = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("imported.jsonl"));
    let dest = sessions_dir.join(filename);

    // Avoid overwriting existing sessions
    if dest.exists() {
        let stem = dest
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported");
        let ext = dest.extension().and_then(|s| s.to_str()).unwrap_or("jsonl");
        let unique_name = format!(
            "{}-{}.{}",
            stem,
            chrono::Utc::now().format("%Y%m%d%H%M%S"),
            ext
        );
        let alt_dest = sessions_dir.join(&unique_name);
        std::fs::copy(path, &alt_dest)?;
        println!("Imported session to {}", alt_dest.display());
    } else {
        std::fs::copy(path, &dest)?;
        println!("Imported session to {}", dest.display());
    }
    Ok(())
}

/// Handle `oxi share [SESSION_ID]`
pub async fn handle_share(session_id: Option<&str>) -> Result<()> {
    // Check if gh CLI is available
    let gh_check = std::process::Command::new("gh")
        .args(["auth", "status"])
        .output()?;

    if !gh_check.status.success() {
        anyhow::bail!("GitHub CLI (gh) is not authenticated. Run: gh auth login");
    }

    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .to_string();

    // Resolve session
    let session_path = if let Some(sid) = session_id {
        let direct = std::path::Path::new(sid);
        if direct.exists() {
            direct.to_path_buf()
        } else {
            anyhow::bail!("Session not found: {}", sid);
        }
    } else {
        let sessions = crate::store::session::SessionManager::list(&cwd, None).await?;
        let most_recent = sessions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No sessions found for this project"))?;
        let session_dir: std::path::PathBuf =
            crate::store::session::get_default_session_dir(&cwd).into();
        session_dir.join(format!("{}.jsonl", most_recent.id))
    };

    if !session_path.exists() {
        anyhow::bail!("Session file not found: {}", session_path.display());
    }

    let sm = crate::store::session::SessionManager::open(
        &session_path.to_string_lossy(),
        None,
        Some(&cwd),
    );
    let branch = sm.get_branch(None);
    let entries: Vec<crate::store::session::SessionEntry> = branch.into_iter().collect();

    let meta = crate::storage::export::ExportMeta {
        model: None,
        provider: None,
        exported_at: chrono::Utc::now().timestamp_millis(),
        total_user_tokens: None,
        total_assistant_tokens: None,
    };

    let html = crate::storage::export::export_to_html(
        &entries,
        &meta,
        &crate::storage::export::HtmlExportOptions::default(),
    )?;

    let temp_path = std::env::temp_dir().join("oxi-share-export.html");
    std::fs::write(&temp_path, &html)?;

    // Create gist
    let output = tokio::process::Command::new("gh")
        .args(["gist", "create", &temp_path.to_string_lossy()])
        .output()
        .await?;

    let _ = std::fs::remove_file(&temp_path);

    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        println!("Gist created: {}", url);
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create gist: {}", stderr.trim());
    }
    Ok(())
}
