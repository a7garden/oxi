//! Linux sandbox implementation using `bwrap(1)` (bubblewrap).
//!
//! Uses bwrap to create a minimal sandbox with configurable filesystem
//! and network access based on the selected [`SandboxProfile`]. Falls
//! back to unsandboxed execution when `bwrap` is not available.

use std::process::Command;

use crate::{SandboxError, SandboxProfile};

/// Linux bwrap sandbox runner.
pub struct Sandbox;

impl Sandbox {
    /// Run a command within a bwrap sandbox.
    ///
    /// Falls back to direct execution when `bwrap` is not available.
    pub fn run(
        profile: &SandboxProfile,
        command: &str,
        args: &[&str],
    ) -> Result<String, SandboxError> {
        let bwrap_available = Command::new("which")
            .arg("bwrap")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !bwrap_available {
            return Self::run_unsandboxed(command, args);
        }

        let mut bwrap_args = build_bwrap_args(profile);
        bwrap_args.push(command);
        bwrap_args.extend(args.iter().copied());

        let output = Command::new("bwrap")
            .args(&bwrap_args)
            .output()
            .map_err(|e| SandboxError::Execution(e.to_string()))?;

        if output.status.success() {
            String::from_utf8(output.stdout).map_err(|e| SandboxError::Execution(e.to_string()))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(SandboxError::Execution(stderr.trim().to_string()))
        }
    }

    fn run_unsandboxed(command: &str, args: &[&str]) -> Result<String, SandboxError> {
        let output = Command::new(command)
            .args(args)
            .output()
            .map_err(|e| SandboxError::Execution(e.to_string()))?;

        if output.status.success() {
            String::from_utf8(output.stdout).map_err(|e| SandboxError::Execution(e.to_string()))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(SandboxError::Execution(stderr.trim().to_string()))
        }
    }
}

/// Build bwrap arguments for the given profile.
fn build_bwrap_args(profile: &SandboxProfile) -> Vec<&'static str> {
    // Minimal root filesystem
    let mut args = vec![
        "--ro-bind",
        "/usr",
        "/usr",
        "--ro-bind",
        "/lib",
        "/lib",
        "--ro-bind",
        "/lib64",
        "/lib64",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
    ];

    // Working directory access
    match profile {
        SandboxProfile::ReadOnly => {
            args.push("--ro-bind");
            args.push(".");
            args.push(".");
        }
        SandboxProfile::WorkspaceWrite | SandboxProfile::Custom(_) => {
            args.push("--bind");
            args.push(".");
            args.push(".");
        }
        SandboxProfile::NetworkRestricted => {
            args.push("--ro-bind");
            args.push(".");
            args.push(".");
            args.push("--unshare-net");
        }
    }

    // Network isolation
    if matches!(
        profile,
        SandboxProfile::ReadOnly | SandboxProfile::NetworkRestricted
    ) {
        args.push("--unshare-net");
    }

    args
}
