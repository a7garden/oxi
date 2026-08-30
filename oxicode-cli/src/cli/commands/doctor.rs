//! `oxicode doctor` — home-layout diagnostics.
//!
//! Prints the active unified home resolution in one glance:
//!
//! - the Oxi home (`oxi_home()`) and the owned oxicode subtree
//!   (`oxicode_home()`),
//! - legacy `~/.oxicode` presence and whether a home-layout migration is
//!   pending,
//! - the well-known oxicode-owned paths derived from the canonical home.
//!
//! Read-only: doctor never mutates state.

use anyhow::Result;
use oxicode_catalog::oxi_home;
use std::path::PathBuf;

use super::reset::display_path;

/// Journal status line for the doctor report.
fn journal_status(journal_path: Option<PathBuf>) -> String {
    let Some(path) = journal_path.filter(|p| p.exists()) else {
        return "absent".to_string();
    };
    match crate::home_migrate::MigrationJournal::load(&path) {
        Some(j) if j.is_in_progress() => "in_progress".to_string(),
        Some(j) if j.status == "complete" => "complete".to_string(),
        Some(_) => "present (unexpected status)".to_string(),
        None => "present (unreadable)".to_string(),
    }
}

/// Handle `oxicode doctor`. Prints home-layout diagnostics; exit code 0.
pub fn handle_doctor() -> Result<()> {
    println!("Oxi home layout");
    println!("---------------");

    let oxi = oxi_home::oxi_home();
    let canonical = oxi_home::oxicode_home();
    let legacy = oxi_home::legacy_home_dir();
    let journal_path = oxi_home::migration_journal_path();

    println!(
        "  oxi home:        {}",
        oxi.as_ref()
            .map(|p| display_path(p))
            .unwrap_or_else(|| "<unresolvable>".to_string())
    );
    println!(
        "  oxicode home:    {}  (canonical, owned by oxicode)",
        canonical
            .as_ref()
            .map(|p| display_path(p))
            .unwrap_or_else(|| "<unresolvable>".to_string())
    );
    if let Some(env_val) = std::env::var_os("OXICODE_HOME") {
        println!(
            "    override:      OXICODE_HOME={}",
            env_val.to_string_lossy()
        );
    } else if let Some(env_val) = std::env::var_os("OXI_HOME") {
        println!("    override:      OXI_HOME={}", env_val.to_string_lossy());
    }

    match &legacy {
        Some(legacy_dir) => {
            println!(
                "  legacy home:     {}  (present, read-only)",
                display_path(legacy_dir)
            );
            let complete = journal_status(journal_path.clone()) == "complete";
            if canonical.as_ref().is_some_and(|c| c.exists()) && complete {
                println!("  migration:       complete");
            } else {
                println!("  migration:       pending  (run `oxicode migrate home`)");
            }
        }
        None => {
            println!("  legacy home:     absent");
            println!("  migration:       nothing to do");
        }
    }
    println!(
        "  journal:         {}",
        journal_status(journal_path).replace(
            "in_progress",
            "in_progress (rerun `oxicode migrate home` to resume)"
        )
    );

    // Well-known owned paths (canonical home when resolvable).
    if let Some(home) = canonical {
        println!();
        println!("Owned paths");
        println!("-----------");
        for (label, rel) in [
            ("settings", "settings.json"),
            ("auth", "auth.json"),
            ("sessions", "sessions"),
            ("skills", "skills"),
            ("extensions", "extensions"),
            ("packages", "packages"),
            ("catalog overrides", "catalog/overrides.toml"),
            ("models.dev cache", "cache/models-dev.json"),
        ] {
            println!(
                "  {:<18} {}",
                format!("{label}:"),
                display_path(&home.join(rel))
            );
        }
    }

    // Project-local `.oxicode/` is a separate namespace — mention it when
    // present so users don't confuse it with the home.
    if let Ok(cwd) = std::env::current_dir() {
        let project = cwd.join(".oxicode");
        if project.is_dir() {
            println!();
            println!(
                "  note: project-local .oxicode/ found at {} (separate namespace, not migrated)",
                display_path(&project)
            );
        }
    }

    Ok(())
}
