//! DAP (Debug Adapter Protocol) host — the reference [`DebugService`]
//! implementation behind the `coding-omp-v1` "Debug service" extension.
//!
//! [`DapClient`] speaks the wire protocol (Content-Length framed JSON over
//! the adapter's stdio): requests are matched to responses by `request_seq`,
//! events accumulate in a shared log. [`DapDebugService`] manages a map of
//! live sessions and implements the agent-side [`super::DebugService`]
//! contract: `start` (initialize + launch/attach), `request` (typed DAP
//! passthrough), `terminate` (terminate + disconnect + cleanup).
//!
//! Session config shape for `start`:
//! `{ "adapter": ["debugpy", "--listen", ...], "request": "launch",
//!    ...launchArguments }` — `adapter` is the adapter process command; all
//! other keys are forwarded verbatim as the launch/attach arguments.

use super::DebugService;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A live DAP adapter connection.
pub struct DapClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    seq: AtomicU64,
    /// Every adapter event received so far (bounded by the session's
    /// lifetime; drained via [`DapClient::wait_for_event`]).
    pub events: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl std::fmt::Debug for DapClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DapClient")
            .field("alive", &self.child.id().is_some())
            .field("events", &self.events.lock().len())
            .finish()
    }
}

impl DapClient {
    /// Spawn an adapter process and connect over its stdio.
    pub fn spawn(adapter: &[String]) -> Result<Self, String> {
        use tokio::process::Command;
        let (program, args) = adapter.split_first().ok_or("adapter command is empty")?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| format!("spawn adapter: {e}"))?;
        let stdin = child.stdin.take().ok_or("adapter: no stdin")?;
        let stdout = child.stdout.take().ok_or("adapter: no stdout")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            seq: AtomicU64::new(0),
            events: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    async fn write_frame(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let body = serde_json::to_vec(value).map_err(|e| format!("serialize frame: {e}"))?;
        self.stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .await
            .map_err(|e| format!("adapter stdin write: {e}"))?;
        self.stdin
            .write_all(&body)
            .await
            .map_err(|e| format!("adapter stdin write: {e}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("adapter stdin flush: {e}"))
    }

    /// Read one wire frame with a deadline; returns the parsed JSON.
    async fn read_frame(&mut self, deadline: Instant) -> Result<serde_json::Value, String> {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout_at(
                tokio::time::Instant::from(deadline),
                self.stdout.read_line(&mut line),
            )
            .await
            .map_err(|_| "timed out reading adapter frame headers".to_string())?
            .map_err(|e| format!("adapter stdout read: {e}"))?;
            if read == 0 {
                return Err("adapter closed the connection".to_string());
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break; // end of headers
            }
            if let Some(v) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(
                    v.trim()
                        .parse()
                        .map_err(|_| "bad Content-Length header".to_string())?,
                );
            }
        }
        let length = content_length.ok_or("adapter frame missing Content-Length")?;
        let mut body = vec![0u8; length];
        tokio::time::timeout_at(
            tokio::time::Instant::from(deadline),
            self.stdout.read_exact(&mut body),
        )
        .await
        .map_err(|_| "timed out reading adapter frame body".to_string())?
        .map_err(|e| format!("adapter stdout read: {e}"))?;
        serde_json::from_slice(&body).map_err(|e| format!("bad adapter frame JSON: {e}"))
    }

    /// Issue a DAP request and await its response. Events seen along the way
    /// are appended to [`Self::events`].
    pub async fn request(
        &mut self,
        command: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let seq = self.next_seq();
        let frame = serde_json::json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        self.write_frame(&frame).await?;
        loop {
            if Instant::now() >= deadline {
                return Err(format!("DAP request '{command}' timed out"));
            }
            let msg = self.read_frame(deadline).await?;
            match msg.get("type").and_then(|t| t.as_str()) {
                Some("response") => {
                    let req_seq = msg.get("request_seq").and_then(|s| s.as_u64());
                    if req_seq == Some(seq) {
                        let success = msg
                            .get("success")
                            .and_then(|s| s.as_bool())
                            .unwrap_or(false);
                        return if success {
                            Ok(msg.get("body").cloned().unwrap_or(serde_json::Value::Null))
                        } else {
                            Err(format!(
                                "DAP '{command}' failed: {}",
                                msg.get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("unknown")
                            ))
                        };
                    }
                    // Response to someone else's request — keep scanning.
                }
                Some("event") => self.events.lock().push(msg),
                _ => {}
            }
        }
    }

    /// Drain every buffered event of the given kind (best-effort wait).
    pub async fn wait_for_event(
        &mut self,
        event: &str,
        wait: Duration,
    ) -> Option<serde_json::Value> {
        let deadline = Instant::now() + wait;
        loop {
            {
                let events = self.events.lock();
                if let Some(found) = events
                    .iter()
                    .find(|e| e.get("event").and_then(|v| v.as_str()) == Some(event))
                {
                    return Some(found.clone());
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            match self.read_frame(deadline).await {
                Ok(msg) => {
                    if msg.get("type").and_then(|t| t.as_str()) == Some("event") {
                        let is_match = msg.get("event").and_then(|v| v.as_str()) == Some(event);
                        self.events.lock().push(msg);
                        if is_match {
                            return self
                                .events
                                .lock()
                                .iter()
                                .find(|e| e.get("event").and_then(|v| v.as_str()) == Some(event))
                                .cloned();
                        }
                    }
                }
                Err(_) => return None,
            }
        }
    }

    async fn shutdown(&mut self) {
        let _ = self
            .request(
                "disconnect",
                &serde_json::json!({"terminateDebuggee": true}),
            )
            .await;
        let _ = self.child.kill().await;
    }
}

/// Reference [`DebugService`]: manages DAP adapter sessions.
#[derive(Debug, Default)]
pub struct DapDebugService {
    sessions: Mutex<HashMap<String, DapClient>>,
}

impl DapDebugService {
    /// Creates an empty service; adapter sessions are added by
    /// [`DebugService::start`].
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DebugService for DapDebugService {
    async fn start(&self, config: &serde_json::Value) -> Result<String, String> {
        let adapter: Vec<String> = config
            .get("adapter")
            .and_then(|a| a.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .ok_or("config must carry an \"adapter\" command array")?;
        let request = config
            .get("request")
            .and_then(|r| r.as_str())
            .unwrap_or("launch");
        let mut launch_args = config.clone();
        if let Some(obj) = launch_args.as_object_mut() {
            obj.remove("adapter");
            obj.insert(
                "request".to_string(),
                serde_json::Value::String(request.to_string()),
            );
        }

        let mut client = DapClient::spawn(&adapter)?;
        client
            .request(
                "initialize",
                &serde_json::json!({"adapterID": config.get("type").and_then(|t| t.as_str()).unwrap_or("oxi"), "clientID": "oxicode"}),
            )
            .await?;
        client.request(request, &launch_args).await?;
        // Most adapters emit `stopped` right after launch/attach; bounded,
        // non-fatal wait so sessions are observable immediately.
        let _ = client
            .wait_for_event("stopped", Duration::from_secs(10))
            .await;

        let session = uuid::Uuid::new_v4().to_string();
        self.sessions.lock().insert(session.clone(), client);
        Ok(session)
    }

    async fn request(
        &self,
        session: &str,
        command: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        // Take the session out so the lock is never held across `.await`
        // (and one session's I/O never blocks another's).
        let mut client = self
            .sessions
            .lock()
            .remove(session)
            .ok_or("unknown debug session")?;
        let result = client.request(command, args).await;
        self.sessions.lock().insert(session.to_string(), client);
        result
    }

    async fn terminate(&self, session: &str) -> Result<(), String> {
        let mut client = self
            .sessions
            .lock()
            .remove(session)
            .ok_or("unknown debug session")?;
        let _ = client.request("terminate", &serde_json::json!({})).await;
        client.shutdown().await;
        Ok(())
    }
}
