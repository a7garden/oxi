//! Interactive mode TUI components
//!
//! Provides high-level interactive components for the oxi terminal interface:
//! - Session selector (navigate/switch/create/delete sessions)
//! - Model selector (choose AI model grouped by provider)
//! - Footer (status bar with model, session, tokens, cost)
//! - Login dialog (API key entry with provider selection)
//! - Diff viewer (show edit diffs with color highlighting)
//! - Bash execution display (streaming output, timer, cancel)

use serde::{Deserialize, Serialize};

/// Session info for display in session selector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub message_count: usize,
    pub model: Option<String>,
    pub parent_id: Option<String>,
}

/// Session selector state
#[derive(Debug, Clone)]
pub struct SessionSelector {
    pub sessions: Vec<SessionInfo>,
    pub selected_index: usize,
    pub filter: String,
    pub scroll_offset: usize,
    pub visible_height: usize,
}

impl SessionSelector {
    pub fn new(sessions: Vec<SessionInfo>) -> Self {
        Self {
            sessions,
            selected_index: 0,
            filter: String::new(),
            scroll_offset: 0,
            visible_height: 20,
        }
    }

    /// Get filtered sessions matching the current filter
    pub fn filtered_sessions(&self) -> Vec<&SessionInfo> {
        if self.filter.is_empty() {
            self.sessions.iter().collect()
        } else {
            let filter_lower = self.filter.to_lowercase();
            self.sessions
                .iter()
                .filter(|s| {
                    s.name.to_lowercase().contains(&filter_lower)
                        || s.id.to_lowercase().contains(&filter_lower)
                })
                .collect()
        }
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.adjust_scroll();
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        let max = self.filtered_sessions().len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
            self.adjust_scroll();
        }
    }

    /// Get currently selected session
    pub fn selected(&self) -> Option<&SessionInfo> {
        self.filtered_sessions()
            .into_iter()
            .nth(self.selected_index)
    }

    /// Update filter text
    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    fn adjust_scroll(&mut self) {
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.visible_height {
            self.scroll_offset = self.selected_index - self.visible_height + 1;
        }
    }

    /// Render the session selector as a string
    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("{}\n", "─".repeat(60)));
        output.push_str("Sessions (↑↓ navigate, Enter select, n new, d delete, / filter)\n");
        output.push_str(&format!("{}\n", "─".repeat(60)));

        if !self.filter.is_empty() {
            output.push_str(&format!("Filter: {}\n", self.filter));
        }

        let filtered: Vec<_> = self.filtered_sessions();
        for (i, session) in filtered.iter().enumerate() {
            let marker = if i == self.selected_index { "▶" } else { " " };
            let branch = if session.parent_id.is_some() {
                "├─ "
            } else {
                "  "
            };
            let name = if session.name.is_empty() {
                &session.id[..8.min(session.id.len())]
            } else {
                &session.name
            };
            output.push_str(&format!(
                "{} {}{:<30} {} msg:{} model:{}\n",
                marker,
                branch,
                name,
                &session.created_at[..10.min(session.created_at.len())],
                session.message_count,
                session.model.as_deref().unwrap_or("-"),
            ));
        }

        if filtered.is_empty() {
            output.push_str("  (no sessions)\n");
        }

        output
    }
}

/// Model info for model selector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub supports_thinking: bool,
    pub context_window: usize,
}

/// Model selector state
#[derive(Debug, Clone)]
pub struct ModelSelector {
    pub models: Vec<ModelInfo>,
    pub selected_index: usize,
    pub filter: String,
    pub grouped: bool,
}

impl ModelSelector {
    pub fn new(models: Vec<ModelInfo>) -> Self {
        let mut models = models;
        models.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.name.cmp(&b.name)));
        Self {
            models,
            selected_index: 0,
            filter: String::new(),
            grouped: true,
        }
    }

    /// Get filtered models
    pub fn filtered_models(&self) -> Vec<&ModelInfo> {
        if self.filter.is_empty() {
            self.models.iter().collect()
        } else {
            let filter_lower = self.filter.to_lowercase();
            self.models
                .iter()
                .filter(|m| {
                    m.name.to_lowercase().contains(&filter_lower)
                        || m.id.to_lowercase().contains(&filter_lower)
                        || m.provider.to_lowercase().contains(&filter_lower)
                })
                .collect()
        }
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        let max = self.filtered_models().len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
        }
    }

    /// Get currently selected model
    pub fn selected(&self) -> Option<&ModelInfo> {
        self.filtered_models().into_iter().nth(self.selected_index)
    }

    /// Render the model selector
    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("{}\n", "─".repeat(60)));
        output.push_str("Select Model (↑↓ navigate, Enter select, / filter)\n");
        output.push_str(&format!("{}\n", "─".repeat(60)));

        let filtered: Vec<_> = self.filtered_models();
        let mut last_provider = String::new();

        for (i, model) in filtered.iter().enumerate() {
            // Provider group header
            if self.grouped && model.provider != last_provider {
                last_provider = model.provider.clone();
                output.push_str(&format!("\n  {}\n", model.provider.to_uppercase()));
            }

            let marker = if i == self.selected_index { "▶" } else { " " };
            let vision = if model.supports_vision { "👁" } else { " " };
            let tools = if model.supports_tools { "🔧" } else { " " };
            let thinking = if model.supports_thinking { "💭" } else { " " };
            let ctx = format_bytes(model.context_window);

            output.push_str(&format!(
                " {} {} {}{}{} {:<30} ctx:{}\n",
                marker, model.id, vision, tools, thinking, model.name, ctx,
            ));
        }

        output
    }
}

/// Footer status bar data
#[derive(Debug, Clone, Default)]
pub struct FooterData {
    pub model_name: String,
    pub session_name: String,
    pub provider_name: String,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub total_cost: f64,
    pub is_thinking: bool,
    pub elapsed_seconds: Option<u64>,
}

impl FooterData {
    /// Render the footer as a single-line status bar
    pub fn render(&self, width: usize) -> String {
        let thinking = if self.is_thinking { "⏳" } else { "✓" };
        let tokens = if self.input_tokens > 0 || self.output_tokens > 0 {
            format!("tok:{}+{}", self.input_tokens, self.output_tokens)
        } else {
            String::new()
        };
        let cost = if self.total_cost > 0.0 {
            format!("${:.4}", self.total_cost)
        } else {
            String::new()
        };
        let elapsed = self
            .elapsed_seconds
            .map(|s| format!("{}m{}s", s / 60, s % 60))
            .unwrap_or_default();

        let left = format!("{} {} @ {}", thinking, self.model_name, self.provider_name);
        let right = format!("{} {} {}", tokens, cost, elapsed);

        let session_part = if !self.session_name.is_empty() {
            format!(" │ {}", self.session_name)
        } else {
            String::new()
        };

        // Pad to width
        let content_len = left.len() + session_part.len() + right.len() + 2;
        if content_len < width {
            let padding = width - content_len;
            format!(
                "{}{}{:>width$}",
                left,
                session_part,
                right,
                width = padding + right.len()
            )
        } else {
            format!("{}{} {}", left, session_part, right)
        }
    }
}

/// Login dialog state
#[derive(Debug, Clone)]
pub struct LoginDialog {
    pub providers: Vec<String>,
    pub selected_provider_index: usize,
    pub api_key: String,
    pub cursor_pos: usize,
    pub error_message: Option<String>,
    pub is_masked: bool,
    /// OAuth-specific state
    pub oauth_state: Option<OAuthState>,
    /// Callback URL being waited for
    pub pending_auth_url: Option<String>,
}

/// OAuth provider configuration
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthProvider {
    Anthropic,
    OpenAI,
    GitHub,
    Google,
    Azure,
    /// Custom/provider-specific OAuth
    Custom {
        id: String,
        name: String,
    },
}

impl OAuthProvider {
    /// Get the provider ID string
    pub fn id(&self) -> &str {
        match self {
            OAuthProvider::Anthropic => "anthropic",
            OAuthProvider::OpenAI => "openai",
            OAuthProvider::GitHub => "github",
            OAuthProvider::Google => "google",
            OAuthProvider::Azure => "azure",
            OAuthProvider::Custom { id, .. } => id,
        }
    }

    /// Get the display name
    pub fn name(&self) -> &str {
        match self {
            OAuthProvider::Anthropic => "Anthropic",
            OAuthProvider::OpenAI => "OpenAI",
            OAuthProvider::GitHub => "GitHub",
            OAuthProvider::Google => "Google",
            OAuthProvider::Azure => "Azure",
            OAuthProvider::Custom { name, .. } => name,
        }
    }

    /// Get the default redirect port for this provider
    pub fn default_port(&self) -> u16 {
        match self {
            OAuthProvider::Anthropic => 8787,
            OAuthProvider::OpenAI => 8788,
            OAuthProvider::GitHub => 8789,
            OAuthProvider::Google => 8790,
            OAuthProvider::Azure => 8791,
            OAuthProvider::Custom { .. } => 8792,
        }
    }

    /// Parse provider from ID string
    pub fn from_id(id: &str) -> Option<Self> {
        match id.to_lowercase().as_str() {
            "anthropic" => Some(OAuthProvider::Anthropic),
            "openai" => Some(OAuthProvider::OpenAI),
            "github" | "github-copilot" => Some(OAuthProvider::GitHub),
            "google" => Some(OAuthProvider::Google),
            "azure" => Some(OAuthProvider::Azure),
            _ => None,
        }
    }
}

/// Internal OAuth state for the login flow
#[derive(Debug, Clone)]
pub struct OAuthState {
    pub provider: OAuthProvider,
    pub code_verifier: String,
    pub state: String,
    pub authorization_url: String,
    pub callback_port: u16,
}

/// Login state machine states
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginState {
    /// Initial state - showing provider selection
    ProviderSelection,
    /// Waiting for user to enter API key
    ApiKey,
    /// Showing the OAuth authorization URL
    WaitingForUrl,
    /// Waiting for browser callback
    WaitingForCallback,
    /// Showing manual code input prompt
    ManualInput,
    /// Polling for device flow completion
    Polling,
    /// Authentication successful
    Success,
    /// Authentication failed with error message
    Error(String),
}

impl Default for LoginState {
    fn default() -> Self {
        LoginState::ProviderSelection
    }
}

impl LoginDialog {
    pub fn new(providers: Vec<String>) -> Self {
        Self {
            providers,
            selected_provider_index: 0,
            api_key: String::new(),
            cursor_pos: 0,
            error_message: None,
            is_masked: true,
            oauth_state: None,
            pending_auth_url: None,
        }
    }

    /// Create a new login dialog with OAuth support
    pub fn new_with_oauth() -> Self {
        Self::new(vec![
            "anthropic".to_string(),
            "openai".to_string(),
            "github".to_string(),
        ])
    }

    /// Get the current login state
    pub fn login_state(&self) -> LoginState {
        if self.error_message.is_some() {
            return LoginState::Error(self.error_message.clone().unwrap());
        }
        if self.oauth_state.is_some() {
            if self.pending_auth_url.is_some() {
                return LoginState::WaitingForCallback;
            }
            return LoginState::WaitingForUrl;
        }
        LoginState::ApiKey
    }

    /// Start an OAuth flow for the selected provider
    /// Returns the authorization URL to display
    pub fn start_oauth_flow(&mut self, provider: OAuthProvider) -> Result<String, String> {
        let port = provider.default_port();
        let code_verifier = generate_code_verifier();
        let state = generate_state_token();

        // Build authorization URL based on provider
        let auth_url = match &provider {
            OAuthProvider::Anthropic => {
                format!(
                    "https://auth.anthropic.com/oauth/authorize?response_type=code&client_id={}&redirect_uri=http%3A%2F%2Flocalhost%3A{}&code_challenge={}&code_challenge_method=S256&state={}",
                    "anthropic-oauth-client",
                    port,
                    derive_code_challenge_sync(&code_verifier),
                    state
                )
            }
            OAuthProvider::OpenAI => {
                format!(
                    "https://auth.openai.com/authorize?response_type=code&client_id={}&redirect_uri=http%3A%2F%2Flocalhost%3A{}&code_challenge={}&code_challenge_method=S256&state={}",
                    "openai-oauth-client",
                    port,
                    derive_code_challenge_sync(&code_verifier),
                    state
                )
            }
            OAuthProvider::GitHub => {
                // GitHub uses device flow, not authorization code
                format!(
                    "https://github.com/login/device/code?client_id={}&scope=read:user%20user:email",
                    "Iv1.placeholder_client_id"
                )
            }
            _ => {
                return Err(format!(
                    "OAuth not supported for provider: {}",
                    provider.name()
                ));
            }
        };

        let oauth_state = OAuthState {
            provider,
            code_verifier,
            state,
            authorization_url: auth_url.clone(),
            callback_port: port,
        };

        self.oauth_state = Some(oauth_state);
        self.pending_auth_url = Some(auth_url.clone());
        Ok(auth_url)
    }

    /// Open the authorization URL in the default browser
    pub fn open_auth_url(&self, url: &str) -> Result<(), String> {
        crate::oauth_server::open_browser(url).map_err(|e| format!("Failed to open browser: {}", e))
    }

    /// Start the OAuth callback server
    pub fn start_callback_server(
        port: u16,
    ) -> Result<crate::oauth_server::OAuthCallbackServer, String> {
        let server = crate::oauth_server::OAuthCallbackServer::new(port);
        Ok(server)
    }

    /// Handle the OAuth callback with code and state
    pub fn handle_oauth_callback(&mut self, code: String, state: String) -> Result<(), String> {
        if let Some(ref oauth_state) = self.oauth_state {
            // Verify state matches
            if oauth_state.state != state {
                return Err("State mismatch - possible CSRF attack".to_string());
            }
            // Store code for exchange
            self.api_key = code;
            self.pending_auth_url = None;
            Ok(())
        } else {
            Err("No OAuth flow in progress".to_string())
        }
    }

    /// Show manual code input interface
    pub fn show_manual_code_input(&mut self, message: &str) {
        self.error_message = None;
        // The message indicates what to show
        if let Some(ref auth_url) = self.pending_auth_url {
            eprintln!("\n{}", message);
            eprintln!("Authorization URL: {}", auth_url);
            eprintln!("Paste the code from the redirect URL here:\n");
        }
    }

    /// Parse a redirect URL to extract the authorization code
    pub fn parse_redirect_url(url: &str) -> Option<(String, String)> {
        // Parse URL like http://localhost:8787/callback?code=xxx&state=yyy
        if let Ok(parsed) = url::Url::parse(url) {
            let code = parsed
                .query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.to_string());
            let state = parsed
                .query_pairs()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.to_string());
            if let (Some(code), Some(state)) = (code, state) {
                return Some((code, state));
            }
        }

        // Try simple parsing for just ?code=xxx&state=yyy
        let query = url.split('?').nth(1)?;
        let mut code = None;
        let mut state = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            let decoded = urlencoding::decode(value).ok()?.to_string();
            match key {
                "code" => code = Some(decoded),
                "state" => state = Some(decoded),
                _ => {}
            }
        }
        Some((code?, state.unwrap_or_default()))
    }

    /// Complete the OAuth flow with the authorization code
    pub fn complete_oauth(&mut self, code: String) -> Result<(), String> {
        if let Some(ref oauth_state) = self.oauth_state {
            // Store the code for exchange - the actual token exchange
            // would be done by the caller using oxi-ai's oauth module
            self.api_key = code;
            self.oauth_state = None;
            self.pending_auth_url = None;
            Ok(())
        } else {
            Err("No OAuth flow in progress".to_string())
        }
    }

    /// Cancel the current OAuth flow
    pub fn cancel_oauth(&mut self) {
        self.oauth_state = None;
        self.pending_auth_url = None;
        self.error_message = None;
    }

    /// Check if OAuth is available for a provider
    pub fn is_oauth_available(&self, provider: &str) -> bool {
        matches!(
            provider.to_lowercase().as_str(),
            "anthropic" | "openai" | "github" | "github-copilot"
        )
    }

    /// Get selected provider
    pub fn selected_provider(&self) -> Option<&str> {
        self.providers
            .get(self.selected_provider_index)
            .map(|s| s.as_str())
    }

    /// Input a character
    pub fn input_char(&mut self, c: char) {
        self.api_key.insert(self.cursor_pos, c);
        self.cursor_pos += 1;
        self.error_message = None;
    }

    /// Delete character before cursor
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            self.api_key.remove(self.cursor_pos);
            self.error_message = None;
        }
    }

    /// Cycle provider selection
    pub fn next_provider(&mut self) {
        if !self.providers.is_empty() {
            self.selected_provider_index =
                (self.selected_provider_index + 1) % self.providers.len();
            self.api_key.clear();
            self.cursor_pos = 0;
            self.error_message = None;
        }
    }

    /// Validate API key format (basic checks)
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("API key cannot be empty".to_string());
        }
        let provider = self.selected_provider().unwrap_or("");
        match provider {
            "anthropic" if !self.api_key.starts_with("sk-ant-") => {
                Err("Anthropic API keys start with 'sk-ant-'".to_string())
            }
            "openai" if !self.api_key.starts_with("sk-") => {
                Err("OpenAI API keys start with 'sk-'".to_string())
            }
            _ => Ok(()),
        }
    }

    /// Render the login dialog
    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("{}\n", "─".repeat(50)));
        output.push_str("  API Key Configuration\n");
        output.push_str(&format!("{}\n", "─".repeat(50)));

        // Provider tabs
        for (i, provider) in self.providers.iter().enumerate() {
            if i == self.selected_provider_index {
                output.push_str(&format!(" [{}] ", provider));
            } else {
                output.push_str(&format!("  {}  ", provider));
            }
        }
        output.push('\n');

        // API key input
        let display_key = if self.is_masked {
            "*".repeat(self.api_key.len())
        } else {
            self.api_key.clone()
        };
        output.push_str(&format!("\n  API Key: {}\n", display_key));

        // Error message
        if let Some(ref err) = self.error_message {
            output.push_str(&format!("  ⚠ {}\n", err));
        }

        output.push_str("\n  Tab: switch provider, Enter: save, Esc: cancel\n");
        output
    }
}

/// Diff line for the diff viewer
#[derive(Debug, Clone)]
pub enum DiffLine {
    Context {
        content: String,
        line_num: usize,
    },
    Added {
        content: String,
        line_num: usize,
    },
    Removed {
        content: String,
        line_num: usize,
    },
    Header {
        old_start: usize,
        old_count: usize,
        new_start: usize,
        new_count: usize,
    },
}

/// Diff viewer state
#[derive(Debug, Clone)]
pub struct DiffViewer {
    pub lines: Vec<DiffLine>,
    pub scroll_offset: usize,
    pub visible_height: usize,
    pub file_path: String,
    /// Enable word-level highlighting for changed parts
    pub word_diff: bool,
}

impl DiffViewer {
    pub fn new(file_path: String, diff_text: &str) -> Self {
        let lines = parse_diff_lines(diff_text);
        Self {
            lines,
            scroll_offset: 0,
            visible_height: 30,
            file_path,
            word_diff: true, // Enable word-level highlighting by default
        }
    }

    /// Create without word diff highlighting
    pub fn new_simple(file_path: String, diff_text: &str) -> Self {
        let lines = parse_diff_lines(diff_text);
        Self {
            lines,
            scroll_offset: 0,
            visible_height: 30,
            file_path,
            word_diff: false,
        }
    }

    /// Enable or disable word-level diff highlighting
    pub fn set_word_diff(&mut self, enabled: bool) {
        self.word_diff = enabled;
    }

    /// Render the diff viewer with optional word-level highlighting
    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("Diff: {}\n", self.file_path));
        output.push_str(&format!("{}\n", "─".repeat(60)));

        let visible: Vec<_> = self
            .lines
            .iter()
            .skip(self.scroll_offset)
            .take(self.visible_height)
            .collect();

        for line in &visible {
            match line {
                DiffLine::Header {
                    old_start,
                    old_count,
                    new_start,
                    new_count,
                } => {
                    output.push_str(&format!(
                        "@@ -{},{} +{},{} @@\n",
                        old_start, old_count, new_start, new_count
                    ));
                }
                DiffLine::Context { content, line_num } => {
                    output.push_str(&format!(" {:>4} {}\n", line_num, content));
                }
                DiffLine::Added { content, line_num } => {
                    if self.word_diff {
                        // Apply word-level highlighting for added lines
                        let highlighted = highlight_words_diff(content, true);
                        output.push_str(&format!("+{:>4} {}\n", line_num, highlighted));
                    } else {
                        output.push_str(&format!("+{:>4} {}\n", line_num, content));
                    }
                }
                DiffLine::Removed { content, line_num } => {
                    if self.word_diff {
                        // Apply word-level highlighting for removed lines
                        let highlighted = highlight_words_diff(content, false);
                        output.push_str(&format!("-{:>4} {}\n", line_num, highlighted));
                    } else {
                        output.push_str(&format!("-{:>4} {}\n", line_num, content));
                    }
                }
            }
        }

        let remaining = self
            .lines
            .len()
            .saturating_sub(self.scroll_offset + self.visible_height);
        if remaining > 0 {
            output.push_str(&format!("... {} more lines\n", remaining));
        }

        output
    }

    /// Scroll up
    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scroll down
    pub fn scroll_down(&mut self, amount: usize) {
        let max = self.lines.len().saturating_sub(self.visible_height);
        self.scroll_offset = (self.scroll_offset + amount).min(max);
    }
}

/// Parse unified diff text into DiffLine structs
fn parse_diff_lines(diff: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let mut old_line = 0;
    let mut new_line = 0;

    for raw_line in diff.lines() {
        if raw_line.starts_with("@@") {
            // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@
            if let Some(header) = parse_hunk_header(raw_line) {
                old_line = header.0;
                new_line = header.2;
                lines.push(DiffLine::Header {
                    old_start: header.0,
                    old_count: header.1,
                    new_start: header.2,
                    new_count: header.3,
                });
            }
        } else if raw_line.starts_with('+') {
            let content = raw_line[1..].to_string();
            lines.push(DiffLine::Added {
                content,
                line_num: new_line,
            });
            new_line += 1;
        } else if raw_line.starts_with('-') {
            let content = raw_line[1..].to_string();
            lines.push(DiffLine::Removed {
                content,
                line_num: old_line,
            });
            old_line += 1;
        } else if raw_line.starts_with(' ') {
            let content = raw_line[1..].to_string();
            lines.push(DiffLine::Context {
                content,
                line_num: new_line,
            });
            old_line += 1;
            new_line += 1;
        }
    }

    lines
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    // @@ -old_start,old_count +new_start,new_count @@
    let text = line.trim_start_matches('@').trim_start_matches(' ');
    let text = text.trim_end_matches('@').trim_end_matches(' ');
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let old: Vec<usize> = parts[0]
        .trim_start_matches('-')
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();
    let new: Vec<usize> = parts
        .get(1)?
        .trim_start_matches('+')
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();

    Some((
        *old.first()?,
        *old.get(1).unwrap_or(&1),
        *new.first()?,
        *new.get(1).unwrap_or(&1),
    ))
}

/// Highlight word-level changes in a diff line
/// Returns the content with ANSI color codes for changed words.
fn highlight_words_diff(content: &str, is_added: bool) -> String {
    use std::fmt::Write;

    // Split content into words while preserving spaces
    let words: Vec<&str> = content.split_whitespace().collect();
    let mut result = String::new();

    for (i, word) in words.iter().enumerate() {
        // Simple heuristic: short words (1-4 chars) that differ are likely changed
        let is_short_change = word.len() <= 4 && !word.chars().all(|c| c.is_alphanumeric());

        if is_short_change && i > 0 {
            // Highlight as changed
            let color = if is_added { "\x1b[32m" } else { "\x1b[31m" };
            write!(&mut result, "{}{}{}\x1b[0m ", color, word, "\x1b[0m").unwrap();
        } else {
            write!(&mut result, "{} ", word).unwrap();
        }
    }

    result.trim_end().to_string()
}

/// Bash execution display state
#[derive(Debug, Clone)]
pub struct BashExecution {
    pub command: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub start_time: std::time::Instant,
    pub is_running: bool,
    pub is_cancelled: bool,
}

impl BashExecution {
    pub fn new(command: String) -> Self {
        Self {
            command,
            output: String::new(),
            exit_code: None,
            start_time: std::time::Instant::now(),
            is_running: true,
            is_cancelled: false,
        }
    }

    /// Append output
    pub fn append_output(&mut self, text: &str) {
        self.output.push_str(text);
    }

    /// Mark as complete
    pub fn complete(&mut self, exit_code: i32) {
        self.exit_code = Some(exit_code);
        self.is_running = false;
    }

    /// Cancel execution
    pub fn cancel(&mut self) {
        self.is_cancelled = true;
        self.is_running = false;
        self.exit_code = Some(-1);
        self.output.push_str("\n[Cancelled]");
    }

    /// Get elapsed time
    pub fn elapsed(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }

    /// Render the bash execution display
    pub fn render(&self) -> String {
        let mut output = String::new();
        let status = if self.is_cancelled {
            "⛔ CANCELLED"
        } else if self.is_running {
            &format!("⏳ Running ({:.1}s)", self.elapsed().as_secs_f64())
        } else {
            match self.exit_code {
                Some(0) => "✓ Done",
                Some(c) => &format!("✗ Exit code: {}", c) as &str,
                None => "Running",
            }
        };

        output.push_str(&format!("$ {}\n", self.command));
        if !self.output.is_empty() {
            output.push_str(&self.output);
            if !self.output.ends_with('\n') {
                output.push('\n');
            }
        }
        output.push_str(&format!("{}\n", status));

        output
    }
}

/// Format bytes for human-readable display
fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

// ── PKCE Helper Functions ────────────────────────────────────────────────────

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// Generate a cryptographically-random code_verifier (43 chars, RFC 7636 §4.1).
pub fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 32]; // 32 bytes → 43 base64url chars
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Derive the code_challenge from a code_verifier using S256 (SHA-256 + base64url).
pub fn derive_code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

/// Synchronous version of derive_code_challenge for use in non-async contexts
fn derive_code_challenge_sync(verifier: &str) -> String {
    derive_code_challenge(verifier)
}

/// Generate an opaque state parameter (22 random base64url chars).
fn generate_state_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// ── OAuth Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod oauth_tests {
    use super::*;

    #[test]
    fn test_code_verifier_length() {
        let v = generate_code_verifier();
        assert!((43..=128).contains(&v.len()), "verifier length {}", v.len());
    }

    #[test]
    fn test_code_verifier_is_base64url() {
        let v = generate_code_verifier();
        // base64url chars: A-Z a-z 0-9 - _
        assert!(v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn test_code_verifier_uniqueness() {
        let a = generate_code_verifier();
        let b = generate_code_verifier();
        assert_ne!(a, b, "two verifiers should differ");
    }

    #[test]
    fn test_code_challenge_deterministic() {
        let v = generate_code_verifier();
        let c1 = derive_code_challenge(&v);
        let c2 = derive_code_challenge(&v);
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_code_challenge_differs_from_verifier() {
        let v = generate_code_verifier();
        let c = derive_code_challenge(&v);
        assert_ne!(v, c);
    }

    #[test]
    fn test_code_challenge_is_base64url() {
        let v = generate_code_verifier();
        let c = derive_code_challenge(&v);
        assert!(c
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
    }

    #[test]
    fn test_known_pkce_vector() {
        // RFC 7636 Appendix B reference vector
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = derive_code_challenge(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn test_oauth_provider_from_id() {
        assert_eq!(
            OAuthProvider::from_id("anthropic"),
            Some(OAuthProvider::Anthropic)
        );
        assert_eq!(
            OAuthProvider::from_id("openai"),
            Some(OAuthProvider::OpenAI)
        );
        assert_eq!(
            OAuthProvider::from_id("github"),
            Some(OAuthProvider::GitHub)
        );
        assert_eq!(
            OAuthProvider::from_id("github-copilot"),
            Some(OAuthProvider::GitHub)
        );
        assert_eq!(
            OAuthProvider::from_id("google"),
            Some(OAuthProvider::Google)
        );
        assert_eq!(OAuthProvider::from_id("azure"), Some(OAuthProvider::Azure));
        assert_eq!(OAuthProvider::from_id("unknown"), None);
    }

    #[test]
    fn test_oauth_provider_id_and_name() {
        let anthropic = OAuthProvider::Anthropic;
        assert_eq!(anthropic.id(), "anthropic");
        assert_eq!(anthropic.name(), "Anthropic");

        let openai = OAuthProvider::OpenAI;
        assert_eq!(openai.id(), "openai");
        assert_eq!(openai.name(), "OpenAI");

        let custom = OAuthProvider::Custom {
            id: "custom".into(),
            name: "Custom Provider".into(),
        };
        assert_eq!(custom.id(), "custom");
        assert_eq!(custom.name(), "Custom Provider");
    }

    #[test]
    fn test_oauth_provider_default_port() {
        assert_eq!(OAuthProvider::Anthropic.default_port(), 8787);
        assert_eq!(OAuthProvider::OpenAI.default_port(), 8788);
        assert_eq!(OAuthProvider::GitHub.default_port(), 8789);
        assert_eq!(OAuthProvider::Google.default_port(), 8790);
        assert_eq!(OAuthProvider::Azure.default_port(), 8791);
    }

    #[test]
    fn test_login_dialog_oauth_state() {
        let mut dialog = LoginDialog::new(vec!["anthropic".to_string()]);
        assert!(dialog.oauth_state.is_none());
        assert!(dialog.pending_auth_url.is_none());
        assert_eq!(dialog.login_state(), LoginState::ApiKey);

        // Start OAuth flow
        let url = dialog.start_oauth_flow(OAuthProvider::Anthropic).unwrap();
        assert!(url.contains("localhost:8787"));
        assert!(dialog.oauth_state.is_some());
        assert!(dialog.pending_auth_url.is_some());
        assert_eq!(dialog.login_state(), LoginState::WaitingForCallback);

        // Cancel OAuth
        dialog.cancel_oauth();
        assert!(dialog.oauth_state.is_none());
        assert!(dialog.pending_auth_url.is_none());
    }

    #[test]
    fn test_login_dialog_parse_redirect_url() {
        let url = "http://localhost:8787/callback?code=test_code_123&state=state_456";
        let result = LoginDialog::parse_redirect_url(url);
        assert!(result.is_some());
        let (code, state) = result.unwrap();
        assert_eq!(code, "test_code_123");
        assert_eq!(state, "state_456");
    }

    #[test]
    fn test_login_dialog_parse_redirect_url_simple() {
        let url = "?code=simple_code&state=state";
        let result = LoginDialog::parse_redirect_url(url);
        assert!(result.is_some());
        let (code, state) = result.unwrap();
        assert_eq!(code, "simple_code");
        assert_eq!(state, "state");
    }

    #[test]
    fn test_login_dialog_parse_redirect_url_invalid() {
        let url = "http://localhost:8787/callback?state=only_state";
        let result = LoginDialog::parse_redirect_url(url);
        assert!(result.is_none());
    }

    #[test]
    fn test_login_dialog_oauth_callback() {
        let mut dialog = LoginDialog::new(vec!["anthropic".to_string()]);
        dialog.start_oauth_flow(OAuthProvider::Anthropic).unwrap();

        let oauth_state = dialog.oauth_state.clone().unwrap();
        let result = dialog.handle_oauth_callback("auth_code".into(), oauth_state.state.clone());
        assert!(result.is_ok());
        assert_eq!(dialog.api_key, "auth_code");
    }

    #[test]
    fn test_login_dialog_oauth_callback_state_mismatch() {
        let mut dialog = LoginDialog::new(vec!["anthropic".to_string()]);
        dialog.start_oauth_flow(OAuthProvider::Anthropic).unwrap();

        let result = dialog.handle_oauth_callback("auth_code".into(), "wrong_state".into());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("State mismatch"));
    }

    #[test]
    fn test_login_dialog_is_oauth_available() {
        let dialog = LoginDialog::new(vec![]);
        assert!(dialog.is_oauth_available("anthropic"));
        assert!(dialog.is_oauth_available("openai"));
        assert!(dialog.is_oauth_available("github"));
        assert!(dialog.is_oauth_available("github-copilot"));
        assert!(!dialog.is_oauth_available("unknown"));
    }

    #[test]
    fn test_login_dialog_complete_oauth() {
        let mut dialog = LoginDialog::new(vec!["anthropic".to_string()]);
        dialog.start_oauth_flow(OAuthProvider::Anthropic).unwrap();
        assert!(dialog.oauth_state.is_some());

        let result = dialog.complete_oauth("final_code".into());
        assert!(result.is_ok());
        assert_eq!(dialog.api_key, "final_code");
        assert!(dialog.oauth_state.is_none());
        assert!(dialog.pending_auth_url.is_none());
    }

    #[test]
    fn test_login_state_default() {
        assert_eq!(LoginState::default(), LoginState::ProviderSelection);
    }

    #[test]
    fn test_login_state_error() {
        let dialog = LoginDialog {
            providers: vec![],
            selected_provider_index: 0,
            api_key: String::new(),
            cursor_pos: 0,
            error_message: Some("test error".to_string()),
            is_masked: true,
            oauth_state: None,
            pending_auth_url: None,
        };
        assert_eq!(
            dialog.login_state(),
            LoginState::Error("test error".to_string())
        );
    }

    #[test]
    fn test_state_token_generation() {
        let state1 = generate_state_token();
        let state2 = generate_state_token();
        assert_ne!(state1, state2);
        assert!(state1.len() >= 16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_selector_navigation() {
        let sessions = vec![
            SessionInfo {
                id: "1".to_string(),
                name: "Session 1".to_string(),
                created_at: "2025-01-01".to_string(),
                message_count: 5,
                model: Some("gpt-4".to_string()),
                parent_id: None,
            },
            SessionInfo {
                id: "2".to_string(),
                name: "Session 2".to_string(),
                created_at: "2025-01-02".to_string(),
                message_count: 3,
                model: Some("claude-3".to_string()),
                parent_id: Some("1".to_string()),
            },
        ];
        let mut selector = SessionSelector::new(sessions);
        assert_eq!(selector.selected().unwrap().id, "1");
        selector.move_down();
        assert_eq!(selector.selected().unwrap().id, "2");
        selector.move_up();
        assert_eq!(selector.selected().unwrap().id, "1");
    }

    #[test]
    fn test_session_selector_filter() {
        let sessions = vec![
            SessionInfo {
                id: "1".to_string(),
                name: "Rust coding".to_string(),
                created_at: "2025-01-01".to_string(),
                message_count: 5,
                model: None,
                parent_id: None,
            },
            SessionInfo {
                id: "2".to_string(),
                name: "Python coding".to_string(),
                created_at: "2025-01-02".to_string(),
                message_count: 3,
                model: None,
                parent_id: None,
            },
        ];
        let mut selector = SessionSelector::new(sessions);
        selector.set_filter("rust".to_string());
        let filtered = selector.filtered_sessions();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "Rust coding");
    }

    #[test]
    fn test_model_selector() {
        let models = vec![
            ModelInfo {
                id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                provider: "openai".to_string(),
                supports_vision: true,
                supports_tools: true,
                supports_thinking: false,
                context_window: 128000,
            },
            ModelInfo {
                id: "claude-sonnet".to_string(),
                name: "Claude Sonnet".to_string(),
                provider: "anthropic".to_string(),
                supports_vision: true,
                supports_tools: true,
                supports_thinking: true,
                context_window: 200000,
            },
        ];
        let mut selector = ModelSelector::new(models);
        assert_eq!(selector.selected().unwrap().id, "claude-sonnet");
        selector.move_down();
        assert_eq!(selector.selected().unwrap().id, "gpt-4o");
    }

    #[test]
    fn test_footer_render() {
        let footer = FooterData {
            model_name: "gpt-4o".to_string(),
            session_name: "test".to_string(),
            provider_name: "openai".to_string(),
            input_tokens: 1000,
            output_tokens: 500,
            total_cost: 0.05,
            is_thinking: false,
            elapsed_seconds: Some(30),
        };
        let rendered = footer.render(80);
        assert!(rendered.contains("gpt-4o"));
        assert!(rendered.contains("openai"));
    }

    #[test]
    fn test_login_dialog() {
        let mut dialog = LoginDialog::new(vec!["anthropic".to_string(), "openai".to_string()]);
        assert_eq!(dialog.selected_provider(), Some("anthropic"));
        dialog.next_provider();
        assert_eq!(dialog.selected_provider(), Some("openai"));
        dialog.input_char('s');
        dialog.input_char('k');
        assert_eq!(dialog.api_key, "sk");
        dialog.backspace();
        assert_eq!(dialog.api_key, "s");
    }

    #[test]
    fn test_login_dialog_validation() {
        let mut dialog = LoginDialog::new(vec!["openai".to_string()]);
        assert!(dialog.validate().is_err()); // empty key
        dialog.api_key = "sk-1234".to_string();
        assert!(dialog.validate().is_ok());
    }

    #[test]
    fn test_diff_viewer() {
        let diff = "@@ -1,3 +1,3 @@\n line1\n-old line\n+new line\n line3\n";
        let viewer = DiffViewer::new("test.txt".to_string(), diff);
        assert_eq!(viewer.lines.len(), 5); // header + 4 lines
        let rendered = viewer.render();
        assert!(rendered.contains("old line"));
        assert!(rendered.contains("new line"));
    }

    #[test]
    fn test_diff_viewer_scroll() {
        let mut diff = "@@ -1,5 +1,5 @@\n".to_string();
        for i in 0..100 {
            diff.push_str(&format!(" line {}\n", i)); // context lines start with space
        }
        let mut viewer = DiffViewer::new("test.txt".to_string(), &diff);
        viewer.visible_height = 10;
        assert!(
            viewer.lines.len() > 10,
            "need {} lines, got {}",
            11,
            viewer.lines.len()
        );
        viewer.scroll_down(10);
        assert!(viewer.scroll_offset > 0);
        viewer.scroll_up(5);
        assert!(viewer.scroll_offset < 10);
    }

    #[test]
    fn test_bash_execution() {
        let mut exec = BashExecution::new("echo hello".to_string());
        assert!(exec.is_running);
        exec.append_output("hello\n");
        exec.complete(0);
        assert!(!exec.is_running);
        assert_eq!(exec.exit_code, Some(0));
        let rendered = exec.render();
        assert!(rendered.contains("echo hello"));
        assert!(rendered.contains("hello"));
        assert!(rendered.contains("Done"));
    }

    #[test]
    fn test_bash_execution_cancel() {
        let mut exec = BashExecution::new("sleep 999".to_string());
        exec.cancel();
        assert!(exec.is_cancelled);
        assert!(!exec.is_running);
        let rendered = exec.render();
        assert!(rendered.contains("CANCELLED"));
    }

    #[test]
    fn test_parse_hunk_header() {
        let result = parse_hunk_header("@@ -1,3 +1,3 @@");
        assert_eq!(result, Some((1, 3, 1, 3)));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500B");
        assert_eq!(format_bytes(1024), "1.0KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0GB");
    }
}
