//! OAuth callback server for handling OAuth redirects
//!
//! This module provides a simple HTTP server that listens for OAuth callback
//! redirects from providers like Anthropic, OpenAI, GitHub, etc.
//!
//! The server runs on localhost and captures the authorization code and state
//! from the callback URL, then returns them via a channel for token exchange.

use anyhow::{Context, Result};
use std::net::TcpListener;
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
    shutdown_tx: oneshot::Sender<()>,
}

impl OAuthCallbackServer {
    /// Create a new OAuth callback server on a specific port
    pub fn new(port: u16) -> Self {
        let (shutdown_tx, _) = oneshot::channel();
        Self {
            port,
            shutdown_tx,
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
    pub async fn start(self) -> Result<OAuthCallbackData> {
        let listener = TcpListener::bind(("127.0.0.1", self.port))
            .context(format!("Failed to bind to port {}", self.port))?;

        // Set TCP_NODELAY for faster response
        listener.set_nonblocking(true)?;

        // Create oneshot channels for communication with the server task
        let (tx, rx) = oneshot::channel::<Result<OAuthCallbackData, OAuthError>>();

        // Spawn the async server task
        tokio::task::spawn_local(async move {
            if let Err(e) = run_server(listener, tx).await {
                eprintln!("OAuth callback server error: {}", e);
            }
        });

        // Wait for the result
        let result = rx.await.map_err(|e| anyhow::anyhow!("OAuth callback error: {}", e))?;

        result.map_err(|e| anyhow::anyhow!("OAuth error: {}", e))
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
    tx: oneshot::Sender<Result<OAuthCallbackData, OAuthError>>,
) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut listener = tokio::net::TcpListener::from_std(listener)?;
    
    // Set a timeout for waiting for the callback
    let timeout_duration = std::time::Duration::from_secs(600); // 10 minutes

    // Accept connections with timeout
    let result = tokio::time::timeout(timeout_duration, listener.accept()).await;

    match result {
        Ok(Ok((mut stream, _))) => {
            // Read the HTTP request
            let mut buf = [0u8; 4096];
            let n = match stream.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return Ok(()),
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
                let _ = tx.send(Ok(callback_data));
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
            // Timeout
            let _ = tx.send(Err(OAuthError::Timeout));
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
        let mut parts = pair.split('=');
        let key = parts.next()?;
        let value = parts.next()?.replace("%3D", "=").replace("%26", "&");
        
        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "url" => callback_url = Some(value),
            _ => {}
        }
    }

    let code = code?;
    let state = state?;

    Some(OAuthCallbackData {
        code,
        state,
        callback_url,
    })
}

/// Open the authorization URL in the default browser
pub fn open_browser(url: &str) -> std::io::Result<std::process::Child> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
    }
    #[cfg(target_os = "linux")]
    {
        // Try common browsers in order
        let browsers = ["xdg-open", "gnome-open", "kde-open", "x-www-browser", "firefox", "google-chrome"];
        for browser in browsers {
            if let Ok(child) = std::process::Command::new(browser)
                .arg(url)
                .spawn()
            {
                return Ok(child);
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No suitable browser found",
        ))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Unsupported platform",
        ))
    }
}

/// Start OAuth flow with browser-based authorization
pub async fn authorize_with_browser(
    auth_url: &str,
) -> Result<OAuthCallbackData> {
    // Open browser
    open_browser(auth_url).map_err(|e| anyhow::anyhow!("Failed to open browser: {}", e))?;

    // Get callback server ready
    let server = OAuthCallbackServer::with_available_port()
        .context("Failed to create callback server")?;

    let port = server.port();
    tracing::info!("OAuth callback server listening on port {}", port);

    // Start the server
    server.start().await
}