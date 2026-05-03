//! OAuth callback server for handling OAuth redirects
//!
//! This module provides a simple HTTP server that listens for OAuth callback
//! redirects from providers like Anthropic, OpenAI, GitHub, etc.
//!
//! The server runs on localhost and captures the authorization code and state
//! from the callback URL, then returns them via a channel for token exchange.

use anyhow::{Context, Result};
use std::net::TcpListener;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Default callback port range - we try ports in this range to find an available one
const DEFAULT_PORT_RANGE_START: u16 = 8787;
const DEFAULT_PORT_RANGE_END: u16 = 8887;

/// OAuth callback data containing the authorization code and state
#[derive(Debug, Clone)]
pub struct OAuthCallbackData {
    /// The authorization code from the OAuth provider
    pub code: String,
    /// The state parameter for CSRF verification
    pub state: String,
    /// The full callback URL (for providers that use redirect URI passthrough)
    pub callback_url: Option<String>,
}

/// OAuth callback server that listens for OAuth redirects
pub struct OAuthCallbackServer {
    /// The port the server is listening on
    port: u16,
    /// Shutdown signal sender
    shutdown_tx: Arc<oneshot::Sender<()>>,
    /// Result sender for the callback data
    result_tx: Arc<oneshot::Sender<Result<OAuthCallbackData, OAuthError>>>,
}

impl OAuthCallbackServer {
    /// Create a new OAuth callback server on a specific port
    pub fn new(port: u16) -> Self {
        let (shutdown_tx, _) = oneshot::channel();
        let (result_tx, _) = oneshot::channel();
        Self {
            port,
            shutdown_tx: Arc::new(shutdown_tx),
            result_tx: Arc::new(result_tx),
        }
    }

    /// Create a new OAuth callback server with auto port selection
    pub fn with_available_port() -> Result<Self> {
        let port = find_available_port(DEFAULT_PORT_RANGE_START, DEFAULT_PORT_RANGE_END)
            .context("No available port in callback range")?;
        Ok(Self::new(port))
    }

    /// Get the redirect URI for this server
    pub fn redirect_uri(&self) -> String {
        format!("http://localhost:{}/callback", self.port)
    }

    /// Get the port the server is listening on
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Start the callback server and wait for the OAuth callback
    /// Returns the callback data (code and state) when received
    pub async fn start(mut self) -> Result<OAuthCallbackData> {
        let listener = TcpListener::bind(("127.0.0.1", self.port))
            .context(format!("Failed to bind to port {}", self.port))?;

        // Set TCP_NODELAY for faster response
        listener.set_nonblocking(true)?;

        let result_tx = Arc::clone(&self.result_tx);
        let shutdown_tx = Arc::clone(&self.shutdown_tx);

        // Spawn the async server task
        tokio::task::spawn_local(async move {
            if let Err(e) = run_server(listener, result_tx, shutdown_tx).await {
                eprintln!("OAuth callback server error: {}", e);
            }
        });

        // Wait for the result
        let result = self.result_tx.await?;

        // Signal shutdown
        let _ = self.shutdown_tx.send(());

        result
    }

    /// Check if the server is still running
    pub fn is_running(&self) -> bool {
        !self.shutdown_tx.is_closed()
    }
}

/// OAuth server errors
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid callback URL: {0}")]
    InvalidCallback(String),

    #[error("Missing authorization code")]
    MissingCode,

    #[error("Missing state parameter")]
    MissingState,

    #[error("Server shutdown")]
    Shutdown,

    #[error("Callback timeout")]
    Timeout,

    #[error("HTTP parse error: {0}")]
    HttpParse(#[from] url::ParseError),
}

/// Find an available port in the given range
fn find_available_port(start: u16, end: u16) -> Option<u16> {
    for port in start..=end {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Some(port);
        }
    }
    None
}

/// Run the async HTTP server
async fn run_server(
    listener: TcpListener,
    result_tx: Arc<oneshot::Sender<Result<OAuthCallbackData, OAuthError>>>,
    _shutdown_tx: Arc<oneshot::Sender<()>>,
) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut listener = tokio::net::TcpListener::from_std(listener)?;
    let mut shutdown_flag = false;

    // Set a timeout for waiting for the callback
    let timeout_duration = std::time::Duration::from_secs(600); // 10 minutes

    loop {
        if shutdown_flag {
            break;
        }

        // Accept connections with timeout
        let result = tokio::time::timeout(timeout_duration, listener.accept()).await;

        match result {
            Ok(Ok((mut stream, _))) => {
                // Read the HTTP request
                let mut buf = [0u8; 4096];
                let n = match stream.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => continue,
                };

                let request = String::from_utf8_lossy(&buf[..n]);

                // Parse the HTTP request to extract the callback URL
                if let Some(callback_data) = parse_oauth_callback(&request) {
                    // Send success response
                    let response = "HTTP/1.1 200 OK\r\n\
                        Content-Type: text/html; charset=utf-8\r\n\
                        Connection: close\r\n\
                        \r\n\
                        <!DOCTYPE html>\
                        <html><head><title>OAuth Callback</title></head>\
                        <body style=\"font-family: system-ui; padding: 40px; text-align: center;\">\
                        <h2>Authentication Successful</h2>\
                        <p>You can close this window and return to the terminal.</p>\
                        <script>window.close();</script>\
                        </body></html>";
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;

                    // Send the callback data
                    let _ = result_tx.send(Ok(callback_data));
                    shutdown_flag = true;
                } else {
                    // Send error response
                    let response = "HTTP/1.1 400 Bad Request\r\n\
                        Content-Type: text/html\r\n\
                        Connection: close\r\n\
                        \r\n\
                        <!DOCTYPE html>\
                        <html><head><title>OAuth Error</title></head>\
                        <body><h2>Invalid OAuth Callback</h2></body></html>";
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                }
            }
            Ok(Err(e)) => {
                eprintln!("Connection error: {}", e);
            }
            Err(_) => {
                // Timeout - send timeout error
                let _ = result_tx.send(Err(OAuthError::Timeout));
                break;
            }
        }
    }

    Ok(())
}

/// Parse OAuth callback from HTTP request
fn parse_oauth_callback(request: &str) -> Option<OAuthCallbackData> {
    // Extract the request line (GET /callback?code=xxx&state=yyy HTTP/1.1)
    let request_line = request.lines().next()?;

    if !request_line.starts_with("GET ") {
        return None;
    }

    // Parse the path and query string
    let path = request_line
        .strip_prefix("GET ")?
        .split_whitespace()
        .next()?;

    if !path.starts_with("/callback") {
        return None;
    }

    // Parse query parameters
    let query = path.split('?').nth(1)?;

    let mut code = None;
    let mut state = None;
    let mut callback_url = None;

    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        let value = urlencoding::decode(value).ok()?.to_string();

        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "url" | "redirect_uri" => callback_url = Some(value),
            _ => {}
        }
    }

    let code = code.or_else(|| {
        // Also check for ?callback= parameter for some providers
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            if key == "callback" {
                Some(urlencoding::decode(value).ok()?.to_string())
            } else {
                None
            }
        })
    });

    let code = code?;
    let state = state.unwrap_or_default();

    // Construct full callback URL if available
    let full_url = if callback_url.is_some() {
        Some(format!(
            "http://localhost/callback?code={}&state={}",
            code, state
        ))
    } else {
        None
    };

    Some(OAuthCallbackData {
        code,
        state,
        callback_url: full_url,
    })
}

/// Open a URL in the default browser (cross-platform)
pub fn open_browser(url: &str) -> std::io::Result<std::process::Child> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(&["/C", "start", "", url])
            .spawn()
    }

    #[cfg(target_os = "linux")]
    {
        // Try xdg-open first, then fallback to sensible-browser or firefox
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .or_else(|_| {
                std::process::Command::new("sensible-browser")
                    .arg(url)
                    .spawn()
            })
            .or_else(|_| std::process::Command::new("firefox").arg(url).spawn())
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        // For other platforms, try common browsers
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .or_else(|_| std::process::Command::new("open").arg(url).spawn())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_oauth_callback_basic() {
        let request =
            "GET /callback?code=auth_code_123&state=state_456 HTTP/1.1\r\nHost: localhost\r\n";
        let result = parse_oauth_callback(request);
        assert!(result.is_some());
        let data = result.unwrap();
        assert_eq!(data.code, "auth_code_123");
        assert_eq!(data.state, "state_456");
    }

    #[test]
    fn test_parse_oauth_callback_with_redirect() {
        let request = "GET /callback?code=auth_code_123&state=state_456&url=http%3A%2F%2Fexample.com HTTP/1.1\r\n";
        let result = parse_oauth_callback(request);
        assert!(result.is_some());
        let data = result.unwrap();
        assert_eq!(data.code, "auth_code_123");
        assert_eq!(data.state, "state_456");
        assert!(data.callback_url.is_some());
    }

    #[test]
    fn test_parse_oauth_callback_no_code() {
        let request = "GET /callback?state=state_456 HTTP/1.1\r\n";
        let result = parse_oauth_callback(request);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_oauth_callback_wrong_path() {
        let request = "GET /other?code=test HTTP/1.1\r\n";
        let result = parse_oauth_callback(request);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_oauth_callback_post_request() {
        let request = "POST /callback?code=test HTTP/1.1\r\n";
        let result = parse_oauth_callback(request);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_oauth_callback_empty_state() {
        let request = "GET /callback?code=auth_code_123&state= HTTP/1.1\r\n";
        let result = parse_oauth_callback(request);
        assert!(result.is_some());
        let data = result.unwrap();
        assert_eq!(data.code, "auth_code_123");
        assert_eq!(data.state, "");
    }

    #[test]
    fn test_oauth_callback_server_new() {
        let server = OAuthCallbackServer::new(8787);
        assert_eq!(server.port(), 8787);
        assert_eq!(server.redirect_uri(), "http://localhost:8787/callback");
    }

    #[test]
    fn test_oauth_callback_server_is_running() {
        let server = OAuthCallbackServer::new(8788);
        // Server should be running initially
        assert!(server.is_running());
    }

    #[test]
    fn test_find_available_port() {
        // This test may fail if no ports are available, which is fine
        let port = find_available_port(9000, 9010);
        // Port should be in range if found
        if let Some(p) = port {
            assert!(p >= 9000 && p <= 9010);
        }
    }
}
