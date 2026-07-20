//! Fallback sandbox implementation — no sandbox, direct execution.
//! Used on platforms where no sandbox mechanism is available.

use std::process::Command;

use crate::{SandboxError, SandboxProfile};

/// Fallback sandbox that runs commands directly without isolation.
pub struct Sandbox;

impl Sandbox {
    /// Run a command directly (no sandbox isolation).
    pub fn run(
        profile: &SandboxProfile,
        command: &str,
        args: &[&str],
    ) -> Result<String, SandboxError> {
        let _ = profile; // unused in fallback
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
