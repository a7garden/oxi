//! Playwright CLI skill for oxi
//!
//! Provides browser automation capabilities through Playwright:
//! - Launching and managing browser instances
//! - Navigating to URLs and waiting for page conditions
//! - Executing page interactions (click, type, select, screenshot)
//! - Running Playwright test suites and collecting results
//! - Generating Playwright test code
//!
//! The skill does NOT embed a browser engine. Instead, it orchestrates the
//! system-installed `npx playwright` CLI via subprocess calls, and provides
//! typed Rust abstractions over the common workflows.
//!
//! This module provides both:
//! - A [`PlaywrightCli`] struct for programmatic browser automation
//! - A [`skill_instructions`] function that produces system-prompt content
//!   for the LLM-driven browser testing workflow

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Output;
use tokio::process::Command;

// ── Configuration ────────────────────────────────────────────────────

/// Browser type to use for automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Browser {
    Chromium,
    Firefox,
    WebKit,
}

impl Default for Browser {
    fn default() -> Self {
        Browser::Chromium
    }
}

impl fmt::Display for Browser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Browser::Chromium => write!(f, "chromium"),
            Browser::Firefox => write!(f, "firefox"),
            Browser::WebKit => write!(f, "webkit"),
        }
    }
}

impl std::str::FromStr for Browser {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "chromium" | "chrome" => Ok(Browser::Chromium),
            "firefox" => Ok(Browser::Firefox),
            "webkit" | "safari" => Ok(Browser::WebKit),
            other => Err(format!(
                "Unknown browser '{}'. Supported: chromium, firefox, webkit",
                other
            )),
        }
    }
}

/// Configuration for a Playwright CLI invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaywrightConfig {
    /// Browser to use (default: Chromium).
    #[serde(default)]
    pub browser: Browser,

    /// Whether to run in headless mode (default: true).
    #[serde(default = "default_true")]
    pub headless: bool,

    /// Base URL to navigate to before performing actions.
    pub base_url: Option<String>,

    /// Working directory for the Playwright process.
    pub working_dir: Option<PathBuf>,

    /// Timeout in milliseconds for page operations (default: 30000).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,

    /// Additional arguments to pass to the browser.
    #[serde(default)]
    pub extra_args: Vec<String>,

    /// Path to the Playwright configuration file.
    pub config_file: Option<PathBuf>,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    30_000
}

impl Default for PlaywrightConfig {
    fn default() -> Self {
        Self {
            browser: Browser::Chromium,
            headless: true,
            base_url: None,
            working_dir: None,
            timeout_ms: default_timeout(),
            extra_args: Vec::new(),
            config_file: None,
        }
    }
}

// ── Action types ─────────────────────────────────────────────────────

/// A single browser action to perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserAction {
    /// Navigate to a URL.
    Navigate {
        url: String,
        /// Optional: wait until this condition is met.
        #[serde(skip_serializing_if = "Option::is_none")]
        wait_until: Option<WaitUntil>,
    },

    /// Click on an element.
    Click {
        selector: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        button: Option<MouseButton>,
    },

    /// Type text into an element.
    Fill {
        selector: String,
        value: String,
    },

    /// Press a keyboard key combination.
    Press {
        key: String,
    },

    /// Select an option in a `<select>` element.
    Select {
        selector: String,
        values: Vec<String>,
    },

    /// Check a checkbox or radio button.
    Check {
        selector: String,
    },

    /// Uncheck a checkbox.
    Uncheck {
        selector: String,
    },

    /// Hover over an element.
    Hover {
        selector: String,
    },

    /// Wait for an element to appear.
    WaitForSelector {
        selector: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<WaitState>,
    },

    /// Wait for a navigation to complete.
    WaitForNavigation {
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },

    /// Take a screenshot.
    Screenshot {
        /// Output file path.
        path: String,
        /// Whether to capture the full page (default: viewport only).
        #[serde(default)]
        full_page: bool,
    },

    /// Get the text content of an element.
    GetText {
        selector: String,
    },

    /// Evaluate JavaScript in the page context.
    Evaluate {
        expression: String,
    },

    /// Upload files to a file input.
    Upload {
        selector: String,
        files: Vec<String>,
    },
}

/// Wait condition for navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitUntil {
    Load,
    DomContentLoaded,
    NetworkIdle,
    Commit,
}

impl fmt::Display for WaitUntil {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WaitUntil::Load => write!(f, "load"),
            WaitUntil::DomContentLoaded => write!(f, "domcontentloaded"),
            WaitUntil::NetworkIdle => write!(f, "networkidle"),
            WaitUntil::Commit => write!(f, "commit"),
        }
    }
}

/// Wait state for an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitState {
    Attached,
    Detached,
    Visible,
    Hidden,
}

impl fmt::Display for WaitState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WaitState::Attached => write!(f, "attached"),
            WaitState::Detached => write!(f, "detached"),
            WaitState::Visible => write!(f, "visible"),
            WaitState::Hidden => write!(f, "hidden"),
        }
    }
}

/// Mouse button for click actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl fmt::Display for MouseButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MouseButton::Left => write!(f, "left"),
            MouseButton::Right => write!(f, "right"),
            MouseButton::Middle => write!(f, "middle"),
        }
    }
}

// ── Test-related types ───────────────────────────────────────────────

/// Configuration for running a Playwright test suite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfig {
    /// Test file pattern(s) or paths to run.
    #[serde(default = "default_test_paths")]
    pub test_paths: Vec<String>,

    /// Project name from playwright.config to use.
    pub project: Option<String>,

    /// Repeat count for each test.
    #[serde(default)]
    pub repeat_each: u32,

    /// Number of retries for failed tests.
    #[serde(default)]
    pub retries: u32,

    /// Number of parallel workers (default: 1).
    #[serde(default = "default_workers")]
    pub workers: u32,

    /// Timeout per test in milliseconds.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,

    /// Whether to update snapshots.
    #[serde(default)]
    pub update_snapshots: bool,

    /// Glob pattern to filter tests.
    pub grep: Option<String>,

    /// Reporter format.
    #[serde(default)]
    pub reporter: TestReporter,

    /// Output directory for test artifacts.
    pub output_dir: Option<PathBuf>,

    /// Working directory.
    pub working_dir: Option<PathBuf>,
}

fn default_workers() -> u32 {
    1
}

fn default_test_paths() -> Vec<String> {
    vec![".".to_string()]
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            test_paths: default_test_paths(),
            project: None,
            repeat_each: 0,
            retries: 0,
            workers: default_workers(),
            timeout_ms: default_timeout(),
            update_snapshots: false,
            grep: None,
            reporter: TestReporter::List,
            output_dir: None,
            working_dir: None,
        }
    }
}

/// Test reporter format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestReporter {
    List,
    Line,
    Dot,
    Html,
    Json,
    Junit,
}

impl Default for TestReporter {
    fn default() -> Self {
        TestReporter::List
    }
}

impl fmt::Display for TestReporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestReporter::List => write!(f, "list"),
            TestReporter::Line => write!(f, "line"),
            TestReporter::Dot => write!(f, "dot"),
            TestReporter::Html => write!(f, "html"),
            TestReporter::Json => write!(f, "json"),
            TestReporter::Junit => write!(f, "junit"),
        }
    }
}

/// Result of a Playwright test run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// Whether all tests passed.
    pub success: bool,
    /// Number of tests that passed.
    pub passed: u32,
    /// Number of tests that failed.
    pub failed: u32,
    /// Number of tests skipped.
    pub skipped: u32,
    /// Number of tests that timed out.
    pub timed_out: u32,
    /// Total test duration in milliseconds.
    pub duration_ms: u64,
    /// Raw stdout output.
    pub stdout: String,
    /// Raw stderr output.
    pub stderr: String,
    /// Exit code of the Playwright process.
    pub exit_code: i32,
}

impl TestResult {
    /// Total number of tests.
    pub fn total(&self) -> u32 {
        self.passed + self.failed + self.skipped + self.timed_out
    }
}

// ── Screenshot result ────────────────────────────────────────────────

/// Result of taking a screenshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotResult {
    /// Path where the screenshot was saved.
    pub path: PathBuf,
    /// File size in bytes.
    pub size_bytes: u64,
}

// ── Command output ───────────────────────────────────────────────────

/// Output from a subprocess command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutput {
    /// stdout content.
    pub stdout: String,
    /// stderr content.
    pub stderr: String,
    /// Whether the command exited successfully.
    pub success: bool,
    /// Process exit code.
    pub exit_code: i32,
}

// ── Action results ───────────────────────────────────────────────────

/// Result of executing a sequence of browser actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResults {
    /// Whether the overall execution succeeded.
    pub success: bool,
    /// Total number of actions that were requested.
    pub actions_total: usize,
    /// Individual results for each action.
    pub results: Vec<ActionResult>,
    /// Combined stdout from the script execution.
    pub stdout: String,
    /// Combined stderr from the script execution.
    pub stderr: String,
}

/// Result of a single browser action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    /// The action that was performed.
    pub action: String,
    /// Whether this action succeeded.
    pub success: bool,
    /// Output data (text content, screenshot path, JS result, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Error message if the action failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ActionResult {
    /// Create a successful action result with output.
    pub fn output(text: &str) -> Self {
        Self {
            action: "unknown".to_string(),
            success: true,
            output: Some(text.to_string()),
            error: None,
        }
    }
}

// ── Main Playwright CLI skill ────────────────────────────────────────

/// The Playwright CLI skill.
///
/// Provides typed methods for browser automation via the `npx playwright` CLI.
/// All methods spawn subprocess calls to Playwright — this module does not
/// embed a browser engine.
///
/// # Example
///
/// ```rust,no_run
/// use oxi::skills::playwright_cli::{PlaywrightCli, PlaywrightConfig};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let cli = PlaywrightCli::new(PlaywrightConfig::default());
///
///     // Check if Playwright is installed
///     cli.ensure_installed().await?;
///
///     // Run a quick automation script
///     cli.run_script(
///         "const { chromium } = require('playwright'); ..."
///     ).await?;
///
///     Ok(())
/// }
/// ```
pub struct PlaywrightCli {
    config: PlaywrightConfig,
}

impl PlaywrightCli {
    /// Create a new Playwright CLI instance with the given configuration.
    pub fn new(config: PlaywrightConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration and a specific browser.
    pub fn with_browser(browser: Browser) -> Self {
        Self {
            config: PlaywrightConfig {
                browser,
                ..Default::default()
            },
        }
    }

    /// Access the current configuration.
    pub fn config(&self) -> &PlaywrightConfig {
        &self.config
    }

    /// Resolve the working directory.
    fn working_dir(&self) -> &Path {
        self.config
            .working_dir
            .as_deref()
            .unwrap_or_else(|| Path::new("."))
    }

    // ── Installation / setup ─────────────────────────────────────────

    /// Check whether the Playwright CLI is available.
    ///
    /// Runs `npx playwright --version` to verify the CLI is accessible.
    pub async fn check_installed(&self) -> Result<bool> {
        let result = self.run_npx(&["--version"]).await;
        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                tracing::debug!("Playwright version: {}", stdout.trim());
                Ok(true)
            }
            Err(e) => {
                tracing::debug!("Playwright not available: {}", e);
                Ok(false)
            }
        }
    }

    /// Ensure Playwright is installed, installing it if necessary.
    ///
    /// 1. Checks if the CLI is available.
    /// 2. If not, runs `npm install` for `@playwright/test`.
    /// 3. Then installs browser binaries via `npx playwright install`.
    pub async fn ensure_installed(&self) -> Result<()> {
        if self.check_installed().await? {
            tracing::info!("Playwright is already installed");
            return Ok(());
        }

        tracing::info!("Installing @playwright/test...");

        // Install the npm package
        let output = Command::new("npm")
            .args(["install", "--save-dev", "@playwright/test"])
            .current_dir(self.working_dir())
            .output()
            .await
            .context("Failed to run npm install")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("npm install failed: {}", stderr);
        }

        // Install browser binaries
        tracing::info!("Installing Playwright browsers...");
        let output = self.run_npx(&["install"]).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Playwright browser install failed: {}", stderr);
        }

        tracing::info!("Playwright installed successfully");
        Ok(())
    }

    /// Install a specific browser (or all browsers if `None`).
    pub async fn install_browser(&self, browser: Option<Browser>) -> Result<()> {
        let mut args = vec!["install".to_string()];
        if let Some(b) = browser {
            args.push(b.to_string());
        }

        let output = self.run_npx(&args).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Browser install failed: {}", stderr);
        }

        Ok(())
    }

    // ── Browser automation via scripts ───────────────────────────────

    /// Run a JavaScript snippet using Node.js with Playwright.
    ///
    /// The script should use Playwright's API. Example:
    /// ```javascript
    /// const { chromium } = require('playwright');
    /// (async () => {
    ///     const browser = await chromium.launch();
    ///     const page = await browser.newPage();
    ///     await page.goto('https://example.com');
    ///     const title = await page.title();
    ///     console.log(title);
    ///     await browser.close();
    /// })();
    /// ```
    pub async fn run_script(&self, script: &str) -> Result<CommandOutput> {
        // Write script to a temp file
        let tmp_dir = tempfile::tempdir().context("Failed to create temp dir")?;
        let script_path = tmp_dir.path().join("playwright-script.js");
        std::fs::write(&script_path, script)
            .context("Failed to write temp script")?;

        let node_output = Command::new("node")
            .arg(&script_path)
            .current_dir(self.working_dir())
            .output()
            .await
            .context("Failed to run node script")?;

        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&node_output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&node_output.stderr).to_string(),
            success: node_output.status.success(),
            exit_code: node_output.status.code().unwrap_or(-1),
        })
    }

    /// Run a fully-formed Playwright automation sequence.
    ///
    /// Takes a list of [`BrowserAction`]s and generates a Node.js script
    /// that executes them in order, then returns the results.
    pub async fn execute_actions(
        &self,
        url: &str,
        actions: &[BrowserAction],
    ) -> Result<ActionResults> {
        let script = Self::generate_action_script(
            url,
            actions,
            self.config.headless,
            self.config.timeout_ms,
        );
        let output = self.run_script(&script).await?;

        let results = if output.success {
            // Try to parse the stdout as JSON results
            match serde_json::from_str::<ActionResults>(&output.stdout.trim()) {
                Ok(r) => r,
                Err(_) => ActionResults {
                    success: true,
                    actions_total: actions.len(),
                    results: vec![ActionResult::output(&output.stdout)],
                    stdout: output.stdout.clone(),
                    stderr: output.stderr.clone(),
                },
            }
        } else {
            ActionResults {
                success: false,
                actions_total: actions.len(),
                results: vec![],
                stdout: output.stdout,
                stderr: output.stderr,
            }
        };

        Ok(results)
    }

    /// Take a screenshot of a URL.
    pub async fn screenshot(
        &self,
        url: &str,
        output_path: &str,
        full_page: bool,
    ) -> Result<ScreenshotResult> {
        let full_page_flag = if full_page { "true" } else { "false" };
        let script = format!(
            r#"
const {{ chromium }} = require('playwright');
(async () => {{
    const browser = await chromium.launch({{
        headless: {headless}
    }});
    const page = await browser.newPage();
    await page.goto('{url}', {{ waitUntil: 'networkidle', timeout: {timeout} }});
    await page.screenshot({{
        path: '{output_path}',
        fullPage: {full_page_flag}
    }});
    await browser.close();

    const fs = require('fs');
    const stats = fs.statSync('{output_path}');
    console.log(JSON.stringify({{ path: '{output_path}', size_bytes: stats.size }}));
}})();
"#,
            headless = self.config.headless,
            url = url,
            timeout = self.config.timeout_ms,
            output_path = output_path,
            full_page_flag = full_page_flag,
        );

        let output = self.run_script(&script).await?;

        if !output.success {
            bail!("Screenshot failed: {}", output.stderr);
        }

        // Parse the JSON from stdout
        let result: serde_json::Value =
            serde_json::from_str(output.stdout.trim()).unwrap_or_else(|_| {
                serde_json::json!({
                    "path": output_path,
                    "size_bytes": 0
                })
            });

        Ok(ScreenshotResult {
            path: PathBuf::from(result["path"].as_str().unwrap_or(output_path)),
            size_bytes: result["size_bytes"].as_u64().unwrap_or(0),
        })
    }

    /// Get the text content of an element on a page.
    pub async fn get_text(&self, url: &str, selector: &str) -> Result<String> {
        let script = format!(
            r#"
const {{ chromium }} = require('playwright');
(async () => {{
    const browser = await chromium.launch({{
        headless: {headless}
    }});
    const page = await browser.newPage();
    await page.goto('{url}', {{ waitUntil: 'networkidle', timeout: {timeout} }});
    const text = await page.textContent('{selector}');
    console.log(JSON.stringify({{ text: text || '' }}));
    await browser.close();
}})();
"#,
            headless = self.config.headless,
            url = url,
            timeout = self.config.timeout_ms,
            selector = selector,
        );

        let output = self.run_script(&script).await?;

        if !output.success {
            bail!("Get text failed: {}", output.stderr);
        }

        let result: serde_json::Value = serde_json::from_str(output.stdout.trim())
            .context("Failed to parse get_text output")?;

        Ok(result["text"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    // ── Test runner ──────────────────────────────────────────────────

    /// Run a Playwright test suite.
    pub async fn run_tests(&self, test_config: &TestConfig) -> Result<TestResult> {
        let mut args = vec!["test".to_string()];

        // Test paths
        args.extend(test_config.test_paths.iter().cloned());

        // Project
        if let Some(ref project) = test_config.project {
            args.push("--project".to_string());
            args.push(project.clone());
        }

        // Workers
        args.push("--workers".to_string());
        args.push(test_config.workers.to_string());

        // Retries
        if test_config.retries > 0 {
            args.push("--retries".to_string());
            args.push(test_config.retries.to_string());
        }

        // Repeat
        if test_config.repeat_each > 0 {
            args.push("--repeat-each".to_string());
            args.push(test_config.repeat_each.to_string());
        }

        // Timeout
        args.push("--timeout".to_string());
        args.push(test_config.timeout_ms.to_string());

        // Reporter
        args.push("--reporter".to_string());
        args.push(test_config.reporter.to_string());

        // Update snapshots
        if test_config.update_snapshots {
            args.push("--update-snapshots".to_string());
        }

        // Grep
        if let Some(ref grep) = test_config.grep {
            args.push("--grep".to_string());
            args.push(grep.clone());
        }

        // Output dir
        if let Some(ref output_dir) = test_config.output_dir {
            args.push("--output".to_string());
            args.push(output_dir.to_string_lossy().to_string());
        }

        // Config file from global config or test config
        if let Some(ref config_file) = self.config.config_file {
            args.push("--config".to_string());
            args.push(config_file.to_string_lossy().to_string());
        }

        let working_dir = test_config
            .working_dir
            .as_deref()
            .or(self.config.working_dir.as_deref())
            .unwrap_or_else(|| Path::new("."));

        let output = Command::new("npx")
            .args(&["playwright"])
            .args(&args)
            .current_dir(working_dir)
            .output()
            .await
            .context("Failed to run Playwright tests")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);
        let success = exit_code == 0;

        // Parse test counts from output
        let (passed, failed, skipped, timed_out, duration_ms) =
            Self::parse_test_output(&stdout, &stderr);

        Ok(TestResult {
            success,
            passed,
            failed,
            skipped,
            timed_out,
            duration_ms,
            stdout,
            stderr,
            exit_code,
        })
    }

    /// Run a single test file.
    pub async fn run_test_file(&self, path: &str) -> Result<TestResult> {
        let config = TestConfig {
            test_paths: vec![path.to_string()],
            ..Default::default()
        };
        self.run_tests(&config).await
    }

    /// Generate a Playwright test file from a list of page interactions.
    ///
    /// Produces a complete TypeScript test file that can be run with
    /// `npx playwright test`.
    pub fn generate_test_file(
        test_name: &str,
        url: &str,
        actions: &[BrowserAction],
    ) -> String {
        let mut code = String::with_capacity(2048);

        code.push_str("import { test, expect } from '@playwright/test';\n\n");

        code.push_str(&format!(
            "test('{}', async ({{ page }}) => {{\n",
            test_name
        ));

        code.push_str(&format!("  await page.goto('{}');\n", url));

        for action in actions {
            code.push_str(&Self::action_to_playwright_code(action));
        }

        code.push_str("});\n");

        code
    }

    // ── Code generation ──────────────────────────────────────────────

    /// Generate a complete Node.js script for a sequence of actions.
    fn generate_action_script(
        url: &str,
        actions: &[BrowserAction],
        headless: bool,
        timeout_ms: u64,
    ) -> String {
        let mut script = String::with_capacity(4096);

        script.push_str("const { chromium } = require('playwright');\n");
        script.push_str("(async () => {\n");
        script.push_str(&format!(
            "  const browser = await chromium.launch({{ headless: {} }});\n",
            headless
        ));
        script.push_str("  const page = await browser.newPage();\n");
        script.push_str("  const results = [];\n\n  try {\n");
        script.push_str(&format!(
            "    await page.goto('{}', {{ waitUntil: 'networkidle', timeout: {} }});\n",
            url, timeout_ms
        ));

        for action in actions {
            script.push_str(&Self::action_to_node_code(action));
        }

        script.push_str("    console.log(JSON.stringify({ success: true, actions_total: ");
        script.push_str(&actions.len().to_string());
        script.push_str(", results }));\n");
        script.push_str("  } catch (error) {\n");
        script.push_str("    console.error(JSON.stringify({ success: false, error: error.message }));\n");
        script.push_str("  } finally {\n");
        script.push_str("    await browser.close();\n");
        script.push_str("  }\n");
        script.push_str("})();\n");

        script
    }

    /// Convert a browser action to Playwright test code (for test file generation).
    fn action_to_playwright_code(action: &BrowserAction) -> String {
        match action {
            BrowserAction::Navigate { url, wait_until } => {
                let wait = wait_until
                    .map(|w| format!(", waitUntil: '{}'", w))
                    .unwrap_or_default();
                format!("  await page.goto('{}'{});\n", url, wait)
            }
            BrowserAction::Click { selector, button } => {
                let opts = button
                    .map(|b| format!(", {{ button: '{}' }}", b))
                    .unwrap_or_default();
                format!("  await page.click('{}'{});\n", selector, opts)
            }
            BrowserAction::Fill { selector, value } => {
                format!("  await page.fill('{}', '{}');\n", selector, value)
            }
            BrowserAction::Press { key } => {
                format!("  await page.keyboard.press('{}');\n", key)
            }
            BrowserAction::Select { selector, values } => {
                let vals: Vec<String> = values.iter().map(|v| format!("'{}'", v)).collect();
                format!("  await page.selectOption('{}', [{}]);\n", selector, vals.join(", "))
            }
            BrowserAction::Check { selector } => {
                format!("  await page.check('{}');\n", selector)
            }
            BrowserAction::Uncheck { selector } => {
                format!("  await page.uncheck('{}');\n", selector)
            }
            BrowserAction::Hover { selector } => {
                format!("  await page.hover('{}');\n", selector)
            }
            BrowserAction::WaitForSelector { selector, state } => {
                let opts = state
                    .map(|s| format!(", {{ state: '{}' }}", s))
                    .unwrap_or_default();
                format!("  await page.waitForSelector('{}'{});\n", selector, opts)
            }
            BrowserAction::WaitForNavigation { url } => {
                let url_opt = url
                    .as_ref()
                    .map(|u| format!("{{ url: '{}' }}", u))
                    .unwrap_or_default();
                format!("  await page.waitForNavigation({});\n", url_opt)
            }
            BrowserAction::Screenshot { path, full_page } => {
                format!(
                    "  await page.screenshot({{ path: '{}', fullPage: {} }});\n",
                    path, full_page
                )
            }
            BrowserAction::GetText { selector } => {
                format!(
                    "  const text = await page.textContent('{}');\n  console.log(text);\n",
                    selector
                )
            }
            BrowserAction::Evaluate { expression } => {
                format!("  await page.evaluate(`{}`);\n", expression)
            }
            BrowserAction::Upload { selector, files } => {
                let files_json = serde_json::to_string(files).unwrap_or_default();
                format!("  await page.setInputFiles('{}', {});\n", selector, files_json)
            }
        }
    }

    /// Convert a browser action to standalone Node.js code (for script generation).
    fn action_to_node_code(action: &BrowserAction) -> String {
        match action {
            BrowserAction::Navigate { url, wait_until } => {
                let wait = wait_until
                    .map(|w| format!(", waitUntil: '{}'", w))
                    .unwrap_or_default();
                format!(
                    "    results.push({{ action: 'navigate', success: true }});\n    await page.goto('{}'{});\n",
                    url, wait
                )
            }
            BrowserAction::Click { selector, button } => {
                let opts = button
                    .map(|b| format!(", {{ button: '{}' }}", b))
                    .unwrap_or_default();
                format!(
                    "    await page.click('{}'{});\n    results.push({{ action: 'click', success: true }});\n",
                    selector, opts
                )
            }
            BrowserAction::Fill { selector, value } => {
                format!(
                    "    await page.fill('{}', '{}');\n    results.push({{ action: 'fill', success: true }});\n",
                    selector, value
                )
            }
            BrowserAction::Press { key } => {
                format!(
                    "    await page.keyboard.press('{}');\n    results.push({{ action: 'press', success: true }});\n",
                    key
                )
            }
            BrowserAction::Select { selector, values } => {
                let vals: Vec<String> = values.iter().map(|v| format!("'{}'", v)).collect();
                format!(
                    "    await page.selectOption('{}', [{}]);\n    results.push({{ action: 'select', success: true }});\n",
                    selector, vals.join(", ")
                )
            }
            BrowserAction::Check { selector } => {
                format!(
                    "    await page.check('{}');\n    results.push({{ action: 'check', success: true }});\n",
                    selector
                )
            }
            BrowserAction::Uncheck { selector } => {
                format!(
                    "    await page.uncheck('{}');\n    results.push({{ action: 'uncheck', success: true }});\n",
                    selector
                )
            }
            BrowserAction::Hover { selector } => {
                format!(
                    "    await page.hover('{}');\n    results.push({{ action: 'hover', success: true }});\n",
                    selector
                )
            }
            BrowserAction::WaitForSelector { selector, state } => {
                let opts = state
                    .map(|s| format!(", {{ state: '{}' }}", s))
                    .unwrap_or_default();
                format!(
                    "    await page.waitForSelector('{}'{});\n    results.push({{ action: 'waitForSelector', success: true }});\n",
                    selector, opts
                )
            }
            BrowserAction::WaitForNavigation { url } => {
                let url_opt = url
                    .as_ref()
                    .map(|u| format!("{{ url: '{}' }}", u))
                    .unwrap_or_default();
                format!(
                    "    await page.waitForNavigation({});\n    results.push({{ action: 'waitForNavigation', success: true }});\n",
                    url_opt
                )
            }
            BrowserAction::Screenshot { path, full_page } => {
                format!(
                    "    await page.screenshot({{ path: '{}', fullPage: {} }});\n    results.push({{ action: 'screenshot', success: true, output: '{}' }});\n",
                    path, full_page, path
                )
            }
            BrowserAction::GetText { selector } => {
                format!(
                    "    const text = await page.textContent('{}');\n    results.push({{ action: 'getText', success: true, output: text || '' }});\n",
                    selector
                )
            }
            BrowserAction::Evaluate { expression } => {
                format!(
                    "    const evalResult = await page.evaluate(`{}`);\n    results.push({{ action: 'evaluate', success: true, output: String(evalResult) }});\n",
                    expression
                )
            }
            BrowserAction::Upload { selector, files } => {
                let files_json = serde_json::to_string(files).unwrap_or_default();
                format!(
                    "    await page.setInputFiles('{}', {});\n    results.push({{ action: 'upload', success: true }});\n",
                    selector, files_json
                )
            }
        }
    }

    /// Parse test result counts from Playwright output.
    ///
    /// Looks for patterns like:
    /// - `5 passed`
    /// - `2 failed`
    /// - `1 skipped`
    /// - `ran in 1234ms` or `finished in 5s`
    fn parse_test_output(stdout: &str, stderr: &str) -> (u32, u32, u32, u32, u64) {
        let combined = format!("{}\n{}", stdout, stderr);
        let mut passed = 0u32;
        let mut failed = 0u32;
        let mut skipped = 0u32;
        let mut timed_out = 0u32;
        let mut duration_ms = 0u64;

        for line in combined.lines() {
            let line_lower = line.to_lowercase();

            // Match patterns like "5 passed", "2 failed", "1 skipped"
            if let Some(count) = Self::extract_count(&line_lower, "passed") {
                passed = count;
            }
            if let Some(count) = Self::extract_count(&line_lower, "failed") {
                failed = count;
            }
            if let Some(count) = Self::extract_count(&line_lower, "skipped") {
                skipped = count;
            }
            if let Some(count) = Self::extract_count(&line_lower, "timed out") {
                timed_out = count;
            }

            // Duration patterns
            if let Some(ms) = Self::extract_duration_ms(&line_lower) {
                duration_ms = ms;
            }
        }

        (passed, failed, skipped, timed_out, duration_ms)
    }

    /// Extract a count from a string like "5 passed" or "passed (5)".
    fn extract_count(text: &str, keyword: &str) -> Option<u32> {
        // Try pattern: "N keyword"
        if let Some(pos) = text.find(keyword) {
            let before = &text[..pos];
            // Find the last number before the keyword, skipping whitespace
            let reversed: String = before.chars().rev().skip_while(|c| c.is_whitespace()).collect();
            let num: String = reversed
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = num.parse::<u32>() {
                return Some(n);
            }
        }
        None
    }

    /// Extract duration in milliseconds from patterns like "ran in 1234ms" or "finished in 5s".
    fn extract_duration_ms(text: &str) -> Option<u64> {
        // Pattern: "Nms"
        if let Some(pos) = text.find("ms") {
            let before = &text[..pos];
            let num: String = before
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            if let Ok(n) = num.parse::<u64>() {
                return Some(n);
            }
        }

        // Pattern: "Ns" (seconds, not ms) — only if it doesn't end in "ms"
        if let Some(pos) = text.rfind(" in ") {
            let after = &text[pos + 4..];
            // Look for pattern like "5s" or "1.2s"
            let s_pos = after.find('s');
            let ms_pos = after.find("ms");
            if let Some(sp) = s_pos {
                // Only match if there's no "ms" or "ms" is after "s"
                if ms_pos.map(|mp| sp < mp).unwrap_or(true) {
                    let num_str = &after[..sp];
                    if let Ok(n) = num_str.parse::<f64>() {
                        return Some((n * 1000.0) as u64);
                    }
                }
            }
        }

        None
    }

    // ── Low-level command execution ──────────────────────────────────

    /// Run `npx playwright` with the given arguments.
    async fn run_npx<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut cmd = Command::new("npx");
        cmd.arg("playwright");
        cmd.args(args);
        cmd.current_dir(self.working_dir());

        let output = cmd.output().await.context("Failed to execute npx playwright")?;
        Ok(output)
    }
}

impl fmt::Debug for PlaywrightCli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaywrightCli")
            .field("browser", &self.config.browser)
            .field("headless", &self.config.headless)
            .field("timeout_ms", &self.config.timeout_ms)
            .finish()
    }
}

// ── Skill instructions ───────────────────────────────────────────────

/// Generate the skill instructions to be injected into the system prompt
/// when the Playwright CLI skill is active.
///
/// This tells the LLM how to use browser automation and testing via
/// Playwright through the available tools (bash, read, write).
pub fn skill_instructions() -> String {
    let prompt = r#"# Playwright CLI Skill

You are now operating in **playwright-cli mode**. Your goal is to automate
browser interactions, test web pages, and work with Playwright test suites.

## Capabilities

### 1. Browser Automation
- Navigate to URLs and wait for page conditions
- Click, type, fill forms, select options
- Take screenshots (full page or viewport)
- Extract text content from elements
- Execute arbitrary JavaScript in page context
- Upload files, check/uncheck checkboxes
- Hover over elements, press keyboard keys

### 2. Page Testing
- Run full Playwright test suites
- Run individual test files
- Generate Playwright test code from actions
- Parse test results (passed, failed, skipped, timed out)
- Support multiple reporters (list, line, dot, html, json, junit)

### 3. Multi-Browser Support
- Chromium (default)
- Firefox
- WebKit

## Workflow

### For Browser Automation
1. Ensure Playwright is installed (`npx playwright --version`)
2. If not installed, install it (`npm install --save-dev @playwright/test && npx playwright install`)
3. Write a Node.js script using Playwright's API
4. Execute the script via `node <script.js>`
5. Parse the output

### For Running Tests
1. Verify the Playwright config file exists (`playwright.config.ts` or `playwright.config.js`)
2. Run tests: `npx playwright test [path...] [options]`
3. Parse the test output for pass/fail counts
4. Review any failure details from the output

### For Generating Tests
1. Define the test scenario as a sequence of actions
2. Generate a TypeScript test file using Playwright's test API
3. Save the file and run it with `npx playwright test`

## Guidelines

- **Always use headless mode** unless the user explicitly requests headed mode
- **Set appropriate timeouts** — 30 seconds is the default, increase for slow pages
- **Wait for conditions** — use `waitForSelector` or `waitForNavigation` instead of arbitrary sleeps
- **Use specific selectors** — prefer `data-testid`, `aria-*`, or CSS selectors
- **Take screenshots on failure** — use the `--output` flag to capture artifacts
- **Clean up** — always close browsers in finally blocks
- **Test across browsers** — if multi-browser support matters, test with `--project` for each browser

## Common Patterns

### Quick Page Check
```bash
node -e "
const { chromium } = require('playwright');
(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto('https://example.com');
  const title = await page.title();
  console.log('Title:', title);
  await browser.close();
})();
"
```

### Run Tests
```bash
npx playwright test tests/example.spec.ts --reporter=list
```

### Take Screenshot
```bash
node -e "
const { chromium } = require('playwright');
(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto('https://example.com');
  await page.screenshot({ path: 'screenshot.png', fullPage: true });
  await browser.close();
})();
"
```
"#;
    prompt.to_string()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Browser type tests ──────────────────────────────────────────

    #[test]
    fn test_browser_default() {
        assert_eq!(Browser::default(), Browser::Chromium);
    }

    #[test]
    fn test_browser_display() {
        assert_eq!(format!("{}", Browser::Chromium), "chromium");
        assert_eq!(format!("{}", Browser::Firefox), "firefox");
        assert_eq!(format!("{}", Browser::WebKit), "webkit");
    }

    #[test]
    fn test_browser_from_str() {
        assert_eq!("chromium".parse::<Browser>().unwrap(), Browser::Chromium);
        assert_eq!("chrome".parse::<Browser>().unwrap(), Browser::Chromium);
        assert_eq!("firefox".parse::<Browser>().unwrap(), Browser::Firefox);
        assert_eq!("webkit".parse::<Browser>().unwrap(), Browser::WebKit);
        assert_eq!("safari".parse::<Browser>().unwrap(), Browser::WebKit);
        assert!("opera".parse::<Browser>().is_err());
    }

    // ── Config tests ────────────────────────────────────────────────

    #[test]
    fn test_config_default() {
        let config = PlaywrightConfig::default();
        assert_eq!(config.browser, Browser::Chromium);
        assert!(config.headless);
        assert!(config.base_url.is_none());
        assert!(config.working_dir.is_none());
        assert_eq!(config.timeout_ms, 30_000);
        assert!(config.extra_args.is_empty());
        assert!(config.config_file.is_none());
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = PlaywrightConfig {
            browser: Browser::Firefox,
            headless: false,
            base_url: Some("http://localhost:3000".to_string()),
            working_dir: Some(PathBuf::from("/tmp/project")),
            timeout_ms: 60_000,
            extra_args: vec!["--disable-gpu".to_string()],
            config_file: Some(PathBuf::from("playwright.config.ts")),
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: PlaywrightConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.browser, Browser::Firefox);
        assert!(!parsed.headless);
        assert_eq!(parsed.base_url, Some("http://localhost:3000".to_string()));
        assert_eq!(parsed.timeout_ms, 60_000);
        assert_eq!(parsed.extra_args.len(), 1);
    }

    // ── Test config tests ───────────────────────────────────────────

    #[test]
    fn test_test_config_default() {
        let config = TestConfig::default();
        assert_eq!(config.test_paths, vec![".".to_string()]);
        assert!(config.project.is_none());
        assert_eq!(config.workers, 1);
        assert_eq!(config.retries, 0);
        assert_eq!(config.timeout_ms, 30_000);
        assert!(!config.update_snapshots);
        assert!(config.grep.is_none());
        assert_eq!(config.reporter, TestReporter::List);
        assert!(config.output_dir.is_none());
    }

    // ── Enum display tests ──────────────────────────────────────────

    #[test]
    fn test_wait_until_display() {
        assert_eq!(format!("{}", WaitUntil::Load), "load");
        assert_eq!(format!("{}", WaitUntil::DomContentLoaded), "domcontentloaded");
        assert_eq!(format!("{}", WaitUntil::NetworkIdle), "networkidle");
        assert_eq!(format!("{}", WaitUntil::Commit), "commit");
    }

    #[test]
    fn test_wait_state_display() {
        assert_eq!(format!("{}", WaitState::Attached), "attached");
        assert_eq!(format!("{}", WaitState::Detached), "detached");
        assert_eq!(format!("{}", WaitState::Visible), "visible");
        assert_eq!(format!("{}", WaitState::Hidden), "hidden");
    }

    #[test]
    fn test_mouse_button_display() {
        assert_eq!(format!("{}", MouseButton::Left), "left");
        assert_eq!(format!("{}", MouseButton::Right), "right");
        assert_eq!(format!("{}", MouseButton::Middle), "middle");
    }

    #[test]
    fn test_reporter_display() {
        assert_eq!(format!("{}", TestReporter::List), "list");
        assert_eq!(format!("{}", TestReporter::Line), "line");
        assert_eq!(format!("{}", TestReporter::Dot), "dot");
        assert_eq!(format!("{}", TestReporter::Html), "html");
        assert_eq!(format!("{}", TestReporter::Json), "json");
        assert_eq!(format!("{}", TestReporter::Junit), "junit");
    }

    #[test]
    fn test_reporter_default() {
        assert_eq!(TestReporter::default(), TestReporter::List);
    }

    // ── BrowserAction serialization tests ───────────────────────────

    #[test]
    fn test_browser_action_navigate() {
        let action = BrowserAction::Navigate {
            url: "https://example.com".to_string(),
            wait_until: Some(WaitUntil::NetworkIdle),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("navigate"));
        assert!(json.contains("example.com"));
        assert!(json.contains("networkidle"));
    }

    #[test]
    fn test_browser_action_click() {
        let action = BrowserAction::Click {
            selector: "#button".to_string(),
            button: Some(MouseButton::Right),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("click"));
        assert!(json.contains("#button"));
        assert!(json.contains("right"));
    }

    #[test]
    fn test_browser_action_fill() {
        let action = BrowserAction::Fill {
            selector: "#input".to_string(),
            value: "hello world".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("fill"));
        assert!(json.contains("hello world"));
    }

    #[test]
    fn test_browser_action_screenshot() {
        let action = BrowserAction::Screenshot {
            path: "output.png".to_string(),
            full_page: true,
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("screenshot"));
        assert!(json.contains("output.png"));
    }

    #[test]
    fn test_browser_action_evaluate() {
        let action = BrowserAction::Evaluate {
            expression: "document.title".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("evaluate"));
        assert!(json.contains("document.title"));
    }

    #[test]
    fn test_browser_action_upload() {
        let action = BrowserAction::Upload {
            selector: "#file-input".to_string(),
            files: vec!["test.pdf".to_string(), "doc.txt".to_string()],
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("upload"));
        assert!(json.contains("test.pdf"));
    }

    // ── Code generation tests ───────────────────────────────────────

    #[test]
    fn test_action_to_playwright_code_navigate() {
        let action = BrowserAction::Navigate {
            url: "https://example.com".to_string(),
            wait_until: None,
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("page.goto('https://example.com')"));
    }

    #[test]
    fn test_action_to_playwright_code_navigate_with_wait() {
        let action = BrowserAction::Navigate {
            url: "https://example.com".to_string(),
            wait_until: Some(WaitUntil::NetworkIdle),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("waitUntil: 'networkidle'"));
    }

    #[test]
    fn test_action_to_playwright_code_click() {
        let action = BrowserAction::Click {
            selector: "#btn".to_string(),
            button: None,
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("page.click('#btn')"));
    }

    #[test]
    fn test_action_to_playwright_code_fill() {
        let action = BrowserAction::Fill {
            selector: "#search".to_string(),
            value: "rust".to_string(),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("page.fill('#search', 'rust')"));
    }

    #[test]
    fn test_action_to_playwright_code_press() {
        let action = BrowserAction::Press {
            key: "Enter".to_string(),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("keyboard.press('Enter')"));
    }

    #[test]
    fn test_action_to_playwright_code_select() {
        let action = BrowserAction::Select {
            selector: "#dropdown".to_string(),
            values: vec!["option1".to_string(), "option2".to_string()],
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("selectOption('#dropdown'"));
        assert!(code.contains("'option1'"));
        assert!(code.contains("'option2'"));
    }

    #[test]
    fn test_action_to_playwright_code_screenshot() {
        let action = BrowserAction::Screenshot {
            path: "shot.png".to_string(),
            full_page: true,
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("screenshot"));
        assert!(code.contains("shot.png"));
        assert!(code.contains("fullPage: true"));
    }

    #[test]
    fn test_action_to_playwright_code_get_text() {
        let action = BrowserAction::GetText {
            selector: "h1".to_string(),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("textContent('h1')"));
    }

    #[test]
    fn test_action_to_playwright_code_evaluate() {
        let action = BrowserAction::Evaluate {
            expression: "document.title".to_string(),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("page.evaluate"));
        assert!(code.contains("document.title"));
    }

    #[test]
    fn test_action_to_playwright_code_hover() {
        let action = BrowserAction::Hover {
            selector: ".menu-item".to_string(),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("page.hover('.menu-item')"));
    }

    #[test]
    fn test_action_to_playwright_code_check() {
        let action = BrowserAction::Check {
            selector: "#agree".to_string(),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("page.check('#agree')"));
    }

    #[test]
    fn test_action_to_playwright_code_uncheck() {
        let action = BrowserAction::Uncheck {
            selector: "#newsletter".to_string(),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("page.uncheck('#newsletter')"));
    }

    #[test]
    fn test_action_to_playwright_code_wait_for_selector() {
        let action = BrowserAction::WaitForSelector {
            selector: ".loaded".to_string(),
            state: Some(WaitState::Visible),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("waitForSelector('.loaded')"));
        assert!(code.contains("state: 'visible'"));
    }

    #[test]
    fn test_action_to_playwright_code_wait_for_selector_no_state() {
        let action = BrowserAction::WaitForSelector {
            selector: ".loaded".to_string(),
            state: None,
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("waitForSelector('.loaded')"));
        assert!(!code.contains("state"));
    }

    #[test]
    fn test_action_to_playwright_code_wait_for_navigation() {
        let action = BrowserAction::WaitForNavigation {
            url: Some("https://example.com/success".to_string()),
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("waitForNavigation"));
        assert!(code.contains("example.com/success"));
    }

    #[test]
    fn test_action_to_playwright_code_wait_for_navigation_no_url() {
        let action = BrowserAction::WaitForNavigation { url: None };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("waitForNavigation()"));
    }

    #[test]
    fn test_action_to_playwright_code_upload() {
        let action = BrowserAction::Upload {
            selector: "#file".to_string(),
            files: vec!["a.pdf".to_string()],
        };
        let code = PlaywrightCli::action_to_playwright_code(&action);
        assert!(code.contains("setInputFiles('#file'"));
        assert!(code.contains("a.pdf"));
    }

    // ── Test file generation ────────────────────────────────────────

    #[test]
    fn test_generate_test_file() {
        let actions = vec![
            BrowserAction::Fill {
                selector: "#search".to_string(),
                value: "rust".to_string(),
            },
            BrowserAction::Click {
                selector: "#search-btn".to_string(),
                button: None,
            },
            BrowserAction::GetText {
                selector: "h1".to_string(),
            },
        ];

        let code = PlaywrightCli::generate_test_file(
            "search works",
            "https://example.com",
            &actions,
        );

        assert!(code.contains("import { test, expect }"));
        assert!(code.contains("test('search works'"));
        assert!(code.contains("page.goto('https://example.com')"));
        assert!(code.contains("page.fill('#search', 'rust')"));
        assert!(code.contains("page.click('#search-btn')"));
        assert!(code.contains("textContent('h1')"));
    }

    // ── Action script generation ────────────────────────────────────

    #[test]
    fn test_generate_action_script() {
        let actions = vec![
            BrowserAction::Navigate {
                url: "https://example.com".to_string(),
                wait_until: None,
            },
            BrowserAction::Click {
                selector: "#btn".to_string(),
                button: None,
            },
        ];

        let script = PlaywrightCli::generate_action_script(
            "https://example.com",
            &actions,
            true,
            30_000,
        );

        assert!(script.contains("require('playwright')"));
        assert!(script.contains("chromium.launch"));
        assert!(script.contains("headless: true"));
        assert!(script.contains("page.goto"));
        assert!(script.contains("page.click('#btn')"));
        assert!(script.contains("browser.close"));
    }

    // ── Test result parsing ─────────────────────────────────────────

    #[test]
    fn test_parse_test_output_basic() {
        let stdout = "running 3 tests\n  ✓ test 1\n  ✓ test 2\n  ✓ test 3\n\n3 passed (5s)";
        let (passed, failed, skipped, timed_out, duration_ms) =
            PlaywrightCli::parse_test_output(stdout, "");
        assert_eq!(passed, 3);
        assert_eq!(failed, 0);
        assert_eq!(skipped, 0);
        assert_eq!(timed_out, 0);
        assert_eq!(duration_ms, 5000);
    }

    #[test]
    fn test_parse_test_output_mixed() {
        let stdout = "5 passed\n2 failed\n1 skipped\n1 timed out\nran in 2500ms";
        let (passed, failed, skipped, timed_out, duration_ms) =
            PlaywrightCli::parse_test_output(stdout, "");
        assert_eq!(passed, 5);
        assert_eq!(failed, 2);
        assert_eq!(skipped, 1);
        assert_eq!(timed_out, 1);
        assert_eq!(duration_ms, 2500);
    }

    #[test]
    fn test_parse_test_output_from_stderr() {
        let stdout = "";
        let stderr = "3 passed (1.5s)";
        let (passed, failed, skipped, timed_out, duration_ms) =
            PlaywrightCli::parse_test_output(stdout, stderr);
        assert_eq!(passed, 3);
        assert_eq!(duration_ms, 1500);
    }

    #[test]
    fn test_parse_test_output_no_results() {
        let stdout = "some unrelated output";
        let (passed, failed, skipped, timed_out, duration_ms) =
            PlaywrightCli::parse_test_output(stdout, "");
        assert_eq!(passed, 0);
        assert_eq!(failed, 0);
        assert_eq!(skipped, 0);
        assert_eq!(timed_out, 0);
        assert_eq!(duration_ms, 0);
    }

    // ── Extract count tests ─────────────────────────────────────────

    #[test]
    fn test_extract_count_pattern() {
        assert_eq!(PlaywrightCli::extract_count("5 passed", "passed"), Some(5));
        assert_eq!(PlaywrightCli::extract_count("2 failed", "failed"), Some(2));
        assert_eq!(PlaywrightCli::extract_count("10 skipped", "skipped"), Some(10));
        assert_eq!(PlaywrightCli::extract_count("1 timed out", "timed out"), Some(1));
    }

    #[test]
    fn test_extract_count_no_match() {
        assert_eq!(PlaywrightCli::extract_count("hello world", "passed"), None);
    }

    // ── Extract duration tests ──────────────────────────────────────

    #[test]
    fn test_extract_duration_ms_pattern() {
        assert_eq!(PlaywrightCli::extract_duration_ms("ran in 2500ms"), Some(2500));
        assert_eq!(PlaywrightCli::extract_duration_ms("finished in 100ms"), Some(100));
    }

    #[test]
    fn test_extract_duration_seconds_pattern() {
        assert_eq!(PlaywrightCli::extract_duration_ms("ran in 5s"), Some(5000));
        assert_eq!(PlaywrightCli::extract_duration_ms("finished in 1.5s"), Some(1500));
    }

    #[test]
    fn test_extract_duration_no_match() {
        assert_eq!(PlaywrightCli::extract_duration_ms("hello world"), None);
    }

    // ── PlaywrightCli construction tests ────────────────────────────

    #[test]
    fn test_cli_new() {
        let cli = PlaywrightCli::new(PlaywrightConfig::default());
        assert_eq!(cli.config().browser, Browser::Chromium);
        assert!(cli.config().headless);
    }

    #[test]
    fn test_cli_with_browser() {
        let cli = PlaywrightCli::with_browser(Browser::Firefox);
        assert_eq!(cli.config().browser, Browser::Firefox);
    }

    #[test]
    fn test_cli_debug() {
        let cli = PlaywrightCli::new(PlaywrightConfig::default());
        let debug = format!("{:?}", cli);
        assert!(debug.contains("PlaywrightCli"));
        assert!(debug.contains("chromium"));
    }

    // ── Test result tests ───────────────────────────────────────────

    #[test]
    fn test_test_result_total() {
        let result = TestResult {
            success: true,
            passed: 5,
            failed: 2,
            skipped: 1,
            timed_out: 0,
            duration_ms: 1000,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        };
        assert_eq!(result.total(), 8);
    }

    // ── Action result tests ─────────────────────────────────────────

    #[test]
    fn test_action_result_output() {
        let result = ActionResult::output("hello");
        assert!(result.success);
        assert_eq!(result.output, Some("hello".to_string()));
        assert!(result.error.is_none());
    }

    // ── Screenshot result tests ─────────────────────────────────────

    #[test]
    fn test_screenshot_result() {
        let result = ScreenshotResult {
            path: PathBuf::from("screenshot.png"),
            size_bytes: 12345,
        };
        assert_eq!(result.path, PathBuf::from("screenshot.png"));
        assert_eq!(result.size_bytes, 12345);
    }

    // ── Skill instructions test ─────────────────────────────────────

    #[test]
    fn test_skill_instructions_not_empty() {
        let instructions = skill_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.contains("Playwright CLI Skill"));
        assert!(instructions.contains("Browser Automation"));
        assert!(instructions.contains("Page Testing"));
        assert!(instructions.contains("Multi-Browser Support"));
        assert!(instructions.contains("chromium"));
    }

    // ── Command output tests ────────────────────────────────────────

    #[test]
    fn test_command_output() {
        let output = CommandOutput {
            stdout: "hello".to_string(),
            stderr: String::new(),
            success: true,
            exit_code: 0,
        };
        assert!(output.success);
        assert_eq!(output.stdout, "hello");
        assert_eq!(output.exit_code, 0);
    }

    // ── Serde roundtrip tests ───────────────────────────────────────

    #[test]
    fn test_test_result_serde_roundtrip() {
        let result = TestResult {
            success: true,
            passed: 10,
            failed: 1,
            skipped: 2,
            timed_out: 0,
            duration_ms: 5000,
            stdout: "output".to_string(),
            stderr: String::new(),
            exit_code: 1,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: TestResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.passed, 10);
        assert_eq!(parsed.failed, 1);
        assert_eq!(parsed.skipped, 2);
        assert_eq!(parsed.duration_ms, 5000);
    }

    #[test]
    fn test_action_results_serde_roundtrip() {
        let results = ActionResults {
            success: true,
            actions_total: 3,
            results: vec![
                ActionResult {
                    action: "navigate".to_string(),
                    success: true,
                    output: None,
                    error: None,
                },
                ActionResult {
                    action: "click".to_string(),
                    success: true,
                    output: Some("clicked".to_string()),
                    error: None,
                },
            ],
            stdout: "out".to_string(),
            stderr: String::new(),
        };
        let json = serde_json::to_string(&results).unwrap();
        let parsed: ActionResults = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.actions_total, 3);
        assert_eq!(parsed.results.len(), 2);
    }

    #[test]
    fn test_screenshot_result_serde_roundtrip() {
        let result = ScreenshotResult {
            path: PathBuf::from("screenshots/home.png"),
            size_bytes: 54321,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ScreenshotResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.path, PathBuf::from("screenshots/home.png"));
        assert_eq!(parsed.size_bytes, 54321);
    }

    #[test]
    fn test_test_config_serde_roundtrip() {
        let config = TestConfig {
            test_paths: vec!["tests/".to_string()],
            project: Some("chromium".to_string()),
            repeat_each: 2,
            retries: 3,
            workers: 4,
            timeout_ms: 60_000,
            update_snapshots: true,
            grep: Some("login".to_string()),
            reporter: TestReporter::Json,
            output_dir: Some(PathBuf::from("test-results")),
            working_dir: Some(PathBuf::from("/tmp/project")),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: TestConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.test_paths, vec!["tests/".to_string()]);
        assert_eq!(parsed.project, Some("chromium".to_string()));
        assert_eq!(parsed.workers, 4);
        assert_eq!(parsed.reporter, TestReporter::Json);
        assert!(parsed.update_snapshots);
    }
}
