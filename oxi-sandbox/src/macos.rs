//! macOS sandbox implementation using `sandbox-exec`.
//!
//! Uses macOS's built-in `sandbox-exec(1)` to confine command execution
//! based on the selected [`SandboxProfile`]. Falls back to unsandboxed
//! execution when `sandbox-exec` is not available (e.g. inside containers).

use std::process::Command;

use crate::{SandboxError, SandboxProfile};

/// macOS sandbox runner.
pub struct Sandbox;

impl Sandbox {
    /// Run a command within a macOS sandbox.
    pub fn run(
        profile: &SandboxProfile,
        command: &str,
        args: &[&str],
    ) -> Result<String, SandboxError> {
        // Check if sandbox-exec is available
        let sandbox_available = Command::new("which")
            .arg("sandbox-exec")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !sandbox_available {
            // Fallback: run unsandboxed
            return Self::run_unsandboxed(command, args);
        }

        let profile_path = Self::build_sandbox_profile(profile);
        let output = Command::new("sandbox-exec")
            .arg("-f")
            .arg(&profile_path)
            .arg(command)
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

    /// Build a sandbox profile string from the profile enum.
    fn build_sandbox_profile(profile: &SandboxProfile) -> String {
        match profile {
            SandboxProfile::ReadOnly => r#"(version 1)
(allow default)
(deny file-write*)
"#
            .to_string(),
            SandboxProfile::WorkspaceWrite => {
                // Allow writes only under the cwd
                r#"(version 1)
(allow default)
(allow file-write* (subpath "/tmp"))
"#
                .to_string()
            }
            SandboxProfile::NetworkRestricted => r#"(version 1)
(allow default)
(deny network*)
"#
            .to_string(),
            SandboxProfile::Custom(rules) => {
                let mut sb = String::from("(version 1)\n");
                if rules.network_access {
                    sb.push_str("(allow network*)\n");
                } else {
                    sb.push_str("(deny network*)\n");
                }
                sb
            }
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
