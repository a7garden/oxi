//! Thin wrappers around the `git` binary for the TUI overlay.
//!
//! All git calls in the overlay funnel through [`run_git`] so:
//!
//! * the binary path lives in exactly one place,
//! * stderr is folded into `anyhow::Error` with the failing argv,
//! * empty-repos / non-repo paths surface as errors the slash command can
//!   reply with instead of panicking.
//!
//! Every helper here shells out to `git`. No in-process git library is
//! pulled in — keeps the TUI overlay's dependency surface bounded.

use std::path::Path;
use std::process::Command;

use super::state::{StatusEntry, parse_status_porcelain_z};

/// Run `git <args…>` in `cwd` and return stdout as a `String`.
///
/// Stderr is captured and surfaced as part of the error so the slash
/// command can `ctx.reply(Error, ...)` with the real reason. Non-zero
/// exit codes are always an error — callers that want to treat empty
/// output as success must inspect the returned `Ok(String)` themselves
/// (see [`diff_head`]).
pub fn run_git(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to spawn git {args:?}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(anyhow::anyhow!(
            "git {args:?} failed ({}): {stderr}",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `git status --porcelain -z` parsed into [`StatusEntry`]s.
pub fn status_porcelain_z(cwd: &Path) -> anyhow::Result<Vec<StatusEntry>> {
    let raw = run_git(cwd, &["status", "--porcelain", "-z"])?;
    Ok(parse_status_porcelain_z(raw.as_bytes()))
}

/// `git diff HEAD --no-ext-diff` with a fallback for fresh repos.
///
/// In a fresh repo `git diff HEAD` exits with status 128 (fatal: bad
/// revision) because there is no HEAD yet. We detect that case by
/// probing `git rev-parse --verify HEAD` first; when HEAD does not
/// resolve we return `Ok(String::new())` so the overlay can render an
/// empty diff doc instead of failing. Real errors (non-zero exit for
/// any other reason) propagate to the caller unchanged.
pub fn diff_head(cwd: &Path) -> anyhow::Result<String> {
    // HEAD existence probe — distinguishes "no HEAD yet" (fresh repo,
    // `git diff HEAD` exits 128) from a real diff failure. When HEAD is
    // missing we return an empty string; the parser turns that into an
    // empty diff doc the overlay can render without erroring.
    let head_ok = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if head_ok {
        run_git(cwd, &["diff", "HEAD", "--no-ext-diff", "--no-color"])
    } else {
        Ok(String::new())
    }
}
