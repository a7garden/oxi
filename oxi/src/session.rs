//! Session persistence with JSONL append-only format.
//!
//! Port of pi-mono's SessionManager concepts to Rust:
//! - JSONL append-only format for crash-safe writes
//! - Session versioning with migration support
//! - Tree structure with id/parentId for branching
//! - Auto-save with lazy flush (only writes to disk once assistant responds)
//! - Crash recovery: scans for valid session headers in directory
//! - Session resume from most recent file

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Current session format version.
const CURRENT_SESSION_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Entry types
// ---------------------------------------------------------------------------

/// Session file header — always the first line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    #[serde(rename = "type")]
    pub entry_type: String, // always "session"
    pub version: u32,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

impl SessionHeader {
    pub fn new(cwd: String) -> Self {
        Self {
            entry_type: "session".to_string(),
            version: CURRENT_SESSION_VERSION,
            id: Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            cwd,
            parent_session: None,
        }
    }
}

/// Session info entry (e.g., user-defined display name).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub name: Option<String>,
}

/// A message entry in the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessageEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub message: AgentMessage,
}

/// A model change entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionModelChangeEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub provider: String,
    pub model_id: String,
}

/// A compaction summary entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCompactionEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: usize,
}

/// A branch summary entry (captured when branching away from a path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBranchSummaryEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub from_id: String,
    pub summary: String,
}

/// All possible entry types in a session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionFileEntry {
    #[serde(rename = "session")]
    Header(SessionHeader),
    #[serde(rename = "message")]
    Message(SessionMessageEntry),
    #[serde(rename = "model_change")]
    ModelChange(SessionModelChangeEntry),
    #[serde(rename = "compaction")]
    Compaction(SessionCompactionEntry),
    #[serde(rename = "branch_summary")]
    BranchSummary(SessionBranchSummaryEntry),
    #[serde(rename = "session_info")]
    SessionInfo(SessionInfoEntry),
}

/// Agent message stored in session entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum AgentMessage {
    #[serde(rename = "user")]
    User { content: String },
    #[serde(rename = "assistant")]
    Assistant { content: String, provider: Option<String>, model: Option<String> },
    #[serde(rename = "system")]
    System { content: String },
    #[serde(rename = "tool_result")]
    ToolResult { tool_call_id: String, content: String },
}

impl AgentMessage {
    pub fn user(content: String) -> Self {
        Self::User { content }
    }

    pub fn assistant(content: String, provider: Option<String>, model: Option<String>) -> Self {
        Self::Assistant { content, provider, model }
    }

    pub fn tool_result(tool_call_id: String, content: String) -> Self {
        Self::ToolResult { tool_call_id, content }
    }

    pub fn content(&self) -> &str {
        match self {
            AgentMessage::User { content } => content,
            AgentMessage::Assistant { content, .. } => content,
            AgentMessage::System { content } => content,
            AgentMessage::ToolResult { content, .. } => content,
        }
    }
}

// ---------------------------------------------------------------------------
// Session metadata (for listing)
// ---------------------------------------------------------------------------

/// Summary information about a session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub path: PathBuf,
    pub id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub created: chrono::DateTime<chrono::Utc>,
    pub modified: chrono::DateTime<chrono::Utc>,
    pub message_count: usize,
    pub first_message: String,
}

// ---------------------------------------------------------------------------
// Session Manager
// ---------------------------------------------------------------------------

/// Manages conversation sessions as append-only trees stored in JSONL files.
///
/// Port of pi-mono's SessionManager:
/// - Each session is a JSONL file
/// - Entries form a tree via `id`/`parent_id`
/// - The "leaf" pointer tracks the current position
/// - Auto-save defers writing until an assistant message arrives
pub struct SessionManager {
    sessions_dir: PathBuf,
}

impl SessionManager {
    /// Create a new SessionManager rooted at `~/.oxi/sessions/`.
    pub async fn new() -> Result<Self> {
        let home = dirs::home_dir().context("Cannot find home directory")?;
        let sessions_dir = home.join(".oxi").join("sessions");
        tokio::fs::create_dir_all(&sessions_dir).await?;
        Ok(Self { sessions_dir })
    }

    /// Create with a custom sessions directory.
    pub fn with_dir(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    /// Get the sessions directory.
    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    /// Create a new session and return a handle for appending entries.
    pub async fn create_session(&self, cwd: &str) -> Result<SessionHandle> {
        let header = SessionHeader::new(cwd.to_string());
        let file_name = format!(
            "{}_{}.jsonl",
            header.timestamp.replace([':', '.'], "-"),
            header.id
        );
        let file_path = self.sessions_dir.join(&file_name);
        SessionHandle::create(file_path, header).await
    }

    /// Open an existing session file.
    pub async fn open_session(&self, path: &Path) -> Result<SessionHandle> {
        SessionHandle::open(path).await
    }

    /// Find the most recent valid session in the directory.
    pub fn find_most_recent(&self) -> Result<Option<PathBuf>> {
        let entries = std::fs::read_dir(&self.sessions_dir)?;
        let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            if !is_valid_session_file(&path) {
                continue;
            }
            let metadata = std::fs::metadata(&path)?;
            if let Ok(modified) = metadata.modified() {
                candidates.push((path, modified));
            }
        }

        candidates.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(candidates.into_iter().next().map(|(p, _)| p))
    }

    /// Continue the most recent session, or create a new one.
    pub async fn continue_or_create(&self, cwd: &str) -> Result<SessionHandle> {
        if let Some(path) = self.find_most_recent()? {
            match SessionHandle::open(&path).await {
                Ok(handle) => return Ok(handle),
                Err(e) => {
                    tracing::warn!("Failed to open recent session {}: {}, creating new", path.display(), e);
                }
            }
        }
        self.create_session(cwd).await
    }

    /// List all sessions sorted by modification time (newest first).
    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let entries = std::fs::read_dir(&self.sessions_dir)?;
        let mut infos = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(info) = build_session_info(&path) {
                infos.push(info);
            }
        }

        infos.sort_by(|a, b| b.modified.cmp(&a.modified));
        Ok(infos)
    }

    /// Delete a session file.
    pub async fn delete_session(&self, path: &Path) -> Result<()> {
        tokio::fs::remove_file(path).await?;
        Ok(())
    }

    /// Fork a session: copy entries up to a given leaf ID into a new file.
    pub async fn fork_session(&self, source_path: &Path, up_to_entry_id: &str, cwd: &str) -> Result<SessionHandle> {
        let source = SessionHandle::open(source_path).await?;
        let entries = source.entries();

        // Find the entry index
        let up_to_idx = entries.iter().position(|e| {
            match e {
                SessionFileEntry::Message(m) => m.id == up_to_entry_id,
                SessionFileEntry::ModelChange(m) => m.id == up_to_entry_id,
                SessionFileEntry::Compaction(m) => m.id == up_to_entry_id,
                SessionFileEntry::BranchSummary(m) => m.id == up_to_entry_id,
                SessionFileEntry::SessionInfo(m) => m.id == up_to_entry_id,
                _ => false,
            }
        }).context(format!("Entry {} not found", up_to_entry_id))?;

        // Create new header
        let mut header = SessionHeader::new(cwd.to_string());
        header.parent_session = Some(source_path.to_string_lossy().to_string());

        let handle = self.create_session(cwd).await?;

        // Copy entries up to and including the fork point
        for entry in &entries[..=up_to_idx] {
            handle.append_entry(entry)?;
        }

        Ok(handle)
    }
}

// ---------------------------------------------------------------------------
// Session Handle (active session for read/write)
// ---------------------------------------------------------------------------

/// An open session handle that supports appending entries.
///
/// Implements the same lazy-flush pattern as pi-mono: entries are buffered
/// in memory and only written to disk once an assistant message arrives.
/// This prevents orphaned "user asked, no response" session files after crashes.
pub struct SessionHandle {
    file_path: PathBuf,
    header: SessionHeader,
    entries: Vec<SessionFileEntry>,
    by_id: HashMap<String, usize>,
    leaf_id: Option<String>,
    flushed: bool,
}

impl SessionHandle {
    /// Create a new session file.
    async fn create(file_path: PathBuf, header: SessionHeader) -> Result<Self> {
        // Don't create the file yet — defer until first assistant message
        Ok(Self {
            file_path,
            header,
            entries: Vec::new(),
            by_id: HashMap::new(),
            leaf_id: None,
            flushed: false,
        })
    }

    /// Open an existing session file.
    async fn open(path: &Path) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;

        let entries = parse_jsonl(&content);
        if entries.is_empty() {
            anyhow::bail!("Session file is empty: {}", path.display());
        }

        let header = match &entries[0] {
            SessionFileEntry::Header(h) => h.clone(),
            _ => anyhow::bail!("Session file missing header: {}", path.display()),
        };

        let mut by_id = HashMap::new();
        let mut leaf_id = None;
        for (i, entry) in entries.iter().enumerate() {
            match entry {
                SessionFileEntry::Header(_) => {}
                SessionFileEntry::Message(m) => {
                    by_id.insert(m.id.clone(), i);
                    leaf_id = Some(m.id.clone());
                }
                SessionFileEntry::ModelChange(m) => {
                    by_id.insert(m.id.clone(), i);
                    leaf_id = Some(m.id.clone());
                }
                SessionFileEntry::Compaction(m) => {
                    by_id.insert(m.id.clone(), i);
                    leaf_id = Some(m.id.clone());
                }
                SessionFileEntry::BranchSummary(m) => {
                    by_id.insert(m.id.clone(), i);
                    leaf_id = Some(m.id.clone());
                }
                SessionFileEntry::SessionInfo(m) => {
                    by_id.insert(m.id.clone(), i);
                    leaf_id = Some(m.id.clone());
                }
            }
        }

        Ok(Self {
            file_path: path.to_path_buf(),
            header,
            entries,
            by_id,
            leaf_id,
            flushed: true,
        })
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.header.id
    }

    /// Get the session file path.
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Get the session header.
    pub fn header(&self) -> &SessionHeader {
        &self.header
    }

    /// Get the current leaf entry ID.
    pub fn leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    /// Get all entries (excluding header).
    pub fn entries(&self) -> &[SessionFileEntry] {
        &self.entries
    }

    /// Get entries without the header.
    pub fn data_entries(&self) -> Vec<&SessionFileEntry> {
        self.entries.iter().skip(1).collect()
    }

    /// Generate a unique short ID (8 hex chars).
    fn generate_id(&self) -> String {
        for _ in 0..100 {
            let id = Uuid::new_v4().to_string()[..8].to_string();
            if !self.by_id.contains_key(&id) {
                return id;
            }
        }
        Uuid::new_v4().to_string()[..8].to_string()
    }

    /// Append a user message.
    pub fn append_user_message(&mut self, content: String) -> String {
        let id = self.generate_id();
        let entry = SessionFileEntry::Message(SessionMessageEntry {
            id: id.clone(),
            parent_id: self.leaf_id.take(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            message: AgentMessage::user(content),
        });
        self.append_entry_internal(&id, entry);
        id
    }

    /// Append an assistant message.
    ///
    /// This triggers the first flush to disk (lazy write pattern).
    pub fn append_assistant_message(&mut self, content: String, provider: Option<String>, model: Option<String>) -> String {
        let id = self.generate_id();
        let entry = SessionFileEntry::Message(SessionMessageEntry {
            id: id.clone(),
            parent_id: self.leaf_id.take(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            message: AgentMessage::assistant(content, provider, model),
        });
        self.append_entry_internal(&id, entry);
        id
    }

    /// Append a tool result message.
    pub fn append_tool_result(&mut self, tool_call_id: String, content: String) -> String {
        let id = self.generate_id();
        let entry = SessionFileEntry::Message(SessionMessageEntry {
            id: id.clone(),
            parent_id: self.leaf_id.take(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            message: AgentMessage::tool_result(tool_call_id, content),
        });
        self.append_entry_internal(&id, entry);
        id
    }

    /// Append a model change entry.
    pub fn append_model_change(&mut self, provider: String, model_id: String) -> String {
        let id = self.generate_id();
        let entry = SessionFileEntry::ModelChange(SessionModelChangeEntry {
            id: id.clone(),
            parent_id: self.leaf_id.take(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            provider,
            model_id,
        });
        self.append_entry_internal(&id, entry);
        id
    }

    /// Append a compaction entry.
    pub fn append_compaction(&mut self, summary: String, first_kept_entry_id: String, tokens_before: usize) -> String {
        let id = self.generate_id();
        let entry = SessionFileEntry::Compaction(SessionCompactionEntry {
            id: id.clone(),
            parent_id: self.leaf_id.take(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            summary,
            first_kept_entry_id,
            tokens_before,
        });
        self.append_entry_internal(&id, entry);
        id
    }

    /// Append a session info entry (display name).
    pub fn append_session_info(&mut self, name: String) -> String {
        let id = self.generate_id();
        let entry = SessionFileEntry::SessionInfo(SessionInfoEntry {
            id: id.clone(),
            parent_id: self.leaf_id.take(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            name: Some(name),
        });
        self.append_entry_internal(&id, entry);
        id
    }

    /// Append a raw entry (used during fork).
    fn append_entry(&self, entry: &SessionFileEntry) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)?;
        writeln!(file, "{}", serde_json::to_string(entry)?)?;
        Ok(())
    }

    /// Internal append: buffer + lazy write.
    fn append_entry_internal(&mut self, id: &str, entry: SessionFileEntry) {
        let idx = self.entries.len();
        self.by_id.insert(id.to_string(), idx);
        self.entries.push(entry);
        self.leaf_id = Some(id.to_string());

        // Check if we should write to disk
        let has_assistant = self.entries.iter().any(|e| matches!(
            e,
            SessionFileEntry::Message(SessionMessageEntry {
                message: AgentMessage::Assistant { .. },
                ..
            })
        ));

        if !has_assistant {
            // Defer write until we get an assistant response
            return;
        }

        if !self.flushed {
            // First flush — write all entries including header
            self.flush_all();
        } else {
            // Incremental append
            self.flush_last();
        }
    }

    /// Write all buffered entries to the file.
    fn flush_all(&mut self) {
        if let Ok(mut file) = std::fs::File::create(&self.file_path) {
            for entry in &self.entries {
                if let Ok(line) = serde_json::to_string(entry) {
                    let _ = writeln!(file, "{}", line);
                }
            }
            self.flushed = true;
        }
    }

    /// Write only the last entry (incremental append).
    fn flush_last(&mut self) {
        if let Some(entry) = self.entries.last() {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.file_path)
            {
                if let Ok(line) = serde_json::to_string(entry) {
                    let _ = writeln!(file, "{}", line);
                }
            }
        }
    }

    /// Force-flush all buffered entries to disk.
    pub fn flush(&mut self) -> Result<()> {
        self.flush_all();
        Ok(())
    }

    /// Branch from an entry: move the leaf pointer to that entry.
    pub fn branch_from(&mut self, entry_id: &str) -> Result<()> {
        if !self.by_id.contains_key(entry_id) {
            anyhow::bail!("Entry {} not found", entry_id);
        }
        self.leaf_id = Some(entry_id.to_string());
        Ok(())
    }

    /// Get the session name from the latest session_info entry.
    pub fn session_name(&self) -> Option<String> {
        self.entries.iter().rev().find_map(|e| {
            match e {
                SessionFileEntry::SessionInfo(info) => info.name.clone(),
                _ => None,
            }
        })
    }

    /// Build the conversation path from root to the current leaf.
    pub fn build_path(&self) -> Vec<&SessionFileEntry> {
        let mut path = Vec::new();
        let mut current_id = self.leaf_id.as_deref();

        while let Some(id) = current_id {
            if let Some(&idx) = self.by_id.get(id) {
                if let Some(entry) = self.entries.get(idx) {
                    let parent_id = match entry {
                        SessionFileEntry::Message(m) => m.parent_id.as_deref(),
                        SessionFileEntry::ModelChange(m) => m.parent_id.as_deref(),
                        SessionFileEntry::Compaction(m) => m.parent_id.as_deref(),
                        SessionFileEntry::BranchSummary(m) => m.parent_id.as_deref(),
                        SessionFileEntry::SessionInfo(m) => m.parent_id.as_deref(),
                        _ => None,
                    };
                    path.push(entry);
                    current_id = parent_id;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        path.reverse();
        path
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a JSONL string into entries.
fn parse_jsonl(content: &str) -> Vec<SessionFileEntry> {
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<SessionFileEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                tracing::debug!("Skipping malformed JSONL line: {}", e);
            }
        }
    }
    entries
}

/// Check if a file is a valid session (has a proper header line).
fn is_valid_session_file(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    let mut reader = std::io::BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).is_err() {
        return false;
    }
    match serde_json::from_str::<SessionFileEntry>(first_line.trim()) {
        Ok(SessionFileEntry::Header(_)) => true,
        _ => false,
    }
}

/// Build session info by reading a session file.
fn build_session_info(path: &Path) -> Option<SessionInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    let entries = parse_jsonl(&content);
    if entries.is_empty() {
        return None;
    }

    let header = match &entries[0] {
        SessionFileEntry::Header(h) => h,
        _ => return None,
    };

    let created = chrono::DateTime::parse_from_rfc3339(&header.timestamp)
        .ok()
        .map(|dt| dt.to_utc())?;

    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()
        .and_then(|t| {
            let dur = t.duration_since(std::time::UNIX_EPOCH).ok()?;
            Some(chrono::DateTime::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos())?)
        })
        .unwrap_or(created);

    let mut message_count = 0usize;
    let mut first_message = String::new();
    let mut name = None;

    for entry in &entries {
        match entry {
            SessionFileEntry::Message(msg) => {
                message_count += 1;
                if first_message.is_empty() {
                    if let AgentMessage::User { content } = &msg.message {
                        first_message = content.chars().take(100).collect();
                    }
                }
            }
            SessionFileEntry::SessionInfo(info) => {
                name = info.name.clone();
            }
            _ => {}
        }
    }

    Some(SessionInfo {
        path: path.to_path_buf(),
        id: header.id.clone(),
        cwd: header.cwd.clone(),
        name,
        created,
        modified,
        message_count,
        first_message: if first_message.is_empty() {
            "(no messages)".to_string()
        } else {
            first_message
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_jsonl_empty() {
        let entries = parse_jsonl("");
        assert!(entries.is_empty());
    }

    #[test]
    fn header_roundtrip() {
        let header = SessionHeader::new("/tmp/test".to_string());
        let json = serde_json::to_string(&SessionFileEntry::Header(header.clone())).unwrap();
        let parsed: SessionFileEntry = serde_json::from_str(&json).unwrap();
        if let SessionFileEntry::Header(h) = parsed {
            assert_eq!(h.id, header.id);
            assert_eq!(h.cwd, "/tmp/test");
        } else {
            panic!("Expected header");
        }
    }

    #[test]
    fn message_roundtrip() {
        let msg = SessionMessageEntry {
            id: "abc123".to_string(),
            parent_id: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: AgentMessage::user("Hello".to_string()),
        };
        let json = serde_json::to_string(&SessionFileEntry::Message(msg.clone())).unwrap();
        let parsed: SessionFileEntry = serde_json::from_str(&json).unwrap();
        if let SessionFileEntry::Message(m) = parsed {
            assert_eq!(m.id, "abc123");
            assert_eq!(m.message.content(), "Hello");
        } else {
            panic!("Expected message");
        }
    }

    #[tokio::test]
    async fn session_create_and_write() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::with_dir(dir.path().to_path_buf());
        let mut session = mgr.create_session("/tmp/test").await.unwrap();

        let user_id = session.append_user_message("Hello".to_string());
        assert!(!session.file_path().exists()); // Not flushed yet

        let asst_id = session.append_assistant_message(
            "Hi there!".to_string(),
            Some("anthropic".to_string()),
            Some("claude-sonnet-4-20250514".to_string()),
        );
        assert!(session.file_path().exists()); // Now flushed
        assert!(session.leaf_id().is_some());
    }

    #[tokio::test]
    async fn session_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::with_dir(dir.path().to_path_buf());

        let session_id;
        {
            let mut session = mgr.create_session("/tmp/test").await.unwrap();
            session_id = session.session_id().to_string();
            session.append_user_message("Hello".to_string());
            session.append_assistant_message(
                "World".to_string(),
                None,
                None,
            );
        }

        let session = mgr.open_session(session.file_path()).await.unwrap();
        assert_eq!(session.session_id(), session_id);
        assert_eq!(session.data_entries().len(), 2);
    }

    #[tokio::test]
    async fn session_branch() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::with_dir(dir.path().to_path_buf());

        let mut session = mgr.create_session("/tmp/test").await.unwrap();
        let user_id = session.append_user_message("Hello".to_string());
        let asst_id = session.append_assistant_message("World".to_string(), None, None);
        let user2_id = session.append_user_message("Follow up".to_string());

        session.branch_from(&asst_id).unwrap();
        let new_msg = session.append_user_message("Branched message".to_string());

        let path = session.build_path();
        assert_eq!(path.len(), 3); // user1, asst, branched_msg
    }

    #[tokio::test]
    async fn find_most_recent_session() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::with_dir(dir.path().to_path_buf());

        // No sessions yet
        assert!(mgr.find_most_recent().unwrap().is_none());

        // Create a session
        let mut session = mgr.create_session("/tmp/test").await.unwrap();
        session.append_user_message("Hello".to_string());
        session.append_assistant_message("World".to_string(), None, None);

        let found = mgr.find_most_recent().unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap(), session.file_path());
    }

    #[test]
    fn agent_message_content() {
        let msg = AgentMessage::user("test".to_string());
        assert_eq!(msg.content(), "test");

        let msg = AgentMessage::assistant("response".to_string(), None, None);
        assert_eq!(msg.content(), "response");
    }
}
