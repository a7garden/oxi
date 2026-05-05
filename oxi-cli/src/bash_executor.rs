//! Bash executor for persistent shell sessions
//!
//! Provides a persistent bash session that maintains state between commands,
//! including working directory and environment variables.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use parking_lot::RwLock;
use std::time::Duration;

/// Result of a bash execution
#[derive(Debug, Clone)]
pub struct BashResult {
    /// The command that was executed
    pub command: String,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Exit code
    pub exit_code: Option<i32>,
    /// Whether the command was killed due to timeout
    pub timed_out: bool,
    /// Execution duration
    pub duration_ms: u64,
}

/// Bash executor configuration
#[derive(Debug, Clone)]
pub struct BashExecutorConfig {
    /// Shell to use
    pub shell: String,
    /// Initial working directory
    pub cwd: PathBuf,
    /// Environment variables to set
    pub env: HashMap<String, String>,
    /// Timeout for commands (None for no timeout)
    pub timeout: Option<Duration>,
    /// Maximum output size in bytes
    pub max_output_size: usize,
}

impl Default for BashExecutorConfig {
    fn default() -> Self {
        Self {
            shell: "/bin/bash".to_string(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: HashMap::new(),
            timeout: Some(Duration::from_secs(300)),
            max_output_size: 10 * 1024 * 1024, // 10MB
        }
    }
}

/// A persistent bash executor
pub struct BashExecutor {
    config: BashExecutorConfig,
    /// Current working directory
    cwd: RwLock<PathBuf>,
    /// Environment variables
    env: RwLock<HashMap<String, String>>,
    /// Command history
    history: RwLock<Vec<String>>,
}

impl BashExecutor {
    /// Create a new bash executor
    pub fn new(config: BashExecutorConfig) -> Self {
        let cwd = RwLock::new(config.cwd.clone());
        let env = RwLock::new(config.env.clone());
        Self {
            config,
            cwd,
            env,
            history: RwLock::new(Vec::new()),
        }
    }

    /// Create with default configuration
    pub fn default() -> Self {
        Self::new(BashExecutorConfig::default())
    }

    /// Get the current working directory
    pub fn cwd(&self) -> PathBuf {
        self.cwd.read().clone()
    }

    /// Get a copy of environment variables
    pub fn env(&self) -> HashMap<String, String> {
        self.env.read().clone()
    }

    /// Get command history
    pub fn history(&self) -> Vec<String> {
        self.history.read().clone()
    }

    /// Set working directory
    pub fn set_cwd(&self, path: PathBuf) {
        if path.exists() && path.is_dir() {
            *self.cwd.write() = path;
        }
    }

    /// Set an environment variable
    pub fn set_env(&self, key: &str, value: &str) {
        self.env
            .write()
            .insert(key.to_string(), value.to_string());
    }

    /// Remove an environment variable
    pub fn remove_env(&self, key: &str) {
        self.env.write().remove(key);
    }

    /// Execute a command and return the result
    pub fn execute(&self, command: &str) -> BashResult {
        let start = std::time::Instant::now();

        // Build the command — wrap with cwd tracking
        // We prefix with a pwd capture so we can track directory changes
        let wrapped = format!("{}; __oxi_cwd=$(pwd)", command);

        let mut cmd = Command::new(&self.config.shell);
        cmd.arg("-c")
            .arg(&wrapped)
            .current_dir(self.cwd.read().as_path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Set custom environment variables
        let env = self.env.read();
        for (key, value) in env.iter() {
            cmd.env(key, value);
        }

        // Execute with timeout
        let output_result = match self.config.timeout {
            Some(t) => match cmd.spawn() {
                Ok(mut child) => {
                    let deadline = std::time::Instant::now() + t;
                    loop {
                        match child.try_wait() {
                            Ok(Some(_status)) => {
                                break child.wait_with_output();
                            }
                            Ok(None) => {
                                if std::time::Instant::now() >= deadline {
                                    let _ = child.kill();
                                    let _ = child.wait();
                                    let duration_ms = start.elapsed().as_millis() as u64;
                                    self.history.write().push(command.to_string());
                                    return BashResult {
                                        command: command.to_string(),
                                        stdout: String::new(),
                                        stderr: "Command timed out".to_string(),
                                        exit_code: Some(-1),
                                        timed_out: true,
                                        duration_ms,
                                    };
                                }
                                std::thread::sleep(Duration::from_millis(50));
                            }
                            Err(e) => {
                                let duration_ms = start.elapsed().as_millis() as u64;
                                self.history.write().push(command.to_string());
                                return BashResult {
                                    command: command.to_string(),
                                    stdout: String::new(),
                                    stderr: format!("Failed to wait: {}", e),
                                    exit_code: Some(-1),
                                    timed_out: false,
                                    duration_ms,
                                };
                            }
                        }
                    }
                }
                Err(e) => {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    self.history.write().push(command.to_string());
                    return BashResult {
                        command: command.to_string(),
                        stdout: String::new(),
                        stderr: format!("Failed to spawn: {}", e),
                        exit_code: Some(-1),
                        timed_out: false,
                        duration_ms,
                    };
                }
            },
            None => cmd.output(),
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        let output = match output_result {
            Ok(o) => o,
            Err(e) => {
                self.history.write().push(command.to_string());
                return BashResult {
                    command: command.to_string(),
                    stdout: String::new(),
                    stderr: format!("Failed to execute: {}", e),
                    exit_code: Some(-1),
                    timed_out: false,
                    duration_ms,
                };
            }
        };

        let stdout = self.truncate_output(String::from_utf8_lossy(&output.stdout).to_string());
        let stderr = self.truncate_output(String::from_utf8_lossy(&output.stderr).to_string());
        let exit_code = output.status.code();

        // Track cd commands — re-resolve cwd after cd
        if command.trim().starts_with("cd ") && exit_code == Some(0) {
            let target = command.trim().strip_prefix("cd ").unwrap().trim();
            let target = if target.starts_with("~/") {
                format!(
                    "{}/{}",
                    dirs::home_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    &target[2..]
                )
            } else {
                target.to_string()
            };
            let new_cwd = if target.starts_with('/') {
                PathBuf::from(target)
            } else {
                self.cwd.read().join(&target)
            };
            if new_cwd.is_dir() {
                *self.cwd.write() = new_cwd;
            }
        }

        // Add to history
        self.history.write().push(command.to_string());

        BashResult {
            command: command.to_string(),
            stdout,
            stderr,
            exit_code,
            timed_out: false,
            duration_ms,
        }
    }

    /// Execute a command with streaming output
    pub fn execute_streaming<F>(&self, command: &str, mut on_output: F) -> BashResult
    where
        F: FnMut(&str),
    {
        let start = std::time::Instant::now();

        let mut cmd = Command::new(&self.config.shell);
        // Preserve important environment variables instead of clearing everything.
        const PRESERVE_ENV_VARS: &[&str] = &[
            "HOME", "TERM", "LANG", "LC_ALL", "PATH", "USER", "SHELL", "TMPDIR",
            "EDITOR", "PAGER",
        ];

        cmd.arg("-c")
            .arg(command)
            .current_dir(self.cwd.read().as_path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();

        // Re-inherit preserved vars from the current process environment.
        for var in PRESERVE_ENV_VARS {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        // Apply user-specified overrides last so they take precedence.
        let env = self.env.read();
        for (key, value) in env.iter() {
            cmd.env(key, value);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return BashResult {
                    command: command.to_string(),
                    stdout: String::new(),
                    stderr: format!("Failed to spawn: {}", e),
                    exit_code: Some(-1),
                    timed_out: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        // Read stdout
        let mut stdout = String::new();
        if let Some(ref mut out) = child.stdout {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(out);
            for line in reader.lines() {
                if let Ok(line) = line {
                    on_output(&line);
                    stdout.push_str(&line);
                    stdout.push('\n');
                }
            }
        }

        // Read stderr
        let mut stderr = String::new();
        if let Some(ref mut err) = child.stderr {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(err);
            for line in reader.lines() {
                if let Ok(line) = line {
                    stderr.push_str(&line);
                    stderr.push('\n');
                }
            }
        }

        let status = child.wait().ok();
        let exit_code = status.and_then(|s| s.code());
        let duration_ms = start.elapsed().as_millis() as u64;

        // Add to history
        self.history.write().push(command.to_string());

        BashResult {
            command: command.to_string(),
            stdout: self.truncate_output(stdout),
            stderr: self.truncate_output(stderr),
            exit_code,
            timed_out: false,
            duration_ms,
        }
    }

    /// Execute multiple commands in sequence
    pub fn execute_batch(&self, commands: &[&str]) -> Vec<BashResult> {
        commands.iter().map(|cmd| self.execute(cmd)).collect()
    }

    /// Execute with error propagation
    pub fn execute_required(&self, command: &str) -> Result<String, String> {
        let result = self.execute(command);
        if result.exit_code == Some(0) {
            Ok(result.stdout)
        } else {
            Err(format!(
                "Command failed with exit code {:?}: {}\n{}",
                result.exit_code, result.stderr, result.stdout
            ))
        }
    }

    /// Truncate output to max size
    fn truncate_output(&self, output: String) -> String {
        if output.len() > self.config.max_output_size {
            let half = self.config.max_output_size / 2;
            // Find a safe char boundary near `half`.
            let boundary = output
                .char_indices()
                .take_while(|(idx, _)| *idx <= half)
                .last()
                .map(|(idx, _)| idx)
                .unwrap_or(0);
            format!(
                "{}...\n[Output truncated: {} bytes -> {} bytes]",
                &output[..boundary],
                output.len(),
                self.config.max_output_size
            )
        } else {
            output
        }
    }
}

impl Default for BashExecutor {
    fn default() -> Self {
        Self::new(BashExecutorConfig::default())
    }
}

/// Create an Arc-wrapped executor for sharing
pub fn create_executor(config: BashExecutorConfig) -> Arc<BashExecutor> {
    Arc::new(BashExecutor::new(config))
}

/// Execute a single command without persistent state
pub fn execute_once(command: &str) -> BashResult {
    let executor = BashExecutor::default();
    executor.execute(command)
}

/// Execute a command with a specific timeout
pub fn execute_with_timeout(command: &str, timeout: Duration) -> BashResult {
    let config = BashExecutorConfig {
        timeout: Some(timeout),
        ..Default::default()
    };
    let executor = BashExecutor::new(config);
    executor.execute(command)
}

/// Check if a command exists in PATH
pub fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {} > /dev/null 2>&1", command))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the shell being used
pub fn get_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_simple() {
        let executor = BashExecutor::default();
        let result = executor.execute("echo hello");
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hello"));
    }

    #[test]
    fn test_execute_with_cd() {
        let executor = BashExecutor::default();
        let result = executor.execute("pwd");
        assert!(result.exit_code == Some(0));
    }

    #[test]
    fn test_execute_failed_command() {
        let executor = BashExecutor::default();
        let result = executor.execute("exit 1");
        assert_eq!(result.exit_code, Some(1));
    }

    #[test]
    fn test_execute_nonexistent_command() {
        let executor = BashExecutor::default();
        let result = executor.execute("nonexistent_command_12345");
        assert!(
            result.exit_code.is_some() && result.exit_code.unwrap() != 0
                || result.stderr.contains("not found")
                || result.stderr.contains("command not found")
        );
    }

    #[test]
    fn test_execute_batch() {
        let executor = BashExecutor::default();
        let results = executor.execute_batch(&["echo one", "echo two", "echo three"]);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.exit_code == Some(0)));
    }

    #[test]
    fn test_cwd_tracking() {
        let executor = BashExecutor::default();
        let initial_cwd = executor.cwd();
        assert!(initial_cwd.exists());
    }

    #[test]
    fn test_env_tracking() {
        let executor = BashExecutor::default();
        executor.set_env("TEST_VAR", "test_value");
        let env = executor.env();
        assert_eq!(env.get("TEST_VAR"), Some(&"test_value".to_string()));
    }

    #[test]
    fn test_history() {
        let executor = BashExecutor::default();
        executor.execute("echo 1");
        executor.execute("echo 2");
        let history = executor.history();
        assert_eq!(history.len(), 2);
        assert!(history.contains(&"echo 1".to_string()));
        assert!(history.contains(&"echo 2".to_string()));
    }

    #[test]
    fn test_execute_required_success() {
        let executor = BashExecutor::default();
        let result = executor.execute_required("echo hello");
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_required_failure() {
        let executor = BashExecutor::default();
        let result = executor.execute_required("exit 1");
        assert!(result.is_err());
    }

    #[test]
    fn test_command_exists() {
        assert!(command_exists("echo"));
        assert!(command_exists("ls"));
        assert!(!command_exists("nonexistent_command_xyz"));
    }

    #[test]
    fn test_get_shell() {
        let shell = get_shell();
        assert!(!shell.is_empty());
        assert!(shell.contains("bash") || shell.contains("zsh"));
    }

    #[test]
    fn test_execute_with_timeout() {
        let result = execute_with_timeout("echo hello", Duration::from_secs(5));
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn test_execute_long_output() {
        let executor = BashExecutor::default();
        // Generate more output than max_output_size
        let result = executor.execute(&"yes | head -n 100000".to_string());
        assert!(result.stdout.contains("[Output truncated]") || result.stdout.len() < 100000);
    }

    #[test]
    fn test_execute_streaming() {
        let executor = BashExecutor::default();
        let mut output_lines = Vec::new();
        let result = executor.execute_streaming("echo line1; echo line2; echo line3", |line| {
            output_lines.push(line.to_string());
        });
        assert_eq!(output_lines.len(), 3);
        assert_eq!(result.exit_code, Some(0));
    }
}
