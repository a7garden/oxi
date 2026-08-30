//! `oxicode migrate` subcommand family.
//!
//! Routes every migrate-style subcommand to the matching handler. The
//! only handler currently shipping is `migrate_brain`, which migrates
//! legacy durable memory (the SQLite / Mnemopi / summary backends) to
//! the Foundation v1 host's only durable-memory authority: the
//! oxibrain daemon.
//!
//! The migration is **resumable**: a checkpoint under
//! `~/.oxicode/migration/brain.json` records the last successfully
//! migrated memory ID. Restarting the migration resumes from the
//! checkpoint; clearing the file restarts from scratch.
//!
//! The migration is **non-destructive**: the legacy store is left
//! intact until the user explicitly archives it with
//! `oxicode migrate brain --archive-legacy`. Archival moves the
//! legacy store out of the active path to
//! `~/.oxicode/archive/memory/<timestamp>/`. The legacy store cannot
//! be re-enabled silently.
//!
//! ## Reference
//!
//! `docs/superpowers/specs/2026-08-17-oxi-foundation-contract.md` §
//! "Migration".

use std::path::PathBuf;

use crate::cli::{MigrateBrainArgs, MigrateHomeArgs, MigrationCommands};

/// Top-level dispatcher. Returns the exit code.
pub async fn handle_migrate(cmd: MigrationCommands) -> i32 {
    match cmd {
        MigrationCommands::Brain(args) => handle_migrate_brain(args).await,
        MigrationCommands::Home(args) => handle_migrate_home(args),
    }
}

/// `oxicode migrate home` — legacy `~/.oxicode` → unified Oxi home.
///
/// Journaled, resumable, copy-only (see [`crate::home_migrate`]). Safe to
/// re-run at any point; `--dry-run` prints the plan and mutates nothing.
fn handle_migrate_home(args: MigrateHomeArgs) -> i32 {
    use crate::home_migrate::{MigrationState, RunOutcome};
    use oxicode_catalog::oxi_home;

    println!("Oxi home layout migration");
    println!("-------------------------");
    println!("dry-run: {}", args.dry_run);

    let Some(source) = oxi_home::legacy_home_dir() else {
        println!("source:              <no legacy home>");
        if std::env::var_os("OXICODE_HOME").is_some() {
            println!(
                "required action:     nothing to do (explicit OXICODE_HOME never merges legacy)"
            );
        } else {
            println!("required action:     nothing to do");
        }
        return 0;
    };
    let Some(destination) = oxi_home::oxicode_home() else {
        eprintln!("error: cannot resolve the canonical oxicode home");
        return 1;
    };
    let Some(journal_path) = oxi_home::migration_journal_path() else {
        eprintln!("error: cannot resolve the oxi home for the migration journal");
        return 1;
    };

    // Preflight summary — cheap, read-only, always printed.
    let plan = match crate::home_migrate::preflight(&source, &destination) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("error: preflight failed: {e}");
            return 1;
        }
    };
    println!("source:              {} (legacy)", source.display());
    println!("destination:         {}", destination.display());
    println!("journal:             {}", journal_path.display());
    println!("files:               {}", plan.file_count);
    println!("bytes:               {}", plan.total_bytes);

    let outcome = match crate::home_migrate::run(&source, &destination, &journal_path, args.dry_run)
    {
        Ok(outcome) => outcome,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    match outcome {
        RunOutcome::DryRun(plan) => match plan.state {
            MigrationState::NothingToDo => {
                println!("state:               nothing to do");
                println!("required action:     none");
            }
            MigrationState::Ready => {
                println!(
                    "state:               ready ({} file(s) to copy)",
                    plan.pending.len()
                );
                println!("required action:     rerun without --dry-run to migrate");
            }
            MigrationState::AlreadyMigrated => {
                println!("state:               already migrated (destination identical)");
                println!("required action:     none");
            }
            MigrationState::Conflict { conflicts } => {
                println!("state:               conflict");
                for (src, dst) in &conflicts {
                    println!("  differs:");
                    println!("    source:      {}", src.display());
                    println!("    destination: {}", dst.display());
                }
                println!(
                    "required action:     resolve the differing files manually (nothing was touched)"
                );
            }
        },
        RunOutcome::NothingToDo => {
            println!("state:               nothing to do");
            println!("required action:     none");
        }
        RunOutcome::Conflict { conflicts } => {
            println!("state:               conflict (nothing was touched)");
            for (src, dst) in &conflicts {
                println!("  differs:");
                println!("    source:      {}", src.display());
                println!("    destination: {}", dst.display());
            }
            println!("required action:     resolve the differing files manually");
            return 1;
        }
        RunOutcome::AlreadyMigrated { completed_journal } => {
            println!("state:               already migrated (destination identical)");
            if completed_journal {
                println!("journal:             stale in_progress entry marked complete");
            }
            println!("required action:     none");
        }
        RunOutcome::Copied { copied, skipped } => {
            println!("copied:              {copied} file(s)");
            println!("skipped (identical): {skipped} file(s)");
            println!("required action:     none — legacy home left intact");
        }
    }

    0
}

async fn handle_migrate_brain(args: MigrateBrainArgs) -> i32 {
    let socket = args
        .socket
        .unwrap_or_else(crate::foundation::brain::default_socket_path);

    println!("Foundation v1 brain migration");
    println!("-----------------------------");
    println!("socket:              {}", socket.display());
    println!("dry-run:             {}", args.dry_run);
    println!("archive-legacy:      {}", args.archive_legacy);
    println!("checkpoint:          {}", args.checkpoint.display());
    println!("batch size:          {}", args.batch_size);

    if args.dry_run {
        println!("\nDRY RUN: no memory will be written, no checkpoint advanced.");
        println!("The legacy store will be enumerated but not read for content.");
    }

    let backend = crate::foundation::brain::BrainMemoryBackend::new(socket.clone());
    // Probe the daemon so the printed health is a live measurement —
    // construction alone leaves the cached state at `Unavailable`.
    let probe = backend.ping().await;
    println!(
        "\nbackend health:      {}",
        match &probe {
            Ok(()) => "ok: oxibrain daemon connected".to_string(),
            Err(e) => format!("degraded ({e})"),
        }
    );

    if args.dry_run {
        println!("\nDry run complete. Re-run without --dry-run to perform the migration.");
        return 0;
    }

    let checkpoint = crate::foundation::migrate::Checkpoint::load(&args.checkpoint);
    if let Some(last) = checkpoint.last_id() {
        println!("resuming after id:   {last}");
    } else {
        println!("starting fresh (no checkpoint found)");
    }

    let legacy = crate::foundation::migrate::LegacyMemoryReader::for_default_home();
    let mut migrated = 0usize;
    let mut failed = 0usize;
    let mut tx = crate::foundation::migrate::Migration::new(&backend, &args.checkpoint);

    for batch in legacy.batches(args.batch_size) {
        for item in batch {
            match tx.migrate_one(item) {
                Ok(crate::foundation::migrate::MigrationOutcome::Inserted(id)) => {
                    println!("  + {id}");
                    migrated += 1;
                }
                Ok(crate::foundation::migrate::MigrationOutcome::Skipped(id)) => {
                    println!("  ~ {id} (already in brain)");
                }
                Err(e) => {
                    println!("  ! {e}");
                    failed += 1;
                }
            }
        }
    }

    println!("\nMigration summary");
    println!("  inserted: {migrated}");
    println!("  failed:   {failed}");

    if args.archive_legacy {
        match crate::foundation::migrate::archive_legacy_default() {
            Ok(archive_path) => {
                println!("\nLegacy store archived to: {}", archive_path.display());
            }
            Err(e) => {
                println!("\nArchive failed: {e}");
                return 2;
            }
        }
    }

    if failed > 0 { 1 } else { 0 }
}

impl Default for MigrateBrainArgs {
    fn default() -> Self {
        Self {
            socket: None,
            dry_run: false,
            archive_legacy: false,
            checkpoint: PathBuf::from("~/.oxicode/migration/brain.json"),
            batch_size: 64,
        }
    }
}
