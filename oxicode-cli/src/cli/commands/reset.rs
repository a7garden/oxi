//! Reset subcommand handler and helper utilities.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Target descriptor for the reset command.
struct ResetTarget {
    label: String,
    path: PathBuf,
    description: String,
}

/// Handle `oxicode reset [--yes] [--include-project]`
///
/// Factory-reset: deletes ALL oxicode data.
/// Optionally also deletes the project-local `.oxicode/` directory.
pub fn handle_reset(yes: bool, include_project: bool) -> Result<()> {
    use std::io::{self, Write};

    // ── Collect targets ──────────────────────────────────────────
    // Canonical home (`$OXICODE_HOME`, else `$OXI_HOME/oxicode`, else
    // `~/.oxi/oxicode`). The legacy `~/.oxicode` is only touched when the
    // canonical home does not exist (pre-unified-layout installs).
    let oxicode_dir = oxicode_catalog::oxi_home::oxicode_home()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine oxicode home directory"))?;
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;

    let config_oxicode_dir = dirs::config_dir()
        .unwrap_or_else(|| home.join(".config"))
        .join("oxicode");
    let cache_oxicode_dir = dirs::cache_dir()
        .unwrap_or_else(|| home.join(".cache"))
        .join("oxicode");

    let project_oxicode = std::env::current_dir().unwrap_or_default().join(".oxicode");
    let (targets, project_target) = collect_reset_targets(
        &oxicode_dir,
        oxicode_catalog::oxi_home::legacy_home_dir().as_deref(),
        oxicode_catalog::oxi_home::migration_journal_path().as_deref(),
        &config_oxicode_dir,
        &cache_oxicode_dir,
        &project_oxicode,
        include_project,
    );

    let total_count = targets.len() + usize::from(project_target.is_some());
    if total_count == 0 {
        println!("Nothing to reset — no oxicode data found.");
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
        eprintln!("     ✓ All oxicode data has been reset.");
        eprintln!("     → Run 'oxicode setup' to reconfigure.");
    } else {
        eprintln!("     ⚠ {} item(s) failed to delete:", errors.len());
        for err in &errors {
            eprintln!("       • {}", err);
        }
        eprintln!("     Some data may need manual cleanup.");
    }

    Ok(())
}

/// Pure target collection for the reset command (testable with injected
/// paths).
///
/// - The canonical oxicode home is reset when it exists; the legacy
///   `~/.oxicode` only when the canonical home is absent.
/// - The home-layout migration journal is collected when present (it is
///   stale once the canonical home is gone).
/// - `config_dir`/`cache_dir` handling is unchanged (always collected when
///   present).
/// - The project-local `.oxicode/` is collected only with `include_project`.
fn collect_reset_targets(
    oxicode_dir: &Path,
    legacy_dir: Option<&Path>,
    journal_path: Option<&Path>,
    config_dir: &Path,
    cache_dir: &Path,
    project_dir: &Path,
    include_project: bool,
) -> (Vec<ResetTarget>, Option<ResetTarget>) {
    let mut targets: Vec<ResetTarget> = vec![];

    // Canonical home (or legacy, when canonical is absent) — split into
    // sub-items for clarity.
    let home_dir_to_reset: Option<&Path> = if oxicode_dir.exists() {
        Some(oxicode_dir)
    } else {
        legacy_dir
    };
    if let Some(reset_dir) = home_dir_to_reset {
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
            let p = reset_dir.join(name);
            if p.exists() {
                has_sub = true;
                targets.push(ResetTarget {
                    label: display_path(&p),
                    path: p,
                    description: desc.to_string(),
                });
            }
        }
        // If no known sub-items found, target the whole directory
        if !has_sub {
            targets.push(ResetTarget {
                label: display_path(reset_dir),
                path: reset_dir.to_path_buf(),
                description: "oxicode home (settings, sessions, skills, extensions, packages)"
                    .to_string(),
            });
        }
    }

    // Home-layout migration journal.
    if let Some(journal) = journal_path
        && journal.exists()
    {
        targets.push(ResetTarget {
            label: display_path(journal),
            path: journal.to_path_buf(),
            description: "home-layout migration journal".to_string(),
        });
    }

    // ~/.config/oxicode/ — MCP config, alternative auth location
    if config_dir.exists() {
        targets.push(ResetTarget {
            label: display_path(config_dir),
            path: config_dir.to_path_buf(),
            description: "MCP config, credentials".to_string(),
        });
    }

    // ~/.cache/oxicode/ — logs
    if cache_dir.exists() {
        targets.push(ResetTarget {
            label: display_path(cache_dir),
            path: cache_dir.to_path_buf(),
            description: "logs, cache".to_string(),
        });
    }

    let mut project_target: Option<ResetTarget> = None;
    if include_project && project_dir.exists() {
        project_target = Some(ResetTarget {
            label: display_path(project_dir),
            path: project_dir.to_path_buf(),
            description: "project settings".to_string(),
        });
    }

    (targets, project_target)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical home present → legacy home is NOT targeted.
    #[test]
    fn canonical_home_wins_over_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical = tmp.path().join("canonical-home");
        let legacy = tmp.path().join("legacy-home");
        let config = tmp.path().join("config");
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project").join(".oxicode");
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(canonical.join("auth.json"), "{}").unwrap();
        std::fs::write(legacy.join("auth.json"), "{}").unwrap();

        let (targets, _) = collect_reset_targets(
            &canonical,
            Some(&legacy),
            None,
            &config,
            &cache,
            &project,
            false,
        );

        let paths: Vec<&Path> = targets.iter().map(|t| t.path.as_path()).collect();
        assert!(paths.contains(&canonical.join("auth.json").as_path()));
        assert!(
            !paths.iter().any(|p| p.starts_with(&legacy)),
            "legacy home must not be reset while the canonical home exists"
        );
    }

    /// Canonical home absent → legacy home is targeted.
    #[test]
    fn legacy_home_targeted_when_canonical_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical = tmp.path().join("canonical-home");
        let legacy = tmp.path().join("legacy-home");
        let config = tmp.path().join("config");
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project").join(".oxicode");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("auth.json"), "{}").unwrap();

        let (targets, _) = collect_reset_targets(
            &canonical,
            Some(&legacy),
            None,
            &config,
            &cache,
            &project,
            false,
        );

        let paths: Vec<&Path> = targets.iter().map(|t| t.path.as_path()).collect();
        assert!(paths.contains(&legacy.join("auth.json").as_path()));
        assert!(!paths.iter().any(|p| p.starts_with(&canonical)));
    }

    /// Journal is collected when present; project dir only with the flag.
    #[test]
    fn journal_and_project_targeting() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical = tmp.path().join("canonical-home");
        let config = tmp.path().join("config");
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project").join(".oxicode");
        std::fs::create_dir_all(&canonical).unwrap();
        let journal = tmp.path().join("oxicode.migration-journal.json");
        std::fs::write(&journal, "{}").unwrap();

        let (targets, project_target) = collect_reset_targets(
            &canonical,
            None,
            Some(&journal),
            &config,
            &cache,
            &project,
            false,
        );
        assert!(targets.iter().any(|t| t.path == journal));
        assert!(project_target.is_none());

        std::fs::create_dir_all(&project).unwrap();
        let (_, project_target) = collect_reset_targets(
            &canonical,
            None,
            Some(&journal),
            &config,
            &cache,
            &project,
            true,
        );
        assert!(project_target.is_some());
    }
}
