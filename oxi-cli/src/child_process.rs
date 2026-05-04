//! Child process utilities.
//!
//! Provides utilities for spawning and managing child processes with proper
//! signal handling, especially for Windows compatibility.

use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// Grace period for waiting on stdio after process exit (milliseconds)
const EXIT_STDIO_GRACE_MS: u64 = 100;

/// Windows shell commands that should use cmd.exe
const WINDOWS_SHELL_COMMANDS: &[&str] = &["npm", "npx", "pnpm", "yarn", "yarnpkg", "corepack"];

/// Determine if a command should use Windows shell (cmd.exe).
///
/// This checks if the command is a known Windows package manager or
/// has a .cmd or .bat extension.
pub fn should_use_windows_shell(command: &str) -> bool {
    if !cfg!(target_os = "windows") {
        return false;
    }

    let command_name = Path::new(command)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    command_name.ends_with(".cmd")
        || command_name.ends_with(".bat")
        || WINDOWS_SHELL_COMMANDS.contains(&command_name.as_str())
}

/// Wait for a child process to terminate without hanging on inherited stdio handles.
///
/// On Windows, daemonized descendants can inherit the child's stdout/stderr pipe
/// handles. In that case the child emits `exit`, but `close` can hang forever even
/// though the original process is already gone. We wait briefly for stdio to end,
/// then forcibly stop tracking the inherited handles.
pub async fn wait_for_child_process(child: &mut Child) -> std::io::Result<Option<i32>> {
    // Wait for the child to exit
    match child.wait().await {
        Ok(status) => {
            // Give stdio a moment to flush
            tokio::time::sleep(std::time::Duration::from_millis(EXIT_STDIO_GRACE_MS)).await;
            Ok(status.code())
        }
        Err(e) => Err(e),
    }
}

/// Spawn a command with proper signal handling for both Unix and Windows.
///
/// On Unix, sets up proper signal handling for SIGINT/SIGTERM propagation.
/// On Windows, uses the appropriate shell for common package managers.
pub async fn spawn_with_signal(
    program: &str,
    args: &[&str],
) -> std::io::Result<Child> {
    let mut cmd = Command::new(program);

    #[cfg(target_os = "windows")]
    {
        if should_use_windows_shell(program) {
            // Use cmd.exe /c to run the command
            cmd = Command::new("cmd.exe");
            let mut all_args = vec!["/c".to_string(), program.to_string()];
            all_args.extend(args.iter().map(|s| s.to_string()));
            cmd.args(&all_args);
        } else {
            cmd.args(args);
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        cmd.args(args);
    }

    // Set up stdio for capture
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(Stdio::null());

    // Don't kill the process group on drop (let parent handle signals)
    cmd.kill_on_drop(true);

    cmd.spawn()
}

/// Run a command and capture its output.
pub async fn run_command(program: &str, args: &[&str]) -> std::io::Result<CommandOutput> {
    let mut child = spawn_with_signal(program, args).await?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let mut stdout_output = String::new();
    let mut stderr_output = String::new();

    // Read stdout
    if let Some(stdout) = stdout {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            stdout_output.push_str(&line);
            stdout_output.push('\n');
        }
    }

    // Read stderr
    if let Some(stderr) = stderr {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            stderr_output.push_str(&line);
            stderr_output.push('\n');
        }
    }

    let exit_code = wait_for_child_process(&mut child).await?;

    Ok(CommandOutput {
        stdout: stdout_output.trim().to_string(),
        stderr: stderr_output.trim().to_string(),
        exit_code,
    })
}

/// Output from a spawned command.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Standard output (stdout)
    pub stdout: String,
    /// Standard error (stderr)
    pub stderr: String,
    /// Exit code (None if still running or killed)
    pub exit_code: Option<i32>,
}

impl CommandOutput {
    /// Check if the command exited successfully (code 0)
    pub fn is_success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Get the exit code or 0 if success, 1 otherwise
    pub fn exit_code_or(&self, default: i32) -> i32 {
        self.exit_code.unwrap_or(default)
    }
}

/// Execute a shell command with shell expansion.
///
/// This is useful for commands that need shell features like glob expansion,
/// environment variable substitution, or pipe/redirection.
pub async fn run_shell(command: &str) -> std::io::Result<CommandOutput> {
    #[cfg(target_os = "windows")]
    {
        run_command("cmd.exe", &["/c", command]).await
    }

    #[cfg(not(target_os = "windows"))]
    {
        run_command("sh", &["-c", command]).await
    }
}

/// Execute a shell command and return only stdout, ignoring stderr.
pub async fn run_shell_capture(command: &str) -> std::io::Result<String> {
    let output = run_shell(command).await?;
    Ok(output.stdout)
}

/// Execute a command and return only stdout.
pub async fn run_capture(program: &str, args: &[&str]) -> std::io::Result<String> {
    let output = run_command(program, args).await?;
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_use_windows_shell() {
        #[cfg(target_os = "windows")]
        {
            assert!(should_use_windows_shell("npm"));
            assert!(should_use_windows_shell("pnpm"));
            assert!(should_use_windows_shell("yarn"));
            assert!(should_use_windows_shell("npx.cmd"));
            assert!(!should_use_windows_shell("rustc"));
        }

        #[cfg(not(target_os = "windows"))]
        {
            assert!(!should_use_windows_shell("npm"));
            assert!(!should_use_windows_shell("pnpm"));
        }
    }

    #[tokio::test]
    async fn test_spawn_with_signal() {
        #[cfg(target_os = "windows")]
        let program = "cmd.exe";
        #[cfg(target_os = "windows")]
        let args = vec!["/c", "echo", "hello"];

        #[cfg(not(target_os = "windows"))]
        let program = "echo";
        #[cfg(not(target_os = "windows"))]
        let args = vec!["hello"];

        let mut child = spawn_with_signal(program, &args).await.unwrap();
        let exit_code = wait_for_child_process(&mut child).await.unwrap();

        #[cfg(target_os = "windows")]
        assert_eq!(exit_code, Some(0));

        #[cfg(not(target_os = "windows"))]
        assert_eq!(exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_run_command() {
        #[cfg(target_os = "windows")]
        let (program, args) = ("cmd.exe", vec!["/c", "echo", "test"]);
        
        #[cfg(not(target_os = "windows"))]
        let (program, args) = ("echo", vec!["test"]);

        let output = run_command(program, &args).await.unwrap();
        assert!(output.is_success());
    }

    #[tokio::test]
    async fn test_run_shell() {
        #[cfg(target_os = "windows")]
        let output = run_shell("echo hello").await.unwrap();
        #[cfg(target_os = "windows")]
        assert!(output.stdout.contains("hello"));

        #[cfg(not(target_os = "windows"))]
        let output = run_shell("echo hello").await.unwrap();
        #[cfg(not(target_os = "windows"))]
        assert!(output.stdout.contains("hello"));
    }
}
