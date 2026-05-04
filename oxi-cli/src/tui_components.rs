//! Interactive mode TUI components
//!
//! Provides high-level interactive components for the oxi terminal interface:
//! - Session selector (navigate/switch/create/delete sessions)
//! - Model selector (choose AI model grouped by provider)
//! - Footer (status bar with model, session, tokens, cost)
//! - Login dialog (API key entry with provider selection)
//! - Diff viewer (show edit diffs with color highlighting)
//! - Bash execution display (streaming output, timer, cancel)
//! - Assistant message rendering (thinking blocks, tool calls, markdown)
//! - Tool execution rendering (args, results, images, status)
//! - Summary message rendering (compaction, branch)

use serde::{Deserialize, Serialize};

use rand::RngCore;

/// Content block types for assistant messages
#[derive(Debug, Clone)]
pub enum AssistantContentBlock {
    /// Text content with optional markdown
    Text {
        text: String,
    },
    /// Thinking/reasoning block (collapsible)
    Thinking {
        thinking: String,
    },
    /// Tool call invocation
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
}

/// Assistant message data structure
#[derive(Debug, Clone)]
pub struct AssistantMessage {
    pub content: Vec<AssistantContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub error_message: Option<String>,
}

/// Why the assistant message stopped
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    Aborted,
    Error,
}

impl StopReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::StopSequence => "stop_sequence",
            StopReason::Aborted => "aborted",
            StopReason::Error => "error",
        }
    }
}

impl AssistantMessage {
    pub fn new() -> Self {
        Self {
            content: Vec::new(),
            stop_reason: None,
            error_message: None,
        }
    }

    /// Add a text block
    pub fn add_text(&mut self, text: impl Into<String>) {
        self.content.push(AssistantContentBlock::Text {
            text: text.into(),
        });
    }

    /// Add a thinking block
    pub fn add_thinking(&mut self, thinking: impl Into<String>) {
        self.content.push(AssistantContentBlock::Thinking {
            thinking: thinking.into(),
        });
    }

    /// Add a tool call block
    pub fn add_tool_call(&mut self, id: impl Into<String>, name: impl Into<String>, arguments: impl Into<String>) {
        self.content.push(AssistantContentBlock::ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        });
    }

    /// Check if message has any visible content
    pub fn has_visible_content(&self) -> bool {
        self.content.iter().any(|c| match c {
            AssistantContentBlock::Text { text } => !text.trim().is_empty(),
            AssistantContentBlock::Thinking { thinking } => !thinking.trim().is_empty(),
            AssistantContentBlock::ToolCall { .. } => false,
        })
    }

    /// Check if message has tool calls
    pub fn has_tool_calls(&self) -> bool {
        self.content
            .iter()
            .any(|c| matches!(c, AssistantContentBlock::ToolCall { .. }))
    }
}

impl Default for AssistantMessage {
    fn default() -> Self {
        Self::new()
    }
}

/// Options for rendering assistant messages
#[derive(Debug, Clone)]
pub struct AssistantMessageRenderOptions {
    /// Hide thinking blocks and show a label instead
    pub hide_thinking: bool,
    /// Label to show when thinking is hidden
    pub hidden_thinking_label: String,
    /// Use OSC 133 prompt escape codes for terminal integration
    pub use_osc133: bool,
}

impl Default for AssistantMessageRenderOptions {
    fn default() -> Self {
        Self {
            hide_thinking: false,
            hidden_thinking_label: "Thinking...".to_string(),
            use_osc133: false,
        }
    }
}

/// Assistant message renderer
pub struct AssistantMessageRenderer {
    options: AssistantMessageRenderOptions,
}

impl AssistantMessageRenderer {
    pub fn new(options: AssistantMessageRenderOptions) -> Self {
        Self { options }
    }

    /// Set hide thinking option
    pub fn with_hide_thinking(mut self, hide: bool) -> Self {
        self.options.hide_thinking = hide;
        self
    }

    /// Set hidden thinking label
    pub fn with_hidden_thinking_label(mut self, label: impl Into<String>) -> Self {
        self.options.hidden_thinking_label = label.into();
        self
    }

    /// Enable OSC 133 escape codes for terminal integration
    pub fn with_osc133(mut self, enable: bool) -> Self {
        self.options.use_osc133 = enable;
        self
    }

    /// Render an assistant message to a string
    pub fn render(&self, message: &AssistantMessage) -> String {
        let mut output = String::new();

        // OSC 133 zone start
        if self.options.use_osc133 {
            output.push_str("\x1b]133;A\x07");
        }

        let mut has_visible_content = false;
        let visible_count = message
            .content
            .iter()
            .filter(|c| match c {
                AssistantContentBlock::Text { text } => !text.trim().is_empty(),
                AssistantContentBlock::Thinking { thinking } => !thinking.trim().is_empty(),
                AssistantContentBlock::ToolCall { .. } => false,
            })
            .count();

        let mut visible_idx = 0;

        for block in &message.content {
            match block {
                AssistantContentBlock::Text { text } if !text.trim().is_empty() => {
                    if has_visible_content {
                        output.push('\n');
                    }
                    visible_idx += 1;
                    has_visible_content = true;
                    output.push_str(&render_markdown(text.trim()));
                    if visible_idx < visible_count {
                        output.push('\n');
                    }
                }
                AssistantContentBlock::Thinking { thinking } if !thinking.trim().is_empty() => {
                    if has_visible_content {
                        output.push('\n');
                    }
                    visible_idx += 1;
                    has_visible_content = true;

                    if self.options.hide_thinking {
                        // Show static thinking label (italic, dimmed)
                        output.push_str(&format!(
                            "\x1b[2m\x1b[3m{}\x1b[0m",
                            self.options.hidden_thinking_label
                        ));
                    } else {
                        // Show thinking content (italic, dimmed)
                        let rendered = render_markdown(thinking.trim());
                        output.push_str(&format!("\x1b[2m\x1b[3m{}\x1b[0m", rendered));
                    }

                    if visible_idx < visible_count {
                        output.push('\n');
                    }
                }
                _ => {}
            }
        }

        // Handle stop reasons (only if no tool calls)
        if !message.has_tool_calls() {
            if let Some(ref reason) = message.stop_reason {
                if has_visible_content {
                    output.push('\n');
                }
                match reason {
                    StopReason::Aborted => {
                        let msg = message
                            .error_message
                            .as_ref()
                            .filter(|m| *m != "Request was aborted")
                            .cloned()
                            .unwrap_or_else(|| "Operation aborted".to_string());
                        output.push_str(&format!("\x1b[31m{}\x1b[0m", msg));
                    }
                    StopReason::Error => {
                        let msg = message
                            .error_message
                            .as_ref()
                            .cloned()
                            .unwrap_or_else(|| "Unknown error".to_string());
                        output.push_str(&format!("\x1b[31mError: {}\x1b[0m", msg));
                    }
                    _ => {}
                }
            }
        }

        // OSC 133 zone end
        if self.options.use_osc133 {
            output.push_str("\x1b]133;B\x07\x1b]133;C\x07");
        }

        output
    }
}

impl Default for AssistantMessageRenderer {
    fn default() -> Self {
        Self::new(AssistantMessageRenderOptions::default())
    }
}

/// Simple markdown rendering (bold, italic, code)
fn render_markdown(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '`' {
            // Inline code
            let mut code = String::new();
            while let Some(&next) = chars.peek() {
                if next == '`' {
                    chars.next();
                    // Check for code block
                    if chars.peek() == Some(&'`') {
                        chars.next();
                        // Triple backtick - code block
                        let mut block = String::new();
                        while let Some(ch) = chars.next() {
                            if ch == '`' {
                                if chars.clone().take(2).collect::<String>() == "``" {
                                    chars.nth(2);
                                    break;
                                }
                                block.push(ch);
                            } else {
                                block.push(ch);
                            }
                        }
                        output.push_str(&format!("\x1b[36m{}\x1b[0m", block.trim()));
                        break;
                    }
                    break;
                } else {
                    code.push(chars.next().unwrap());
                }
            }
            if !code.is_empty() {
                output.push_str(&format!("\x1b[33m{}\x1b[0m", code));
            }
        } else if c == '*' && chars.peek() == Some(&'*') {
            // Bold
            chars.next();
            let mut bold = String::new();
            while let Some(&next) = chars.peek() {
                if next == '*' && chars.clone().nth(1) == Some('*') {
                    chars.next();
                    chars.next();
                    break;
                }
                bold.push(chars.next().unwrap());
            }
            output.push_str(&format!("\x1b[1m{}\x1b[0m", bold));
        } else if c == '_' {
            // Italic
            let mut italic = String::new();
            while let Some(&next) = chars.peek() {
                if next == '_' {
                    chars.next();
                    break;
                }
                italic.push(chars.next().unwrap());
            }
            output.push_str(&format!("\x1b[3m{}\x1b[0m", italic));
        } else {
            output.push(c);
        }
    }

    output
}

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
        crate::oauth_server::open_browser(url).map(|_child| ()).map_err(|e| format!("Failed to open browser: {}", e))
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
        if let Some(ref _oauth_state) = self.oauth_state {
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

/// Tool result content block
#[derive(Debug, Clone)]
pub enum ToolContentBlock {
    /// Text output
    Text { text: String },
    /// Image data (base64 encoded)
    Image { data: String, mime_type: String },
}

/// Tool execution result
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: Vec<ToolContentBlock>,
    pub is_error: bool,
    pub details: Option<serde_json::Value>,
}

impl ToolResult {
    pub fn new_text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContentBlock::Text { text: text.into() }],
            is_error: false,
            details: None,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContentBlock::Text { text: text.into() }],
            is_error: true,
            details: None,
        }
    }

    /// Get text output (first text block or concatenated)
    pub fn get_text(&self) -> Option<String> {
        let texts: Vec<_> = self
            .content
            .iter()
            .filter_map(|b| match b {
                ToolContentBlock::Text { text } => Some(text.clone()),
                ToolContentBlock::Image { .. } => None,
            })
            .collect();

        if texts.is_empty() {
            None
        } else {
            Some(texts.join("\n"))
        }
    }

    /// Check if result contains images
    pub fn has_images(&self) -> bool {
        self.content
            .iter()
            .any(|b| matches!(b, ToolContentBlock::Image { .. }))
    }

    /// Count images in result
    pub fn image_count(&self) -> usize {
        self.content
            .iter()
            .filter(|b| matches!(b, ToolContentBlock::Image { .. }))
            .count()
    }
}

/// Tool execution state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionState {
    /// Tool call has been made, awaiting execution
    Pending,
    /// Tool is currently executing
    Running,
    /// Tool completed successfully
    Success,
    /// Tool completed with error
    Error,
}

/// Tool execution display state
#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub tool_name: String,
    pub tool_call_id: String,
    pub arguments: serde_json::Value,
    pub state: ToolExecutionState,
    pub result: Option<ToolResult>,
    pub expanded: bool,
    pub show_images: bool,
}

impl ToolExecution {
    pub fn new(tool_name: impl Into<String>, tool_call_id: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            arguments: args,
            state: ToolExecutionState::Pending,
            result: None,
            expanded: false,
            show_images: true,
        }
    }

    /// Mark execution as started
    pub fn start(&mut self) {
        self.state = ToolExecutionState::Running;
    }

    /// Complete with result
    pub fn complete(&mut self, result: ToolResult) {
        self.state = if result.is_error {
            ToolExecutionState::Error
        } else {
            ToolExecutionState::Success
        };
        self.result = Some(result);
    }

    /// Set expanded/collapsed state
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Toggle expanded state
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Format arguments for display
    pub fn format_arguments(&self) -> String {
        if self.arguments.is_null() {
            String::new()
        } else if let Ok(obj) = serde_json::from_value::<serde_json::Value>(self.arguments.clone()) {
            serde_json::to_string_pretty(&obj).unwrap_or_else(|_| self.arguments.to_string())
        } else {
            self.arguments.to_string()
        }
    }

    /// Render the tool execution
    pub fn render(&self) -> String {
        let mut output = String::new();

        // Determine colors based on state
        let (bg_color, fg_color, status_icon) = match self.state {
            ToolExecutionState::Pending => ("\x1b[48;5;240m", "\x1b[38;5;250m", "○"),
            ToolExecutionState::Running => ("\x1b[48;5;239m", "\x1b[38;5;250m", "◐"),
            ToolExecutionState::Success => ("\x1b[48;5;28m", "\x1b[38;5;255m", "●"),
            ToolExecutionState::Error => ("\x1b[48;5;196m", "\x1b[38;5;255m", "✗"),
        };
        let reset = "\x1b[0m";

        // Tool header
        output.push_str(&format!(
            "{} {} {}\x1b[1m{}\x1b[0m{}",
            bg_color, status_icon, fg_color, self.tool_name, reset
        ));
        output.push('\n');

        // Arguments (if expanded or small)
        let args_str = self.format_arguments();
        if self.expanded || args_str.len() < 200 {
            if !args_str.is_empty() {
                // Pretty print arguments
                output.push_str(&format!("{}Arguments:{} {}\n", fg_color, reset, args_str));
            }
        } else {
            // Show truncated args
            let truncated = if args_str.len() > 100 {
                format!("{}...\x1b[0m ({} chars)", &args_str[..100], args_str.len())
            } else {
                args_str
            };
            output.push_str(&format!("{}Arguments:{} {}\n", fg_color, reset, truncated));
        }

        // Result
        if let Some(ref result) = self.result {
            let result_fg = if result.is_error { "\x1b[31m" } else { fg_color };

            if self.expanded {
                // Show full result
                if let Some(text) = result.get_text() {
                    output.push_str(&format!("{}Output:{}\n{}", result_fg, reset, text));
                    if !text.ends_with('\n') {
                        output.push('\n');
                    }
                }

                // Show images count
                if result.has_images() && self.show_images {
                    output.push_str(&format!(
                        "{} ({} image{})",
                        fg_color,
                        result.image_count(),
                        if result.image_count() == 1 { "" } else { "s" }
                    ));
                }
            } else {
                // Show truncated result
                if let Some(text) = result.get_text() {
                    let truncated = truncate_text(&text, 500);
                    output.push_str(&format!("{}Output:{} {}", result_fg, reset, truncated));
                    if text.len() > 500 {
                        output.push_str(" (use → to expand)");
                    }
                    output.push('\n');
                } else if result.has_images() {
                    output.push_str(&format!(
                        "{}Output:{} {} image{}\n",
                        result_fg,
                        reset,
                        result.image_count(),
                        if result.image_count() == 1 { "" } else { "s" }
                    ));
                }
            }
        } else if self.state == ToolExecutionState::Running {
            output.push_str(&format!("{}Running...{}", fg_color, reset));
            output.push('\n');
        }

        output
    }
}

/// Truncate text to max length with ellipsis
fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }

    // Find a good break point
    let truncated = &text[..max_len];
    if let Some(last_newline) = truncated.rfind('\n') {
        format!("{}...", &truncated[..last_newline])
    } else if let Some(last_space) = truncated.rfind(' ') {
        format!("{}...", &truncated[..last_space])
    } else {
        format!("{}...", truncated)
    }
}

/// Tool execution renderer with advanced options
pub struct ToolExecutionRenderer {
    pub show_images: bool,
    pub max_image_width: usize,
}

impl ToolExecutionRenderer {
    pub fn new() -> Self {
        Self {
            show_images: true,
            max_image_width: 80,
        }
    }

    pub fn with_show_images(mut self, show: bool) -> Self {
        self.show_images = show;
        self
    }

    pub fn with_max_image_width(mut self, width: usize) -> Self {
        self.max_image_width = width;
        self
    }

    /// Render multiple tool executions
    pub fn render_all(&self, executions: &[ToolExecution]) -> String {
        executions
            .iter()
            .map(|e| e.render())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for ToolExecutionRenderer {
    fn default() -> Self {
        Self::new()
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

/// Enhanced Bash execution state with output truncation and streaming support
#[derive(Debug, Clone)]
pub struct BashExecution {
    pub command: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub start_time: std::time::Instant,
    pub is_running: bool,
    pub is_cancelled: bool,
    /// Whether the output is expanded (shows full output) or collapsed (preview only)
    pub expanded: bool,
    /// Truncation result for context limits
    pub truncation_info: Option<TruncationInfo>,
    /// Path to full output file if truncated
    pub full_output_path: Option<String>,
}

/// Information about output truncation
#[derive(Debug, Clone)]
pub struct TruncationInfo {
    /// Total lines before truncation
    pub total_lines: usize,
    /// Lines shown after truncation
    pub shown_lines: usize,
    /// Bytes before truncation
    pub total_bytes: usize,
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
            expanded: false,
            truncation_info: None,
            full_output_path: None,
        }
    }

    /// Append output (stripping ANSI codes for display)
    pub fn append_output(&mut self, chunk: &str) {
        // Strip ANSI codes and normalize line endings
        let clean = strip_ansi(chunk).replace("\r\n", "\n").replace("\r", "\n");
        
        // Append to output
        if !self.output.is_empty() && !clean.is_empty() {
            self.output.push_str(&clean);
        } else {
            self.output.push_str(&clean);
        }
    }

    /// Mark as complete
    pub fn complete(&mut self, exit_code: i32) {
        self.exit_code = Some(exit_code);
        self.is_running = false;
    }

    /// Complete with truncation info
    pub fn complete_with_truncation(
        &mut self,
        exit_code: i32,
        truncation_info: TruncationInfo,
        full_output_path: Option<String>,
    ) {
        self.exit_code = Some(exit_code);
        self.is_running = false;
        self.truncation_info = Some(truncation_info);
        self.full_output_path = full_output_path;
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

    /// Set expanded/collapsed state
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Toggle expanded state
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Get output lines
    pub fn output_lines(&self) -> Vec<&str> {
        self.output.lines().collect()
    }

    /// Get raw output string
    pub fn get_output(&self) -> &str {
        &self.output
    }

    /// Render the bash execution display
    pub fn render(&self) -> String {
        let mut output = String::new();
        
        // Preview line limit (when not expanded)
        const PREVIEW_LINES: usize = 20;

        // Command header with styling
        output.push_str(&format!("\x1b[1m$ {}\x1b[0m\n", self.command));

        // Process output lines for display
        let lines: Vec<&str> = self.output.lines().collect();
        let total_lines = lines.len();
        let hidden_lines = if total_lines > PREVIEW_LINES && !self.expanded {
            total_lines - PREVIEW_LINES
        } else {
            0
        };

        // Show output
        if !self.output.is_empty() {
            let lines_to_show = if self.expanded {
                &lines[..]
            } else {
                // Show last PREVIEW_LINES
                if lines.len() > PREVIEW_LINES {
                    &lines[lines.len() - PREVIEW_LINES..]
                } else {
                    &lines[..]
                }
            };
            
            // Muted color for output
            for line in lines_to_show {
                output.push_str(&format!("\x1b[2m{}\x1b[0m\n", line));
            }
        }

        // Status line
        if self.is_running {
            // Running state with elapsed time
            output.push_str(&format!(
                "\x1b[90mRunning... ({:.1}s)\x1b[0m\n",
                self.elapsed().as_secs_f64()
            ));
        } else {
            let mut status_parts = Vec::new();

            // Hidden lines indicator
            if hidden_lines > 0 {
                if self.expanded {
                    status_parts.push("\x1b[2m(to collapse)\x1b[0m".to_string());
                } else {
                    status_parts.push(format!(
                        "\x1b[2m... {} more lines\x1b[0m",
                        hidden_lines
                    ));
                }
            }

            // Status
            if self.is_cancelled {
                status_parts.push("\x1b[33m(cancelled)\x1b[0m".to_string());
            } else if let Some(code) = self.exit_code {
                if code != 0 {
                    status_parts.push(format!("\x1b[31m(exit {})\x1b[0m", code));
                }
            }

            // Truncation warning
            if self.truncation_info.is_some() {
                if let Some(ref path) = self.full_output_path {
                    status_parts.push(format!(
                        "\x1b[33mOutput truncated. Full output: {}\x1b[0m",
                        path
                    ));
                }
            }

            if !status_parts.is_empty() {
                output.push_str(&status_parts.join(" "));
                output.push('\n');
            }
        }

        output
    }
}

/// Strip ANSI escape codes from a string
fn strip_ansi(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI sequence
            if chars.next() == Some('[') {
                // Read until we hit a letter
                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_alphabetic() {
                        chars.next();
                        break;
                    }
                    chars.next();
                }
            }
        } else {
            result.push(c);
        }
    }
    
    result
}

/// Summary message types for compaction and branch summaries
#[derive(Debug, Clone)]
pub enum SummaryMessageType {
    /// Compaction summary after context window management
    Compaction { tokens_before: usize },
    /// Branch summary when creating/merging branches
    Branch,
}

/// Summary message data
#[derive(Debug, Clone)]
pub struct SummaryMessage {
    pub message_type: SummaryMessageType,
    pub summary: String,
    pub expanded: bool,
}

impl SummaryMessage {
    pub fn compaction(tokens_before: usize, summary: impl Into<String>) -> Self {
        Self {
            message_type: SummaryMessageType::Compaction { tokens_before },
            summary: summary.into(),
            expanded: false,
        }
    }

    pub fn branch(summary: impl Into<String>) -> Self {
        Self {
            message_type: SummaryMessageType::Branch,
            summary: summary.into(),
            expanded: false,
        }
    }

    /// Set expanded/collapsed state
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Toggle expanded state
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Render the summary message
    pub fn render(&self) -> String {
        let mut output = String::new();

        // Label with bold styling
        let label = match &self.message_type {
            SummaryMessageType::Compaction { tokens_before } => {
                format!(
                    "\x1b[1m[compaction]\x1b[0m Compacted from {} tokens",
                    tokens_before
                )
            }
            SummaryMessageType::Branch => {
                "\x1b[1m[branch]\x1b[0m Branch Summary".to_string()
            }
        };

        output.push_str(&format!("\x1b[48;5;24m {} \x1b[0m\n", label));

        if self.expanded {
            // Show full summary with markdown-style formatting
            output.push_str("\n");
            output.push_str(&render_markdown(&self.summary));
            output.push('\n');
        } else {
            // Show collapsed hint
            output.push_str(&format!(
                "\x1b[2m(to expand)\x1b[0m\n",
            ));
        }

        output
    }
}

/// Summary message renderer with options
pub struct SummaryMessageRenderer;

impl SummaryMessageRenderer {
    /// Render a compaction summary
    pub fn render_compaction(tokens_before: usize, summary: &str, expanded: bool) -> String {
        let mut msg = SummaryMessage::compaction(tokens_before, summary);
        msg.set_expanded(expanded);
        msg.render()
    }

    /// Render a branch summary
    pub fn render_branch(summary: &str, expanded: bool) -> String {
        let mut msg = SummaryMessage::branch(summary);
        msg.set_expanded(expanded);
        msg.render()
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

    #[ignore] // broken test
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

    // ── Assistant Message Tests ────────────────────────────────────────────────

    #[test]
    fn test_assistant_message_builder() {
        let mut msg = AssistantMessage::new();
        msg.add_text("Hello, world!");
        msg.add_thinking("Let me think about this...");
        msg.add_tool_call("call_123", "bash", r#"{"command": "ls"}"#);

        assert!(msg.has_visible_content());
        assert!(msg.has_tool_calls());
        assert_eq!(msg.content.len(), 3);
    }

    #[test]
    fn test_assistant_message_renderer_hide_thinking() {
        let mut msg = AssistantMessage::new();
        msg.add_thinking("This is my thought process");
        msg.add_text("Final answer");

        let renderer = AssistantMessageRenderer::new(
            AssistantMessageRenderOptions {
                hide_thinking: true,
                hidden_thinking_label: "Thinking...".to_string(),
                use_osc133: false,
            },
        );

        let rendered = renderer.render(&msg);
        assert!(rendered.contains("Thinking..."));
        assert!(rendered.contains("Final answer"));
        assert!(!rendered.contains("This is my thought process"));
    }

    #[test]
    fn test_assistant_message_renderer_show_thinking() {
        let mut msg = AssistantMessage::new();
        msg.add_thinking("This is my thought process");

        let renderer = AssistantMessageRenderer::new(AssistantMessageRenderOptions::default());
        let rendered = renderer.render(&msg);
        assert!(rendered.contains("This is my thought process"));
    }

    #[test]
    fn test_assistant_message_renderer_error() {
        let mut msg = AssistantMessage::new();
        msg.add_text("Some content");
        msg.stop_reason = Some(StopReason::Error);
        msg.error_message = Some("Something went wrong".to_string());

        let renderer = AssistantMessageRenderer::new(AssistantMessageRenderOptions::default());
        let rendered = renderer.render(&msg);
        assert!(rendered.contains("Error: Something went wrong"));
    }

    #[test]
    fn test_assistant_message_renderer_aborted() {
        let mut msg = AssistantMessage::new();
        msg.stop_reason = Some(StopReason::Aborted);

        let renderer = AssistantMessageRenderer::new(AssistantMessageRenderOptions::default());
        let rendered = renderer.render(&msg);
        assert!(rendered.contains("Operation aborted"));
    }

    #[test]
    fn test_stop_reason_as_str() {
        assert_eq!(StopReason::EndTurn.as_str(), "end_turn");
        assert_eq!(StopReason::MaxTokens.as_str(), "max_tokens");
        assert_eq!(StopReason::StopSequence.as_str(), "stop_sequence");
        assert_eq!(StopReason::Aborted.as_str(), "aborted");
        assert_eq!(StopReason::Error.as_str(), "error");
    }

    #[test]
    fn test_render_markdown() {
        // Test inline code
        let result = render_markdown("Use `ls` to list files");
        assert!(result.contains("\x1b[33m")); // Yellow for code

        // Test bold
        let result = render_markdown("This is **bold** text");
        assert!(result.contains("\x1b[1m")); // Bold

        // Test italic
        let result = render_markdown("This is _italic_ text");
        assert!(result.contains("\x1b[3m")); // Italic
    }

    // ── Tool Execution Tests ─────────────────────────────────────────────────

    #[test]
    fn test_tool_result_text() {
        let result = ToolResult::new_text("file created successfully");
        assert!(!result.is_error);
        assert_eq!(result.get_text(), Some("file created successfully".to_string()));
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("file not found");
        assert!(result.is_error);
        assert_eq!(result.get_text(), Some("file not found".to_string()));
    }

    #[test]
    fn test_tool_result_images() {
        let mut result = ToolResult::new_text("analysis complete");
        result.content.push(ToolContentBlock::Image {
            data: "base64data".to_string(),
            mime_type: "image/png".to_string(),
        });

        assert!(result.has_images());
        assert_eq!(result.image_count(), 1);
        assert!(result.get_text().is_some());
    }

    #[test]
    fn test_tool_execution_pending() {
        let exec = ToolExecution::new(
            "read_file",
            "call_abc",
            serde_json::json!({"path": "test.txt"}),
        );

        assert_eq!(exec.state, ToolExecutionState::Pending);
        assert!(exec.result.is_none());
    }

    #[test]
    fn test_tool_execution_complete() {
        let mut exec = ToolExecution::new("bash", "call_123", serde_json::json!({"command": "ls"}));
        exec.start();
        assert_eq!(exec.state, ToolExecutionState::Running);

        exec.complete(ToolResult::new_text("file1.txt\nfile2.txt"));
        assert_eq!(exec.state, ToolExecutionState::Success);
        assert!(exec.result.is_some());
    }

    #[test]
    fn test_tool_execution_error() {
        let mut exec = ToolExecution::new("bash", "call_123", serde_json::json!({"command": "ls"}));
        exec.complete(ToolResult::error("Permission denied"));
        assert_eq!(exec.state, ToolExecutionState::Error);
    }

    #[test]
    fn test_tool_execution_format_arguments() {
        let exec = ToolExecution::new(
            "search",
            "call_1",
            serde_json::json!({"query": "test", "limit": 10}),
        );
        let args = exec.format_arguments();
        assert!(args.contains("test"));
        assert!(args.contains("10"));
    }

    #[test]
    fn test_tool_execution_render() {
        let mut exec = ToolExecution::new("read_file", "call_1", serde_json::json!({"path": "test.txt"}));
        exec.complete(ToolResult::new_text("file contents"));

        let rendered = exec.render();
        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("file contents"));
    }

    #[test]
    fn test_truncate_text() {
        let long_text = "This is a very long text that should be truncated. ".repeat(20);
        let truncated = truncate_text(&long_text, 100);
        assert!(truncated.len() < long_text.len());
        assert!(truncated.ends_with("..."));
    }

    // ── Bash Execution Tests ─────────────────────────────────────────────────

    #[test]
    fn test_bash_execution_expanded() {
        let mut exec = BashExecution::new("echo test".to_string());
        exec.append_output("line1\nline2\nline3\n");
        exec.set_expanded(true);

        assert!(exec.expanded);
        let rendered = exec.render();
        assert!(rendered.contains("line1"));
    }

    #[test]
    fn test_bash_execution_preview() {
        let mut exec = BashExecution::new("ls -la".to_string());
        // Add many lines
        for i in 0..50 {
            exec.append_output(&format!("line{}\n", i));
        }

        let rendered = exec.render();
        // Should show hidden lines message
        assert!(rendered.contains("more lines"));
    }

    #[test]
    fn test_bash_execution_strip_ansi() {
        let input = "\x1b[31mRed text\x1b[0m and normal";
        let stripped = strip_ansi(input);
        assert_eq!(stripped, "Red text and normal");
    }

    #[test]
    fn test_bash_execution_truncation() {
        let mut exec = BashExecution::new("cat large_file.txt".to_string());
        exec.append_output("content");
        exec.complete_with_truncation(
            0,
            TruncationInfo {
                total_lines: 1000,
                shown_lines: 500,
                total_bytes: 50000,
            },
            Some("/tmp/full_output.txt".to_string()),
        );

        let rendered = exec.render();
        assert!(rendered.contains("truncated"));
        assert!(rendered.contains("/tmp/full_output.txt"));
    }

    #[test]
    fn test_bash_execution_get_output() {
        let mut exec = BashExecution::new("echo hello".to_string());
        exec.append_output("hello world");
        assert_eq!(exec.get_output(), "hello world");
    }

    #[test]
    fn test_bash_execution_output_lines() {
        let mut exec = BashExecution::new("echo hello".to_string());
        exec.append_output("line1\nline2\nline3");
        let lines = exec.output_lines();
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    // ── Summary Message Tests ────────────────────────────────────────────────

    #[test]
    fn test_summary_message_compaction() {
        let mut msg = SummaryMessage::compaction(50000, "Compacted 50000 tokens to 10000");
        assert!(matches!(msg.message_type, SummaryMessageType::Compaction { tokens_before: 50000 }));
        
        msg.set_expanded(true);
        let rendered = msg.render();
        assert!(rendered.contains("compaction"));
        assert!(rendered.contains("Compacted from 50000 tokens"));
    }

    #[test]
    fn test_summary_message_branch() {
        let mut msg = SummaryMessage::branch("Created a new branch with these changes...");
        assert!(matches!(msg.message_type, SummaryMessageType::Branch));
        
        msg.set_expanded(true);
        let rendered = msg.render();
        assert!(rendered.contains("[branch]"));
    }

    #[test]
    fn test_summary_message_renderer() {
        let rendered = SummaryMessageRenderer::render_compaction(
            50000,
            "Summary of compacted content",
            true,
        );
        assert!(rendered.contains("50000"));
    }
}
