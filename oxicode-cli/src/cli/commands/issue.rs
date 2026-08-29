//! Issue subcommand handler.

use crate::cli::IssueCommands;
use anyhow::Result;

/// Handle `oxicode issue …` subcommands. Opens the local issue store rooted at
/// the project and dispatches to the requested action.
pub async fn handle_issue(action: &IssueCommands) -> Result<()> {
    use oxicode_sdk::format_issue_full;
    use oxicode_sdk::{IssueFilter, Priority, Status};

    let cwd = std::env::current_dir()?;

    // `reap` is special: it resolves the issues dir WITHOUT opening a store,
    // because the store constructor runs its own lazy reap — opening one here
    // would double-reap and underreport the count we actually removed.
    if let IssueCommands::Reap = action {
        let dir = oxicode_sdk::issues_dir(&cwd);
        let removed = oxicode_sdk::liveness::reap_orphans(&dir)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        println!("reaped {removed} dead alive-lock file(s)");
        return Ok(());
    }

    let store = oxicode_sdk::FileIssueStore::open_from_cwd(&cwd)?;

    match action {
        IssueCommands::List { all, label, text } => {
            let filter = IssueFilter {
                status: if *all { None } else { Some(Status::Open) },
                priority: None,
                label: label.clone(),
                assigned_to_session: None,
                text: text.clone(),
            };
            let issues = store.list(&filter)?;
            if issues.is_empty() {
                println!("(no issues)");
            } else {
                for i in &issues {
                    println!("{}", oxicode_sdk::format_issue_line(i));
                }
            }
        }
        IssueCommands::Show { id } => {
            let (issue, hash) = store.read(*id)?;
            println!("{}", format_issue_full(&issue, &hash));
        }
        IssueCommands::New {
            title,
            body,
            priority,
            labels,
        } => {
            let body = body.clone().unwrap_or_default();
            let prio = match priority.as_deref() {
                Some("low") => Priority::Low,
                Some("medium") | None => Priority::Medium,
                Some("high") => Priority::High,
                Some("critical") => Priority::Critical,
                Some(other) => anyhow::bail!("invalid priority: {other}"),
            };
            let labels: Vec<String> = labels
                .as_deref()
                .map(|s| s.split(',').map(|l| l.trim().to_string()).collect())
                .unwrap_or_default();
            let issue = store.create(title.clone(), body, prio, labels, None)?;
            println!("created issue #{}: {}", issue.meta.id, issue.meta.title);
        }
        IssueCommands::Close { id, hash } => {
            let session = format!(
                "cli-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let _guard = oxicode_sdk::liveness::acquire(&store.issues_dir(), &session).ok();
            let (issue, current_hash) = store.read(*id)?;
            if let Some(a) = &issue.meta.assigned_to
                && a.session != session
                && oxicode_sdk::liveness::is_session_alive(&store.issues_dir(), &a.session)
            {
                anyhow::bail!(
                    "issue #{id} is currently being worked on by session {} (since {}); cannot close from CLI",
                    a.session,
                    a.acquired_at,
                );
            }
            let effective_hash = hash.clone().unwrap_or(current_hash);
            store
                .start(*id, &session, Some(effective_hash))
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let (_, fresh_hash) = store.read(*id)?;
            let closed = store
                .close(*id, &session, Some(fresh_hash))
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            println!("closed issue #{}: {}", closed.meta.id, closed.meta.title);
        }
        IssueCommands::Reopen { id, hash } => {
            let effective_hash = match hash.clone() {
                Some(h) => h,
                None => store.read(*id)?.1,
            };
            let reopened = store
                .reopen(*id, Some(effective_hash))
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            println!(
                "reopened issue #{}: {}",
                reopened.meta.id, reopened.meta.title
            );
        }
        IssueCommands::Reap => unreachable!("reap handled before store open"),
    }
    Ok(())
}
