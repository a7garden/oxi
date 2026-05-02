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

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            test_paths: vec![".".to_string()],
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
/// use oxi::skills::playwright_cli::{PlaywrightCli, PlaywrightConfig, BrowserAction};
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
///         "navigate('https://example.com');",
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

    /// Create with default configuration.
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

    /// Install a specific browser (or all browsers).
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

    /// Run a JavaScript/TypeScript snippet using `playwright test --project`
    /// in single-file mode.
    ///
    /// The script should use Playwright's test API. Example:
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
        let script = self.generate_action_script(url, actions);
        let output = self.run_script(&script).await?;

        let results = if output.success {
            // Try to parse the stdout as JSON results
            match serde_json::from_str::<ActionResults>(&output.stdout) {
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
            serde_json::from_str(&output.stdout.trim()).unwrap_or_else(|_| {
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

        let result: serde_json::Value = serde_json::from_str(&output.stdout.trim())
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
    fn generate_action_script(&self, url: &str, actions: &[BrowserAction]) -> String {
        let mut script = String::with_capacity(4096);

        script.push_str("const { chromium } = require('playwright');\n");
        script.push_str("(async () => {\n");
        script.push_str(&format!(
            "  const browser = await chromium.launch({{ headless: {} }});\n",
            self.config.headless
        ));
        script.push_str("  const page = await browser.newPage();\n");
        script.push_str(&format!(
            "  const results = [];\n\n  try {{\n"
        ));
        script.push_str(&format!(
            "    await page.goto('{}', {{ waitUntil: 'networkidle', timeout: {} }});\n",
            url, self.config.timeout_ms
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