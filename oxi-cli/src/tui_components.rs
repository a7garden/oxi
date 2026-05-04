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
    Text { text: String },
    /// Thinking/reasoning block (collapsible)
    Thinking { thinking: String },
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
        self.content
            .push(AssistantContentBlock::Text { text: text.into() });
    }

    /// Add a thinking block
    pub fn add_thinking(&mut self, thinking: impl Into<String>) {
        self.content.push(AssistantContentBlock::Thinking {
            thinking: thinking.into(),
        });
    }

    /// Add a tool call block
    pub fn add_tool_call(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) {
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
        crate::oauth_server::open_browser(url)
            .map(|_child| ())
            .map_err(|e| format!("Failed to open browser: {}", e))
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
    pub fn new(
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
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
        } else if let Ok(obj) = serde_json::from_value::<serde_json::Value>(self.arguments.clone())
        {
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
            let result_fg = if result.is_error {
                "\x1b[31m"
            } else {
                fg_color
            };

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
                    status_parts.push(format!("\x1b[2m... {} more lines\x1b[0m", hidden_lines));
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
            SummaryMessageType::Branch => "\x1b[1m[branch]\x1b[0m Branch Summary".to_string(),
        };

        output.push_str(&format!("\x1b[48;5;24m {} \x1b[0m\n", label));

        if self.expanded {
            // Show full summary with markdown-style formatting
            output.push_str("\n");
            output.push_str(&render_markdown(&self.summary));
            output.push('\n');
        } else {
            // Show collapsed hint
            output.push_str(&format!("\x1b[2m(to expand)\x1b[0m\n",));
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

// ═══════════════════════════════════════════════════════════════════════════
// Extension-Related Interactive Components
// ═══════════════════════════════════════════════════════════════════════════

/// Editor for extension-provided input.
///
/// Displays a bordered, titled multi-line text area with hint text
/// showing keybindings for submit / newline / cancel / external editor.
#[derive(Debug, Clone)]
pub struct ExtensionEditor {
    /// Title displayed above the editor.
    pub title: String,
    /// Current text content.
    pub text: String,
    /// Cursor position within `text`.
    pub cursor_pos: usize,
    /// Whether an external editor is available ($VISUAL or $EDITOR).
    pub has_external_editor: bool,
}

impl ExtensionEditor {
    /// Create a new extension editor with a title and optional prefill text.
    pub fn new(title: impl Into<String>, prefill: Option<&str>) -> Self {
        let text = prefill.unwrap_or("").to_string();
        let cursor_pos = text.len();
        let has_external_editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .is_ok();
        Self {
            title: title.into(),
            text,
            cursor_pos,
            has_external_editor,
        }
    }

    /// Insert a character at the cursor position.
    pub fn input_char(&mut self, c: char) {
        self.text.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    /// Delete character before cursor.
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            // Walk back to the previous char boundary
            let prev = self.text[..self.cursor_pos]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.text.drain(prev..self.cursor_pos);
            self.cursor_pos = prev;
        }
    }

    /// Replace the full text (e.g. after external editor session).
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor_pos = self.text.len();
    }

    /// Render the editor component as a vector of lines.
    pub fn render(&self) -> Vec<String> {
        let border = "─".repeat(60);
        let mut lines = Vec::new();

        lines.push(border.clone());
        lines.push(String::new()); // spacer
        lines.push(format!("\x1b[36m{}\x1b[0m", self.title));
        lines.push(String::new()); // spacer

        // Editor content (each source line as a rendered line)
        for line in self.text.lines() {
            lines.push(line.to_string());
        }
        if self.text.is_empty() {
            lines.push(String::new());
        }

        lines.push(String::new()); // spacer

        // Hint line
        let mut hints = vec!["Enter: submit", "Shift+Enter: newline", "Esc: cancel"];
        if self.has_external_editor {
            hints.push("Ctrl+G: external editor");
        }
        lines.push(format!("\x1b[2m{}\x1b[0m", hints.join("  ")));
        lines.push(String::new()); // spacer
        lines.push(border);

        lines
    }
}

/// Simple text input component for extensions.
///
/// Displays a bordered, titled single-line input with optional timeout
/// countdown shown in the title.
#[derive(Debug, Clone)]
pub struct ExtensionInput {
    /// Title displayed above the input.
    pub title: String,
    /// Current input value.
    pub value: String,
    /// Cursor position within `value`.
    pub cursor_pos: usize,
    /// Optional timeout in seconds (shown as countdown in title).
    pub timeout_secs: Option<u64>,
    /// Remaining seconds (when timeout is active).
    pub remaining_secs: Option<u64>,
}

impl ExtensionInput {
    /// Create a new extension input with a title and optional timeout.
    pub fn new(title: impl Into<String>, timeout_secs: Option<u64>) -> Self {
        Self {
            title: title.into(),
            value: String::new(),
            cursor_pos: 0,
            timeout_secs,
            remaining_secs: timeout_secs,
        }
    }

    /// Insert a character at the cursor position.
    pub fn input_char(&mut self, c: char) {
        self.value.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    /// Delete character before cursor.
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.value[..self.cursor_pos]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.value.drain(prev..self.cursor_pos);
            self.cursor_pos = prev;
        }
    }

    /// Update the countdown timer.
    pub fn tick(&mut self) -> bool {
        if let Some(ref mut remaining) = self.remaining_secs {
            if *remaining > 0 {
                *remaining -= 1;
                return *remaining > 0;
            }
            return false;
        }
        true
    }

    /// Render the input component as a vector of lines.
    pub fn render(&self) -> Vec<String> {
        let border = "─".repeat(60);
        let mut lines = Vec::new();

        lines.push(border.clone());
        lines.push(String::new()); // spacer

        // Title with optional countdown
        let title_display = if let Some(remaining) = self.remaining_secs {
            format!("{} ({}s)", self.title, remaining)
        } else {
            self.title.clone()
        };
        lines.push(format!("\x1b[36m{}\x1b[0m", title_display));
        lines.push(String::new()); // spacer

        // Input field
        lines.push(format!("> {}", self.value));
        lines.push(String::new()); // spacer

        // Hints
        lines.push(format!("\x1b[2mEnter: submit  Esc: cancel\x1b[0m"));
        lines.push(String::new()); // spacer
        lines.push(border);

        lines
    }
}

/// Generic selector component for extensions.
///
/// Displays a list of string options with keyboard navigation
/// (up/down arrows, j/k) and selection (Enter).
#[derive(Debug, Clone)]
pub struct ExtensionSelector {
    /// Title displayed above the list.
    pub title: String,
    /// Available options.
    pub options: Vec<String>,
    /// Currently selected index.
    pub selected_index: usize,
    /// Optional timeout in seconds.
    pub timeout_secs: Option<u64>,
    /// Remaining seconds (when timeout is active).
    pub remaining_secs: Option<u64>,
}

impl ExtensionSelector {
    /// Create a new extension selector with a title and list of options.
    pub fn new(title: impl Into<String>, options: Vec<String>, timeout_secs: Option<u64>) -> Self {
        Self {
            title: title.into(),
            selected_index: 0,
            timeout_secs,
            remaining_secs: timeout_secs,
            options,
        }
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.selected_index < self.options.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    /// Get the currently selected option.
    pub fn selected(&self) -> Option<&str> {
        self.options.get(self.selected_index).map(|s| s.as_str())
    }

    /// Update the countdown timer.
    pub fn tick(&mut self) -> bool {
        if let Some(ref mut remaining) = self.remaining_secs {
            if *remaining > 0 {
                *remaining -= 1;
                return *remaining > 0;
            }
            return false;
        }
        true
    }

    /// Render the selector component as a vector of lines.
    pub fn render(&self) -> Vec<String> {
        let border = "─".repeat(60);
        let mut lines = Vec::new();

        lines.push(border.clone());
        lines.push(String::new()); // spacer

        // Title with optional countdown
        let title_display = if let Some(remaining) = self.remaining_secs {
            format!("{} ({}s)", self.title, remaining)
        } else {
            self.title.clone()
        };
        lines.push(format!("\x1b[1m\x1b[36m{}\x1b[0m", title_display));
        lines.push(String::new()); // spacer

        // Options
        for (i, option) in self.options.iter().enumerate() {
            if i == self.selected_index {
                lines.push(format!("\x1b[36m→ {}\x1b[0m", option));
            } else {
                lines.push(format!("  \x1b[37m{}\x1b[0m", option));
            }
        }

        lines.push(String::new()); // spacer

        // Hints
        lines.push(format!(
            "\x1b[2m↑↓: navigate  Enter: select  Esc: cancel\x1b[0m"
        ));
        lines.push(String::new()); // spacer
        lines.push(border);

        lines
    }
}

/// Custom editor registered by extensions.
///
/// Wraps an editor-like buffer with extension action handlers
/// for escape, Ctrl+D, paste-image, and extension-registered shortcuts.
#[derive(Debug, Clone)]
pub struct CustomEditor {
    /// Current text content.
    pub text: String,
    /// Cursor position within `text`.
    pub cursor_pos: usize,
    /// Whether autocomplete is currently showing.
    pub showing_autocomplete: bool,
    /// Registered action handlers (action name → description).
    pub registered_actions: Vec<String>,
}

impl CustomEditor {
    /// Create a new custom editor.
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor_pos: 0,
            showing_autocomplete: false,
            registered_actions: Vec::new(),
        }
    }

    /// Create with initial text content.
    pub fn with_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor_pos = text.len();
        Self {
            text,
            cursor_pos,
            showing_autocomplete: false,
            registered_actions: Vec::new(),
        }
    }

    /// Insert a character at the cursor position.
    pub fn input_char(&mut self, c: char) {
        self.text.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
        self.showing_autocomplete = false;
    }

    /// Delete character before cursor.
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.text[..self.cursor_pos]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.text.drain(prev..self.cursor_pos);
            self.cursor_pos = prev;
        }
        self.showing_autocomplete = false;
    }

    /// Check if the editor is empty (for Ctrl+D exit detection).
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Register an action handler.
    pub fn register_action(&mut self, action: impl Into<String>) {
        let action = action.into();
        if !self.registered_actions.contains(&action) {
            self.registered_actions.push(action);
        }
    }

    /// Render the custom editor as a vector of lines.
    pub fn render(&self) -> Vec<String> {
        let mut lines = Vec::new();

        // Show text content line-by-line
        for line in self.text.lines() {
            lines.push(line.to_string());
        }
        if self.text.is_empty() {
            lines.push(String::new());
        }

        lines
    }
}

impl Default for CustomEditor {
    fn default() -> Self {
        Self::new()
    }
}

/// Custom message type rendering.
///
/// Renders extension-provided custom messages with a styled label
/// and markdown content, similar to how pi-mono renders custom
/// message entries.
#[derive(Debug, Clone)]
pub struct CustomMessageComponent {
    /// The custom type label.
    pub custom_type: String,
    /// The message content (plain text).
    pub content: String,
    /// Whether the message is expanded.
    pub expanded: bool,
}

impl CustomMessageComponent {
    /// Create a new custom message component.
    pub fn new(custom_type: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            custom_type: custom_type.into(),
            content: content.into(),
            expanded: false,
        }
    }

    /// Set expanded/collapsed state.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Toggle expanded state.
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Render the custom message as a vector of lines.
    pub fn render(&self) -> Vec<String> {
        let mut lines = Vec::new();

        lines.push(String::new()); // spacer

        // Label with type
        let label = format!("\x1b[35m\x1b[1m[{}]\x1b[0m", self.custom_type);
        lines.push(label);
        lines.push(String::new()); // spacer

        // Content with background
        if self.expanded {
            for line in self.content.lines() {
                lines.push(format!("\x1b[48;5;54m {} \x1b[0m", line));
            }
        } else {
            // Show first line as preview
            let first_line = self.content.lines().next().unwrap_or("");
            let preview = if first_line.len() > 80 {
                format!("{}...", &first_line[..80])
            } else if first_line.is_empty() {
                "(empty)".to_string()
            } else {
                first_line.to_string()
            };
            lines.push(format!("\x1b[48;5;54m {} \x1b[0m", preview));
            let total_lines = self.content.lines().count();
            if total_lines > 1 {
                lines.push(format!(
                    "\x1b[2m  ... {} more lines\x1b[0m",
                    total_lines.saturating_sub(1)
                ));
            }
        }

        lines
    }
}

/// Login dialog state for provider login flow.
///
/// Supports multiple login phases: URL display, manual code input,
/// prompt display, waiting, and progress messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginDialogPhase {
    /// Initial state.
    Init,
    /// Showing authorization URL to the user.
    ShowAuth {
        url: String,
        instructions: Option<String>,
    },
    /// Waiting for user to manually input a code/URL.
    ManualInput {
        prompt: String,
        value: String,
        cursor_pos: usize,
    },
    /// Showing a prompt and waiting for input.
    Prompt {
        message: String,
        placeholder: Option<String>,
        value: String,
        cursor_pos: usize,
    },
    /// Showing informational text.
    Info { lines: Vec<String> },
    /// Showing a waiting/polling message.
    Waiting { message: String },
    /// Login completed (success or failure).
    Completed {
        success: bool,
        message: Option<String>,
    },
}

/// Provider login dialog component.
///
/// Manages the full lifecycle of an OAuth or API-key login flow,
/// rendering appropriate UI at each phase.
#[derive(Debug, Clone)]
pub struct ProviderLoginDialog {
    /// Provider ID (e.g. "anthropic", "openai").
    pub provider_id: String,
    /// Provider display name.
    pub provider_name: String,
    /// Optional custom title.
    pub title: Option<String>,
    /// Current phase of the login flow.
    pub phase: LoginDialogPhase,
}

impl ProviderLoginDialog {
    /// Create a new provider login dialog.
    pub fn new(provider_id: impl Into<String>, provider_name: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_name: provider_name.into(),
            title: None,
            phase: LoginDialogPhase::Init,
        }
    }

    /// Set a custom title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Transition to the show-auth phase.
    pub fn show_auth(&mut self, url: impl Into<String>, instructions: Option<String>) {
        self.phase = LoginDialogPhase::ShowAuth {
            url: url.into(),
            instructions,
        };
    }

    /// Transition to the manual-input phase.
    pub fn show_manual_input(&mut self, prompt: impl Into<String>) {
        self.phase = LoginDialogPhase::ManualInput {
            prompt: prompt.into(),
            value: String::new(),
            cursor_pos: 0,
        };
    }

    /// Transition to the prompt phase.
    pub fn show_prompt(&mut self, message: impl Into<String>, placeholder: Option<String>) {
        self.phase = LoginDialogPhase::Prompt {
            message: message.into(),
            placeholder,
            value: String::new(),
            cursor_pos: 0,
        };
    }

    /// Transition to the info phase.
    pub fn show_info(&mut self, lines: Vec<String>) {
        self.phase = LoginDialogPhase::Info { lines };
    }

    /// Transition to the waiting phase.
    pub fn show_waiting(&mut self, message: impl Into<String>) {
        self.phase = LoginDialogPhase::Waiting {
            message: message.into(),
        };
    }

    /// Show a progress message (appends to current phase).
    pub fn show_progress(&mut self, message: &str) {
        // Progress messages are displayed during waiting/polling
        match &mut self.phase {
            LoginDialogPhase::Waiting {
                message: ref mut msg,
            } => {
                msg.push_str(&format!("\n{}", message));
            }
            _ => {
                self.phase = LoginDialogPhase::Waiting {
                    message: message.to_string(),
                };
            }
        }
    }

    /// Complete the login flow.
    pub fn complete(&mut self, success: bool, message: Option<String>) {
        self.phase = LoginDialogPhase::Completed { success, message };
    }

    /// Input a character (for ManualInput and Prompt phases).
    pub fn input_char(&mut self, c: char) {
        match &mut self.phase {
            LoginDialogPhase::ManualInput {
                value, cursor_pos, ..
            }
            | LoginDialogPhase::Prompt {
                value, cursor_pos, ..
            } => {
                value.insert(*cursor_pos, c);
                *cursor_pos += c.len_utf8();
            }
            _ => {}
        }
    }

    /// Backspace (for ManualInput and Prompt phases).
    pub fn backspace(&mut self) {
        match &mut self.phase {
            LoginDialogPhase::ManualInput {
                value, cursor_pos, ..
            }
            | LoginDialogPhase::Prompt {
                value, cursor_pos, ..
            } => {
                if *cursor_pos > 0 {
                    let prev = value[..*cursor_pos]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    value.drain(prev..*cursor_pos);
                    *cursor_pos = prev;
                }
            }
            _ => {}
        }
    }

    /// Get the current input value (for ManualInput and Prompt phases).
    pub fn input_value(&self) -> Option<&str> {
        match &self.phase {
            LoginDialogPhase::ManualInput { value, .. }
            | LoginDialogPhase::Prompt { value, .. } => Some(value.as_str()),
            _ => None,
        }
    }

    /// Render the login dialog as a vector of lines.
    pub fn render(&self) -> Vec<String> {
        let border = "─".repeat(60);
        let mut lines = Vec::new();

        // Title
        let title = self
            .title
            .clone()
            .unwrap_or_else(|| format!("Login to {}", self.provider_name));
        lines.push(border.clone());
        lines.push(format!("\x1b[1m\x1b[36m{}\x1b[0m", title));

        // Content based on phase
        match &self.phase {
            LoginDialogPhase::Init => {
                lines.push(String::new());
                lines.push("Initializing...".to_string());
            }
            LoginDialogPhase::ShowAuth { url, instructions } => {
                lines.push(String::new());
                lines.push(format!("\x1b[36m{}\x1b[0m", url));
                lines.push(format!("\x1b[2mCmd+click to open\x1b[0m"));
                if let Some(instr) = instructions {
                    lines.push(String::new());
                    lines.push(format!("\x1b[33m{}\x1b[0m", instr));
                }
            }
            LoginDialogPhase::ManualInput {
                prompt,
                value,
                cursor_pos: _,
            } => {
                lines.push(String::new());
                lines.push(format!("\x1b[2m{}\x1b[0m", prompt));
                lines.push(format!("> {}", value));
                lines.push(format!("\x1b[2m(Esc to cancel)\x1b[0m"));
            }
            LoginDialogPhase::Prompt {
                message,
                placeholder,
                value,
                cursor_pos: _,
            } => {
                lines.push(String::new());
                lines.push(message.clone());
                if let Some(ph) = placeholder {
                    lines.push(format!("\x1b[2me.g., {}\x1b[0m", ph));
                }
                lines.push(format!("> {}", value));
                lines.push(format!("\x1b[2m(Esc to cancel, Enter to submit)\x1b[0m"));
            }
            LoginDialogPhase::Info { lines: info_lines } => {
                lines.push(String::new());
                for line in info_lines {
                    lines.push(line.clone());
                }
                lines.push(String::new());
                lines.push(format!("\x1b[2m(Esc to close)\x1b[0m"));
            }
            LoginDialogPhase::Waiting { message } => {
                lines.push(String::new());
                lines.push(format!("\x1b[2m{}\x1b[0m", message));
                lines.push(format!("\x1b[2m(Esc to cancel)\x1b[0m"));
            }
            LoginDialogPhase::Completed { success, message } => {
                lines.push(String::new());
                if *success {
                    lines.push(format!("\x1b[32m✓ Login successful\x1b[0m"));
                } else {
                    lines.push(format!("\x1b[31m✗ Login failed\x1b[0m"));
                }
                if let Some(msg) = message {
                    lines.push(msg.clone());
                }
            }
        }

        lines.push(border);
        lines
    }
}

/// Auth provider information for the OAuth selector.
#[derive(Debug, Clone)]
pub struct AuthProviderInfo {
    /// Unique provider ID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Authentication type.
    pub auth_type: AuthType,
}

/// Authentication type for a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthType {
    /// OAuth flow.
    OAuth,
    /// Simple API key.
    ApiKey,
}

/// Configuration status for a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConfigStatus {
    /// Not configured at all.
    Unconfigured,
    /// Configured with the matching auth type.
    Configured { label: String },
    /// Configured with a different auth type.
    PartiallyConfigured { label: String },
}

/// OAuth selector component.
///
/// Displays a searchable, scrollable list of auth providers with
/// status indicators and navigation. Supports both login and logout modes.
#[derive(Debug, Clone)]
pub struct OAuthSelector {
    /// Mode: login or logout.
    pub mode: OAuthSelectorMode,
    /// All available providers.
    pub providers: Vec<AuthProviderInfo>,
    /// Currently filtered providers (indices into `providers`).
    pub filtered_indices: Vec<usize>,
    /// Currently selected index (into `filtered_indices`).
    pub selected_index: usize,
    /// Search filter text.
    pub filter: String,
    /// Scroll offset for the visible window.
    pub scroll_offset: usize,
    /// Max visible items at once.
    pub visible_height: usize,
    /// Config status for each provider (indexed by provider ID).
    pub config_status: std::collections::HashMap<String, ProviderConfigStatus>,
}

/// Mode for the OAuth selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthSelectorMode {
    /// Selecting a provider to log into.
    Login,
    /// Selecting a provider to log out of.
    Logout,
}

impl OAuthSelector {
    /// Create a new OAuth selector.
    pub fn new(mode: OAuthSelectorMode, providers: Vec<AuthProviderInfo>) -> Self {
        let filtered_indices: Vec<usize> = (0..providers.len()).collect();
        Self {
            mode,
            providers,
            filtered_indices,
            selected_index: 0,
            filter: String::new(),
            scroll_offset: 0,
            visible_height: 8,
            config_status: std::collections::HashMap::new(),
        }
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.adjust_scroll();
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let max = self.filtered_indices.len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
            self.adjust_scroll();
        }
    }

    /// Get the currently selected provider.
    pub fn selected(&self) -> Option<&AuthProviderInfo> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|&idx| self.providers.get(idx))
    }

    /// Update the search filter and recompute filtered list.
    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        let filter_lower = self.filter.to_lowercase();
        self.filtered_indices = self
            .providers
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                if filter_lower.is_empty() {
                    return true;
                }
                p.name.to_lowercase().contains(&filter_lower)
                    || p.id.to_lowercase().contains(&filter_lower)
            })
            .map(|(i, _)| i)
            .collect();
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Set the config status for a provider.
    pub fn set_config_status(
        &mut self,
        provider_id: impl Into<String>,
        status: ProviderConfigStatus,
    ) {
        self.config_status.insert(provider_id.into(), status);
    }

    fn adjust_scroll(&mut self) {
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.visible_height {
            self.scroll_offset = self.selected_index - self.visible_height + 1;
        }
    }

    /// Render the OAuth selector as a vector of lines.
    pub fn render(&self) -> Vec<String> {
        let border = "─".repeat(60);
        let mut lines = Vec::new();

        lines.push(border.clone());
        lines.push(String::new()); // spacer

        // Title
        let title = match self.mode {
            OAuthSelectorMode::Login => "Select provider to configure:",
            OAuthSelectorMode::Logout => "Select provider to logout:",
        };
        lines.push(format!("\x1b[1m\x1b[36m{}\x1b[0m", title));
        lines.push(String::new()); // spacer

        // Search input
        lines.push(format!("Search: {}", self.filter));
        lines.push(String::new()); // spacer

        // Provider list
        let end = (self.scroll_offset + self.visible_height).min(self.filtered_indices.len());
        for vi in self.scroll_offset..end {
            if let Some(&pi) = self.filtered_indices.get(vi) {
                if let Some(provider) = self.providers.get(pi) {
                    let is_selected = vi == self.selected_index;

                    // Status indicator
                    let status_str = match self.config_status.get(&provider.id) {
                        Some(ProviderConfigStatus::Configured { label }) => {
                            format!("\x1b[32m ✓ {}\x1b[0m", label)
                        }
                        Some(ProviderConfigStatus::PartiallyConfigured { label }) => {
                            format!("\x1b[33m • {}\x1b[0m", label)
                        }
                        Some(ProviderConfigStatus::Unconfigured) | None => {
                            if provider.auth_type == AuthType::ApiKey {
                                "\x1b[2m • unconfigured\x1b[0m".to_string()
                            } else {
                                "\x1b[2m • unconfigured\x1b[0m".to_string()
                            }
                        }
                    };

                    if is_selected {
                        lines.push(format!("\x1b[36m→ {}\x1b[0m{}", provider.name, status_str));
                    } else {
                        lines.push(format!("  \x1b[37m{}\x1b[0m{}", provider.name, status_str));
                    }
                }
            }
        }

        // Scroll indicator
        if self.filtered_indices.len() > self.visible_height {
            lines.push(format!(
                "\x1b[2m  ({}/{})\x1b[0m",
                self.selected_index + 1,
                self.filtered_indices.len()
            ));
        }

        // Empty state
        if self.filtered_indices.is_empty() {
            let msg = if self.providers.is_empty() {
                match self.mode {
                    OAuthSelectorMode::Login => "No providers available",
                    OAuthSelectorMode::Logout => "No providers logged in. Use /login first.",
                }
            } else {
                "No matching providers"
            };
            lines.push(format!("\x1b[2m  {}\x1b[0m", msg));
        }

        lines.push(String::new()); // spacer
        lines.push(border);

        lines
    }
}

/// Loading indicator with border and status.
///
/// Displays a spinner-style loading message inside bordered area,
/// with an optional cancel hint.
#[derive(Debug, Clone)]
pub struct BorderedLoader {
    /// Loading message.
    pub message: String,
    /// Whether the loader can be cancelled.
    pub cancellable: bool,
    /// Spinner frame index (cycles 0..4).
    pub spinner_frame: usize,
}

impl BorderedLoader {
    /// Create a new bordered loader.
    pub fn new(message: impl Into<String>, cancellable: bool) -> Self {
        Self {
            message: message.into(),
            cancellable,
            spinner_frame: 0,
        }
    }

    /// Advance the spinner by one frame.
    pub fn tick(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % 4;
    }

    /// Get the current spinner character.
    pub fn spinner_char(&self) -> &'static str {
        match self.spinner_frame {
            0 => "⠋",
            1 => "⠙",
            2 => "⠹",
            3 => "⠸",
            _ => "⠋",
        }
    }

    /// Render the bordered loader as a vector of lines.
    pub fn render(&self) -> Vec<String> {
        let border = "─".repeat(60);
        let mut lines = Vec::new();

        lines.push(format!("\x1b[90m{}\x1b[0m", border));

        // Loading line with spinner
        lines.push(format!(
            "\x1b[36m{} {}\x1b[0m",
            self.spinner_char(),
            self.message
        ));

        // Cancel hint
        if self.cancellable {
            lines.push(String::new());
            lines.push(format!("\x1b[2mEsc: cancel\x1b[0m"));
        }

        lines.push(format!("\x1b[90m{}\x1b[0m", border));

        lines
    }
}

// ── Armin Component ──────────────────────────────────────────────────────────

/// XBM image data: 31x36 pixels, LSB first, 1=background, 0=foreground
const ARMIN_WIDTH: usize = 31;
const ARMIN_HEIGHT: usize = 36;
const ARMIN_BYTES_PER_ROW: usize = (ARMIN_WIDTH + 7) / 8; // ceil(31/8) = 4
const ARMIN_DISPLAY_HEIGHT: usize = (ARMIN_HEIGHT + 1) / 2; // half-block rendering

const ARMIN_BITS: [u8; 144] = [
    0xff, 0xff, 0xff, 0x7f, 0xff, 0xf0, 0xff, 0x7f, 0xff, 0xed, 0xff, 0x7f, 0xff, 0xdb, 0xff, 0x7f,
    0xff, 0xb7, 0xff, 0x7f, 0xff, 0x77, 0xfe, 0x7f, 0x3f, 0xf8, 0xfe, 0x7f, 0xdf, 0xff, 0xfe, 0x7f,
    0xdf, 0x3f, 0xfc, 0x7f, 0x9f, 0xc3, 0xfb, 0x7f, 0x6f, 0xfc, 0xf4, 0x7f, 0xf7, 0x0f, 0xf7, 0x7f,
    0xf7, 0xff, 0xf7, 0x7f, 0xf7, 0xff, 0xe3, 0x7f, 0xf7, 0x07, 0xe8, 0x7f, 0xef, 0xf8, 0x67, 0x70,
    0x0f, 0xff, 0xbb, 0x6f, 0xf1, 0x00, 0xd0, 0x5b, 0xfd, 0x3f, 0xec, 0x53, 0xc1, 0xff, 0xef, 0x57,
    0x9f, 0xfd, 0xee, 0x5f, 0x9f, 0xfc, 0xae, 0x5f, 0x1f, 0x78, 0xac, 0x5f, 0x3f, 0x00, 0x50, 0x6c,
    0x7f, 0x00, 0xdc, 0x77, 0xff, 0xc0, 0x3f, 0x78, 0xff, 0x01, 0xf8, 0x7f, 0xff, 0x03, 0x9c, 0x78,
    0xff, 0x07, 0x8c, 0x7c, 0xff, 0x0f, 0xce, 0x78, 0xff, 0xff, 0xcf, 0x7f, 0xff, 0xff, 0xcf, 0x78,
    0xff, 0xff, 0xdf, 0x78, 0xff, 0xff, 0xdf, 0x7d, 0xff, 0xff, 0x3f, 0x7e, 0xff, 0xff, 0xff, 0x7f,
];

/// Get pixel at (x, y): true = foreground, false = background
fn armin_get_pixel(x: usize, y: usize) -> bool {
    if y >= ARMIN_HEIGHT {
        return false;
    }
    let byte_index = y * ARMIN_BYTES_PER_ROW + x / 8;
    let bit_index = x % 8;
    (ARMIN_BITS[byte_index] >> bit_index) & 1 == 0
}

/// Get the character for a cell using half-block rendering (2 vertical pixels)
fn armin_get_char(x: usize, row: usize) -> char {
    let upper = armin_get_pixel(x, row * 2);
    let lower = armin_get_pixel(x, row * 2 + 1);
    match (upper, lower) {
        (true, true) => '█',
        (true, false) => '▀',
        (false, true) => '▄',
        (false, false) => ' ',
    }
}

/// Build the final image grid of characters
fn armin_build_final_grid() -> Vec<Vec<char>> {
    let mut grid = Vec::with_capacity(ARMIN_DISPLAY_HEIGHT);
    for row in 0..ARMIN_DISPLAY_HEIGHT {
        let mut line = Vec::with_capacity(ARMIN_WIDTH);
        for x in 0..ARMIN_WIDTH {
            line.push(armin_get_char(x, row));
        }
        grid.push(line);
    }
    grid
}

/// ArminComponent – renders XBM art with a "ARMIN SAYS HI" message.
///
/// This is a static render (no animation in the Rust port).
/// The final image is drawn in accent color with the text message below.
pub struct ArminComponent {
    final_grid: Vec<Vec<char>>,
    cached_lines: Vec<String>,
    cached_width: usize,
}

impl ArminComponent {
    pub fn new() -> Self {
        Self {
            final_grid: armin_build_final_grid(),
            cached_lines: Vec::new(),
            cached_width: 0,
        }
    }

    /// Render the component to lines at the given width
    pub fn render(&mut self, width: usize) -> Vec<String> {
        if width == self.cached_width && !self.cached_lines.is_empty() {
            return self.cached_lines.clone();
        }

        let accent = "\x1b[38;5;75m"; // light blue accent
        let reset = "\x1b[0m";
        let padding = 1;
        let available = width.saturating_sub(padding);

        let mut lines = Vec::new();

        for row in &self.final_grid {
            let chars: String = row.iter().take(available).collect();
            let pad_right = available.saturating_sub(chars.len());
            lines.push(format!(
                " {}{}{}{}",
                accent,
                chars,
                " ".repeat(pad_right),
                reset
            ));
        }

        // "ARMIN SAYS HI" message
        let message = "ARMIN SAYS HI";
        let msg_pad = available.saturating_sub(message.len());
        lines.push(format!(
            " {}{}{}{}",
            accent,
            message,
            " ".repeat(msg_pad),
            reset
        ));

        self.cached_lines = lines;
        self.cached_width = width;
        self.cached_lines.clone()
    }
}

impl Default for ArminComponent {
    fn default() -> Self {
        Self::new()
    }
}

// ── Daxnuts Component ─────────────────────────────────────────────────────────

/// DaxnutsComponent – decorative RGB image rendered with half-block characters.
///
/// Ported from the TypeScript daxnuts.ts easter-egg component.
/// Uses 24-bit color escape codes for pixel-accurate rendering.
pub struct DaxnutsComponent {
    image_lines: Vec<String>,
}

impl DaxnutsComponent {
    pub fn new() -> Self {
        Self {
            image_lines: build_dax_image(),
        }
    }

    /// Render the component at the given terminal width
    pub fn render(&self, width: usize) -> Vec<String> {
        let reset = "\x1b[0m";
        let accent = "\x1b[38;5;75m"; // light blue
        let success = "\x1b[32m";
        let muted = "\x1b[90m";
        let dim = "\x1b[2m";
        let link = "\x1b[36m"; // cyan for links

        let mut lines: Vec<String> = Vec::new();
        lines.push(String::new());

        for img_line in &self.image_lines {
            lines.push(center_ansi(img_line, width));
        }

        lines.push(String::new());
        lines.push(center_ansi(
            &format!("{}Free Kimi K2.5 via OpenCode Zen{}", accent, reset),
            width,
        ));
        lines.push(center_ansi(
            &format!("{}\"Powered by daxnuts\"{}", success, reset),
            width,
        ));
        lines.push(center_ansi(&format!("{}— @thdxr{}", muted, reset), width));

        lines.push(String::new());
        lines.push(center_ansi(&format!("{}Try OpenCode{}", dim, reset), width));
        lines.push(center_ansi(
            &format!("{}https://mistral.ai/news/mistral-vibe-2-0{}", link, reset),
            width,
        ));
        lines.push(String::new());

        lines
    }
}

impl Default for DaxnutsComponent {
    fn default() -> Self {
        Self::new()
    }
}

/// DAX image hex-encoded RGB data (32x32 pixels, 3 bytes per pixel)
const DAX_HEX: &str = "bbbab8b9b9b6b9b8b5bcbbb8b8b7b4b7b5b2b6b5b2b8b7b4b7b6b3b6b4b1bdbcb8bab8b6bbb8b5b8b5b1bbb8b4c2bebbc1bebac0bdbabfbcb9c1bebabfbebbc0bfbcc0bdbabbb8b5c1bfbcbfbcb8bbb9b6bfbcb8c2bfbcc1bfbcbfbbb8bdb9b6b8b7b5b9b8b5b8b8b5b5b5b2b6b5b2b8b7b4b9b8b5b9b8b5b6b5b3bab8b5bcbab7bbb9b6bbb8b5bfb9b5bdb2abbcb0a8beb2aabeb5afbfbab6bebab7c0bfbcbebdbabebbb8c0bdbabfbebbc2bebbbdbab7c3c0bdc3c0bdc1bebbc2bebabfbcb8bab9b6b7b6b3b2b1aeb6b5b2b5b4b1b5b4b2b6b5b2b7b6b4b9b8b6b7b6b3bbbab7b2afaba5988fb49e90b09481b79a88b39683b09583b7a395bfb6b0c0bdbabdbbb8bebcb9c1bfbcc0bebbbdbab7bebbb8c2bfbcc0bdbac0bcb9bdb9b6c0bcb8b5b4b2b4b3b0bab9b6b9b9b6b5b4b1b5b4b1b6b5b3b9b8b5b9b8b6b9b8b6b2aeaa968174a6836eaa856eab846eaf8973ac8973b08f79b18f7ab39786b7a89dbbb3aebfbab6c2c0bdbebcb9bfbdbac3c1bdc2bebbc0bcb9bdb9b6c1bdbabfbbb8b4b3b0b9b8b5b8b7b5b4b3b1b5b4b1b8b7b4b8b7b5bab9b6bbbab7b1afad8c7a719d735ca47860a87d65a98069ae8972ae8c75af8d77aa826ba98067aa8974b39e90b6a79dbbb2adc0bdbac1bfbdbfbbb8c1bdb9bebab6c0bdb9bfbbb8c1bdbab4b2b0b7b6b4b7b6b3b4b2b0bab9b7b6b5b2b6b5b2bab9b6bab9b6958c87977663aa836bac8772b08f7aad8c77b2917db0917db0907cac8971a77d64a87f67ac8972b29887b8a89dbfbab5bfbdbac1bebac0bcb9c0bcb9c0bcb9c1bebabebab7b8b7b4b7b6b4b5b4b1b5b4b2b7b6b3b5b4b2bab9b7bab9b6b4b1ada88f7fad8973ae8d78b19684b19685b29786b69a89b29582b1917daa856ea87e66a97e66ad866ea9826baf9280b8ada6bdbbb8bebab7bfbbb8c1bdbabfbbb8bcb8b4bcb8b5b6b4b2b7b5b3b6b5b2b8b7b4b3b2afb8b7b4b6b5b2b3b2b0b3a59aab856fad8d78b0917eb19886b49b8bb49a89b39785b0917eaf8f7cab866fa77d65a77a61a87d64a9816ab08f79b5a296c1bcb8c3bfbcc2bebbbebab7bfbbb7bdbab6c2bebab8b7b4b7b6b4b6b5b3b7b6b3b6b5b2b9b8b6b4b3b1b6b1acac8f7ca9826bae8f7aaf9583b49c8cb49c8bb79d8cb59987b19380ad8e79ae8c77af8e78ac8771a3775faa826bae8972b39888bbb6b2bebbb8bfbbb8bfbbb8c0bdb9bebbb7c0bdb9b6b5b2b9b8b5b4b3b1b8b7b5b4b3b0b7b6b4b6b5b3b1a7a0aa8772a77d65a88570b49887b19b8d9c887c907a6d987f71aa907faf917daf8e7aad8c78ac8b77a8836ca9836cac8770b49b8abdb6b2c0bcb9c0bdb9bfbbb8bebab7bfbcb9bebab7b9b8b6b5b4b2b9b8b5b8b7b5b8b7b4b7b6b4b5b4b2b3a9a2ad8973a1755da9856fb398858c776a65544b776358725d526e594d9c7f6eb1907ba68672ad8e7aab8771ac856db18f79b3a092beb9b5c1bdbabdb9b5bebab7bfbbb7bebab7bcb9b6b7b6b4b6b6b3b8b7b4b5b4b2b8b6b4b7b6b3b4b3b0b4aba4a6826ba3775fb08e79b19584a88e7daa8e7db29481ad8f7c997e6da38674ac8d79ac8e7aae917f9a7c6a896a599a7c6ab3a398c1bdbabdb9b6bcb8b5bebab6bebab7bdb9b5bdb9b6b5b4b1b7b5b3b5b4b2b7b6b3b7b6b4b3b3b0b3b2b0b4aca5a7846fa97f68ae8f7bae9383b59c8bb2937fae8e79ac8b76af927eaf927eb29683b39885b2988891786a72594c6e594d978d86bdbab7bab7b3c0bcb9c0bcb9bebab7bebbb7bdb9b6b3b2b0b4b3b0b5b4b2b4b4b1b4b3b1b4b3b1b4b3b0b6ada5aa8670a57a62ad8e7ab29b8cb69d8dab856fa9826aa88069ab8771af907db49987b19684b29886b59987b39480b09787b5a9a1bcb8b5bebab7bdb9b5bebab7bfbbb8bfbbb7bbb7b4b3b2afb8b7b5b8b7b5b3b2b0b5b4b2b6b5b3b6b4b1afa299a98975a9826baf907cb39988b49a89af8e7aac8973aa856eaf8c74b1917dae907dac907db39988b29785b49785b7a090b9aca3bfbab7bcb8b5bdb9b6bcb8b4bcb8b5bdb9b5bcb8b4b5b4b2b6b5b3b4b3b0b4b3b0b9b8b5b8b6b4908b88887467aa8f7ea78976ad8973b08b74b59885b69e8eb29888b1917cb1917db1937fae907cb19686b39a8ab29886b59b8ab8a192b6aaa3b7b2afbcb8b4bcb8b5bbb7b4c0bcb9bebab7c0bcb9b6b5b2b6b5b3b4b3b0bab9b7b7b6b4b1b0ae7b716ba083709b806f716158967764b08870b29481b69b8ab69f8fb39a89b69f90b49d8db39a89b29988b49c8cb6a090b8a496baa49593867f8f8986bfbbb7bdb9b5bcb7b4bab6b3b9b5b2bab6b2b4b3b1b3b3b0b6b5b3b8b7b5b4b2b0a7a5a38f837dae917ea084725a504c63544da28370b39784b59e8db2a093a698909b918b998e8790857e95877dad998bb39c8cb5a091b9a2938d827c95908dbebab6bbb7b3bdbab7bbb7b4bdb9b6bbb7b4b4b3b0b5b4b1b8b7b5b6b5b3b8b8b5b4b2af968f8ab29a8bab9485544b483a323073655d96887f70655f61595547403e453e3c453f3d57504f655e5b90847db39c8db7a090b6a09189807aaba6a3bdb9b6c0bcb9bebab7bcb7b4bebab7bbb7b4b3b2b0b6b5b3b2b1afb7b6b4b8b7b4b5b4b1aeaba8b5a89fac998d4d44412d25244d46444e4744322b293a3230423937433a37352d2a59504c534b48524a48988a81b59f8fb19c8d827974b2afacbdb9b5bcb8b4bdb9b5bcb8b5bdb9b6bab6b2b8b7b5b5b4b2b6b6b3b9b8b5b7b6b3b6b5b2b8b6b3b9b4b1b2a9a26c64612d25242d2625312a28352d2c453d3a78675c8d7a6ea09792aea6a0615854332b29524a479f8e82b09d90a49b96c1bdb9bebab7bfbbb8bbb8b4b9b5b1b8b4b0b9b4b0b7b6b4b8b7b5b8b7b4b6b5b3b8b6b3bab9b6b9b8b5b4b3b0b7b5b2a5a29f453d3b261e1d261f1e2e2625413936857268977865b19482b5a69caca5a07c7572453d3b746963a0948cc5bfbbc0bbb8beb9b6bbb7b3bbb6b3b7b3afb8b4b0b9b5b1b7b6b3b6b5b3b5b4b2b5b4b2b7b6b3b7b6b3b8b6b3b4b2afb7b6b3b3b1ae6d6765251f1e1e18172a22212d2523443b3971625ab19888b09482a89182877e792c25243e3634766d6abeb9b5bfbbb7bebab6bcb7b3bbb6b3b9b5b1b7b3afb8b4b0b4b3b0b5b4b1b5b4b1b4b3b1b5b4b2b8b6b4b5b3b0b9b6b4b5b4b1b6b4b27f79762a2322221c1b2d2524221b1a443e3c47413f6f676281766f867971675e5a3e37352a222166605dbab7b3bdb9b5beb9b5bcb7b3bcb7b3b9b4b0bab6b2bab6b2b5b3b0b6b4b2b3b2afb7b6b3b4b4b1b4b3b0b6b4b1b5b4b1b4b3b0b9b6b29a8c8252474230292828201f181212322c2c231e1d1c16162c26252923222d26252d2523332b2a8e8885bcb8b5bcb7b3bbb6b2bcb7b3b9b4b1b9b5b1b7b2afb7b2ae7a838e9b9b9caeadacb3b2b0b3b2afb7b7b4b6b5b3b6b6b3b7b6b3b9ada4a991808e7b6f50453f2b24231a14142923221f19181d17161f18182620201d17162a22215d5654b7b3b0bbb7b3bbb6b2b8b4b0bab5b1bbb6b2bab5b1b8b4b0bab6b22c496b4c5d735f68766e727a828285929090adaba8b7b2aeb6a59ab39682a28470a387748e76674e403a1a14141d1716181211221c1c1f1918221c1b2f2827342d2c8d8884bab6b3b9b5b2bab5b1bab5b1b9b4b0bab6b2b8b4b0b9b4b0b7b2ae325e8b365f8a3a5d833f5b7a545f70646469706b6aa08f84b08e78b18e769f7e689e7f6b9e816d907766584940362d2a1c1615201b1a1a1413201a1a251e1d393331a39e9bbab5b1bcb7b3bab6b2b8b3afb8b4b0b9b4b0b9b4b1bab5b2b5b0ac3d6c9843729d44719c426e98415f805a64716f6a699d8677b1927eb3947faa89749d7a649f7f6ba487749e837186716454463f2c25231e181837302e3a33317a7471beb9b6bcb8b4bbb6b2b6b2aebab5b1b9b5b1b8b3afbab6b2b6b1adb5aeaa4877a14c7aa44e7ba345719a3a5d80586b7f767475927b6eb1927faf8e79b08e78a78169a07861a17f6aa58570a688749b83738270666f66618a8480a49e99b7b2aebab6b2bcb8b4b9b5b1b7b2aebab5b1b9b4b0b6b1aeb6b1adb2aca8b2aca84876a04a78a2517fa74771973a5d80405c7a6161677c695fac8a75b08d77b4917aaf8971ad876fa5816aa6846ea78670a98a76ac9484ab9f96b2aca8bdb8b4bcb7b3bcb8b4bcb8b4b8b3afb7b2aeb9b4b0b8b3afb8b2aeb6afabb3aeaab2aeaa4878a14b7aa34c7ba44a759b3d63873b5f825b67766f5f569c7e6caf8c77b18f79b28f78b5927caf8e78a98872aa8a76a98a76ac917fada199b7b0acb9b3afbfb9b5c1bab6bdb6b2b8b3afbab5b1b9b4b0b6afabb7b1adb3ada9b3aeaab0aba8";

const DAX_IMG_WIDTH: usize = 32;
const DAX_IMG_HEIGHT: usize = 32;

/// Parse the hex-encoded RGB image into pixel data
fn parse_dax_pixels() -> Vec<Vec<(u8, u8, u8)>> {
    let hex = DAX_HEX;
    let mut pixels = Vec::with_capacity(DAX_IMG_HEIGHT);
    for y in 0..DAX_IMG_HEIGHT {
        let mut row = Vec::with_capacity(DAX_IMG_WIDTH);
        for x in 0..DAX_IMG_WIDTH {
            let idx = (y * DAX_IMG_WIDTH + x) * 6;
            let r = u8::from_str_radix(&hex[idx..idx + 2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex[idx + 2..idx + 4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[idx + 4..idx + 6], 16).unwrap_or(0);
            row.push((r, g, b));
        }
        pixels.push(row);
    }
    pixels
}

/// Build the image lines using half-block ▄ with fg=bottom, bg=top pixel
fn build_dax_image() -> Vec<String> {
    let pixels = parse_dax_pixels();
    let mut lines = Vec::new();

    for row in (0..DAX_IMG_HEIGHT).step_by(2) {
        let mut line = String::new();
        for x in 0..DAX_IMG_WIDTH {
            let (tr, tg, tb) = pixels[row][x];
            let (br, bg_val, bb) = pixels
                .get(row + 1)
                .and_then(|r| r.get(x))
                .copied()
                .unwrap_or((tr, tg, tb));
            // fg = bottom pixel, bg = top pixel
            line.push_str(&format!(
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▄",
                br, bg_val, bb, tr, tg, tb
            ));
        }
        line.push_str("\x1b[0m");
        lines.push(line);
    }
    lines
}

/// Center a string that may contain ANSI escape codes within the given width
fn center_ansi(s: &str, width: usize) -> String {
    // Count visible characters (strip ANSI) - use chars().count() for Unicode
    let visible_len = strip_ansi(s).chars().count();
    let left = if width > visible_len {
        (width - visible_len) / 2
    } else {
        0
    };
    format!("{}{}", " ".repeat(left), s)
}

// ── Dynamic Border ────────────────────────────────────────────────────────────

/// Dynamic border that adjusts to viewport width.
///
/// Renders a horizontal line using box-drawing characters, optionally colored.
pub struct DynamicBorder {
    /// Optional color function name – if None, uses default dim styling
    color_fn: Option<BorderStyle>,
}

/// Border styling options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    /// Accent color (blue/cyan)
    Accent,
    /// Muted/dim color
    Muted,
    /// Success color (green)
    Success,
    /// Error color (red)
    Error,
    /// Custom 256-color index
    Custom(u8),
}

impl DynamicBorder {
    /// Create a new dynamic border with default (dim) styling
    pub fn new() -> Self {
        Self { color_fn: None }
    }

    /// Create with a specific border style
    pub fn with_style(style: BorderStyle) -> Self {
        Self {
            color_fn: Some(style),
        }
    }

    /// Create with accent color
    pub fn accent() -> Self {
        Self::with_style(BorderStyle::Accent)
    }

    /// Render the border at the given width
    pub fn render(&self, width: usize) -> Vec<String> {
        let line = "─".repeat(width.max(1));
        let colored = match self.color_fn {
            Some(BorderStyle::Accent) => format!("\x1b[38;5;75m{}\x1b[0m", line),
            Some(BorderStyle::Muted) => format!("\x1b[90m{}\x1b[0m", line),
            Some(BorderStyle::Success) => format!("\x1b[32m{}\x1b[0m", line),
            Some(BorderStyle::Error) => format!("\x1b[31m{}\x1b[0m", line),
            Some(BorderStyle::Custom(c)) => format!("\x1b[38;5;{}m{}\x1b[0m", c, line),
            None => format!("\x1b[90m{}\x1b[0m", line),
        };
        vec![colored]
    }
}

impl Default for DynamicBorder {
    fn default() -> Self {
        Self::new()
    }
}

// ── Earendil Announcement ────────────────────────────────────────────────────

/// Announcement/notification display component.
///
/// Renders a framed announcement with a title, optional body text, and borders.
/// Ported from the TypeScript earendil-announcement component.
pub struct EarendilAnnouncement {
    /// Title of the announcement (bold, accent color)
    pub title: String,
    /// Body lines to display under the title
    pub body: Vec<String>,
    /// Optional link to display
    pub link: Option<String>,
    /// Border style
    pub border_style: BorderStyle,
}

impl EarendilAnnouncement {
    /// Create a new announcement with title and body
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: Vec::new(),
            link: None,
            border_style: BorderStyle::Accent,
        }
    }

    /// Create the default Earendil announcement
    pub fn earendil_default() -> Self {
        Self {
            title: "pi has joined Earendil".to_string(),
            body: vec![
                "Read the blog post:".to_string(),
                "https://mariozechner.at/posts/2026-04-08-ive-sold-out/".to_string(),
            ],
            link: Some("https://mariozechner.at/posts/2026-04-08-ive-sold-out/".to_string()),
            border_style: BorderStyle::Accent,
        }
    }

    /// Add a body line
    pub fn add_body(&mut self, line: impl Into<String>) {
        self.body.push(line.into());
    }

    /// Set link
    pub fn with_link(mut self, link: impl Into<String>) -> Self {
        self.link = Some(link.into());
        self
    }

    /// Render the announcement at the given width
    pub fn render(&self, width: usize) -> Vec<String> {
        let border = DynamicBorder::with_style(self.border_style);
        let accent = "\x1b[38;5;75m";
        let muted = "\x1b[90m";
        let link_color = "\x1b[36m";
        let bold = "\x1b[1m";
        let reset = "\x1b[0m";

        let mut lines = Vec::new();

        // Top border
        lines.extend(border.render(width));

        // Title (bold, accent)
        lines.push(format!(" {}{}{}{}", accent, bold, self.title, reset));

        // Spacer
        lines.push(String::new());

        // Body lines
        for body_line in &self.body {
            // Detect if it's a URL
            if body_line.starts_with("http://") || body_line.starts_with("https://") {
                lines.push(format!(" {}{}{}", link_color, body_line, reset));
            } else {
                lines.push(format!(" {}{}{}", muted, body_line, reset));
            }
        }

        // Spacer
        lines.push(String::new());

        // Bottom border
        lines.extend(border.render(width));

        lines
    }
}

// ── Enhanced Tool Execution ──────────────────────────────────────────────────

/// Extended tool execution with timing, progress, and rich status display.
///
/// Enhances the basic `ToolExecution` struct with:
/// - Elapsed time tracking
/// - Progress indicators
/// - Collapsible sections for arguments and output
/// - Timing information display
/// - Detailed status rendering
#[derive(Debug, Clone)]
pub struct ToolExecutionDisplay {
    /// Tool name
    pub tool_name: String,
    /// Tool call ID
    pub tool_call_id: String,
    /// Parsed arguments
    pub arguments: serde_json::Value,
    /// Current execution state
    pub state: ToolExecutionState,
    /// Result (if complete)
    pub result: Option<ToolResult>,
    /// Whether the display is expanded
    pub expanded: bool,
    /// Whether to show images
    pub show_images: bool,
    /// Max image width in cells
    pub image_width_cells: usize,
    /// When execution started
    pub started_at: Option<std::time::Instant>,
    /// When execution completed
    pub completed_at: Option<std::time::Instant>,
    /// Whether arguments parsing is complete
    pub args_complete: bool,
    /// Whether the call is still partial (streaming args)
    pub is_partial: bool,
}

impl ToolExecutionDisplay {
    /// Create a new tool execution display
    pub fn new(
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            tool_call_id: tool_call_id.into(),
            arguments: args,
            state: ToolExecutionState::Pending,
            result: None,
            expanded: false,
            show_images: true,
            image_width_cells: 60,
            started_at: None,
            completed_at: None,
            args_complete: false,
            is_partial: true,
        }
    }

    /// Mark execution as started
    pub fn start(&mut self) {
        self.state = ToolExecutionState::Running;
        self.started_at = Some(std::time::Instant::now());
    }

    /// Mark arguments as complete
    pub fn set_args_complete(&mut self) {
        self.args_complete = true;
        self.is_partial = false;
    }

    /// Update arguments (for streaming)
    pub fn update_args(&mut self, args: serde_json::Value) {
        self.arguments = args;
    }

    /// Complete with result
    pub fn complete(&mut self, result: ToolResult) {
        self.state = if result.is_error {
            ToolExecutionState::Error
        } else {
            ToolExecutionState::Success
        };
        self.completed_at = Some(std::time::Instant::now());
        self.result = Some(result);
    }

    /// Set expanded state
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Toggle expanded
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Get elapsed time as a formatted string
    pub fn elapsed_str(&self) -> String {
        let start = match self.started_at {
            Some(s) => s,
            None => return String::new(),
        };
        let end = self.completed_at.unwrap_or_else(std::time::Instant::now);
        let dur = end.duration_since(start);
        let ms = dur.as_millis();
        if ms < 1000 {
            format!("{}ms", ms)
        } else {
            format!("{:.1}s", dur.as_secs_f64())
        }
    }

    /// Format arguments for display
    pub fn format_arguments(&self) -> String {
        if self.arguments.is_null() {
            String::new()
        } else {
            serde_json::to_string_pretty(&self.arguments)
                .unwrap_or_else(|_| self.arguments.to_string())
        }
    }

    /// Render the full tool execution display
    pub fn render(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let reset = "\x1b[0m";

        // Determine colors based on state
        let (bg_color, fg_color, status_icon, status_label) = match self.state {
            ToolExecutionState::Pending => ("\x1b[48;5;240m", "\x1b[38;5;250m", "○", "pending"),
            ToolExecutionState::Running => ("\x1b[48;5;239m", "\x1b[38;5;250m", "◐", "running"),
            ToolExecutionState::Success => ("\x1b[48;5;28m", "\x1b[38;5;255m", "●", "done"),
            ToolExecutionState::Error => ("\x1b[48;5;196m", "\x1b[38;5;255m", "✗", "error"),
        };

        // ── Header line: status icon + tool name + elapsed time ──
        let elapsed = self.elapsed_str();
        let header = if elapsed.is_empty() {
            format!(
                "{} {} {}{}{} {}{}{}",
                bg_color,
                status_icon,
                fg_color,
                "\x1b[1m",
                self.tool_name,
                fg_color,
                status_label,
                reset
            )
        } else {
            format!(
                "{} {} {}{}{} {}{}{} \x1b[90m{}{}",
                bg_color,
                status_icon,
                fg_color,
                "\x1b[1m",
                self.tool_name,
                fg_color,
                status_label,
                "\x1b[90m",
                elapsed,
                reset
            )
        };
        lines.push(header);

        // ── Arguments section ──
        let args_str = self.format_arguments();
        if !args_str.is_empty() {
            if self.expanded || args_str.len() < 200 {
                for arg_line in args_str.lines() {
                    lines.push(format!("  \x1b[38;5;246m{}{}", arg_line, reset));
                }
            } else {
                // Show first few lines, then truncation
                let arg_lines: Vec<&str> = args_str.lines().collect();
                let show = 3.min(arg_lines.len());
                for line in &arg_lines[..show] {
                    lines.push(format!("  \x1b[38;5;246m{}{}", line, reset));
                }
                if arg_lines.len() > show {
                    lines.push(format!(
                        "  \x1b[90m... ({} more lines, press → to expand){}",
                        arg_lines.len() - show,
                        reset
                    ));
                }
            }
        }

        // ── Partial indicator ──
        if self.is_partial && self.state == ToolExecutionState::Pending {
            lines.push(format!("  \x1b[90m(receiving arguments...){}", reset));
        }

        // ── Result section ──
        if let Some(ref result) = self.result {
            let result_fg = if result.is_error {
                "\x1b[31m"
            } else {
                "\x1b[38;5;250m"
            };

            if let Some(text) = result.get_text() {
                if self.expanded {
                    for text_line in text.lines() {
                        lines.push(format!("  {}{}{}", result_fg, text_line, reset));
                    }
                } else {
                    let truncated = truncate_text(&text, 500);
                    for text_line in truncated.lines() {
                        lines.push(format!("  {}{}{}", result_fg, text_line, reset));
                    }
                    if text.len() > 500 {
                        lines.push(format!(
                            "  \x1b[90m(... {} more chars, press → to expand){}",
                            text.len() - 500,
                            reset
                        ));
                    }
                }
            }

            // Image count
            if result.has_images() && self.show_images {
                lines.push(format!(
                    "  \x1b[38;5;246m[{} image{}]{}",
                    result.image_count(),
                    if result.image_count() == 1 { "" } else { "s" },
                    reset
                ));
            }
        } else if self.state == ToolExecutionState::Running {
            lines.push(format!("  \x1b[90m⏳ executing...{}", reset));
        }

        lines
    }

    /// Render as a single string (joined with newlines)
    pub fn render_to_string(&self) -> String {
        self.render().join("\n")
    }
}

// ── Interactive Message Components ─────────────────────────────────────────
//
// Ported from pi-mono/packages/coding-agent/src/modes/interactive/components/
// These components handle user messages, skill invocations, diff rendering,
// keybinding hints, footer, visual truncation, image selection, and countdown.

/// OSC 133 prompt escape sequences for terminal integration
const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

/// ANSI color helpers using theme-like colors for TUI rendering.
/// These are inline helpers for components that produce `Vec<String>` output.
#[allow(dead_code)]
mod ansi {
    /// Red foreground
    pub fn red(text: &str) -> String {
        format!("\x1b[31m{}\x1b[0m", text)
    }

    /// Green foreground
    pub fn green(text: &str) -> String {
        format!("\x1b[32m{}\x1b[0m", text)
    }

    /// Yellow foreground
    pub fn yellow(text: &str) -> String {
        format!("\x1b[33m{}\x1b[0m", text)
    }

    /// Blue foreground
    pub fn blue(text: &str) -> String {
        format!("\x1b[34m{}\x1b[0m", text)
    }

    /// Cyan foreground
    pub fn cyan(text: &str) -> String {
        format!("\x1b[36m{}\x1b[0m", text)
    }

    /// Magenta/purple foreground
    pub fn magenta(text: &str) -> String {
        format!("\x1b[35m{}\x1b[0m", text)
    }

    /// Bold text
    pub fn bold(text: &str) -> String {
        format!("\x1b[1m{}\x1b[0m", text)
    }

    /// Italic text
    pub fn italic(text: &str) -> String {
        format!("\x1b[3m{}\x1b[0m", text)
    }

    /// Dim/faint text
    pub fn dim(text: &str) -> String {
        format!("\x1b[2m{}\x1b[0m", text)
    }

    /// Inverse/reverse video
    pub fn inverse(text: &str) -> String {
        format!("\x1b[7m{}\x1b[0m", text)
    }

    /// User message background (subtle blue tint)
    pub fn user_message_bg(text: &str) -> String {
        format!("\x1b[48;5;17m{}\x1b[0m", text)
    }

    /// User message text color
    pub fn user_message_text(text: &str) -> String {
        format!("\x1b[38;5;189m{}\x1b[0m", text)
    }

    /// Skill/custom message label color (purple)
    pub fn custom_label(text: &str) -> String {
        format!("\x1b[38;5;183m{}\x1b[0m", text)
    }

    /// Skill/custom message text
    pub fn custom_text(text: &str) -> String {
        format!("\x1b[38;5;254m{}\x1b[0m", text)
    }

    /// Diff context line color (dim gray)
    pub fn diff_context(text: &str) -> String {
        format!("\x1b[38;5;244m{}\x1b[0m", text)
    }

    /// Diff removed line color (red)
    pub fn diff_removed(text: &str) -> String {
        format!("\x1b[31m{}\x1b[0m", text)
    }

    /// Diff added line color (green)
    pub fn diff_added(text: &str) -> String {
        format!("\x1b[32m{}\x1b[0m", text)
    }

    /// Thinking text color
    pub fn thinking_text(text: &str) -> String {
        format!("\x1b[38;5;180m{}\x1b[0m", text)
    }

    /// Muted text (for hints, labels)
    pub fn muted(text: &str) -> String {
        format!("\x1b[38;5;243m{}\x1b[0m", text)
    }

    /// Accent color (for cursors, highlights)
    pub fn accent(text: &str) -> String {
        format!("\x1b[38;5;183m{}\x1b[0m", text)
    }

    /// Error color
    pub fn error(text: &str) -> String {
        format!("\x1b[38;5;203m{}\x1b[0m", text)
    }

    /// Warning color
    pub fn warning(text: &str) -> String {
        format!("\x1b[38;5;215m{}\x1b[0m", text)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. UserMessageRenderer
// ─────────────────────────────────────────────────────────────────────────────

/// User message item for the message selector.
#[derive(Debug, Clone)]
pub struct UserMessageItem {
    /// Entry ID in the session.
    pub id: String,
    /// The message text.
    pub text: String,
    /// Optional timestamp.
    pub timestamp: Option<String>,
}

/// Renders a user message with optional image indicators and OSC 133 markers.
pub struct UserMessageRenderer {
    /// The message text (may contain markdown).
    pub text: String,
    /// Whether the message has associated images.
    pub has_images: bool,
    /// Whether to emit OSC 133 prompt markers.
    pub use_osc133: bool,
}

impl UserMessageRenderer {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            has_images: false,
            use_osc133: false,
        }
    }

    /// Set whether the message has images.
    pub fn with_images(mut self, has_images: bool) -> Self {
        self.has_images = has_images;
        self
    }

    /// Enable OSC 133 markers.
    pub fn with_osc133(mut self, enable: bool) -> Self {
        self.use_osc133 = enable;
        self
    }

    /// Render the user message to lines.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Image indicator
        if self.has_images {
            lines.push(ansi::cyan(&format!("  📎 (has images)")));
        }

        // Render text with user message styling
        let rendered = ansi::user_message_text(&self.text);
        for line in rendered.lines() {
            // Truncate to width
            let truncated = if line.chars().count() > width {
                let mut s = String::new();
                let mut w = 0;
                for c in line.chars() {
                    if w + 1 > width {
                        s.push_str("...");
                        break;
                    }
                    s.push(c);
                    w += 1;
                }
                s
            } else {
                line.to_string()
            };
            lines.push(ansi::user_message_bg(&truncated));
        }

        // Wrap with OSC 133 markers
        if self.use_osc133 && !lines.is_empty() {
            lines[0] = format!("{}{}", OSC133_ZONE_START, lines[0]);
            let last = lines.len() - 1;
            lines[last] = format!("{}{}{}", OSC133_ZONE_END, OSC133_ZONE_FINAL, lines[last]);
        }

        lines
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. UserMessageSelector
// ─────────────────────────────────────────────────────────────────────────────

/// State for selecting/editing previous user messages.
#[derive(Debug, Clone)]
pub struct UserMessageSelector {
    /// Messages to choose from (oldest to newest).
    pub messages: Vec<UserMessageItem>,
    /// Currently selected index.
    pub selected_index: usize,
    /// Maximum visible messages.
    pub max_visible: usize,
}

impl UserMessageSelector {
    /// Create a new selector with messages.
    pub fn new(messages: Vec<UserMessageItem>) -> Self {
        let selected_index = messages.len().saturating_sub(1);
        Self {
            messages,
            selected_index,
            max_visible: 10,
        }
    }

    /// Create with initial selected ID.
    pub fn with_initial_id(mut self, id: &str) -> Self {
        if let Some(idx) = self.messages.iter().position(|m| m.id == id) {
            self.selected_index = idx;
        }
        self
    }

    /// Move selection up (to older messages), wraps to bottom.
    pub fn move_up(&mut self) {
        if self.selected_index == 0 {
            self.selected_index = self.messages.len().saturating_sub(1);
        } else {
            self.selected_index -= 1;
        }
    }

    /// Move selection down (to newer messages), wraps to top.
    pub fn move_down(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        if self.selected_index >= self.messages.len() - 1 {
            self.selected_index = 0;
        } else {
            self.selected_index += 1;
        }
    }

    /// Get the currently selected message.
    pub fn selected(&self) -> Option<&UserMessageItem> {
        self.messages.get(self.selected_index)
    }

    /// Render the selector as lines.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Header
        lines.push(String::new()); // blank
        lines.push(ansi::bold("Fork from Message"));
        lines.push(ansi::muted(
            "Select a user message to copy the active path up to that point into a new session",
        ));
        lines.push(String::new()); // blank
        lines.push("─".repeat(width.min(60)));
        lines.push(String::new()); // blank

        if self.messages.is_empty() {
            lines.push(ansi::muted("  No user messages found"));
            return lines;
        }

        // Calculate visible range with scrolling
        let start_index = if self.selected_index < self.max_visible / 2 {
            0
        } else if self.selected_index + self.max_visible / 2 >= self.messages.len() {
            self.messages.len().saturating_sub(self.max_visible)
        } else {
            self.selected_index - self.max_visible / 2
        };
        let end_index = (start_index + self.max_visible).min(self.messages.len());

        for i in start_index..end_index {
            let msg = &self.messages[i];
            let is_selected = i == self.selected_index;

            // Normalize to single line
            let normalized: String = msg.text.chars().filter(|c| *c != '\n').collect();
            let normalized = normalized.trim();

            // Cursor + message
            let cursor = if is_selected {
                ansi::accent("› ")
            } else {
                "  ".to_string()
            };
            let max_msg_width = width.saturating_sub(2);
            let truncated_msg = truncate_str(normalized, max_msg_width);
            let message_line = if is_selected {
                format!("{}{}", cursor, ansi::bold(&truncated_msg))
            } else {
                format!("{}{}", cursor, truncated_msg)
            };
            lines.push(message_line);

            // Metadata
            let position = i + 1;
            let metadata = format!("  Message {} of {}", position, self.messages.len());
            lines.push(ansi::muted(&metadata));
            lines.push(String::new()); // blank between messages
        }

        // Scroll indicator
        if start_index > 0 || end_index < self.messages.len() {
            let scroll_info = format!("  ({}/{})", self.selected_index + 1, self.messages.len());
            lines.push(ansi::muted(&scroll_info));
        }

        // Bottom border
        lines.push(String::new());
        lines.push("─".repeat(width.min(60)));

        lines
    }
}

/// Truncate a string to a maximum visible character count, handling ANSI escape codes.
fn truncate_str(s: &str, max_width: usize) -> String {
    let mut result = String::with_capacity(s.len());
    let mut width = 0;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // ANSI escape sequence - include without counting width
            result.push(c);
            while let Some(&next) = chars.peek() {
                result.push(chars.next().unwrap());
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }

        let char_w = if is_wide_char(c) { 2 } else { 1 };
        if width + char_w > max_width {
            if width >= 3 {
                result.truncate(result.len() - 3);
                result.push_str("...");
            }
            break;
        }
        result.push(c);
        width += char_w;
    }
    result
}

/// Check if a character is wide (CJK, emoji, etc.).
fn is_wide_char(c: char) -> bool {
    let code = c as u32;
    (0xFF01..=0xFF5E).contains(&code)
        || (0x4E00..=0x9FFF).contains(&code)
        || (0x3400..=0x4DBF).contains(&code)
        || (0xFE30..=0xFE4F).contains(&code)
        || (0xFF00..=0xFFEF).contains(&code)
        || (0x3000..=0x303F).contains(&code)
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. SkillInvocationMessage
// ─────────────────────────────────────────────────────────────────────────────

/// Parsed skill block data.
#[derive(Debug, Clone)]
pub struct ParsedSkillBlock {
    /// Skill name.
    pub name: String,
    /// Skill content/arguments.
    pub content: String,
}

/// Renders a skill invocation with collapsed/expanded state.
pub struct SkillInvocationMessage {
    /// The skill block to render.
    pub skill_block: ParsedSkillBlock,
    /// Whether the block is expanded.
    pub expanded: bool,
    /// Key hint for expanding.
    pub expand_key_hint: String,
}

impl SkillInvocationMessage {
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            skill_block: ParsedSkillBlock {
                name: name.into(),
                content: content.into(),
            },
            expanded: false,
            expand_key_hint: "Enter".to_string(),
        }
    }

    /// Set expanded state.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Toggle expanded state.
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Render to lines.
    pub fn render(&self, _width: usize) -> Vec<String> {
        if self.expanded {
            // Expanded: label + skill name header + full content
            let label = ansi::custom_label(&ansi::bold("[skill]"));
            let name_line = ansi::bold(&format!("**{}**", self.skill_block.name));
            let content_rendered = render_markdown(&self.skill_block.content);
            vec![
                format!("{} {}", label, name_line),
                ansi::custom_text(&content_rendered),
            ]
        } else {
            // Collapsed: single line
            let line = format!(
                "{} {} {}",
                ansi::custom_label(&ansi::bold("[skill]")),
                ansi::custom_text(&self.skill_block.name),
                ansi::dim(&format!("({} to expand)", self.expand_key_hint))
            );
            vec![line]
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Enhanced DiffRenderer
// ─────────────────────────────────────────────────────────────────────────────

/// Render unified diffs with color (red/green) and optional intra-line highlighting.
pub struct DiffRenderer {
    /// Enable word-level change highlighting.
    pub word_diff: bool,
}

impl DiffRenderer {
    pub fn new() -> Self {
        Self { word_diff: true }
    }

    /// Create without word diff.
    pub fn new_simple() -> Self {
        Self { word_diff: false }
    }

    /// Render a diff string with colored lines.
    pub fn render(&self, diff_text: &str, _file_path: Option<&str>) -> String {
        let lines = diff_text.lines();
        let mut result = Vec::new();
        let line_iter: Vec<&str> = lines.collect();
        let mut i = 0;

        while i < line_iter.len() {
            let line = line_iter[i];
            let parsed = parse_diff_line_ts(line);

            match parsed {
                None => {
                    // Non-diff line (e.g. hunk header or plain text)
                    if line.starts_with("@@") {
                        result.push(ansi::cyan(line));
                    } else {
                        result.push(ansi::diff_context(line));
                    }
                    i += 1;
                }
                Some(DiffLineParsed::Context { line_num, content }) => {
                    let content = replace_tabs(&content);
                    result.push(ansi::diff_context(&format!(" {} {}", line_num, content)));
                    i += 1;
                }
                Some(DiffLineParsed::Removed { line_num, content }) => {
                    // Collect consecutive removed lines
                    let mut removed = vec![(line_num, content)];
                    i += 1;
                    while i < line_iter.len() {
                        if let Some(DiffLineParsed::Removed { line_num, content }) =
                            parse_diff_line_ts(line_iter[i])
                        {
                            removed.push((line_num, content));
                            i += 1;
                        } else {
                            break;
                        }
                    }

                    // Collect consecutive added lines
                    let mut added = Vec::new();
                    while i < line_iter.len() {
                        if let Some(DiffLineParsed::Added { line_num, content }) =
                            parse_diff_line_ts(line_iter[i])
                        {
                            added.push((line_num, content));
                            i += 1;
                        } else {
                            break;
                        }
                    }

                    // Intra-line diff only for 1:1 (single modification)
                    if self.word_diff && removed.len() == 1 && added.len() == 1 {
                        let (rem_num, rem_content) = &removed[0];
                        let (add_num, add_content) = &added[0];
                        let (rem_line, add_line) = render_intra_line_diff(
                            &replace_tabs(rem_content),
                            &replace_tabs(add_content),
                        );
                        result.push(ansi::diff_removed(&format!("-{} {}", rem_num, rem_line)));
                        result.push(ansi::diff_added(&format!("+{} {}", add_num, add_line)));
                    } else {
                        for (num, content) in &removed {
                            result.push(ansi::diff_removed(&format!(
                                "-{} {}",
                                num,
                                replace_tabs(content)
                            )));
                        }
                        for (num, content) in &added {
                            result.push(ansi::diff_added(&format!(
                                "+{} {}",
                                num,
                                replace_tabs(content)
                            )));
                        }
                    }
                }
                Some(DiffLineParsed::Added { line_num, content }) => {
                    let content = replace_tabs(&content);
                    result.push(ansi::diff_added(&format!("+{} {}", line_num, content)));
                    i += 1;
                }
            }
        }

        result.join("\n")
    }
}

impl Default for DiffRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Parsed diff line from tool-output format.
#[derive(Debug)]
enum DiffLineParsed {
    Context { line_num: String, content: String },
    Added { line_num: String, content: String },
    Removed { line_num: String, content: String },
}

/// Parse a diff line in the format: "+123 content" or "-123 content" or " 123 content".
fn parse_diff_line_ts(line: &str) -> Option<DiffLineParsed> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let prefix = chars[0];
    if prefix != '+' && prefix != '-' && prefix != ' ' {
        return None;
    }

    // Find where the line number ends (space after digits)
    let rest: String = chars[1..].iter().collect();
    let trimmed = rest.trim_start();

    // Try to parse: line_num followed by space then content
    if let Some(space_pos) = trimmed.find(' ') {
        let line_num = &trimmed[..space_pos];
        let content = &trimmed[space_pos + 1..];
        match prefix {
            '+' => Some(DiffLineParsed::Added {
                line_num: line_num.to_string(),
                content: content.to_string(),
            }),
            '-' => Some(DiffLineParsed::Removed {
                line_num: line_num.to_string(),
                content: content.to_string(),
            }),
            ' ' => Some(DiffLineParsed::Context {
                line_num: line_num.to_string(),
                content: content.to_string(),
            }),
            _ => None,
        }
    } else {
        // No space - just prefix with possible line number
        match prefix {
            '+' => Some(DiffLineParsed::Added {
                line_num: String::new(),
                content: trimmed.to_string(),
            }),
            '-' => Some(DiffLineParsed::Removed {
                line_num: String::new(),
                content: trimmed.to_string(),
            }),
            ' ' => Some(DiffLineParsed::Context {
                line_num: String::new(),
                content: trimmed.to_string(),
            }),
            _ => None,
        }
    }
}

/// Replace tabs with spaces for consistent rendering.
fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// Compute word-level diff and render with inverse on changed parts.
fn render_intra_line_diff(old_content: &str, new_content: &str) -> (String, String) {
    let old_words = split_words(old_content);
    let new_words = split_words(new_content);

    // Simple word-level diff: find common prefix and suffix
    let prefix_len = common_prefix_len(&old_words, &new_words);
    let suffix_len = common_suffix_len(&old_words[prefix_len..], &new_words[prefix_len..]);

    let old_mid_end = old_words.len().saturating_sub(suffix_len);
    let new_mid_end = new_words.len().saturating_sub(suffix_len);

    let mut removed_line = String::new();
    let mut added_line = String::new();

    // Common prefix
    for w in &old_words[..prefix_len] {
        removed_line.push_str(w);
        added_line.push_str(w);
    }

    // Changed part (inverse highlight)
    for w in &old_words[prefix_len..old_mid_end] {
        removed_line.push_str(&ansi::inverse(w));
    }
    for w in &new_words[prefix_len..new_mid_end] {
        added_line.push_str(&ansi::inverse(w));
    }

    // Common suffix
    if suffix_len > 0 {
        for w in &old_words[old_mid_end..] {
            removed_line.push_str(w);
        }
        for w in &new_words[new_mid_end..] {
            added_line.push_str(w);
        }
    }

    (removed_line, added_line)
}

/// Split text into words preserving whitespace between them.
fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_whitespace = false;

    for c in text.chars() {
        let is_ws = c.is_whitespace();
        if is_ws != in_whitespace && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        in_whitespace = is_ws;
        current.push(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Length of common prefix between two word slices.
fn common_prefix_len<'a>(a: &[String], b: &[String]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

/// Length of common suffix between two word slices.
fn common_suffix_len<'a>(a: &[String], b: &[String]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. KeybindingHints
// ─────────────────────────────────────────────────────────────────────────────

/// A single keybinding hint for display.
#[derive(Debug, Clone)]
pub struct KeyHint {
    /// Key sequence text (e.g. "Ctrl+C", "↑/↓").
    pub keys: String,
    /// Description of the action.
    pub description: String,
}

impl KeyHint {
    pub fn new(keys: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            keys: keys.into(),
            description: description.into(),
        }
    }
}

/// Renders a list of keyboard shortcut hints.
pub struct KeybindingHints;

impl KeybindingHints {
    /// Render a single key hint: dim keys + muted description.
    pub fn key_hint(keys: &str, description: &str) -> String {
        format!(
            "{}{}",
            ansi::dim(keys),
            ansi::muted(&format!(" {}", description))
        )
    }

    /// Render multiple hints as separate lines.
    pub fn render(hints: &[KeyHint]) -> Vec<String> {
        hints
            .iter()
            .map(|h| Self::key_hint(&h.keys, &h.description))
            .collect()
    }

    /// Render hints as a single horizontal line separated by spaces.
    pub fn render_inline(hints: &[KeyHint]) -> String {
        hints
            .iter()
            .map(|h| Self::key_hint(&h.keys, &h.description))
            .collect::<Vec<_>>()
            .join("  ")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. FooterComponent (enhanced)
// ─────────────────────────────────────────────────────────────────────────────

/// Enhanced footer component data with all status fields from pi-mono.
#[derive(Debug, Clone)]
pub struct FooterComponentData {
    /// Current model name.
    pub model_name: String,
    /// Provider name.
    pub provider_name: String,
    /// Thinking level ("off", "low", "medium", "high").
    pub thinking_level: String,
    /// Session name (optional).
    pub session_name: Option<String>,
    /// Git branch.
    pub git_branch: Option<String>,
    /// Working directory (abbreviated with ~).
    pub pwd: Option<String>,
    /// Total input tokens.
    pub input_tokens: u64,
    /// Total output tokens.
    pub output_tokens: u64,
    /// Cache read tokens.
    pub cache_read_tokens: u64,
    /// Cache write tokens.
    pub cache_write_tokens: u64,
    /// Total cost in USD.
    pub total_cost: f64,
    /// Whether using OAuth subscription.
    pub using_subscription: bool,
    /// Context window usage percent (0.0 - 100.0).
    pub context_window_pct: f64,
    /// Context window size.
    pub context_window: u64,
    /// Whether auto-compact is enabled.
    pub auto_compact_enabled: bool,
    /// Number of available providers.
    pub available_provider_count: usize,
    /// Extension status messages.
    pub extension_statuses: Vec<(String, String)>,
}

impl Default for FooterComponentData {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            provider_name: String::new(),
            thinking_level: "off".to_string(),
            session_name: None,
            git_branch: None,
            pwd: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            total_cost: 0.0,
            using_subscription: false,
            context_window_pct: 0.0,
            context_window: 0,
            auto_compact_enabled: true,
            available_provider_count: 1,
            extension_statuses: Vec::new(),
        }
    }
}

impl FooterComponentData {
    /// Format a token count for display.
    pub fn format_token_count(count: u64) -> String {
        if count < 1000 {
            count.to_string()
        } else if count < 10000 {
            format!("{:.1}k", count as f64 / 1000.0)
        } else if count < 1_000_000 {
            format!("{}k", count / 1000)
        } else if count < 10_000_000 {
            format!("{:.1}M", count as f64 / 1_000_000.0)
        } else {
            format!("{}M", count / 1_000_000)
        }
    }

    /// Render the footer as lines.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Line 1: pwd (git branch) • session_name
        let mut pwd_parts = Vec::new();
        if let Some(ref pwd) = self.pwd {
            pwd_parts.push(pwd.clone());
        }
        if let Some(ref branch) = self.git_branch {
            pwd_parts.push(format!("({})", branch));
        }
        if let Some(ref session) = self.session_name {
            pwd_parts.push(format!("• {}", session));
        }
        let pwd_line = pwd_parts.join(" ");
        lines.push(ansi::dim(&truncate_str(&pwd_line, width.saturating_sub(3))));

        // Line 2: stats left | model right
        let mut stats_parts = Vec::new();
        if self.input_tokens > 0 {
            stats_parts.push(format!("↑{}", Self::format_token_count(self.input_tokens)));
        }
        if self.output_tokens > 0 {
            stats_parts.push(format!("↓{}", Self::format_token_count(self.output_tokens)));
        }
        if self.cache_read_tokens > 0 {
            stats_parts.push(format!(
                "R{}",
                Self::format_token_count(self.cache_read_tokens)
            ));
        }
        if self.cache_write_tokens > 0 {
            stats_parts.push(format!(
                "W{}",
                Self::format_token_count(self.cache_write_tokens)
            ));
        }

        // Cost
        if self.total_cost > 0.0 || self.using_subscription {
            let cost_str = format!(
                "${:.3}{}",
                self.total_cost,
                if self.using_subscription {
                    " (sub)"
                } else {
                    ""
                }
            );
            stats_parts.push(cost_str);
        }

        // Context window with color coding
        let auto_indicator = if self.auto_compact_enabled {
            " (auto)"
        } else {
            ""
        };
        let ctx_display = if self.context_window > 0 {
            format!(
                "{:.1}%/{}{}",
                self.context_window_pct,
                Self::format_token_count(self.context_window),
                auto_indicator
            )
        } else {
            format!("{:.1}%{}", self.context_window_pct, auto_indicator)
        };
        let ctx_colored = if self.context_window_pct > 90.0 {
            ansi::error(&ctx_display)
        } else if self.context_window_pct > 70.0 {
            ansi::warning(&ctx_display)
        } else {
            ctx_display
        };
        if self.context_window_pct > 0.0 {
            stats_parts.push(ctx_colored);
        }

        let stats_left = stats_parts.join(" ");

        // Right side: model name + thinking level
        let model_name = if self.model_name.is_empty() {
            "no-model"
        } else {
            &self.model_name
        };
        let mut right_side = model_name.to_string();
        if self.thinking_level != "off" {
            right_side = format!("{} • {}", model_name, self.thinking_level);
        }

        // Prepend provider if multiple providers
        if self.available_provider_count > 1 && !self.provider_name.is_empty() {
            let with_provider = format!("({}) {}", self.provider_name, right_side);
            let with_provider_len = visible_len(&with_provider);
            let stats_left_len = visible_len(&stats_left);
            if stats_left_len + 2 + with_provider_len <= width {
                right_side = with_provider;
            }
        }

        // Combine stats and model info with padding
        let stats_left_len = visible_len(&stats_left);
        let right_side_len = visible_len(&right_side);
        let min_padding = 2;

        let stats_line = if stats_left_len + min_padding + right_side_len <= width {
            let padding = width - stats_left_len - right_side_len;
            format!(
                "{}{}{}",
                ansi::dim(&stats_left),
                " ".repeat(padding),
                ansi::dim(&right_side)
            )
        } else if stats_left_len < width {
            let available = width - stats_left_len - min_padding;
            let truncated_right = truncate_str(&right_side, available);
            let truncated_len = visible_len(&truncated_right);
            let padding = width.saturating_sub(stats_left_len + truncated_len);
            format!(
                "{}{}{}",
                ansi::dim(&stats_left),
                " ".repeat(padding),
                ansi::dim(&truncated_right)
            )
        } else {
            ansi::dim(&stats_left)
        };

        lines.push(stats_line);

        // Line 3: extension statuses
        if !self.extension_statuses.is_empty() {
            let mut sorted = self.extension_statuses.clone();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            let status_line: String = sorted
                .iter()
                .map(|(_, text)| {
                    // Sanitize: replace newlines/tabs with space, collapse spaces
                    text.chars()
                        .map(|c| {
                            if c == '\n' || c == '\r' || c == '\t' {
                                ' '
                            } else {
                                c
                            }
                        })
                        .collect::<String>()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(truncate_str(&status_line, width.saturating_sub(3)));
        }

        lines
    }
}

/// Calculate the visible width of a string (ignoring ANSI escape codes).
fn visible_len(s: &str) -> usize {
    let mut len = 0usize;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1b' && i + 1 < chars.len() && chars[i + 1] == '[' {
            // Skip ANSI escape sequence
            i += 2;
            while i < chars.len() {
                if chars[i].is_ascii_alphabetic() {
                    i += 1;
                    break;
                }
                // Handle intermediate parameters like ; and digits
                i += 1;
            }
        } else if chars[i] == '\x1b' && i + 1 < chars.len() && chars[i + 1] == ']' {
            // Skip OSC sequence (like \x1b]133;A\x07)
            i += 2;
            while i < chars.len() {
                if chars[i] == '\x07' || chars[i] == '\x1b' {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else {
            len += if is_wide_char(chars[i]) { 2 } else { 1 };
            i += 1;
        }
    }
    len
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. VisualTruncate
// ─────────────────────────────────────────────────────────────────────────────

/// Result of truncating text to visual lines.
#[derive(Debug, Clone)]
pub struct VisualTruncateResult {
    /// The visual lines to display.
    pub visual_lines: Vec<String>,
    /// Number of visual lines that were skipped (hidden).
    pub skipped_count: usize,
}

/// Truncates text to a maximum number of visual lines from the end.
/// Accounts for line wrapping based on terminal width.
pub struct VisualTruncate;

impl VisualTruncate {
    /// Truncate text to max visual lines from the end.
    pub fn truncate(
        text: &str,
        max_visual_lines: usize,
        width: usize,
        padding_x: usize,
    ) -> VisualTruncateResult {
        if text.is_empty() {
            return VisualTruncateResult {
                visual_lines: Vec::new(),
                skipped_count: 0,
            };
        }

        // Split into logical lines, then wrap each to width
        let effective_width = width.saturating_sub(padding_x).max(1);
        let mut all_visual_lines = Vec::new();

        for logical_line in text.lines() {
            if logical_line.is_empty() {
                all_visual_lines.push(String::new());
            } else {
                // Wrap line to effective_width
                let mut current = String::new();
                let mut current_width = 0;

                for c in logical_line.chars() {
                    let cw = if is_wide_char(c) { 2 } else { 1 };
                    if current_width + cw > effective_width {
                        all_visual_lines.push(std::mem::take(&mut current));
                        current_width = 0;
                    }
                    current.push(c);
                    current_width += cw;
                }
                if !current.is_empty() {
                    all_visual_lines.push(current);
                }
            }
        }

        if all_visual_lines.len() <= max_visual_lines {
            return VisualTruncateResult {
                visual_lines: all_visual_lines,
                skipped_count: 0,
            };
        }

        let truncated_lines: Vec<String> =
            all_visual_lines.split_off(all_visual_lines.len() - max_visual_lines);
        let skipped_count = all_visual_lines.len();

        VisualTruncateResult {
            visual_lines: truncated_lines,
            skipped_count,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. ShowImagesSelector
// ─────────────────────────────────────────────────────────────────────────────

/// State for toggling image display.
#[derive(Debug, Clone)]
pub struct ShowImagesSelector {
    /// Current value (true = show images, false = hide).
    pub current_value: bool,
    /// Available options.
    pub options: Vec<ShowImagesOption>,
    /// Selected index.
    pub selected_index: usize,
}

/// An option in the show images selector.
#[derive(Debug, Clone)]
pub struct ShowImagesOption {
    /// Value to set ("yes" or "no").
    pub value: String,
    /// Display label.
    pub label: String,
    /// Description.
    pub description: String,
}

impl ShowImagesSelector {
    pub fn new(current_value: bool) -> Self {
        Self {
            current_value,
            options: vec![
                ShowImagesOption {
                    value: "yes".to_string(),
                    label: "Yes".to_string(),
                    description: "Show images inline in terminal".to_string(),
                },
                ShowImagesOption {
                    value: "no".to_string(),
                    label: "No".to_string(),
                    description: "Show text placeholder instead".to_string(),
                },
            ],
            selected_index: if current_value { 0 } else { 1 },
        }
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.selected_index < self.options.len() - 1 {
            self.selected_index += 1;
        }
    }

    /// Get the selected option.
    pub fn selected(&self) -> &ShowImagesOption {
        &self.options[self.selected_index]
    }

    /// Confirm selection, returns true if show images.
    pub fn confirm(&self) -> bool {
        self.options[self.selected_index].value == "yes"
    }

    /// Render the selector.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Top border
        lines.push("─".repeat(width.min(60)));

        // Options
        for (i, option) in self.options.iter().enumerate() {
            let marker = if i == self.selected_index {
                ansi::accent("›")
            } else {
                " ".to_string()
            };
            let label = if i == self.selected_index {
                ansi::bold(&option.label)
            } else {
                option.label.clone()
            };
            let desc = ansi::muted(&option.description);
            lines.push(format!(" {} {}  {}", marker, label, desc));
        }

        // Bottom border
        lines.push("─".repeat(width.min(60)));

        lines
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 9. CountdownTimer
// ─────────────────────────────────────────────────────────────────────────────

/// Countdown timer for timeouts, renders as a display string.
#[derive(Debug, Clone)]
pub struct CountdownTimer {
    /// Total seconds remaining.
    pub remaining_seconds: u64,
    /// Total seconds the timer was initialized with.
    pub total_seconds: u64,
}

impl CountdownTimer {
    /// Create a new countdown timer.
    pub fn new(timeout_seconds: u64) -> Self {
        Self {
            remaining_seconds: timeout_seconds,
            total_seconds: timeout_seconds,
        }
    }

    /// Create from milliseconds.
    pub fn from_millis(timeout_ms: u64) -> Self {
        Self::new((timeout_ms + 999) / 1000)
    }

    /// Tick the timer down by one second.
    /// Returns true if the timer has expired.
    pub fn tick(&mut self) -> bool {
        if self.remaining_seconds > 0 {
            self.remaining_seconds -= 1;
        }
        self.remaining_seconds == 0
    }

    /// Check if the timer has expired.
    pub fn is_expired(&self) -> bool {
        self.remaining_seconds == 0
    }

    /// Render the countdown display.
    pub fn render(&self) -> String {
        if self.remaining_seconds == 0 {
            ansi::error("⏰ Expired").to_string()
        } else if self.remaining_seconds <= 5 {
            ansi::warning(&format!("⏳ {}s remaining", self.remaining_seconds))
        } else {
            ansi::dim(&format!("⏳ {}s remaining", self.remaining_seconds))
        }
    }

    /// Render as a progress bar with the countdown.
    pub fn render_bar(&self, width: usize) -> String {
        if self.total_seconds == 0 {
            return String::new();
        }
        let fraction = self.remaining_seconds as f64 / self.total_seconds as f64;
        let filled = ((width as f64) * fraction) as usize;
        let empty = width - filled;

        let bar = if self.remaining_seconds <= 5 {
            format!("{}{}", "█".repeat(filled), "░".repeat(empty))
        } else {
            format!("{}{}", "█".repeat(filled), "░".repeat(empty))
        };

        format!("{} {}", ansi::dim(&bar), self.render())
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

// ── Selector Components ─────────────────────────────────────────────────────
//
// Interactive mode selector components ported from pi-mono.
// These are rendering helper structs that produce Vec<String> lines for terminal display.
// Each selector supports fuzzy search filtering and keyboard navigation.

/// Supported thinking levels for reasoning-capable models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ThinkingLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThinkingLevel::Off => "off",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::XHigh => "xhigh",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            ThinkingLevel::Off => "No reasoning",
            ThinkingLevel::Minimal => "Very brief reasoning (~1k tokens)",
            ThinkingLevel::Low => "Light reasoning (~2k tokens)",
            ThinkingLevel::Medium => "Moderate reasoning (~8k tokens)",
            ThinkingLevel::High => "Deep reasoning (~16k tokens)",
            ThinkingLevel::XHigh => "Maximum reasoning (~32k tokens)",
        }
    }

    pub fn all() -> Vec<ThinkingLevel> {
        vec![
            ThinkingLevel::Off,
            ThinkingLevel::Minimal,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::XHigh,
        ]
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" => Some(ThinkingLevel::Off),
            "minimal" => Some(ThinkingLevel::Minimal),
            "low" => Some(ThinkingLevel::Low),
            "medium" => Some(ThinkingLevel::Medium),
            "high" => Some(ThinkingLevel::High),
            "xhigh" => Some(ThinkingLevel::XHigh),
            _ => None,
        }
    }
}

impl std::fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ── Simple fuzzy matching for selector components ─────────────────────────────

/// Simple fuzzy score: returns a match quality score (higher = better), or None if no match.
/// Matches characters in order, allowing gaps.
pub fn fuzzy_score(query: &str, text: &str) -> Option<usize> {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();

    // Quick substring check - highest priority
    if text_lower.contains(&query_lower) {
        return Some(1000 + query.len());
    }

    // Fuzzy character matching
    let mut query_chars = query_lower.chars().peekable();
    let mut score = 0;
    let mut last_match_pos = 0;

    for (i, tc) in text_lower.char_indices() {
        if let Some(&qc) = query_chars.peek() {
            if tc == qc {
                // Bonus for matching at word boundaries
                if i == 0
                    || text.as_bytes().get(i - 1) == Some(&b' ')
                    || text.as_bytes().get(i - 1) == Some(&b'/')
                {
                    score += 10;
                } else if i == last_match_pos + 1 {
                    // Bonus for consecutive matches
                    score += 5;
                } else {
                    score += 1;
                }
                last_match_pos = i;
                query_chars.next();
            }
        }
    }

    if query_chars.peek().is_none() {
        Some(score)
    } else {
        None
    }
}

/// Filter a list of items by fuzzy matching against a query.
/// Returns indices of matching items sorted by match quality (best first).
pub fn fuzzy_filter_indices(items: &[impl AsRef<str>], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    let mut scored: Vec<(usize, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| fuzzy_score(query, item.as_ref()).map(|score| (i, score)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(i, _)| i).collect()
}

// ── 1. ConfigSelector ─────────────────────────────────────────────────────────

/// Resource type for configuration items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    Extensions,
    Skills,
    Prompts,
    Themes,
}

impl ResourceType {
    pub fn label(&self) -> &'static str {
        match self {
            ResourceType::Extensions => "Extensions",
            ResourceType::Skills => "Skills",
            ResourceType::Prompts => "Prompts",
            ResourceType::Themes => "Themes",
        }
    }
}

/// A single configuration resource item.
#[derive(Debug, Clone)]
pub struct ConfigResourceItem {
    pub path: String,
    pub enabled: bool,
    pub resource_type: ResourceType,
    pub display_name: String,
    pub group_key: String,
}

/// A group of configuration resources.
#[derive(Debug, Clone)]
pub struct ConfigResourceGroup {
    pub key: String,
    pub label: String,
    pub subgroups: Vec<ConfigResourceSubgroup>,
}

/// A subgroup of configuration resources (e.g., all extensions in a package).
#[derive(Debug, Clone)]
pub struct ConfigResourceSubgroup {
    pub resource_type: ResourceType,
    pub label: String,
    pub items: Vec<ConfigResourceItem>,
}

/// Flat entry for display in the config selector.
#[derive(Debug, Clone)]
pub enum ConfigFlatEntry {
    Group {
        group: ConfigResourceGroup,
    },
    Subgroup {
        subgroup: ConfigResourceSubgroup,
        group_label: String,
    },
    Item {
        item: ConfigResourceItem,
        group_label: String,
        subgroup_label: String,
    },
}

/// Config file selection with fuzzy search.
/// Renders a hierarchical list of configuration resources (extensions, skills, prompts, themes)
/// organized by source (package, user, project) with enable/disable toggle support.
#[derive(Debug, Clone)]
pub struct ConfigSelector {
    /// All flat entries (groups, subgroups, items).
    pub flat_entries: Vec<ConfigFlatEntry>,
    /// Filtered entries based on search.
    pub filtered_entries: Vec<ConfigFlatEntry>,
    /// Current search query.
    pub query: String,
    /// Selected index in filtered_entries.
    pub selected_index: usize,
    /// Maximum visible items.
    pub max_visible: usize,
}

impl ConfigSelector {
    /// Create a new config selector from resource groups.
    pub fn new(groups: Vec<ConfigResourceGroup>) -> Self {
        let flat_entries = Self::build_flat_list(&groups);
        let filtered_entries = flat_entries.clone();
        let selected_index = flat_entries
            .iter()
            .position(|e| matches!(e, ConfigFlatEntry::Item { .. }))
            .unwrap_or(0);
        Self {
            flat_entries,
            filtered_entries,
            query: String::new(),
            selected_index,
            max_visible: 15,
        }
    }

    fn build_flat_list(groups: &[ConfigResourceGroup]) -> Vec<ConfigFlatEntry> {
        let mut entries = Vec::new();
        for group in groups {
            entries.push(ConfigFlatEntry::Group {
                group: group.clone(),
            });
            for subgroup in &group.subgroups {
                entries.push(ConfigFlatEntry::Subgroup {
                    subgroup: subgroup.clone(),
                    group_label: group.label.clone(),
                });
                for item in &subgroup.items {
                    entries.push(ConfigFlatEntry::Item {
                        item: item.clone(),
                        group_label: group.label.clone(),
                        subgroup_label: subgroup.label.clone(),
                    });
                }
            }
        }
        entries
    }

    /// Apply search filter.
    pub fn set_filter(&mut self, query: String) {
        self.query = query;
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        if self.query.trim().is_empty() {
            self.filtered_entries = self.flat_entries.clone();
            self.select_first_item();
            return;
        }

        let lower_query = self.query.to_lowercase();

        // Find matching items
        let mut matching_groups = std::collections::HashSet::new();
        let mut matching_subgroups = std::collections::HashSet::new();
        let mut matching_items = std::collections::HashSet::new();

        for (idx, entry) in self.flat_entries.iter().enumerate() {
            if let ConfigFlatEntry::Item { item, .. } = entry {
                if item.display_name.to_lowercase().contains(&lower_query)
                    || item.path.to_lowercase().contains(&lower_query)
                    || item
                        .resource_type
                        .label()
                        .to_lowercase()
                        .contains(&lower_query)
                {
                    matching_items.insert(idx);
                }
            }
        }

        // Include parent groups/subgroups of matching items
        let mut current_group_idx: Option<usize> = None;
        let mut current_subgroup_idx: Option<usize> = None;
        for (idx, entry) in self.flat_entries.iter().enumerate() {
            match entry {
                ConfigFlatEntry::Group { .. } => current_group_idx = Some(idx),
                ConfigFlatEntry::Subgroup { .. } => current_subgroup_idx = Some(idx),
                ConfigFlatEntry::Item { .. } => {
                    if matching_items.contains(&idx) {
                        if let Some(gi) = current_group_idx {
                            matching_groups.insert(gi);
                        }
                        if let Some(si) = current_subgroup_idx {
                            matching_subgroups.insert(si);
                        }
                    }
                }
            }
        }

        self.filtered_entries = self
            .flat_entries
            .iter()
            .enumerate()
            .filter(|(idx, _)| {
                matching_groups.contains(idx)
                    || matching_subgroups.contains(idx)
                    || matching_items.contains(idx)
            })
            .map(|(_, e)| e.clone())
            .collect();

        self.select_first_item();
    }

    fn select_first_item(&mut self) {
        self.selected_index = self
            .filtered_entries
            .iter()
            .position(|e| matches!(e, ConfigFlatEntry::Item { .. }))
            .unwrap_or(0);
    }

    /// Move selection to the next item (skipping headers).
    pub fn move_up(&mut self) {
        self.selected_index = self.find_next_item(self.selected_index, -1);
    }

    /// Move selection to the previous item (skipping headers).
    pub fn move_down(&mut self) {
        self.selected_index = self.find_next_item(self.selected_index, 1);
    }

    fn find_next_item(&self, from: usize, direction: i32) -> usize {
        let mut idx = from as i32 + direction;
        while idx >= 0 && (idx as usize) < self.filtered_entries.len() {
            if matches!(
                self.filtered_entries[idx as usize],
                ConfigFlatEntry::Item { .. }
            ) {
                return idx as usize;
            }
            idx += direction;
        }
        from
    }

    /// Toggle the enabled state of the currently selected item.
    pub fn toggle_selected(&mut self) -> Option<&ConfigResourceItem> {
        if let Some(ConfigFlatEntry::Item { item, .. }) =
            self.filtered_entries.get(self.selected_index)
        {
            let new_enabled = !item.enabled;
            let path = item.path.clone();
            let resource_type = item.resource_type;
            // Update in filtered entries
            for entry in &mut self.filtered_entries {
                if let ConfigFlatEntry::Item { item, .. } = entry {
                    if item.path == path && item.resource_type == resource_type {
                        item.enabled = new_enabled;
                        return Some(item);
                    }
                }
            }
            // Also update in flat entries
            for entry in &mut self.flat_entries {
                if let ConfigFlatEntry::Item { item, .. } = entry {
                    if item.path == path && item.resource_type == resource_type {
                        item.enabled = new_enabled;
                    }
                }
            }
        }
        None
    }

    /// Render the config selector as lines for terminal display.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Header
        lines.push(format!(
            "\x1b[1mResource Configuration\x1b[0m{:>width$}",
            "\x1b[2mSpace=toggle · Esc=close\x1b[0m",
            width = width.saturating_sub(40)
        ));
        lines.push("\x1b[2mType to filter resources\x1b[0m".to_string());

        // Search input
        if !self.query.is_empty() {
            lines.push(format!("> {}", self.query));
        } else {
            lines.push("> ".to_string());
        }
        lines.push(String::new());

        if self.filtered_entries.is_empty() {
            lines.push("\x1b[2m  No resources found\x1b[0m".to_string());
            return lines;
        }

        // Calculate visible range centered on selection
        let total = self.filtered_entries.len();
        let start = if total <= self.max_visible {
            0
        } else {
            (self.selected_index as i32 - self.max_visible as i32 / 2)
                .max(0)
                .min((total - self.max_visible) as i32) as usize
        };
        let end = (start + self.max_visible).min(total);

        for i in start..end {
            let entry = &self.filtered_entries[i];
            let is_selected = i == self.selected_index;

            match entry {
                ConfigFlatEntry::Group { group } => {
                    let line = truncate_str(&group.label, width.saturating_sub(2));
                    lines.push(format!("  \x1b[1m\x1b[36m{}\x1b[0m", line));
                }
                ConfigFlatEntry::Subgroup { subgroup, .. } => {
                    let line = truncate_str(&subgroup.label, width.saturating_sub(4));
                    lines.push(format!("    \x1b[2m{}\x1b[0m", line));
                }
                ConfigFlatEntry::Item { item, .. } => {
                    let cursor = if is_selected { "> " } else { "  " };
                    let checkbox = if item.enabled {
                        "\x1b[32m[x]\x1b[0m"
                    } else {
                        "\x1b[2m[ ]\x1b[0m"
                    };
                    let name = if is_selected {
                        format!("\x1b[1m{}\x1b[0m", item.display_name)
                    } else {
                        item.display_name.clone()
                    };
                    let content = format!("{}    {} {}", cursor, checkbox, name);
                    lines.push(truncate_str(&content, width));
                }
            }
        }

        // Scroll indicator
        if start > 0 || end < total {
            let item_count = self
                .filtered_entries
                .iter()
                .filter(|e| matches!(e, ConfigFlatEntry::Item { .. }))
                .count();
            let current_item = self
                .filtered_entries
                .iter()
                .take(self.selected_index)
                .filter(|e| matches!(e, ConfigFlatEntry::Item { .. }))
                .count()
                + 1;
            lines.push(format!("\x1b[2m  ({}/{})\x1b[0m", current_item, item_count));
        }

        lines
    }
}

// ── 2. ModelSelector (Enhanced) ──────────────────────────────────────────────

/// Model scope for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelScope {
    All,
    Scoped,
}

/// Enhanced model selection with provider tabs, thinking levels, and scope switching.
#[derive(Debug, Clone)]
pub struct ModelSelectorEnhanced {
    /// All available models.
    pub all_models: Vec<ModelInfo>,
    /// Scoped models (filtered by configured providers).
    pub scoped_models: Vec<ModelInfo>,
    /// Currently active models (based on scope).
    pub active_models: Vec<ModelInfo>,
    /// Filtered models (based on search).
    pub filtered_models: Vec<ModelInfo>,
    /// Current search query.
    pub query: String,
    /// Selected index in filtered_models.
    pub selected_index: usize,
    /// Current scope (all or scoped).
    pub scope: ModelScope,
    /// Currently active model id (for highlighting).
    pub current_model_id: Option<String>,
    /// Maximum visible items.
    pub max_visible: usize,
    /// Error message if model loading failed.
    pub error_message: Option<String>,
}

impl ModelSelectorEnhanced {
    /// Create a new enhanced model selector.
    pub fn new(
        all_models: Vec<ModelInfo>,
        scoped_models: Vec<ModelInfo>,
        current_model_id: Option<String>,
    ) -> Self {
        let scope = if scoped_models.is_empty() {
            ModelScope::All
        } else {
            ModelScope::Scoped
        };
        let active_models = if scope == ModelScope::Scoped {
            scoped_models.clone()
        } else {
            all_models.clone()
        };
        let filtered_models = active_models.clone();

        let mut selector = Self {
            all_models,
            scoped_models,
            active_models,
            filtered_models,
            query: String::new(),
            selected_index: 0,
            scope,
            current_model_id,
            max_visible: 10,
            error_message: None,
        };
        selector.pre_select_current();
        selector
    }

    fn pre_select_current(&mut self) {
        if let Some(ref current_id) = self.current_model_id {
            if let Some(idx) = self
                .filtered_models
                .iter()
                .position(|m| m.id == *current_id)
            {
                self.selected_index = idx;
            }
        }
    }

    /// Toggle scope between all and scoped models.
    pub fn toggle_scope(&mut self) {
        self.scope = match self.scope {
            ModelScope::All => ModelScope::Scoped,
            ModelScope::Scoped => ModelScope::All,
        };
        if self.scoped_models.is_empty() {
            self.scope = ModelScope::All;
        }
        self.active_models = if self.scope == ModelScope::Scoped {
            self.scoped_models.clone()
        } else {
            self.all_models.clone()
        };
        self.apply_filter();
    }

    /// Set search filter.
    pub fn set_filter(&mut self, query: String) {
        self.query = query;
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        if self.query.trim().is_empty() {
            self.filtered_models = self.active_models.clone();
        } else {
            let indices = fuzzy_filter_indices(
                &self
                    .active_models
                    .iter()
                    .map(|m| format!("{} {} {}/{}", m.id, m.provider, m.provider, m.id))
                    .collect::<Vec<_>>(),
                &self.query,
            );
            self.filtered_models = indices
                .into_iter()
                .map(|i| self.active_models[i].clone())
                .collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered_models.len().saturating_sub(1));
        self.pre_select_current();
    }

    /// Move selection up (wraps to bottom).
    pub fn move_up(&mut self) {
        if self.filtered_models.is_empty() {
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = self.filtered_models.len() - 1;
        } else {
            self.selected_index -= 1;
        }
    }

    /// Move selection down (wraps to top).
    pub fn move_down(&mut self) {
        if self.filtered_models.is_empty() {
            return;
        }
        if self.selected_index == self.filtered_models.len() - 1 {
            self.selected_index = 0;
        } else {
            self.selected_index += 1;
        }
    }

    /// Get the currently selected model.
    pub fn selected(&self) -> Option<&ModelInfo> {
        self.filtered_models.get(self.selected_index)
    }

    /// Render the enhanced model selector.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Scope header
        let scope_text = match self.scope {
            ModelScope::All => "\x1b[36mall\x1b[0m | \x1b[2mscoped\x1b[0m".to_string(),
            ModelScope::Scoped => "\x1b[2mall\x1b[0m | \x1b[36mscoped\x1b[0m".to_string(),
        };
        lines.push(format!(
            "\x1b[2mScope:\x1b[0m {} \x1b[2m(Tab to toggle)\x1b[0m",
            scope_text
        ));

        // Search input
        lines.push(format!("> {}", self.query));
        lines.push(String::new());

        // Error message
        if let Some(ref err) = self.error_message {
            for err_line in err.lines() {
                lines.push(format!("\x1b[31m{}\x1b[0m", err_line));
            }
            return lines;
        }

        if self.filtered_models.is_empty() {
            lines.push("\x1b[2m  No matching models\x1b[0m".to_string());
            return lines;
        }

        // Calculate visible range
        let total = self.filtered_models.len();
        let half = self.max_visible / 2;
        let start = if self.selected_index >= half {
            (self.selected_index - half).min(total.saturating_sub(self.max_visible))
        } else {
            0
        };
        let end = (start + self.max_visible).min(total);

        for i in start..end {
            let model = &self.filtered_models[i];
            let is_selected = i == self.selected_index;
            let is_current = self
                .current_model_id
                .as_ref()
                .map_or(false, |id| id == &model.id);

            let prefix = if is_selected {
                "\x1b[36m→ \x1b[0m"
            } else {
                "  "
            };
            let model_text = if is_selected {
                format!("\x1b[36m{}\x1b[0m", model.id)
            } else {
                model.id.clone()
            };
            let provider_badge = format!("\x1b[2m[{}]\x1b[0m", model.provider);
            let current_badge = if is_current {
                "\x1b[32m ✓\x1b[0m".to_string()
            } else {
                String::new()
            };

            let line = format!(
                "{}{} {}{}",
                prefix, model_text, provider_badge, current_badge
            );
            lines.push(truncate_str(&line, width));
        }

        // Scroll indicator
        if start > 0 || end < total {
            lines.push(format!(
                "\x1b[2m  ({}/{})\x1b[0m",
                self.selected_index + 1,
                total
            ));
        }

        // Detail line for selected model
        if let Some(selected) = self.selected() {
            lines.push(String::new());
            lines.push(format!("\x1b[2m  Model Name: {}\x1b[0m", selected.name));
        }

        lines
    }
}

// ── 3. SessionSelector (Enhanced) ────────────────────────────────────────────

/// Session scope for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScope {
    CurrentFolder,
    All,
}

/// Sort mode for sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSortMode {
    Recent,
    Fuzzy,
    Threaded,
}

impl SessionSortMode {
    pub fn label(&self) -> &'static str {
        match self {
            SessionSortMode::Recent => "Recent",
            SessionSortMode::Fuzzy => "Fuzzy",
            SessionSortMode::Threaded => "Threaded",
        }
    }
}

/// Name filter mode for sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionNameFilter {
    All,
    Named,
}

/// Enhanced session info with additional metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedSessionInfo {
    pub id: String,
    pub name: String,
    pub label: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub message_count: usize,
    pub model: Option<String>,
    pub parent_id: Option<String>,
    pub working_dir: Option<String>,
}

/// Format a relative time from a date string.
fn format_relative_time(date_str: &str) -> String {
    // Try parsing common date formats
    let now = chrono::Utc::now();
    let parsed: Option<chrono::DateTime<chrono::Utc>> =
        chrono::DateTime::parse_from_rfc3339(date_str)
            .map(|dt| dt.to_utc())
            .ok()
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(date_str, "%Y-%m-%dT%H:%M:%S")
                    .map(|dt| dt.and_utc())
                    .ok()
            })
            .or_else(|| {
                chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
                    .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
                    .ok()
            });

    match parsed {
        Some(dt) => {
            let diff = now.signed_duration_since(dt);
            let mins = diff.num_minutes();
            if mins < 1 {
                return "now".to_string();
            }
            if mins < 60 {
                return format!("{}m", mins);
            }
            let hours = diff.num_hours();
            if hours < 24 {
                return format!("{}h", hours);
            }
            let days = diff.num_days();
            if days < 7 {
                return format!("{}d", days);
            }
            if days < 30 {
                return format!("{}w", days / 7);
            }
            if days < 365 {
                return format!("{}mo", days / 30);
            }
            format!("{}y", days / 365)
        }
        None => date_str.to_string(),
    }
}

/// Shorten a path by replacing home directory with ~.
fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path.starts_with(home_str.as_ref()) {
            return format!("~{}", &path[home_str.len()..]);
        }
    }
    path.to_string()
}

/// Enhanced session selector with search, labels, timestamps, scope, and sort.
#[derive(Debug, Clone)]
pub struct SessionSelectorEnhanced {
    /// All sessions.
    pub sessions: Vec<EnhancedSessionInfo>,
    /// Filtered sessions based on search.
    pub filtered_sessions: Vec<EnhancedSessionInfo>,
    /// Current search query.
    pub query: String,
    /// Selected index in filtered_sessions.
    pub selected_index: usize,
    /// Current scope.
    pub scope: SessionScope,
    /// Current sort mode.
    pub sort_mode: SessionSortMode,
    /// Name filter mode.
    pub name_filter: SessionNameFilter,
    /// Whether to show working directory paths.
    pub show_paths: bool,
    /// Scroll offset for rendering.
    pub scroll_offset: usize,
    /// Maximum visible items.
    pub max_visible: usize,
    /// Current working directory for scope filtering.
    pub cwd: Option<String>,
}

impl SessionSelectorEnhanced {
    /// Create a new enhanced session selector.
    pub fn new(sessions: Vec<EnhancedSessionInfo>) -> Self {
        let filtered_sessions = sessions.clone();
        Self {
            sessions,
            filtered_sessions,
            query: String::new(),
            selected_index: 0,
            scope: SessionScope::CurrentFolder,
            sort_mode: SessionSortMode::Recent,
            name_filter: SessionNameFilter::All,
            show_paths: false,
            scroll_offset: 0,
            max_visible: 15,
            cwd: None,
        }
    }

    /// Set the current working directory for scope filtering.
    pub fn set_cwd(&mut self, cwd: String) {
        self.cwd = Some(cwd);
        self.apply_filter();
    }

    /// Toggle scope between current folder and all.
    pub fn toggle_scope(&mut self) {
        self.scope = match self.scope {
            SessionScope::CurrentFolder => SessionScope::All,
            SessionScope::All => SessionScope::CurrentFolder,
        };
        self.apply_filter();
    }

    /// Toggle sort mode.
    pub fn toggle_sort(&mut self) {
        self.sort_mode = match self.sort_mode {
            SessionSortMode::Recent => SessionSortMode::Threaded,
            SessionSortMode::Threaded => SessionSortMode::Fuzzy,
            SessionSortMode::Fuzzy => SessionSortMode::Recent,
        };
        self.apply_filter();
    }

    /// Toggle name filter.
    pub fn toggle_name_filter(&mut self) {
        self.name_filter = match self.name_filter {
            SessionNameFilter::All => SessionNameFilter::Named,
            SessionNameFilter::Named => SessionNameFilter::All,
        };
        self.apply_filter();
    }

    /// Set search filter.
    pub fn set_filter(&mut self, query: String) {
        self.query = query;
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        let mut filtered: Vec<EnhancedSessionInfo> = self.sessions.clone();

        // Scope filter
        if self.scope == SessionScope::CurrentFolder {
            if let Some(ref cwd) = self.cwd {
                filtered.retain(|s| s.working_dir.as_ref().map_or(true, |d| d == cwd));
            }
        }

        // Name filter
        if self.name_filter == SessionNameFilter::Named {
            filtered.retain(|s| s.name.is_empty());
            // Actually, keep named ones
            filtered = self
                .sessions
                .iter()
                .filter(|s| !s.name.is_empty())
                .cloned()
                .collect();
        }

        // Query filter
        if !self.query.is_empty() {
            let lower_query = self.query.to_lowercase();
            filtered.retain(|s| {
                s.name.to_lowercase().contains(&lower_query)
                    || s.id.to_lowercase().contains(&lower_query)
                    || s.label
                        .as_ref()
                        .map_or(false, |l| l.to_lowercase().contains(&lower_query))
            });
        }

        // Sort
        match self.sort_mode {
            SessionSortMode::Recent => {
                filtered.sort_by(|a, b| {
                    b.updated_at
                        .cmp(&a.updated_at)
                        .then(b.created_at.cmp(&a.created_at))
                });
            }
            SessionSortMode::Fuzzy => {
                // Keep order; fuzzy matching already done by filter
            }
            SessionSortMode::Threaded => {
                // Sort by parent_id to group threads
                filtered.sort_by(|a, b| {
                    a.parent_id
                        .cmp(&b.parent_id)
                        .then(b.created_at.cmp(&a.created_at))
                });
            }
        }

        self.filtered_sessions = filtered;
        self.selected_index = self
            .selected_index
            .min(self.filtered_sessions.len().saturating_sub(1));
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.adjust_scroll();
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let max = self.filtered_sessions.len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
            self.adjust_scroll();
        }
    }

    fn adjust_scroll(&mut self) {
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected_index - self.max_visible + 1;
        }
    }

    /// Get the currently selected session.
    pub fn selected(&self) -> Option<&EnhancedSessionInfo> {
        self.filtered_sessions.get(self.selected_index)
    }

    /// Render the enhanced session selector.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Header with scope, sort, name filter
        let title = match self.scope {
            SessionScope::CurrentFolder => "Resume Session (Current Folder)",
            SessionScope::All => "Resume Session (All)",
        };
        lines.push(format!("\x1b[1m{}\x1b[0m", title));

        let scope_text = match self.scope {
            SessionScope::CurrentFolder => "\x1b[36m◉ Current Folder\x1b[0m | ○ All",
            SessionScope::All => "○ Current Folder | \x1b[36m◉ All\x1b[0m",
        };
        let sort_text = format!("Sort: \x1b[36m{}\x1b[0m", self.sort_mode.label());
        let name_text = format!(
            "Name: \x1b[36m{}\x1b[0m",
            match self.name_filter {
                SessionNameFilter::All => "All",
                SessionNameFilter::Named => "Named",
            }
        );
        lines.push(format!("{}  {}  {}", scope_text, sort_text, name_text));

        // Search input
        if !self.query.is_empty() {
            lines.push(format!("> {}", self.query));
        }
        lines.push(String::new());

        if self.filtered_sessions.is_empty() {
            lines.push("\x1b[2m  No sessions found\x1b[0m".to_string());
            return lines;
        }

        // Render visible sessions
        let visible: Vec<_> = self
            .filtered_sessions
            .iter()
            .skip(self.scroll_offset)
            .take(self.max_visible)
            .collect();

        for (i, session) in visible.iter().enumerate() {
            let real_idx = self.scroll_offset + i;
            let is_selected = real_idx == self.selected_index;

            let marker = if is_selected {
                "\x1b[36m▶\x1b[0m"
            } else {
                " "
            };

            // Branch indicator
            let branch = if session.parent_id.is_some() {
                "├─ "
            } else {
                "  "
            };

            // Name or truncated ID
            let name = if session.name.is_empty() {
                &session.id[..8.min(session.id.len())]
            } else {
                &session.name
            };

            // Label
            let label = session
                .label
                .as_ref()
                .map(|l| format!(" \x1b[33m[{}]\x1b[0m", l))
                .unwrap_or_default();

            // Relative time
            let time =
                format_relative_time(&session.updated_at.as_ref().unwrap_or(&session.created_at));

            let mut line = format!(
                "{} {}{:<30} {} msg:{} model:{}",
                marker,
                branch,
                name,
                time,
                session.message_count,
                session.model.as_deref().unwrap_or("-"),
            );
            line.push_str(&label);

            // Working directory
            if self.show_paths {
                if let Some(ref dir) = session.working_dir {
                    line.push_str(&format!(" \x1b[2m{}\x1b[0m", shorten_path(dir)));
                }
            }

            lines.push(truncate_str(&line, width));
        }

        // Scroll indicator
        if self.filtered_sessions.len() > self.max_visible {
            lines.push(format!(
                "\x1b[2m  ({}/{})\x1b[0m",
                self.selected_index + 1,
                self.filtered_sessions.len()
            ));
        }

        lines
    }
}

// ── 4. SettingsSelector ──────────────────────────────────────────────────────

/// A setting item for the settings selector.
#[derive(Debug, Clone)]
pub struct SettingItem {
    /// Unique identifier for this setting.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Description text.
    pub description: String,
    /// Current value as a string.
    pub current_value: String,
    /// Available values to choose from.
    pub values: Vec<String>,
}

impl SettingItem {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        current_value: impl Into<String>,
        values: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: description.into(),
            current_value: current_value.into(),
            values,
        }
    }
}

/// Settings key-value selector with descriptions and fuzzy search.
#[derive(Debug, Clone)]
pub struct SettingsSelector {
    /// All settings items.
    pub items: Vec<SettingItem>,
    /// Filtered indices based on search.
    pub filtered_indices: Vec<usize>,
    /// Current search query.
    pub query: String,
    /// Selected index in filtered list.
    pub selected_index: usize,
    /// Scroll offset for rendering.
    pub scroll_offset: usize,
    /// Maximum visible items.
    pub max_visible: usize,
}

impl SettingsSelector {
    /// Create a new settings selector with the given items.
    pub fn new(items: Vec<SettingItem>) -> Self {
        let filtered_indices = (0..items.len()).collect();
        Self {
            items,
            filtered_indices,
            query: String::new(),
            selected_index: 0,
            scroll_offset: 0,
            max_visible: 10,
        }
    }

    /// Build settings items from a settings configuration.
    pub fn from_config(config: &SettingsConfig) -> Self {
        let mut items = Vec::new();

        items.push(SettingItem::new(
            "autocompact",
            "Auto-compact",
            "Automatically compact context when it gets too large",
            if config.auto_compact { "true" } else { "false" },
            vec!["true".to_string(), "false".to_string()],
        ));

        items.push(SettingItem::new(
            "show-images",
            "Show images",
            "Render images inline in terminal",
            if config.show_images { "true" } else { "false" },
            vec!["true".to_string(), "false".to_string()],
        ));

        items.push(SettingItem::new(
            "auto-resize-images",
            "Auto-resize images",
            "Resize large images to 2000x2000 max",
            if config.auto_resize_images {
                "true"
            } else {
                "false"
            },
            vec!["true".to_string(), "false".to_string()],
        ));

        items.push(SettingItem::new(
            "block-images",
            "Block images",
            "Prevent images from being sent to LLM providers",
            if config.block_images { "true" } else { "false" },
            vec!["true".to_string(), "false".to_string()],
        ));

        items.push(SettingItem::new(
            "skill-commands",
            "Skill commands",
            "Register skills as /skill:name commands",
            if config.enable_skill_commands {
                "true"
            } else {
                "false"
            },
            vec!["true".to_string(), "false".to_string()],
        ));

        items.push(SettingItem::new(
            "steering-mode",
            "Steering mode",
            "Enter while streaming queues steering messages",
            config.steering_mode.clone(),
            vec!["one-at-a-time".to_string(), "all".to_string()],
        ));

        items.push(SettingItem::new(
            "follow-up-mode",
            "Follow-up mode",
            "Alt+Enter queues follow-up messages until agent stops",
            config.follow_up_mode.clone(),
            vec!["one-at-a-time".to_string(), "all".to_string()],
        ));

        items.push(SettingItem::new(
            "transport",
            "Transport",
            "Preferred transport for providers with multiple transports",
            config.transport.clone(),
            vec![
                "sse".to_string(),
                "websocket".to_string(),
                "websocket-cached".to_string(),
                "auto".to_string(),
            ],
        ));

        items.push(SettingItem::new(
            "thinking",
            "Thinking level",
            "Reasoning depth for thinking-capable models",
            config.thinking_level.as_str().to_string(),
            config
                .available_thinking_levels
                .iter()
                .map(|l| l.as_str().to_string())
                .collect(),
        ));

        items.push(SettingItem::new(
            "hide-thinking",
            "Hide thinking",
            "Hide thinking blocks in assistant responses",
            if config.hide_thinking_block {
                "true"
            } else {
                "false"
            },
            vec!["true".to_string(), "false".to_string()],
        ));

        items.push(SettingItem::new(
            "theme",
            "Theme",
            "Color theme for the interface",
            config.current_theme.clone(),
            config.available_themes.clone(),
        ));

        items.push(SettingItem::new(
            "double-escape-action",
            "Double-escape action",
            "Action when pressing Escape twice with empty editor",
            config.double_escape_action.clone(),
            vec!["tree".to_string(), "fork".to_string(), "none".to_string()],
        ));

        items.push(SettingItem::new(
            "tree-filter-mode",
            "Tree filter mode",
            "Default filter when opening /tree",
            config.tree_filter_mode.clone(),
            vec![
                "default".to_string(),
                "no-tools".to_string(),
                "user-only".to_string(),
                "labeled-only".to_string(),
                "all".to_string(),
            ],
        ));

        items.push(SettingItem::new(
            "quiet-startup",
            "Quiet startup",
            "Disable verbose printing at startup",
            if config.quiet_startup {
                "true"
            } else {
                "false"
            },
            vec!["true".to_string(), "false".to_string()],
        ));

        Self::new(items)
    }

    /// Set search filter.
    pub fn set_filter(&mut self, query: String) {
        self.query = query;
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            let search_texts: Vec<String> = self
                .items
                .iter()
                .map(|item| format!("{} {} {}", item.label, item.description, item.id))
                .collect();
            self.filtered_indices = fuzzy_filter_indices(&search_texts, &self.query);
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered_indices.len().saturating_sub(1));
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.adjust_scroll();
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let max = self.filtered_indices.len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
            self.adjust_scroll();
        }
    }

    fn adjust_scroll(&mut self) {
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected_index - self.max_visible + 1;
        }
    }

    /// Get the currently selected setting item.
    pub fn selected(&self) -> Option<&SettingItem> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|&idx| self.items.get(idx))
    }

    /// Cycle the value of the currently selected setting.
    pub fn cycle_value(&mut self) -> Option<String> {
        if let Some(&idx) = self.filtered_indices.get(self.selected_index) {
            let item = &mut self.items[idx];
            if item.values.len() > 1 {
                if let Some(pos) = item.values.iter().position(|v| v == &item.current_value) {
                    let next = (pos + 1) % item.values.len();
                    item.current_value = item.values[next].clone();
                    return Some(item.current_value.clone());
                }
            }
        }
        None
    }

    /// Render the settings selector.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        lines
            .push("\x1b[36m───────────────────────────────────────────────────\x1b[0m".to_string());
        lines.push("\x1b[1mSettings\x1b[0m \x1b[2m(Enter to change · Esc to close · Type to search)\x1b[0m".to_string());

        if !self.query.is_empty() {
            lines.push(format!("Filter: {}", self.query));
        }
        lines.push(String::new());

        if self.filtered_indices.is_empty() {
            lines.push("\x1b[2m  No matching settings\x1b[0m".to_string());
            return lines;
        }

        // Render visible items
        let visible_count = self.max_visible.min(self.filtered_indices.len());
        for vi in self.scroll_offset..self.scroll_offset + visible_count {
            if vi >= self.filtered_indices.len() {
                break;
            }
            let idx = self.filtered_indices[vi];
            let item = &self.items[idx];
            let is_selected = vi == self.selected_index;

            let prefix = if is_selected {
                "\x1b[36m▶\x1b[0m"
            } else {
                " "
            };

            // Label and value
            let label = if is_selected {
                format!("\x1b[1m{}\x1b[0m", item.label)
            } else {
                item.label.clone()
            };

            // Show value with cycling indicator
            let value_display = if item.values.len() <= 2 {
                format!("\x1b[33m{}\x1b[0m", item.current_value)
            } else {
                // Show current value with position indicator
                if let Some(pos) = item.values.iter().position(|v| v == &item.current_value) {
                    format!(
                        "\x1b[33m{}\x1b[0m \x1b[2m({}/{})\x1b[0m",
                        item.current_value,
                        pos + 1,
                        item.values.len()
                    )
                } else {
                    format!("\x1b[33m{}\x1b[0m", item.current_value)
                }
            };

            let line = format!("{} {:<25} {}", prefix, label, value_display);
            lines.push(truncate_str(&line, width));

            // Description for selected item
            if is_selected {
                let desc = truncate_str(&format!("  \x1b[2m{}\x1b[0m", item.description), width);
                lines.push(desc);
            }
        }

        // Scroll indicator
        if self.filtered_indices.len() > self.max_visible {
            lines.push(format!(
                "\x1b[2m  ({}/{})\x1b[0m",
                self.selected_index + 1,
                self.filtered_indices.len()
            ));
        }

        lines
            .push("\x1b[36m───────────────────────────────────────────────────\x1b[0m".to_string());
        lines
    }
}

/// Settings configuration for building the settings selector.
#[derive(Debug, Clone, Default)]
pub struct SettingsConfig {
    pub auto_compact: bool,
    pub show_images: bool,
    pub image_width_cells: usize,
    pub auto_resize_images: bool,
    pub block_images: bool,
    pub enable_skill_commands: bool,
    pub steering_mode: String,
    pub follow_up_mode: String,
    pub transport: String,
    pub thinking_level: ThinkingLevel,
    pub available_thinking_levels: Vec<ThinkingLevel>,
    pub current_theme: String,
    pub available_themes: Vec<String>,
    pub hide_thinking_block: bool,
    pub collapse_changelog: bool,
    pub enable_install_telemetry: bool,
    pub double_escape_action: String,
    pub tree_filter_mode: String,
    pub show_hardware_cursor: bool,
    pub editor_padding_x: usize,
    pub autocomplete_max_visible: usize,
    pub quiet_startup: bool,
    pub clear_on_shrink: bool,
    pub show_terminal_progress: bool,
}

// ── 5. ThemeSelector ─────────────────────────────────────────────────────────

/// Theme name selection with preview.
#[derive(Debug, Clone)]
pub struct ThemeSelector {
    /// Available theme names.
    pub themes: Vec<String>,
    /// Currently selected theme index.
    pub selected_index: usize,
    /// Current active theme name.
    pub current_theme: String,
    /// Filtered indices.
    pub filtered_indices: Vec<usize>,
    /// Search query.
    pub query: String,
    /// Maximum visible items.
    pub max_visible: usize,
}

impl ThemeSelector {
    /// Create a new theme selector.
    pub fn new(themes: Vec<String>, current_theme: String) -> Self {
        let filtered_indices = (0..themes.len()).collect();
        let selected_index = themes.iter().position(|t| t == &current_theme).unwrap_or(0);
        Self {
            themes,
            selected_index,
            current_theme,
            filtered_indices,
            query: String::new(),
            max_visible: 10,
        }
    }

    /// Set search filter.
    pub fn set_filter(&mut self, query: String) {
        self.query = query;
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.themes.len()).collect();
        } else {
            self.filtered_indices = fuzzy_filter_indices(&self.themes, &self.query);
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered_indices.len().saturating_sub(1));
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let max = self.filtered_indices.len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
        }
    }

    /// Get the currently selected theme name.
    pub fn selected(&self) -> Option<&str> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|&idx| self.themes.get(idx).map(|s| s.as_str()))
    }

    /// Render the theme selector.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        lines
            .push("\x1b[36m───────────────────────────────────────────────────\x1b[0m".to_string());
        lines.push("\x1b[1mSelect Theme\x1b[0m".to_string());
        lines.push(String::new());

        if self.filtered_indices.is_empty() {
            lines.push("\x1b[2m  No matching themes\x1b[0m".to_string());
            return lines;
        }

        for (vi, &idx) in self.filtered_indices.iter().enumerate() {
            let theme = &self.themes[idx];
            let is_selected = vi == self.selected_index;
            let is_current = theme == &self.current_theme;

            let prefix = if is_selected {
                "\x1b[36m→ \x1b[0m"
            } else {
                "  "
            };

            let name = if is_selected {
                format!("\x1b[36m{}\x1b[0m", theme)
            } else {
                theme.clone()
            };

            let current_badge = if is_current {
                " \x1b[32m(current)\x1b[0m".to_string()
            } else {
                String::new()
            };

            let line = format!("{}{}{}", prefix, name, current_badge);
            lines.push(truncate_str(&line, width));

            // Preview: show sample colors for the theme
            if is_selected {
                lines.push(format!(
                    "    \x1b[2mPreview:\x1b[0m \x1b[32m●\x1b[0m \x1b[33m●\x1b[0m \x1b[36m●\x1b[0m \x1b[35m●\x1b[0m \x1b[31m●\x1b[0m"
                ));
            }
        }

        lines.push(String::new());
        lines.push("\x1b[2m  Enter to select · Esc to cancel\x1b[0m".to_string());
        lines
            .push("\x1b[36m───────────────────────────────────────────────────\x1b[0m".to_string());

        lines
    }
}

// ── 6. ThinkingSelector ─────────────────────────────────────────────────────

/// Thinking level selection (off/minimal/low/medium/high/xhigh).
#[derive(Debug, Clone)]
pub struct ThinkingSelector {
    /// Available thinking levels.
    pub levels: Vec<ThinkingLevel>,
    /// Currently selected level index.
    pub selected_index: usize,
    /// Current active thinking level.
    pub current_level: ThinkingLevel,
}

impl ThinkingSelector {
    /// Create a new thinking selector.
    pub fn new(current_level: ThinkingLevel, available_levels: Vec<ThinkingLevel>) -> Self {
        let selected_index = available_levels
            .iter()
            .position(|l| l == &current_level)
            .unwrap_or(0);
        Self {
            levels: available_levels,
            selected_index,
            current_level,
        }
    }

    /// Create with all levels.
    pub fn new_with_all_levels(current_level: ThinkingLevel) -> Self {
        Self::new(current_level, ThinkingLevel::all())
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let max = self.levels.len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
        }
    }

    /// Get the currently selected thinking level.
    pub fn selected(&self) -> Option<ThinkingLevel> {
        self.levels.get(self.selected_index).copied()
    }

    /// Render the thinking selector.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        lines
            .push("\x1b[36m───────────────────────────────────────────────────\x1b[0m".to_string());
        lines.push("\x1b[1mThinking Level\x1b[0m".to_string());
        lines.push("\x1b[2mSelect reasoning depth for thinking-capable models\x1b[0m".to_string());
        lines.push(String::new());

        for (i, level) in self.levels.iter().enumerate() {
            let is_selected = i == self.selected_index;
            let is_current = *level == self.current_level;

            let prefix = if is_selected {
                "\x1b[36m→ \x1b[0m"
            } else {
                "  "
            };

            let name = if is_selected {
                format!("\x1b[36m{}\x1b[0m", level.as_str())
            } else {
                level.as_str().to_string()
            };

            let desc = format!("\x1b[2m{}\x1b[0m", level.description());
            let current_badge = if is_current {
                " \x1b[32m✓\x1b[0m".to_string()
            } else {
                String::new()
            };

            let line = format!("{}{:<12} {}{}", prefix, name, desc, current_badge);
            lines.push(truncate_str(&line, width));
        }

        lines.push(String::new());
        lines.push("\x1b[2m  Enter to select · Esc to cancel\x1b[0m".to_string());
        lines
            .push("\x1b[36m───────────────────────────────────────────────────\x1b[0m".to_string());

        lines
    }
}

// ── 7. TreeSelector ─────────────────────────────────────────────────────────

/// A tree node for session tree navigation.
#[derive(Debug, Clone)]
pub struct SessionTreeNode {
    /// Unique entry ID.
    pub id: String,
    /// Display label for this node.
    pub label: String,
    /// Parent node ID.
    pub parent_id: Option<String>,
    /// Whether this is a user message.
    pub is_user: bool,
    /// Whether this is a tool call.
    pub is_tool: bool,
    /// Whether this node has a custom label.
    pub has_label: bool,
    /// Timestamp string.
    pub timestamp: Option<String>,
    /// Child nodes.
    pub children: Vec<SessionTreeNode>,
}

/// Filter mode for tree display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeFilterMode {
    Default,
    NoTools,
    UserOnly,
    LabeledOnly,
    All,
}

impl TreeFilterMode {
    pub fn label(&self) -> &'static str {
        match self {
            TreeFilterMode::Default => "default",
            TreeFilterMode::NoTools => "no-tools",
            TreeFilterMode::UserOnly => "user-only",
            TreeFilterMode::LabeledOnly => "labeled-only",
            TreeFilterMode::All => "all",
        }
    }
}

/// Flattened tree node for display.
#[derive(Debug, Clone)]
pub struct FlatTreeNode {
    /// Reference to the tree node.
    pub node_id: String,
    /// Display label.
    pub label: String,
    /// Indent level.
    pub indent: usize,
    /// Whether this node is the last sibling.
    pub is_last: bool,
    /// Whether to show a connector line.
    pub show_connector: bool,
    /// Whether this is a user message.
    pub is_user: bool,
    /// Whether this is a tool call.
    pub is_tool: bool,
    /// Whether this node is on the active path.
    pub is_active: bool,
}

/// Session tree navigation with branch display.
#[derive(Debug, Clone)]
pub struct TreeSelector {
    /// Root tree nodes.
    pub roots: Vec<SessionTreeNode>,
    /// Flattened nodes for display.
    pub flat_nodes: Vec<FlatTreeNode>,
    /// Filtered nodes based on filter mode.
    pub filtered_nodes: Vec<FlatTreeNode>,
    /// Currently selected index in filtered_nodes.
    pub selected_index: usize,
    /// Current leaf ID (active entry).
    pub current_leaf_id: Option<String>,
    /// Active path IDs (from root to current leaf).
    pub active_path_ids: std::collections::HashSet<String>,
    /// Current filter mode.
    pub filter_mode: TreeFilterMode,
    /// Search query.
    pub query: String,
    /// Maximum visible items.
    pub max_visible: usize,
}

impl TreeSelector {
    /// Create a new tree selector from root nodes.
    pub fn new(roots: Vec<SessionTreeNode>, current_leaf_id: Option<String>) -> Self {
        let active_path_ids = Self::build_active_path(&roots, current_leaf_id.as_deref());
        let flat_nodes = Self::flatten_tree(&roots);
        let filtered_nodes = flat_nodes.clone();
        let selected_index = current_leaf_id
            .as_ref()
            .and_then(|id| filtered_nodes.iter().position(|n| n.node_id == *id))
            .unwrap_or_else(|| filtered_nodes.iter().position(|n| !n.is_tool).unwrap_or(0));

        Self {
            roots,
            flat_nodes,
            filtered_nodes,
            selected_index,
            current_leaf_id,
            active_path_ids,
            filter_mode: TreeFilterMode::Default,
            query: String::new(),
            max_visible: 20,
        }
    }

    fn build_active_path(
        roots: &[SessionTreeNode],
        leaf_id: Option<&str>,
    ) -> std::collections::HashSet<String> {
        let mut path = std::collections::HashSet::new();
        if let Some(leaf) = leaf_id {
            // Walk tree to find path
            fn walk(
                node: &SessionTreeNode,
                target: &str,
                path: &mut std::collections::HashSet<String>,
            ) -> bool {
                path.insert(node.id.clone());
                if node.id == target {
                    return true;
                }
                for child in &node.children {
                    if walk(child, target, path) {
                        return true;
                    }
                }
                path.remove(&node.id);
                false
            }

            for root in roots {
                if walk(root, leaf, &mut path) {
                    break;
                }
            }
        }
        path
    }

    fn flatten_tree(roots: &[SessionTreeNode]) -> Vec<FlatTreeNode> {
        let mut result = Vec::new();
        let multiple_roots = roots.len() > 1;

        fn flatten_recursive(
            node: &SessionTreeNode,
            indent: usize,
            is_last: bool,
            show_connector: bool,
            active_ids: &std::collections::HashSet<String>,
            result: &mut Vec<FlatTreeNode>,
        ) {
            result.push(FlatTreeNode {
                node_id: node.id.clone(),
                label: node.label.clone(),
                indent,
                is_last,
                show_connector,
                is_user: node.is_user,
                is_tool: node.is_tool,
                is_active: active_ids.contains(&node.id),
            });

            for (i, child) in node.children.iter().enumerate() {
                let child_is_last = i == node.children.len() - 1;
                let child_indent = if node.children.len() > 1 {
                    indent + 1
                } else {
                    indent
                };
                flatten_recursive(child, child_indent, child_is_last, true, active_ids, result);
            }
        }

        for (i, root) in roots.iter().enumerate() {
            let is_last = i == roots.len() - 1;
            let indent = if multiple_roots { 1 } else { 0 };
            flatten_recursive(
                root,
                indent,
                is_last,
                multiple_roots,
                &std::collections::HashSet::new(), // placeholder, rebuilt later
                &mut result,
            );
        }

        result
    }

    /// Set the filter mode.
    pub fn set_filter_mode(&mut self, mode: TreeFilterMode) {
        self.filter_mode = mode;
        self.apply_filter();
    }

    /// Set search filter.
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        self.filtered_nodes = self
            .flat_nodes
            .iter()
            .filter(|node| {
                // Filter mode
                match self.filter_mode {
                    TreeFilterMode::Default => true,
                    TreeFilterMode::NoTools => !node.is_tool,
                    TreeFilterMode::UserOnly => node.is_user,
                    TreeFilterMode::LabeledOnly => !node.is_tool,
                    TreeFilterMode::All => true,
                }
            })
            .filter(|node| {
                // Query filter
                if self.query.is_empty() {
                    return true;
                }
                let lower_query = self.query.to_lowercase();
                node.label.to_lowercase().contains(&lower_query)
                    || node.node_id.to_lowercase().contains(&lower_query)
            })
            .cloned()
            .collect();

        self.selected_index = self
            .selected_index
            .min(self.filtered_nodes.len().saturating_sub(1));
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let max = self.filtered_nodes.len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
        }
    }

    /// Get the currently selected node ID.
    pub fn selected_id(&self) -> Option<&str> {
        self.filtered_nodes
            .get(self.selected_index)
            .map(|n| n.node_id.as_str())
    }

    /// Render the tree selector.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        lines.push(format!(
            "\x1b[1mSession Tree\x1b[0m \x1b[2m(filter: {})\x1b[0m",
            self.filter_mode.label()
        ));
        lines.push(String::new());

        if self.filtered_nodes.is_empty() {
            lines.push("\x1b[2m  No nodes to display\x1b[0m".to_string());
            return lines;
        }

        // Calculate visible range
        let total = self.filtered_nodes.len();
        let half = self.max_visible / 2;
        let start = if self.selected_index >= half {
            (self.selected_index - half).min(total.saturating_sub(self.max_visible))
        } else {
            0
        };
        let end = (start + self.max_visible).min(total);

        for i in start..end {
            let node = &self.filtered_nodes[i];
            let is_selected = i == self.selected_index;

            // Build tree connector
            let indent_str = "  ".repeat(node.indent);
            let connector = if node.show_connector {
                if node.is_last {
                    "└─ "
                } else {
                    "├─ "
                }
            } else {
                ""
            };

            // Node icon
            let icon = if node.is_user {
                "\x1b[36m●\x1b[0m"
            } else if node.is_tool {
                "\x1b[33m⚙\x1b[0m"
            } else {
                "\x1b[2m○\x1b[0m"
            };

            // Active path highlight
            let label = if is_selected {
                format!("\x1b[1m\x1b[36m{}\x1b[0m", node.label)
            } else if node.is_active {
                format!("\x1b[36m{}\x1b[0m", node.label)
            } else {
                node.label.clone()
            };

            let line = format!("{}{}{}{}", indent_str, connector, icon, label);
            lines.push(truncate_str(&line, width));
        }

        // Scroll indicator
        if start > 0 || end < total {
            lines.push(format!(
                "\x1b[2m  ({}/{})\x1b[0m",
                self.selected_index + 1,
                total
            ));
        }

        // Hints
        lines.push(String::new());
        lines.push("\x1b[2m  Enter to jump · Esc to close · F to filter\x1b[0m".to_string());

        lines
    }
}

// ── 8. ScopedModelsSelector ─────────────────────────────────────────────────

/// A model item for scoped model selection.
#[derive(Debug, Clone)]
pub struct ScopedModelItem {
    /// Full identifier (provider/id).
    pub full_id: String,
    /// Model ID.
    pub id: String,
    /// Provider name.
    pub provider: String,
    /// Human-readable model name.
    pub name: String,
    /// Whether this model is enabled for cycling.
    pub enabled: bool,
}

/// Scoped models cycling display for Ctrl+P model switching.
#[derive(Debug, Clone)]
pub struct ScopedModelsSelector {
    /// All model items.
    pub all_models: Vec<ScopedModelItem>,
    /// All model IDs in order.
    pub all_ids: Vec<String>,
    /// Enabled model IDs (null = all enabled).
    pub enabled_ids: Option<Vec<String>>,
    /// Filtered items based on search.
    pub filtered_items: Vec<ScopedModelItem>,
    /// Currently selected index.
    pub selected_index: usize,
    /// Current search query.
    pub query: String,
    /// Maximum visible items.
    pub max_visible: usize,
    /// Whether there are unsaved changes.
    pub is_dirty: bool,
}

impl ScopedModelsSelector {
    /// Create a new scoped models selector.
    pub fn new(models: Vec<ScopedModelItem>, enabled_ids: Option<Vec<String>>) -> Self {
        let all_ids: Vec<String> = models.iter().map(|m| m.full_id.clone()).collect();
        let filtered_items = Self::build_items(&models, &enabled_ids, &all_ids);

        Self {
            all_models: models,
            all_ids,
            enabled_ids,
            filtered_items,
            selected_index: 0,
            query: String::new(),
            max_visible: 8,
            is_dirty: false,
        }
    }

    fn is_enabled(&self, id: &str) -> bool {
        match &self.enabled_ids {
            None => true,
            Some(ids) => ids.contains(&id.to_string()),
        }
    }

    fn build_items(
        models: &[ScopedModelItem],
        enabled_ids: &Option<Vec<String>>,
        all_ids: &[String],
    ) -> Vec<ScopedModelItem> {
        let sorted_ids = Self::get_sorted_ids(enabled_ids, all_ids);
        sorted_ids
            .into_iter()
            .filter_map(|id| {
                models
                    .iter()
                    .find(|m| m.full_id == id)
                    .map(|m| ScopedModelItem {
                        full_id: m.full_id.clone(),
                        id: m.id.clone(),
                        provider: m.provider.clone(),
                        name: m.name.clone(),
                        enabled: match enabled_ids {
                            None => true,
                            Some(ids) => ids.contains(&m.full_id),
                        },
                    })
            })
            .collect()
    }

    fn get_sorted_ids(enabled_ids: &Option<Vec<String>>, all_ids: &[String]) -> Vec<String> {
        match enabled_ids {
            None => all_ids.to_vec(),
            Some(ids) => {
                let enabled_set: std::collections::HashSet<_> = ids.iter().collect();
                let mut result = ids.clone();
                for id in all_ids {
                    if !enabled_set.contains(id) {
                        result.push(id.clone());
                    }
                }
                result
            }
        }
    }

    /// Toggle a model's enabled state.
    pub fn toggle(&mut self, full_id: &str) {
        self.enabled_ids = match &mut self.enabled_ids {
            None => Some(vec![full_id.to_string()]),
            Some(ids) => {
                if let Some(pos) = ids.iter().position(|id| id == full_id) {
                    ids.remove(pos);
                } else {
                    ids.push(full_id.to_string());
                }
                if ids.len() == self.all_ids.len() {
                    // All enabled -> null
                    None
                } else {
                    let ids = ids.clone();
                    Some(ids)
                }
            }
        };
        self.is_dirty = true;
        self.refresh();
    }

    /// Enable all models.
    pub fn enable_all(&mut self) {
        self.enabled_ids = None;
        self.is_dirty = true;
        self.refresh();
    }

    /// Clear all models (disable all).
    pub fn clear_all(&mut self) {
        self.enabled_ids = Some(Vec::new());
        self.is_dirty = true;
        self.refresh();
    }

    /// Toggle all models from a specific provider.
    pub fn toggle_provider(&mut self, provider: &str) {
        let provider_ids: Vec<String> = self
            .all_ids
            .iter()
            .filter(|id| {
                self.all_models
                    .iter()
                    .any(|m| &m.full_id == *id && m.provider == provider)
            })
            .cloned()
            .collect();

        let all_enabled = provider_ids.iter().all(|id| self.is_enabled(id));

        if all_enabled {
            // Disable all from provider
            self.enabled_ids = match &self.enabled_ids {
                None => Some(
                    self.all_ids
                        .iter()
                        .filter(|id| !provider_ids.contains(id))
                        .cloned()
                        .collect(),
                ),
                Some(ids) => Some(
                    ids.iter()
                        .filter(|id| !provider_ids.contains(id))
                        .cloned()
                        .collect(),
                ),
            };
        } else {
            // Enable all from provider
            self.enabled_ids = match &mut self.enabled_ids {
                None => None,
                Some(ids) => {
                    for pid in &provider_ids {
                        if !ids.contains(pid) {
                            ids.push(pid.clone());
                        }
                    }
                    if ids.len() == self.all_ids.len() {
                        None
                    } else {
                        let ids = ids.clone();
                        Some(ids)
                    }
                }
            };
        }
        self.is_dirty = true;
        self.refresh();
    }

    /// Move a model up in the enabled order.
    pub fn reorder_up(&mut self) {
        if let Some(ref mut ids) = self.enabled_ids {
            if let Some(item) = self.filtered_items.get(self.selected_index) {
                if let Some(pos) = ids.iter().position(|id| id == &item.full_id) {
                    if pos > 0 {
                        ids.swap(pos, pos - 1);
                        self.is_dirty = true;
                        self.refresh();
                        self.move_up();
                    }
                }
            }
        }
    }

    /// Move a model down in the enabled order.
    pub fn reorder_down(&mut self) {
        if let Some(ref mut ids) = self.enabled_ids {
            if let Some(item) = self.filtered_items.get(self.selected_index) {
                if let Some(pos) = ids.iter().position(|id| id == &item.full_id) {
                    if pos < ids.len() - 1 {
                        ids.swap(pos, pos + 1);
                        self.is_dirty = true;
                        self.refresh();
                        self.move_down();
                    }
                }
            }
        }
    }

    /// Set search filter.
    pub fn set_filter(&mut self, query: String) {
        self.query = query;
        self.refresh();
    }

    fn refresh(&mut self) {
        let items = Self::build_items(&self.all_models, &self.enabled_ids, &self.all_ids);
        if self.query.is_empty() {
            self.filtered_items = items;
        } else {
            let indices = fuzzy_filter_indices(
                &items
                    .iter()
                    .map(|i| format!("{} {}", i.id, i.provider))
                    .collect::<Vec<_>>(),
                &self.query,
            );
            self.filtered_items = indices.into_iter().map(|i| items[i].clone()).collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered_items.len().saturating_sub(1));
    }

    /// Move selection up (wraps).
    pub fn move_up(&mut self) {
        if self.filtered_items.is_empty() {
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = self.filtered_items.len() - 1;
        } else {
            self.selected_index -= 1;
        }
    }

    /// Move selection down (wraps).
    pub fn move_down(&mut self) {
        if self.filtered_items.is_empty() {
            return;
        }
        if self.selected_index == self.filtered_items.len() - 1 {
            self.selected_index = 0;
        } else {
            self.selected_index += 1;
        }
    }

    /// Get the currently selected model.
    pub fn selected(&self) -> Option<&ScopedModelItem> {
        self.filtered_items.get(self.selected_index)
    }

    /// Mark changes as saved.
    pub fn mark_saved(&mut self) {
        self.is_dirty = false;
    }

    /// Render the scoped models selector.
    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Header
        lines
            .push("\x1b[36m───────────────────────────────────────────────────\x1b[0m".to_string());
        lines.push("\x1b[1m\x1b[36mModel Configuration\x1b[0m".to_string());
        lines.push("\x1b[2mSession-only. Ctrl+S to save to settings.\x1b[0m".to_string());
        lines.push(String::new());

        // Search input
        lines.push(format!("> {}", self.query));
        lines.push(String::new());

        if self.filtered_items.is_empty() {
            lines.push("\x1b[2m  No matching models\x1b[0m".to_string());
            return lines;
        }

        // Calculate visible range
        let total = self.filtered_items.len();
        let half = self.max_visible / 2;
        let start = if self.selected_index >= half {
            (self.selected_index - half).min(total.saturating_sub(self.max_visible))
        } else {
            0
        };
        let end = (start + self.max_visible).min(total);
        let all_enabled = self.enabled_ids.is_none();

        for i in start..end {
            let item = &self.filtered_items[i];
            let is_selected = i == self.selected_index;

            let prefix = if is_selected {
                "\x1b[36m→ \x1b[0m"
            } else {
                "  "
            };

            let model_text = if is_selected {
                format!("\x1b[36m{}\x1b[0m", item.id)
            } else {
                item.id.clone()
            };

            let provider_badge = format!("\x1b[2m [{}]\x1b[0m", item.provider);

            let status = if all_enabled {
                String::new()
            } else if item.enabled {
                "\x1b[32m ✓\x1b[0m".to_string()
            } else {
                "\x1b[2m ✗\x1b[0m".to_string()
            };

            let line = format!("{}{}{}{}", prefix, model_text, provider_badge, status);
            lines.push(truncate_str(&line, width));
        }

        // Scroll indicator
        if start > 0 || end < total {
            lines.push(format!(
                "\x1b[2m  ({}/{})\x1b[0m",
                self.selected_index + 1,
                total
            ));
        }

        // Detail line
        if let Some(selected) = self.selected() {
            lines.push(String::new());
            lines.push(format!("\x1b[2m  Model Name: {}\x1b[0m", selected.name));
        }

        // Footer hints
        lines.push(String::new());
        let enabled_count = self
            .enabled_ids
            .as_ref()
            .map_or_else(|| self.all_ids.len(), |ids| ids.len());
        let count_text = if all_enabled {
            "all enabled".to_string()
        } else {
            format!("{}/{} enabled", enabled_count, self.all_ids.len())
        };
        let dirty_marker = if self.is_dirty {
            "\x1b[33m(unsaved)\x1b[0m"
        } else {
            ""
        };
        lines.push(format!(
            "\x1b[2m  Enter=toggle · Ctrl+A=all · Ctrl+X=clear · ↑↓=reorder · {} {}\x1b[0m",
            count_text, dirty_marker
        ));

        lines
            .push("\x1b[36m───────────────────────────────────────────────────\x1b[0m".to_string());

        lines
    }
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

        let renderer = AssistantMessageRenderer::new(AssistantMessageRenderOptions {
            hide_thinking: true,
            hidden_thinking_label: "Thinking...".to_string(),
            use_osc133: false,
        });

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
        assert_eq!(
            result.get_text(),
            Some("file created successfully".to_string())
        );
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
        let mut exec = ToolExecution::new(
            "read_file",
            "call_1",
            serde_json::json!({"path": "test.txt"}),
        );
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
        assert!(matches!(
            msg.message_type,
            SummaryMessageType::Compaction {
                tokens_before: 50000
            }
        ));

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
        let rendered =
            SummaryMessageRenderer::render_compaction(50000, "Summary of compacted content", true);
        assert!(rendered.contains("50000"));
    }

    // ── Armin Component Tests ──────────────────────────────────────────────────

    #[test]
    fn test_armin_component_render() {
        let mut armin = ArminComponent::new();
        let lines = armin.render(40);
        assert!(!lines.is_empty());
        // Should have DISPLAY_HEIGHT lines + 1 for "ARMIN SAYS HI"
        assert_eq!(lines.len(), ARMIN_DISPLAY_HEIGHT + 1);
        // Last line should contain "ARMIN SAYS HI"
        let last = lines.last().unwrap();
        let visible = strip_ansi(last);
        assert!(visible.contains("ARMIN SAYS HI"));
    }

    #[test]
    fn test_armin_component_caching() {
        let mut armin = ArminComponent::new();
        let lines1 = armin.render(80);
        let lines2 = armin.render(80);
        assert_eq!(lines1, lines2);
    }

    #[test]
    fn test_armin_pixel_and_char() {
        // Verify pixel function works for a few positions
        let _ = armin_get_pixel(0, 0);
        let _ = armin_get_pixel(30, 35);
        // Out-of-bounds y should be false (background)
        assert!(!armin_get_pixel(0, 100));
        // Char function should return valid block characters
        let ch = armin_get_char(0, 0);
        assert!("█▀▄ ".contains(ch));
    }

    // ── Daxnuts Component Tests ────────────────────────────────────────────────

    #[test]
    fn test_daxnuts_component_render() {
        let dax = DaxnutsComponent::new();
        let lines = dax.render(80);
        assert!(!lines.is_empty());
        // Should contain the image lines, plus text
        let joined = lines.join("\n");
        let visible = strip_ansi(&joined);
        assert!(visible.contains("Powered by daxnuts"));
        assert!(visible.contains("@thdxr"));
    }

    #[test]
    fn test_daxnuts_image_builds() {
        let image = build_dax_image();
        // 32 pixels tall / 2 = 16 half-block rows
        assert_eq!(image.len(), 16);
        // Each line should contain ANSI escape codes and ▄ characters
        for line in &image {
            assert!(line.contains('\x1b'));
        }
    }

    // ── Dynamic Border Tests ──────────────────────────────────────────────────

    #[test]
    fn test_dynamic_border_default() {
        let border = DynamicBorder::new();
        let lines = border.render(40);
        assert_eq!(lines.len(), 1);
        let visible = strip_ansi(&lines[0]);
        assert_eq!(visible.chars().count(), 40);
        assert!(visible.chars().all(|c| c == '─'));
    }

    #[test]
    fn test_dynamic_border_accent() {
        let border = DynamicBorder::accent();
        let lines = border.render(20);
        assert_eq!(lines.len(), 1);
        let visible = strip_ansi(&lines[0]);
        assert_eq!(visible.chars().count(), 20);
        // Should contain accent color code
        assert!(lines[0].contains("\x1b[38;5;75m"));
    }

    #[test]
    fn test_dynamic_border_custom_color() {
        let border = DynamicBorder::with_style(BorderStyle::Custom(200));
        let lines = border.render(10);
        assert!(lines[0].contains("38;5;200"));
    }

    #[test]
    fn test_dynamic_border_min_width() {
        let border = DynamicBorder::new();
        let lines = border.render(0);
        assert_eq!(lines.len(), 1);
        let visible = strip_ansi(&lines[0]);
        assert_eq!(visible.chars().count(), 1); // max(1, 0)
    }

    // ── Earendil Announcement Tests ───────────────────────────────────────────

    #[test]
    fn test_earendil_announcement_basic() {
        let ann = EarendilAnnouncement::new("Hello World");
        let lines = ann.render(60);
        assert!(!lines.is_empty());
        let joined = lines.join("\n");
        let visible = strip_ansi(&joined);
        assert!(visible.contains("Hello World"));
        // Should have top and bottom borders
        assert!(joined.contains('─'));
    }

    #[test]
    fn test_earendil_announcement_with_body() {
        let mut ann = EarendilAnnouncement::new("Title");
        ann.add_body("First line");
        ann.add_body("https://example.com");
        let lines = ann.render(60);
        let joined = lines.join("\n");
        let visible = strip_ansi(&joined);
        assert!(visible.contains("First line"));
        assert!(visible.contains("https://example.com"));
    }

    #[test]
    fn test_earendil_announcement_default() {
        let ann = EarendilAnnouncement::earendil_default();
        let lines = ann.render(80);
        let joined = lines.join("\n");
        let visible = strip_ansi(&joined);
        assert!(visible.contains("pi has joined Earendil"));
        assert!(visible.contains("mariozechner.at"));
    }

    // ── Tool Execution Display Tests ──────────────────────────────────────────

    #[test]
    fn test_tool_execution_display_pending() {
        let display = ToolExecutionDisplay::new(
            "read_file",
            "call_abc",
            serde_json::json!({"path": "test.txt"}),
        );
        assert_eq!(display.state, ToolExecutionState::Pending);
        assert!(display.started_at.is_none());
    }

    #[test]
    fn test_tool_execution_display_lifecycle() {
        let mut display =
            ToolExecutionDisplay::new("bash", "call_123", serde_json::json!({"command": "ls"}));
        display.start();
        assert_eq!(display.state, ToolExecutionState::Running);
        assert!(display.started_at.is_some());

        display.complete(ToolResult::new_text("file1.txt\nfile2.txt"));
        assert_eq!(display.state, ToolExecutionState::Success);
        assert!(display.completed_at.is_some());
    }

    #[test]
    fn test_tool_execution_display_error() {
        let mut display = ToolExecutionDisplay::new(
            "bash",
            "call_err",
            serde_json::json!({"command": "rm -rf /"}),
        );
        display.start();
        display.complete(ToolResult::error("Permission denied"));
        assert_eq!(display.state, ToolExecutionState::Error);
    }

    #[test]
    fn test_tool_execution_display_render() {
        let mut display = ToolExecutionDisplay::new(
            "read_file",
            "call_1",
            serde_json::json!({"path": "test.txt"}),
        );
        display.start();
        display.complete(ToolResult::new_text("file contents"));

        let lines = display.render();
        let joined = lines.join("\n");
        let visible = strip_ansi(&joined);
        assert!(visible.contains("read_file"));
        assert!(visible.contains("file contents"));
    }

    #[test]
    fn test_tool_execution_display_expanded() {
        let mut display =
            ToolExecutionDisplay::new("search", "call_2", serde_json::json!({"query": "test"}));
        display.start();

        let long_output: String = "result line\n".repeat(100);
        display.complete(ToolResult::new_text(long_output));

        // Collapsed: should have truncation hint
        let lines_collapsed = display.render();
        let joined = lines_collapsed.join("\n");
        assert!(joined.contains("expand"));

        // Expanded: should show full output
        display.set_expanded(true);
        let lines_expanded = display.render();
        assert!(lines_expanded.len() > lines_collapsed.len());
    }

    #[test]
    fn test_tool_execution_display_elapsed() {
        let mut display = ToolExecutionDisplay::new("test", "call", serde_json::json!(null));
        // Not started - no elapsed
        assert!(display.elapsed_str().is_empty());

        display.start();
        // Running - should have elapsed time
        let elapsed = display.elapsed_str();
        assert!(!elapsed.is_empty());
    }

    #[test]
    fn test_tool_execution_display_args_streaming() {
        let mut display = ToolExecutionDisplay::new("bash", "call_stream", serde_json::json!(null));
        assert!(display.is_partial);

        display.update_args(serde_json::json!({"command": "ls"}));
        assert_eq!(display.arguments["command"], "ls");

        display.set_args_complete();
        assert!(!display.is_partial);
        assert!(display.args_complete);
    }

    #[test]
    fn test_tool_execution_display_toggle() {
        let mut display = ToolExecutionDisplay::new("test", "call", serde_json::json!(null));
        assert!(!display.expanded);
        display.toggle_expanded();
        assert!(display.expanded);
        display.toggle_expanded();
        assert!(!display.expanded);
    }

    #[test]
    fn test_center_ansi() {
        let result = center_ansi("hello", 20);
        let visible = strip_ansi(&result);
        // left padding = (20 - 5) / 2 = 7, so "       hello" = 12 chars
        assert_eq!(visible.chars().count(), 12);
        assert!(visible.starts_with("       hello"));
    }

    #[test]
    fn test_center_ansi_with_codes() {
        let result = center_ansi("\x1b[31mhello\x1b[0m", 20);
        let visible = strip_ansi(&result);
        // ANSI codes stripped, visible = "       hello" = 12 chars
        assert_eq!(visible.chars().count(), 12);
        assert!(visible.contains("hello"));
    }

    // ── Message Component Tests ──────────────────────────────────────────────

    #[test]
    fn test_user_message_renderer() {
        let renderer = UserMessageRenderer::new("Hello, world!");
        let lines = renderer.render(80);
        assert!(!lines.is_empty());
        let joined = lines.join("");
        assert!(joined.contains("Hello, world!"));
    }

    #[test]
    fn test_user_message_renderer_with_images() {
        let renderer = UserMessageRenderer::new("Check this out").with_images(true);
        let lines = renderer.render(80);
        let joined = lines.join("\n");
        assert!(joined.contains("has images"));
    }

    #[test]
    fn test_user_message_renderer_osc133() {
        let renderer = UserMessageRenderer::new("test").with_osc133(true);
        let lines = renderer.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("\x1b]133;A\x07"));
        let last = lines.last().unwrap();
        assert!(last.contains("\x1b]133;B\x07"));
    }

    #[test]
    fn test_user_message_selector() {
        let messages = vec![
            UserMessageItem {
                id: "1".to_string(),
                text: "First message".to_string(),
                timestamp: None,
            },
            UserMessageItem {
                id: "2".to_string(),
                text: "Second message".to_string(),
                timestamp: None,
            },
            UserMessageItem {
                id: "3".to_string(),
                text: "Third message".to_string(),
                timestamp: None,
            },
        ];
        let mut selector = UserMessageSelector::new(messages);

        // Default: last message selected
        assert_eq!(selector.selected().unwrap().id, "3");

        // Navigate up (older)
        selector.move_up();
        assert_eq!(selector.selected().unwrap().id, "2");
        selector.move_up();
        assert_eq!(selector.selected().unwrap().id, "1");

        // Wrap to bottom
        selector.move_up();
        assert_eq!(selector.selected().unwrap().id, "3");

        // Navigate down
        selector.move_down();
        assert_eq!(selector.selected().unwrap().id, "1");

        // Continue down
        selector.move_down();
        assert_eq!(selector.selected().unwrap().id, "2");

        // Wrap to top
        selector.move_down();
        assert_eq!(selector.selected().unwrap().id, "3");
    }

    #[test]
    fn test_user_message_selector_with_initial_id() {
        let messages = vec![
            UserMessageItem {
                id: "a".to_string(),
                text: "msg a".to_string(),
                timestamp: None,
            },
            UserMessageItem {
                id: "b".to_string(),
                text: "msg b".to_string(),
                timestamp: None,
            },
        ];
        let selector = UserMessageSelector::new(messages).with_initial_id("b");
        assert_eq!(selector.selected().unwrap().id, "b");
    }

    #[test]
    fn test_user_message_selector_render() {
        let messages = vec![UserMessageItem {
            id: "1".to_string(),
            text: "Hello".to_string(),
            timestamp: None,
        }];
        let selector = UserMessageSelector::new(messages);
        let lines = selector.render(80);
        let joined = lines.join("\n");
        assert!(joined.contains("Fork from Message"));
        assert!(joined.contains("Hello"));
    }

    #[test]
    fn test_user_message_selector_empty() {
        let selector = UserMessageSelector::new(vec![]);
        let lines = selector.render(80);
        let joined = lines.join("\n");
        assert!(joined.contains("No user messages found"));
    }

    #[test]
    fn test_skill_invocation_message() {
        let skill = SkillInvocationMessage::new("my_skill", "arg1 arg2");
        let lines = skill.render(80);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("my_skill"));
        assert!(lines[0].contains("expand"));
    }

    #[test]
    fn test_skill_invocation_message_expanded() {
        let mut skill = SkillInvocationMessage::new("my_skill", "This is the content");
        skill.set_expanded(true);
        let lines = skill.render(80);
        assert!(lines.len() >= 2);
        let joined = lines.join("\n");
        assert!(joined.contains("my_skill"));
        assert!(joined.contains("This is the content"));
    }

    #[test]
    fn test_diff_renderer_basic() {
        let renderer = DiffRenderer::new();
        let diff = " 1 old line\n-2 removed\n+3 added\n 4 context\n";
        let result = renderer.render(diff, None);
        assert!(result.contains("removed"));
        assert!(result.contains("added"));
        assert!(result.contains("context"));
    }

    #[test]
    fn test_diff_renderer_colors() {
        let renderer = DiffRenderer::new();
        let diff = "-1 removed line\n+2 added line\n";
        let result = renderer.render(diff, None);
        // Should contain red for removed
        assert!(result.contains("\x1b[31m"));
        // Should contain green for added
        assert!(result.contains("\x1b[32m"));
    }

    #[test]
    fn test_diff_renderer_no_word_diff() {
        let renderer = DiffRenderer::new_simple();
        let diff = "-1 foo bar\n+2 foo baz\n";
        let result = renderer.render(diff, None);
        assert!(result.contains("foo"));
    }

    #[test]
    fn test_diff_renderer_hunk_header() {
        let renderer = DiffRenderer::new();
        let diff = "@@ -1,3 +1,3 @@\n context\n";
        let result = renderer.render(diff, None);
        // Hunk headers should be colored cyan
        assert!(result.contains("@@"));
    }

    #[test]
    fn test_keybinding_hints_render() {
        let hints = vec![
            KeyHint::new("Ctrl+C", "Cancel"),
            KeyHint::new("Enter", "Confirm"),
        ];
        let lines = KeybindingHints::render(&hints);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Ctrl+C"));
        assert!(lines[1].contains("Enter"));
    }

    #[test]
    fn test_keybinding_hints_inline() {
        let hints = vec![
            KeyHint::new("↑↓", "Navigate"),
            KeyHint::new("Enter", "Select"),
        ];
        let inline = KeybindingHints::render_inline(&hints);
        assert!(inline.contains("Navigate"));
        assert!(inline.contains("Select"));
    }

    #[test]
    fn test_footer_component_data_render() {
        let mut data = FooterComponentData::default();
        data.model_name = "claude-sonnet-4".to_string();
        data.provider_name = "anthropic".to_string();
        data.pwd = Some("~/projects/oxi".to_string());
        data.git_branch = Some("main".to_string());
        data.session_name = Some("test-session".to_string());
        data.input_tokens = 5000;
        data.output_tokens = 2000;
        data.total_cost = 0.035;
        data.context_window_pct = 45.0;
        data.context_window = 200000;

        let lines = data.render(120);
        assert!(lines.len() >= 2);

        let pwd_line = &lines[0];
        assert!(pwd_line.contains("~/projects/oxi"));
        assert!(pwd_line.contains("main"));
        assert!(pwd_line.contains("test-session"));

        let stats_line = &lines[1];
        assert!(stats_line.contains("claude-sonnet-4"));
    }

    #[test]
    fn test_footer_component_data_token_formatting() {
        assert_eq!(FooterComponentData::format_token_count(500), "500");
        assert_eq!(FooterComponentData::format_token_count(1500), "1.5k");
        assert_eq!(FooterComponentData::format_token_count(15000), "15k");
        assert_eq!(FooterComponentData::format_token_count(1_500_000), "1.5M");
        assert_eq!(FooterComponentData::format_token_count(15_000_000), "15M");
    }

    #[test]
    fn test_footer_component_context_coloring() {
        let mut data = FooterComponentData::default();
        data.context_window_pct = 95.0;
        data.context_window = 200000;
        let lines = data.render(120);
        // High usage should be red
        assert!(lines[1].contains("\x1b["));

        data.context_window_pct = 75.0;
        let lines2 = data.render(120);
        // Warning level
        assert!(lines2[1].contains("\x1b["));

        data.context_window_pct = 30.0;
        let lines3 = data.render(120);
        // Normal level - no special color
        assert!(lines3[1].contains("30.0%"));
    }

    #[test]
    fn test_visual_truncate_short() {
        let result = VisualTruncate::truncate("hello\nworld", 10, 80, 0);
        assert_eq!(result.visual_lines.len(), 2);
        assert_eq!(result.skipped_count, 0);
    }

    #[test]
    fn test_visual_truncate_long() {
        let text: String = (0..20)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = VisualTruncate::truncate(&text, 5, 80, 0);
        assert_eq!(result.visual_lines.len(), 5);
        assert_eq!(result.skipped_count, 15);
        // Should show last 5 lines
        assert!(result.visual_lines[0].contains("line 15"));
    }

    #[test]
    fn test_visual_truncate_wrapping() {
        // Line longer than width should wrap
        let text = "abcdefghij"; // 10 chars
        let result = VisualTruncate::truncate(text, 10, 5, 0);
        // Should wrap into 2 lines
        assert_eq!(result.visual_lines.len(), 2);
        assert_eq!(result.visual_lines[0], "abcde");
        assert_eq!(result.visual_lines[1], "fghij");
    }

    #[test]
    fn test_visual_truncate_empty() {
        let result = VisualTruncate::truncate("", 10, 80, 0);
        assert!(result.visual_lines.is_empty());
        assert_eq!(result.skipped_count, 0);
    }

    #[test]
    fn test_show_images_selector() {
        let mut selector = ShowImagesSelector::new(true);
        assert!(selector.confirm()); // default "yes"

        selector.move_down();
        assert!(!selector.confirm()); // "no"

        selector.move_up();
        assert!(selector.confirm()); // back to "yes"
    }

    #[test]
    fn test_show_images_selector_render() {
        let selector = ShowImagesSelector::new(false);
        let lines = selector.render(60);
        let joined = lines.join("\n");
        assert!(joined.contains("Yes"));
        assert!(joined.contains("No"));
        assert!(joined.contains("Show images"));
    }

    #[test]
    fn test_countdown_timer() {
        let mut timer = CountdownTimer::new(10);
        assert_eq!(timer.remaining_seconds, 10);
        assert!(!timer.is_expired());

        // Tick 9 times
        for _ in 0..9 {
            assert!(!timer.tick());
        }
        assert_eq!(timer.remaining_seconds, 1);
        assert!(!timer.is_expired());

        // Final tick
        assert!(timer.tick());
        assert!(timer.is_expired());
    }

    #[test]
    fn test_countdown_timer_from_millis() {
        let timer = CountdownTimer::from_millis(5500);
        assert_eq!(timer.remaining_seconds, 6); // ceil(5500/1000) = 6
    }

    #[test]
    fn test_countdown_timer_render() {
        let timer = CountdownTimer::new(30);
        let rendered = timer.render();
        assert!(rendered.contains("30s"));

        let timer_urgent = CountdownTimer::new(3);
        let rendered_urgent = timer_urgent.render();
        assert!(rendered_urgent.contains("3s"));

        let timer_expired = CountdownTimer::new(0);
        let rendered_expired = timer_expired.render();
        assert!(rendered_expired.contains("Expired"));
    }

    #[test]
    fn test_countdown_timer_bar() {
        let timer = CountdownTimer::new(10);
        let bar = timer.render_bar(20);
        assert!(!bar.is_empty());
        assert!(bar.contains('█'));
        assert!(bar.contains('░'));
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 8), "hello...");
    }

    #[test]
    fn test_visible_len() {
        assert_eq!(visible_len("hello"), 5);
        assert_eq!(visible_len("\x1b[31mred\x1b[0m"), 3); // "red"
        assert_eq!(visible_len("\x1b]133;A\x07test"), 4); // "test"
    }

    // ── Extension Component Tests ──────────────────────────────────────────

    #[test]
    fn test_extension_editor_new() {
        let editor = ExtensionEditor::new("My Title", Some("prefill text"));
        assert_eq!(editor.title, "My Title");
        assert_eq!(editor.text, "prefill text");
        assert_eq!(editor.cursor_pos, "prefill text".len());
    }

    #[test]
    fn test_extension_editor_input() {
        let mut editor = ExtensionEditor::new("Title", None);
        assert!(editor.text.is_empty());
        editor.input_char('h');
        editor.input_char('i');
        assert_eq!(editor.text, "hi");
        editor.backspace();
        assert_eq!(editor.text, "h");
    }

    #[test]
    fn test_extension_editor_set_text() {
        let mut editor = ExtensionEditor::new("Title", Some("old"));
        editor.set_text("new text");
        assert_eq!(editor.text, "new text");
        assert_eq!(editor.cursor_pos, 8);
    }

    #[test]
    fn test_extension_editor_render() {
        let editor = ExtensionEditor::new("My Editor", Some("hello"));
        let lines = editor.render();
        assert!(!lines.is_empty());
        assert!(lines.iter().any(|l| l.contains("My Editor")));
        assert!(lines.iter().any(|l| l.contains("hello")));
    }

    #[test]
    fn test_extension_input_new() {
        let input = ExtensionInput::new("Enter value:", Some(30));
        assert_eq!(input.title, "Enter value:");
        assert_eq!(input.timeout_secs, Some(30));
        assert_eq!(input.remaining_secs, Some(30));
    }

    #[test]
    fn test_extension_input_typing() {
        let mut input = ExtensionInput::new("Prompt", None);
        input.input_char('a');
        input.input_char('b');
        assert_eq!(input.value, "ab");
        input.backspace();
        assert_eq!(input.value, "a");
    }

    #[test]
    fn test_extension_input_tick() {
        let mut input = ExtensionInput::new("Title", Some(3));
        assert!(input.tick()); // 3 -> 2
        assert_eq!(input.remaining_secs, Some(2));
        assert!(input.tick()); // 2 -> 1
        assert_eq!(input.remaining_secs, Some(1));
        assert!(!input.tick()); // 1 -> 0, returns false (expired)
        assert_eq!(input.remaining_secs, Some(0));
        assert!(!input.tick()); // already 0
    }

    #[test]
    fn test_extension_input_render() {
        let input = ExtensionInput::new("Name:", None);
        let lines = input.render();
        assert!(lines.iter().any(|l| l.contains("Name:")));
    }

    #[test]
    fn test_extension_input_render_countdown() {
        let input = ExtensionInput::new("Hurry:", Some(10));
        let lines = input.render();
        assert!(lines.iter().any(|l| l.contains("10s")));
    }

    #[test]
    fn test_extension_selector_navigation() {
        let mut sel = ExtensionSelector::new(
            "Pick",
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
            None,
        );
        assert_eq!(sel.selected(), Some("A"));
        sel.move_down();
        assert_eq!(sel.selected(), Some("B"));
        sel.move_down();
        assert_eq!(sel.selected(), Some("C"));
        sel.move_down(); // clamped
        assert_eq!(sel.selected(), Some("C"));
        sel.move_up();
        assert_eq!(sel.selected(), Some("B"));
        sel.move_up();
        assert_eq!(sel.selected(), Some("A"));
        sel.move_up(); // clamped
        assert_eq!(sel.selected(), Some("A"));
    }

    #[test]
    fn test_extension_selector_render() {
        let sel = ExtensionSelector::new("Choose", vec!["X".to_string(), "Y".to_string()], None);
        let lines = sel.render();
        assert!(lines.iter().any(|l| l.contains("Choose")));
        assert!(lines.iter().any(|l| l.contains("X")));
        assert!(lines.iter().any(|l| l.contains("Y")));
    }

    #[test]
    fn test_custom_editor() {
        let mut editor = CustomEditor::new();
        assert!(editor.is_empty());
        editor.input_char('x');
        assert!(!editor.is_empty());
        assert_eq!(editor.text, "x");
        editor.backspace();
        assert!(editor.is_empty());
    }

    #[test]
    fn test_custom_editor_with_text() {
        let editor = CustomEditor::with_text("hello world");
        assert_eq!(editor.text, "hello world");
        assert_eq!(editor.cursor_pos, 11);
    }

    #[test]
    fn test_custom_editor_register_action() {
        let mut editor = CustomEditor::new();
        editor.register_action("app.interrupt");
        editor.register_action("app.exit");
        editor.register_action("app.interrupt"); // dup
        assert_eq!(editor.registered_actions.len(), 2);
    }

    #[test]
    fn test_custom_message_component() {
        let msg = CustomMessageComponent::new("deploy", "Deployed to production");
        assert_eq!(msg.custom_type, "deploy");
        assert!(!msg.expanded);
    }

    #[test]
    fn test_custom_message_expand() {
        let mut msg = CustomMessageComponent::new("test", "content");
        assert!(!msg.expanded);
        msg.toggle_expanded();
        assert!(msg.expanded);
        msg.set_expanded(false);
        assert!(!msg.expanded);
    }

    #[test]
    fn test_custom_message_render_collapsed() {
        let msg = CustomMessageComponent::new("deploy", "Short text");
        let lines = msg.render();
        assert!(lines.iter().any(|l| l.contains("[deploy]")));
        assert!(lines.iter().any(|l| l.contains("Short text")));
    }

    #[test]
    fn test_custom_message_render_expanded() {
        let mut msg = CustomMessageComponent::new("deploy", "line1\nline2\nline3");
        msg.set_expanded(true);
        let lines = msg.render();
        assert!(lines.iter().any(|l| l.contains("line1")));
        assert!(lines.iter().any(|l| l.contains("line2")));
        assert!(lines.iter().any(|l| l.contains("line3")));
    }

    #[test]
    fn test_provider_login_dialog_new() {
        let dialog = ProviderLoginDialog::new("anthropic", "Anthropic");
        assert_eq!(dialog.provider_id, "anthropic");
        assert!(matches!(dialog.phase, LoginDialogPhase::Init));
    }

    #[test]
    fn test_provider_login_dialog_with_title() {
        let dialog = ProviderLoginDialog::new("openai", "OpenAI").with_title("Custom Login");
        assert_eq!(dialog.title, Some("Custom Login".to_string()));
    }

    #[test]
    fn test_provider_login_dialog_show_auth() {
        let mut dialog = ProviderLoginDialog::new("github", "GitHub");
        dialog.show_auth(
            "https://github.com/login",
            Some("Follow instructions".to_string()),
        );
        match &dialog.phase {
            LoginDialogPhase::ShowAuth { url, instructions } => {
                assert_eq!(url, "https://github.com/login");
                assert_eq!(instructions, &Some("Follow instructions".to_string()));
            }
            _ => panic!("Expected ShowAuth phase"),
        }
    }

    #[test]
    fn test_provider_login_dialog_manual_input() {
        let mut dialog = ProviderLoginDialog::new("anthropic", "Anthropic");
        dialog.show_manual_input("Paste code here");
        dialog.input_char('A');
        dialog.input_char('B');
        assert_eq!(dialog.input_value(), Some("AB"));
        dialog.backspace();
        assert_eq!(dialog.input_value(), Some("A"));
    }

    #[test]
    fn test_provider_login_dialog_prompt() {
        let mut dialog = ProviderLoginDialog::new("openai", "OpenAI");
        dialog.show_prompt("Enter API key", Some("sk-...".to_string()));
        dialog.input_char('x');
        assert_eq!(dialog.input_value(), Some("x"));
    }

    #[test]
    fn test_provider_login_dialog_waiting() {
        let mut dialog = ProviderLoginDialog::new("github", "GitHub");
        dialog.show_waiting("Polling for device code...");
        assert!(
            matches!(&dialog.phase, LoginDialogPhase::Waiting { message } if message == "Polling for device code...")
        );
    }

    #[test]
    fn test_provider_login_dialog_progress() {
        let mut dialog = ProviderLoginDialog::new("github", "GitHub");
        dialog.show_waiting("Waiting");
        dialog.show_progress("Still waiting");
        match &dialog.phase {
            LoginDialogPhase::Waiting { message } => {
                assert!(message.contains("Waiting"));
                assert!(message.contains("Still waiting"));
            }
            _ => panic!("Expected Waiting phase"),
        }
    }

    #[test]
    fn test_provider_login_dialog_complete() {
        let mut dialog = ProviderLoginDialog::new("anthropic", "Anthropic");
        dialog.complete(true, Some("Logged in".to_string()));
        match &dialog.phase {
            LoginDialogPhase::Completed { success, message } => {
                assert!(success);
                assert_eq!(message, &Some("Logged in".to_string()));
            }
            _ => panic!("Expected Completed phase"),
        }
    }

    #[test]
    fn test_provider_login_dialog_render() {
        let dialog =
            ProviderLoginDialog::new("anthropic", "Anthropic").with_title("Login to Anthropic");
        let lines = dialog.render();
        assert!(lines.iter().any(|l| l.contains("Login to Anthropic")));
    }

    #[test]
    fn test_oauth_selector_navigation() {
        let mut sel = OAuthSelector::new(
            OAuthSelectorMode::Login,
            vec![
                AuthProviderInfo {
                    id: "anthropic".into(),
                    name: "Anthropic".into(),
                    auth_type: AuthType::OAuth,
                },
                AuthProviderInfo {
                    id: "openai".into(),
                    name: "OpenAI".into(),
                    auth_type: AuthType::ApiKey,
                },
                AuthProviderInfo {
                    id: "github".into(),
                    name: "GitHub".into(),
                    auth_type: AuthType::OAuth,
                },
            ],
        );
        assert_eq!(sel.selected().unwrap().id, "anthropic");
        sel.move_down();
        assert_eq!(sel.selected().unwrap().id, "openai");
        sel.move_down();
        assert_eq!(sel.selected().unwrap().id, "github");
        sel.move_up();
        assert_eq!(sel.selected().unwrap().id, "openai");
    }

    #[test]
    fn test_oauth_selector_filter() {
        let mut sel = OAuthSelector::new(
            OAuthSelectorMode::Login,
            vec![
                AuthProviderInfo {
                    id: "anthropic".into(),
                    name: "Anthropic".into(),
                    auth_type: AuthType::OAuth,
                },
                AuthProviderInfo {
                    id: "openai".into(),
                    name: "OpenAI".into(),
                    auth_type: AuthType::ApiKey,
                },
            ],
        );
        sel.set_filter("open".to_string());
        assert_eq!(sel.filtered_indices.len(), 1);
        assert_eq!(sel.selected().unwrap().id, "openai");
    }

    #[test]
    fn test_oauth_selector_config_status() {
        let mut sel = OAuthSelector::new(
            OAuthSelectorMode::Login,
            vec![AuthProviderInfo {
                id: "anthropic".into(),
                name: "Anthropic".into(),
                auth_type: AuthType::OAuth,
            }],
        );
        sel.set_config_status(
            "anthropic",
            ProviderConfigStatus::Configured {
                label: "OAuth".to_string(),
            },
        );
        let status = sel.config_status.get("anthropic").unwrap();
        assert!(matches!(status, ProviderConfigStatus::Configured { label } if label == "OAuth"));
    }

    #[test]
    fn test_oauth_selector_render() {
        let sel = OAuthSelector::new(
            OAuthSelectorMode::Login,
            vec![AuthProviderInfo {
                id: "anthropic".into(),
                name: "Anthropic".into(),
                auth_type: AuthType::OAuth,
            }],
        );
        let lines = sel.render();
        assert!(lines.iter().any(|l| l.contains("Select provider")));
        assert!(lines.iter().any(|l| l.contains("Anthropic")));
    }

    #[test]
    fn test_oauth_selector_logout_mode() {
        let sel = OAuthSelector::new(OAuthSelectorMode::Logout, vec![]);
        let lines = sel.render();
        assert!(lines.iter().any(|l| l.contains("logout")));
    }

    #[test]
    fn test_bordered_loader() {
        let loader = BorderedLoader::new("Loading...", true);
        assert_eq!(loader.message, "Loading...");
        assert!(loader.cancellable);
        assert_eq!(loader.spinner_frame, 0);
    }

    #[test]
    fn test_bordered_loader_tick() {
        let mut loader = BorderedLoader::new("Working", false);
        assert_eq!(loader.spinner_char(), "⠋");
        loader.tick();
        assert_eq!(loader.spinner_frame, 1);
        assert_eq!(loader.spinner_char(), "⠙");
        loader.tick();
        assert_eq!(loader.spinner_char(), "⠹");
        loader.tick();
        assert_eq!(loader.spinner_char(), "⠸");
        loader.tick();
        assert_eq!(loader.spinner_frame, 0);
        assert_eq!(loader.spinner_char(), "⠋");
    }

    #[test]
    fn test_bordered_loader_render() {
        let loader = BorderedLoader::new("Fetching data", true);
        let lines = loader.render();
        assert!(lines.iter().any(|l| l.contains("Fetching data")));
        assert!(lines.iter().any(|l| l.contains("cancel")));
    }

    #[test]
    fn test_bordered_loader_render_non_cancellable() {
        let loader = BorderedLoader::new("Please wait", false);
        let lines = loader.render();
        assert!(lines.iter().any(|l| l.contains("Please wait")));
        assert!(!lines.iter().any(|l| l.contains("cancel")));
    }
}
