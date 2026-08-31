//! Shared stateful coding runtime contracts.
//!
//! These traits declare the persistent shell / eval kernel / debug service
//! capabilities required by the `coding-omp-v1` behavior pack (see
//! `docs/designs/2026-08-31-omp-compatible-behavior-pack-design.md`,
//! "Coding extensions"). No implementations ship yet — hosts and future
//! pack extensions implement them. Until then the SDK behavior installer
//! reports those extensions as degraded/unavailable in its manifest.

use async_trait::async_trait;
use std::time::Duration;

/// Output of one command in a persistent shell session.
#[derive(Debug, Clone, Default)]
pub struct ShellOutput {
    /// Captured stdout (bounded by the host).
    pub stdout: String,
    /// Captured stderr (bounded by the host).
    pub stderr: String,
    /// Process exit code.
    pub exit_code: i32,
    /// True when the host applied an output bound and elided bytes.
    pub truncated: bool,
}

/// Persistent shell session contract ("Shell session" extension).
///
/// Required behavior: persistent command environment across calls,
/// cancellation, bounded output, explicit reset (design table row 3).
#[async_trait]
pub trait ShellSession: Send + Sync + std::fmt::Debug {
    /// Execute `command` in the persistent environment.
    async fn execute(&self, command: &str, timeout: Duration) -> Result<ShellOutput, String>;
    /// Cancel the currently running command, if any.
    fn cancel(&self);
    /// Reset to a fresh environment; the working directory returns to the
    /// workspace root.
    async fn reset(&self) -> Result<(), String>;
}

/// Language of an eval kernel session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalLanguage {
    /// Persistent Python kernel.
    Python,
    /// Persistent JavaScript (Bun) kernel.
    JavaScript,
}

/// Output of one persistent-kernel evaluation.
#[derive(Debug, Clone, Default)]
pub struct EvalOutput {
    /// The kernel's result/repr value, if any.
    pub result: String,
    /// Captured stdout (bounded by the host).
    pub stdout: String,
    /// Captured stderr (bounded by the host).
    pub stderr: String,
    /// Structured error, when the evaluation failed.
    pub error: Option<String>,
    /// True when the host applied an output bound and elided bytes.
    pub truncated: bool,
}

/// Persistent eval kernel contract ("Eval kernel" extension).
///
/// Required behavior: persistent Python/Bun state across calls, bounded
/// execution, explicit reset (design table row 4).
#[async_trait]
pub trait EvalKernel: Send + Sync + std::fmt::Debug {
    /// Language this kernel evaluates.
    fn language(&self) -> EvalLanguage;
    /// Evaluate `code` in the persistent kernel state.
    async fn execute(&self, code: &str, timeout: Duration) -> Result<EvalOutput, String>;
    /// Drop all kernel state; the next execute starts fresh.
    async fn reset(&self) -> Result<(), String>;
}

/// Debug service contract ("Debug service" extension): a real DAP session
/// lifecycle (design table row 5).
///
/// Requests use DAP command names (`setBreakpoints`, `continue`, `next`,
/// `variables`, ...) with raw JSON payloads — typed methods arrive with the
/// first real implementation.
#[async_trait]
pub trait DebugService: Send + Sync + std::fmt::Debug {
    /// Launch or attach a session per the DAP launch/attach config; returns
    /// a session id.
    async fn start(&self, config: &serde_json::Value) -> Result<String, String>;
    /// Issue a DAP request against the session; returns the raw response
    /// payload.
    async fn request(
        &self,
        session: &str,
        command: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String>;
    /// Terminate the session and clean up the adapter process.
    async fn terminate(&self, session: &str) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug)]
    struct NoopShell;

    #[async_trait]
    impl ShellSession for NoopShell {
        async fn execute(&self, _command: &str, _timeout: Duration) -> Result<ShellOutput, String> {
            Ok(ShellOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                truncated: false,
            })
        }
        fn cancel(&self) {}
        async fn reset(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn shell_session_contract_is_object_safe() {
        let shell: Arc<dyn ShellSession> = Arc::new(NoopShell);
        let out = shell.execute("true", Duration::from_secs(1)).await.unwrap();
        assert_eq!(out.exit_code, 0);
        shell.cancel();
        shell.reset().await.unwrap();
    }
}
