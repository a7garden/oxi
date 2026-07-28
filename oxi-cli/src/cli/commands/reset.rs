//! Reset subcommand handler and helper utilities.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Target descriptor for the reset command.
struct ResetTarget {
    label: String,
    path: PathBuf,
    description: String,
}

/// Handle `oxi reset [--yes] [--include-project]`
///
/// Factory-reset: deletes ALL oxi data.
/// Optionally also deletes the project-local `.oxi/` directory.
pub fn handle_reset(yes: bool, include_project: bool) -> Result<()> {
    use std::io::{self, Write};

    // ── Collect targets ──────────────────────────────────────────
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;

    let oxi_dir = home.join(".oxi");
    let config_oxi_dir = dirs::config_dir()
        .unwrap_or_else(|| home.join(".config"))
        .join("oxi");
    let cache_oxi_dir = dirs::cache_dir()
        .unwrap_or_else(|| home.join(".cache"))
        .join("oxi");

    let mut targets: Vec<ResetTarget> = vec![];

    // ~/.oxi/ — split into sub-items for clarity
    if oxi_dir.exists() {
        let sub_items = [
            ("settings.toml", "global settings"),
            ("settings.json", "global settings (JSON)"),
            ("auth.json", "credentials (API keys, OAuth tokens)"),
            ("sessions", "session history"),
            ("skills", "skills"),
            ("extensions", "extensions"),
            ("packages", "packages"),
        ];
        let mut has_sub = false;
        for (name, desc) in &sub_items {
            let p = oxi_dir.join(name);
            if p.exists() {
                has_sub = true;
                targets.push(ResetTarget {
                    label: format!("~/.oxi/{}", name),
                    path: p,
                    description: desc.to_string(),
                });
            }
        }
        // If no known sub-items found, target the whole directory
        if !has_sub {
            targets.push(ResetTarget {
                label: "~/.oxi".to_string(),
                path: oxi_dir.clone(),
                description: "oxi home (settings, sessions, skills, extensions, packages)"
                    .to_string(),
            });
        }
    }

    // ~/.config/oxi/ — MCP config, alternative auth location
    if config_oxi_dir.exists() {
        targets.push(ResetTarget {
            label: display_path(&config_oxi_dir),
            path: config_oxi_dir,
            description: "MCP config, credentials".to_string(),
        });
    }

    // ~/.cache/oxi/ — logs
    if cache_oxi_dir.exists() {
        targets.push(ResetTarget {
            label: display_path(&cache_oxi_dir),
            path: cache_oxi_dir,
            description: "logs, cache".to_string(),
        });
    }

    // Project-local .oxi/
    let project_oxi = std::env::current_dir().unwrap_or_default().join(".oxi");
    let mut project_target: Option<ResetTarget> = None;
    if include_project && project_oxi.exists() {
        project_target = Some(ResetTarget {
            label: display_path(&project_oxi),
            path: project_oxi.clone(),
            description: "project settings".to_string(),
        });
    }

    let total_count = targets.len() + usize::from(project_target.is_some());
    if total_count == 0 {
        println!("Nothing to reset — no oxi data found.");
        return Ok(());
    }

    // ── Calculate total size ─────────────────────────────────────
    let mut total_bytes: u64 = 0;
    for t in &targets {
        total_bytes += dir_size_bytes(&t.path);
    }
    if let Some(ref pt) = project_target {
        total_bytes += dir_size_bytes(&pt.path);
    }

    // ── Show what will be deleted ────────────────────────────────
    eprintln!();
    eprintln!("     ⚠ Warning: The following will be permanently deleted:");
    eprintln!();
    for (i, t) in targets.iter().enumerate() {
        eprintln!(
            "       {}. {} ({})",
            i + 1,
            display_path(&t.path),
            dir_size_human(&t.path)
        );
        eprintln!("          {}", t.description);
    }
    if let Some(ref pt) = project_target {
        eprintln!(
            "       {}. {} ({})",
            total_count,
            display_path(&pt.path),
            dir_size_human(&pt.path)
        );
        eprintln!("          {}", pt.description);
    }
    eprintln!();
    eprintln!(
        "     Total: {} item(s), {}",
        total_count,
        bytes_human(total_bytes)
    );
    eprintln!();
    eprintln!(
        "     This cannot be undone. All sessions, skills, extensions, and settings will be deleted."
    );
    eprintln!();

    if !yes {
        eprint!("     Type RESET to continue: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim() != "RESET" {
            eprintln!();
            eprintln!();
            eprintln!("     Cancelled.");
            return Ok(());
        }
    }

    // ── Delete ───────────────────────────────────────────────────
    eprintln!();
    let mut errors = Vec::new();

    for t in &targets {
        eprint!("     ● Deleting {}...", t.label);
        io::stdout().flush()?;
        match remove_path(&t.path) {
            Ok(()) => eprintln!(" done"),
            Err(e) => {
                eprintln!(" failed");
                eprintln!("       ✗ {}: {}", t.label, e);
                errors.push(format!("{}: {}", t.label, e));
            }
        }
    }
    if let Some(ref pt) = project_target {
        eprint!("     ● Deleting {}...", pt.label);
        io::stdout().flush()?;
        match remove_path(&pt.path) {
            Ok(()) => eprintln!(" done"),
            Err(e) => {
                eprintln!(" failed");
                eprintln!("       ✗ {}: {}", pt.label, e);
                errors.push(format!("{}: {}", pt.label, e));
            }
        }
    }

    eprintln!();
    if errors.is_empty() {
        eprintln!("     ✓ All oxi data has been reset.");
        eprintln!("     → Run 'oxi setup' to reconfigure.");
    } else {
        eprintln!("     ⚠ {} item(s) failed to delete:", errors.len());
        for err in &errors {
            eprintln!("       • {}", err);
        }
        eprintln!("     Some data may need manual cleanup.");
    }

    Ok(())
}

/// Remove a file or directory (including all contents).
pub fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Display path with ~/ abbreviation for home directory.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        let path_str = path.to_string_lossy();
        if let Some(rest) = path_str.strip_prefix(home_str.as_ref()) {
            return format!("~{}", rest);
        }
    }
    path.display().to_string()
}

/// Calculate total bytes in a directory or file.
pub fn dir_size_bytes(path: &Path) -> u64 {
    let mut total: u64 = 0;
    if path.is_dir() {
        if let Ok(entries) = walkdir_recursive(path) {
            for entry in entries {
                if let Ok(meta) = std::fs::metadata(&entry)
                    && meta.is_file()
                {
                    total += meta.len();
                }
            }
        }
    } else if let Ok(meta) = std::fs::metadata(path) {
        total = meta.len();
    }
    total
}

/// Calculate a human-readable directory or file size.
pub fn dir_size_human(path: &Path) -> String {
    bytes_human(dir_size_bytes(path))
}

/// Format bytes as a human-readable string.
pub fn bytes_human(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Walk a directory recursively, collecting all file paths.
pub fn walkdir_recursive(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                result.extend(walkdir_recursive(&path)?);
            } else {
                result.push(path);
            }
        }
    }
    Ok(result)
}
