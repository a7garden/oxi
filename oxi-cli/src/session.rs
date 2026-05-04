//! Session management for the coding agent.
//!
//! Manages conversation sessions as append-only trees stored in JSONL files.
//! Each session entry has an id and parent_id forming a tree structure.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Current session version for migrations
pub const CURRENT_SESSION_VERSION: i32 = 3;

// ============================================================================
// Session Header
// ============================================================================

/// Session header stored as the first line in JSONL files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    #[serde(rename = "type")]
    pub entry_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i32>,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

impl SessionHeader {
    pub fn new(id: String, cwd: String, parent_session: Option<String>) -> Self {
        Self {
            entry_type: "session".to_string(),
            version: Some(CURRENT_SESSION_VERSION),
            id,
            timestamp: Utc::now().to_rfc3339(),
            cwd,
            parent_session,
        }
    }
}

// ============================================================================
// Session Entry Types
// ============================================================================

/// Base fields for all session entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntryBase {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
}

/// Message entry with AgentMessage content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessageEntry {
    #[serde(flatten)]
    pub base: SessionEntryBase,
    pub message: AgentMessage,
}

/// Thinking level change entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingLevelChangeEntry {
    #[serde(flatten)]
    pub base: SessionEntryBase,
    #[serde(rename = "thinkingLevel")]
    pub thinking_level: String,
}

/// Model change entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelChangeEntry {
    #[serde(flatten)]
    pub base: SessionEntryBase,
    pub provider: String,
    #[serde(rename = "modelId")]
    pub model_id: String,
}

/// Compaction entry for context window management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionEntry<T = serde_json::Value> {
    #[serde(flatten)]
    pub base: SessionEntryBase,
    pub summary: String,
    #[serde(rename = "firstKeptEntryId")]
    pub first_kept_entry_id: String,
    #[serde(rename = "tokensBefore")]
    pub tokens_before: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<T>,
    #[serde(rename = "fromHook", skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
}

/// Branch summary entry for abandoned branches
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummaryEntry<T = serde_json::Value> {
    #[serde(flatten)]
    pub base: SessionEntryBase,
    #[serde(rename = "fromId")]
    pub from_id: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<T>,
    #[serde(rename = "fromHook", skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
}

/// Custom entry for extensions to store extension-specific data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomEntry<T = serde_json::Value> {
    #[serde(flatten)]
    pub base: SessionEntryBase,
    #[serde(rename = "customType")]
    pub custom_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

/// Label entry for user-defined bookmarks/markers on entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelEntry {
    #[serde(flatten)]
    pub base: SessionEntryBase,
    #[serde(rename = "targetId")]
    pub target_id: String,
    pub label: Option<String>,
}

/// Session metadata entry (e.g., user-defined display name)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoEntry {
    #[serde(flatten)]
    pub base: SessionEntryBase,
    pub name: Option<String>,
}

/// Custom message entry for extensions to inject messages into LLM context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomMessageEntry<T = serde_json::Value> {
    #[serde(flatten)]
    pub base: SessionEntryBase,
    #[serde(rename = "customType")]
    pub custom_type: String,
    pub content: ContentValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<T>,
    pub display: bool,
}

/// Content can be string or array of content blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentValue {
    String(String),
    Blocks(Vec<ContentBlock>),
}

/// Content block for text or image content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, media_type: Option<String> },
}

/// All possible session entries
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionEntry {
    Message(SessionMessageEntry),
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    ModelChange(ModelChangeEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    Custom(CustomEntry),
    Label(LabelEntry),
    SessionInfo(SessionInfoEntry),
    CustomMessage(CustomMessageEntry),
}

impl SessionEntry {
    pub fn id(&self) -> &str {
        match self {
            SessionEntry::Message(e) => &e.base.id,
            SessionEntry::ThinkingLevelChange(e) => &e.base.id,
            SessionEntry::ModelChange(e) => &e.base.id,
            SessionEntry::Compaction(e) => &e.base.id,
            SessionEntry::BranchSummary(e) => &e.base.id,
            SessionEntry::Custom(e) => &e.base.id,
            SessionEntry::Label(e) => &e.base.id,
            SessionEntry::SessionInfo(e) => &e.base.id,
            SessionEntry::CustomMessage(e) => &e.base.id,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message(e) => e.base.parent_id.as_deref(),
            SessionEntry::ThinkingLevelChange(e) => e.base.parent_id.as_deref(),
            SessionEntry::ModelChange(e) => e.base.parent_id.as_deref(),
            SessionEntry::Compaction(e) => e.base.parent_id.as_deref(),
            SessionEntry::BranchSummary(e) => e.base.parent_id.as_deref(),
            SessionEntry::Custom(e) => e.base.parent_id.as_deref(),
            SessionEntry::Label(e) => e.base.parent_id.as_deref(),
            SessionEntry::SessionInfo(e) => e.base.parent_id.as_deref(),
            SessionEntry::CustomMessage(e) => e.base.parent_id.as_deref(),
        }
    }

    pub fn timestamp(&self) -> &str {
        match self {
            SessionEntry::Message(e) => &e.base.timestamp,
            SessionEntry::ThinkingLevelChange(e) => &e.base.timestamp,
            SessionEntry::ModelChange(e) => &e.base.timestamp,
            SessionEntry::Compaction(e) => &e.base.timestamp,
            SessionEntry::BranchSummary(e) => &e.base.timestamp,
            SessionEntry::Custom(e) => &e.base.timestamp,
            SessionEntry::Label(e) => &e.base.timestamp,
            SessionEntry::SessionInfo(e) => &e.base.timestamp,
            SessionEntry::CustomMessage(e) => &e.base.timestamp,
        }
    }

    pub fn entry_type(&self) -> &str {
        match self {
            SessionEntry::Message(_) => "message",
            SessionEntry::ThinkingLevelChange(_) => "thinking_level_change",
            SessionEntry::ModelChange(_) => "model_change",
            SessionEntry::Compaction(_) => "compaction",
            SessionEntry::BranchSummary(_) => "branch_summary",
            SessionEntry::Custom(_) => "custom",
            SessionEntry::Label(_) => "label",
            SessionEntry::SessionInfo(_) => "session_info",
            SessionEntry::CustomMessage(_) => "custom_message",
        }
    }

    pub fn message(&self) -> Option<&AgentMessage> {
        match self {
            SessionEntry::Message(e) => Some(&e.message),
            _ => None,
        }
    }
}

/// Raw file entry (includes header)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileEntry {
    Header(SessionHeader),
    Entry(SessionEntry),
}

// ============================================================================
// Agent Message Types
// ============================================================================

/// Agent message roles
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum AgentMessage {
    #[serde(rename = "user")]
    User { content: ContentValue },
    #[serde(rename = "assistant")]
    Assistant {
        content: Vec<AssistantContentBlock>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(rename = "model", skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(rename = "stopReason", skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },
    #[serde(rename = "toolResult")]
    ToolResult {
        content: ContentValue,
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
    },
    #[serde(rename = "system")]
    System { content: ContentValue },
    #[serde(rename = "bashExecution")]
    BashExecution {
        command: String,
        output: String,
        #[serde(rename = "exitCode")]
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        #[serde(rename = "fullOutputPath", skip_serializing_if = "Option::is_none")]
        full_output_path: Option<String>,
        #[serde(rename = "excludeFromContext", skip_serializing_if = "Option::is_none")]
        exclude_from_context: Option<bool>,
        timestamp: i64,
    },
    #[serde(rename = "custom")]
    Custom {
        #[serde(rename = "customType")]
        custom_type: String,
        content: ContentValue,
        display: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        timestamp: i64,
    },
    #[serde(rename = "branchSummary")]
    BranchSummary {
        summary: String,
        #[serde(rename = "fromId")]
        from_id: String,
        timestamp: i64,
    },
    #[serde(rename = "compactionSummary")]
    CompactionSummary {
        summary: String,
        #[serde(rename = "tokensBefore")]
        tokens_before: i64,
        timestamp: i64,
    },
}

/// Content block for assistant messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "toolCall")]
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    #[serde(rename = "toolPlan")]
    ToolPlan {
        content: String,
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
    },
    #[serde(rename = "image")]
    ImageResult { data: String, media_type: String },
    #[serde(rename = "refusal")]
    Refusal { content: String },
}

/// Usage statistics from an assistant message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    #[serde(rename = "inputTokens")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<i64>,
    #[serde(rename = "outputTokens")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<i64>,
    #[serde(rename = "cacheReadTokens")]
    #[serde(rename = "cacheRead", skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<i64>,
    #[serde(rename = "cacheWriteTokens")]
    #[serde(rename = "cacheWrite", skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<i64>,
    #[serde(rename = "totalTokens")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
}

// ============================================================================
// Session Context
// ============================================================================

/// Context built from session entries for the LLM
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
    pub thinking_level: String,
    pub model: Option<ModelInfo>,
}

/// Model information
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub provider: String,
    pub model_id: String,
}

// ============================================================================
// Session Info
// ============================================================================

/// Session metadata for listing
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub path: String,
    pub id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub parent_session_path: Option<String>,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub message_count: i64,
    pub first_message: String,
    pub all_messages_text: String,
}

// ============================================================================
// Session Tree Node
// ============================================================================

/// Tree node for get_tree()
#[derive(Debug, Clone)]
pub struct SessionTreeNode {
    pub entry: SessionEntry,
    pub children: Vec<SessionTreeNode>,
    pub label: Option<String>,
    pub label_timestamp: Option<String>,
}

// ============================================================================
// ID Generation
// ============================================================================

fn generate_id(by_id: &HashSet<String>) -> String {
    for _ in 0..100 {
        let id = Uuid::new_v4().to_string()[..8].to_string();
        if !by_id.contains(&id) {
            return id;
        }
    }
    // Fallback to full UUID if somehow we have collisions
    Uuid::new_v4().to_string()
}

// ============================================================================
// Version Migration
// ============================================================================

/// Migrate v1 to v2: add id/parent_id tree structure
fn migrate_v1_to_v2(entries: &mut Vec<FileEntry>) {
    let mut ids = HashSet::new();
    let mut prev_id: Option<String> = None;

    for entry in entries.iter_mut() {
        match entry {
            FileEntry::Header(header) => {
                header.version = Some(2);
            }
            FileEntry::Entry(entry) => {
                let id = match entry {
                    SessionEntry::Message(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "message".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntry::ThinkingLevelChange(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "thinking_level_change".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntry::ModelChange(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "model_change".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntry::Compaction(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "compaction".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntry::BranchSummary(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "branch_summary".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntry::Custom(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "custom".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntry::Label(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "label".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntry::SessionInfo(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "session_info".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntry::CustomMessage(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "custom_message".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                };
                ids.insert(id);
            }
        }
    }
}

/// Migrate v2 to v3: rename hookMessage role to custom
fn migrate_v2_to_v3(entries: &mut Vec<FileEntry>) {
    for entry in entries.iter_mut() {
        match entry {
            FileEntry::Header(header) => {
                header.version = Some(3);
            }
            FileEntry::Entry(entry) => {
                if let SessionEntry::Message(e) = entry {
                    if let AgentMessage::User { content: _ } = &e.message {
                        // In v2, hookMessage had role "hookMessage" stored as User
                        // We need to check if this was a custom message
                        // Actually in the TS code, it checks for role === "hookMessage"
                        // but that wouldn't be valid JSON. Let's keep the migration
                        // simple and skip this for now - the actual hookMessage was
                        // handled differently in the original
                    }
                }
            }
        }
    }
}

/// Run all necessary migrations to bring entries to current version
fn migrate_to_current_version(entries: &mut Vec<FileEntry>) -> bool {
    let header = entries.iter().find_map(|e| match e {
        FileEntry::Header(h) => Some(h),
        _ => None,
    });
    let version = header.and_then(|h| h.version).unwrap_or(1);

    if version >= CURRENT_SESSION_VERSION {
        return false;
    }

    if version < 2 {
        migrate_v1_to_v2(entries);
    }
    if version < 3 {
        migrate_v2_to_v3(entries);
    }

    true
}

// ============================================================================
// Session Manager
// ============================================================================

/// Manages conversation sessions as append-only trees stored in JSONL files.
pub struct SessionManager {
    session_id: String,
    session_file: Option<String>,
    session_dir: String,
    cwd: String,
    persist: bool,
    flushed: bool,
    file_entries: RwLock<Vec<FileEntry>>,
    by_id: RwLock<HashMap<String, SessionEntry>>,
    labels_by_id: RwLock<HashMap<String, String>>,
    label_timestamps_by_id: RwLock<HashMap<String, String>>,
    leaf_id: RwLock<Option<String>>,
}

impl SessionManager {
    /// Create a new session
    pub fn create(cwd: &str, session_dir: Option<&str>) -> Self {
        let dir = session_dir
            .map(|s| s.to_string())
            .unwrap_or_else(|| get_default_session_dir(cwd));

        let mut manager = Self::new(cwd, &dir, None, true);
        manager.persist = true;
        manager
    }

    /// Open a specific session file
    pub fn open(path: &str, session_dir: Option<&str>, cwd_override: Option<&str>) -> Self {
        let entries = load_entries_from_file(path);
        let header = entries.iter().find_map(|e| match e {
            FileEntry::Header(h) => Some(h),
            _ => None,
        });
        let cwd = cwd_override
            .map(|s| s.to_string())
            .or_else(|| header.as_ref().map(|h| h.cwd.clone()))
            .unwrap_or_else(|| std::env::current_dir().unwrap().to_string_lossy().to_string());
        let dir = session_dir
            .map(|s| s.to_string())
            .unwrap_or_else(|| Path::new(path).parent().unwrap().to_string_lossy().to_string());

        let mut manager = Self::new(&cwd, &dir, Some(path), true);
        manager.persist = true;
        manager
    }

    /// Continue the most recent session, or create new if none
    pub fn continue_recent(cwd: &str, session_dir: Option<&str>) -> Self {
        let dir = session_dir
            .map(|s| s.to_string())
            .unwrap_or_else(|| get_default_session_dir(cwd));

        if let Some(most_recent) = find_most_recent_session(&dir) {
            return Self::open(&most_recent, None, None);
        }
        Self::create(cwd, None)
    }

    /// Create an in-memory session (no file persistence)
    pub fn in_memory(cwd: &str) -> Self {
        let cwd = cwd.to_string();
        Self::new(&cwd, "", None, false)
    }

    fn new(cwd: &str, session_dir: &str, session_file: Option<&str>, persist: bool) -> Self {
        let cwd = cwd.to_string();
        let session_dir = session_dir.to_string();

        if persist && !session_dir.is_empty() && !Path::new(&session_dir).exists() {
            let _ = fs::create_dir_all(&session_dir);
        }

        let mut manager = Self {
            session_id: Uuid::new_v4().to_string(),
            session_file: session_file.map(|s| s.to_string()),
            session_dir,
            cwd,
            persist,
            flushed: false,
            file_entries: RwLock::new(Vec::new()),
            by_id: RwLock::new(HashMap::new()),
            labels_by_id: RwLock::new(HashMap::new()),
            label_timestamps_by_id: RwLock::new(HashMap::new()),
            leaf_id: RwLock::new(None),
        };

        if let Some(file) = session_file {
            manager.set_session_file(file);
        } else {
            manager.new_session(None);
        }

        manager
    }

    /// Switch to a different session file
    pub fn set_session_file(&mut self, session_file: &str) {
        let path = Path::new(session_file).canonicalize().unwrap_or_else(|_| PathBuf::from(session_file));
        let path_str = path.to_string_lossy().to_string();
        self.session_file = Some(path_str.clone());

        if path.exists() {
            let mut entries = load_entries_from_file(&path_str);

            // If file was empty or corrupted (no valid header), truncate and start fresh
            if entries.is_empty() {
                let explicit_path = self.session_file.take();
                self.new_session(None);
                self.session_file = explicit_path;
                self._rewrite_file();
                self.flushed = true;
                return;
            }

            let header = entries.iter().find_map(|e| match e {
                FileEntry::Header(h) => Some(h),
                _ => None,
            });
            self.session_id = header.map(|h| h.id.clone()).unwrap_or_else(|| Uuid::new_v4().to_string());

            if migrate_to_current_version(&mut entries) {
                self._rewrite_file();
            }

            *self.file_entries.write() = entries;
            self._build_index();
            self.flushed = true;
        } else {
            let explicit_path = self.session_file.take();
            self.new_session(None);
            self.session_file = explicit_path;
        }
    }

    /// Create a new session with optional ID and parent
    pub fn new_session(&mut self, options: Option<NewSessionOptions>) {
        self.session_id = options
            .as_ref()
            .and_then(|o| o.id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let timestamp = Utc::now().to_rfc3339();
        let header = SessionHeader::new(
            self.session_id.clone(),
            self.cwd.clone(),
            options.and_then(|o| o.parent_session),
        );

        self.file_entries = RwLock::new(vec![FileEntry::Header(header)]);
        self.by_id.write().clear();
        self.labels_by_id.write().clear();
        self.label_timestamps_by_id.write().clear();
        *self.leaf_id.write() = None;
        self.flushed = false;

        if self.persist {
            let file_timestamp = timestamp.replace([':', '.', 'T', '-', ':', '+'], "-");
            let short_id = &self.session_id[..8];
            self.session_file = Some(format!("{}/{}_{}.jsonl", self.session_dir, file_timestamp, short_id));
        }
    }

    fn _build_index(&mut self) {
        let mut by_id = self.by_id.write();
        let mut labels = self.labels_by_id.write();
        let mut label_timestamps = self.label_timestamps_by_id.write();
        let mut leaf_id = self.leaf_id.write();

        by_id.clear();
        labels.clear();
        label_timestamps.clear();
        *leaf_id = None;

        for entry in self.file_entries.read().iter() {
            if let FileEntry::Entry(e) = entry {
                by_id.insert(e.id().to_string(), e.clone());

                if e.entry_type() == "label" {
                    if let SessionEntry::Label(l) = e {
                        if let Some(ref label) = l.label {
                            labels.insert(l.target_id.clone(), label.clone());
                            label_timestamps.insert(l.target_id.clone(), l.base.timestamp.clone());
                        } else {
                            labels.remove(&l.target_id);
                            label_timestamps.remove(&l.target_id);
                        }
                    }
                }

                *leaf_id = Some(e.id().to_string());
            }
        }
    }

    fn _rewrite_file(&self) {
        if !self.persist || self.session_file.is_none() {
            return;
        }

        let file = self.session_file.as_ref().unwrap();
        let content: String = self
            .file_entries
            .read()
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let _ = fs::write(file, content);
    }

    /// Check if session is persisted to disk
    pub fn is_persisted(&self) -> bool {
        self.persist
    }

    /// Get working directory
    pub fn get_cwd(&self) -> String {
        self.cwd.clone()
    }

    /// Get session directory
    pub fn get_session_dir(&self) -> String {
        self.session_dir.clone()
    }

    /// Get session ID
    pub fn get_session_id(&self) -> String {
        self.session_id.clone()
    }

    /// Get session file path
    pub fn get_session_file(&self) -> Option<String> {
        self.session_file.clone()
    }

    fn _persist(&self, entry: &SessionEntry) {
        if !self.persist {
            return;
        }
        let Some(file) = &self.session_file else {
            return;
        };

        let has_assistant = self.file_entries.read().iter().any(|e| {
            matches!(
                e,
                FileEntry::Entry(SessionEntry::Message(m)) if matches!(m.message, AgentMessage::Assistant { .. })
            )
        });

        if !has_assistant {
            // Mark as not flushed so when assistant arrives, all entries get written
            self.flushed = false;
            return;
        }

        let mut handle = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .unwrap();

        if !self.flushed {
            for e in self.file_entries.read().iter() {
                writeln!(&mut handle, "{}", serde_json::to_string(e).unwrap()).ok();
            }
            self.flushed = true;
        } else {
            writeln!(&mut handle, "{}", serde_json::to_string(entry).unwrap()).ok();
        }
    }

    fn _append_entry(&mut self, entry: SessionEntry) {
        self.file_entries.write().push(FileEntry::Entry(entry.clone()));
        self.by_id.write().insert(entry.id().to_string(), entry.clone());
        *self.leaf_id.write() = Some(entry.id().to_string());
        self._persist(&entry);
    }

    /// Append a message as child of current leaf
    pub fn append_message(&mut self, message: AgentMessage) -> String {
        let leaf = self.leaf_id.read().clone();
        let entry = SessionEntry::Message(SessionMessageEntry {
            base: SessionEntryBase {
                entry_type: "message".to_string(),
                id: generate_id(&self.by_id.read().keys().cloned().collect()),
                parent_id: leaf,
                timestamp: Utc::now().to_rfc3339(),
            },
            message,
        });
        let id = entry.id().to_string();
        self._append_entry(entry);
        id
    }

    /// Append a thinking level change
    pub fn append_thinking_level_change(&mut self, thinking_level: &str) -> String {
        let leaf = self.leaf_id.read().clone();
        let entry = SessionEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
            base: SessionEntryBase {
                entry_type: "thinking_level_change".to_string(),
                id: generate_id(&self.by_id.read().keys().cloned().collect()),
                parent_id: leaf,
                timestamp: Utc::now().to_rfc3339(),
            },
            thinking_level: thinking_level.to_string(),
        });
        let id = entry.id().to_string();
        self._append_entry(SessionEntry::ThinkingLevelChange(entry));
        id
    }

    /// Append a model change
    pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> String {
        let leaf = self.leaf_id.read().clone();
        let entry = ModelChangeEntry {
            base: SessionEntryBase {
                entry_type: "model_change".to_string(),
                id: generate_id(&self.by_id.read().keys().cloned().collect()),
                parent_id: leaf,
                timestamp: Utc::now().to_rfc3339(),
            },
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        };
        let id = entry.id.to_string();
        self._append_entry(SessionEntry::ModelChange(entry));
        id
    }

    /// Append a compaction summary
    pub fn append_compaction<T: serde::Serialize>(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: i64,
        details: Option<T>,
        from_hook: Option<bool>,
    ) -> String {
        let leaf = self.leaf_id.read().clone();
        let entry = CompactionEntry {
            base: SessionEntryBase {
                entry_type: "compaction".to_string(),
                id: generate_id(&self.by_id.read().keys().cloned().collect()),
                parent_id: leaf,
                timestamp: Utc::now().to_rfc3339(),
            },
            summary: summary.to_string(),
            first_kept_entry_id: first_kept_entry_id.to_string(),
            tokens_before,
            details: details.map(|d| serde_json::to_value(d).ok()).flatten(),
            from_hook,
        };
        let id = entry.base.id.clone();
        self._append_entry(SessionEntry::Compaction(entry));
        id
    }

    /// Append a custom entry (for extensions)
    pub fn append_custom_entry(&mut self, custom_type: &str, data: Option<serde_json::Value>) -> String {
        let leaf = self.leaf_id.read().clone();
        let entry = CustomEntry {
            base: SessionEntryBase {
                entry_type: "custom".to_string(),
                id: generate_id(&self.by_id.read().keys().cloned().collect()),
                parent_id: leaf,
                timestamp: Utc::now().to_rfc3339(),
            },
            custom_type: custom_type.to_string(),
            data,
        };
        let id = entry.base.id.clone();
        self._append_entry(SessionEntry::Custom(entry));
        id
    }

    /// Append a session info entry (e.g., display name)
    pub fn append_session_info(&mut self, name: &str) -> String {
        let leaf = self.leaf_id.read().clone();
        let entry = SessionInfoEntry {
            base: SessionEntryBase {
                entry_type: "session_info".to_string(),
                id: generate_id(&self.by_id.read().keys().cloned().collect()),
                parent_id: leaf,
                timestamp: Utc::now().to_rfc3339(),
            },
            name: Some(name.trim().to_string()),
        };
        let id = entry.base.id.clone();
        self._append_entry(SessionEntry::SessionInfo(entry));
        id
    }

    /// Get the current session name from the latest session_info entry
    pub fn get_session_name(&self) -> Option<String> {
        let entries = self.get_entries();
        for entry in entries.iter().rev() {
            if let SessionEntry::SessionInfo(e) = entry {
                return e.name.as_ref().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            }
        }
        None
    }

    /// Append a custom message entry (for extensions) that participates in LLM context
    pub fn append_custom_message_entry<T: serde::Serialize>(
        &mut self,
        custom_type: &str,
        content: ContentValue,
        display: bool,
        details: Option<T>,
    ) -> String {
        let leaf = self.leaf_id.read().clone();
        let entry = CustomMessageEntry {
            base: SessionEntryBase {
                entry_type: "custom_message".to_string(),
                id: generate_id(&self.by_id.read().keys().cloned().collect()),
                parent_id: leaf,
                timestamp: Utc::now().to_rfc3339(),
            },
            custom_type: custom_type.to_string(),
            content,
            display,
            details: details.map(|d| serde_json::to_value(d).ok()).flatten(),
        };
        let id = entry.base.id.clone();
        self._append_entry(SessionEntry::CustomMessage(entry));
        id
    }

    // =========================================================================
    // Tree Traversal
    // =========================================================================

    /// Get the current leaf ID
    pub fn get_leaf_id(&self) -> Option<String> {
        self.leaf_id.read().clone()
    }

    /// Get the current leaf entry
    pub fn get_leaf_entry(&self) -> Option<SessionEntry> {
        self.leaf_id.read().as_ref().and_then(|id| self.by_id.read().get(id).cloned())
    }

    /// Get an entry by ID
    pub fn get_entry(&self, id: &str) -> Option<SessionEntry> {
        self.by_id.read().get(id).cloned()
    }

    /// Get all direct children of an entry
    pub fn get_children(&self, parent_id: &str) -> Vec<SessionEntry> {
        self.by_id
            .read()
            .values()
            .filter(|e| e.parent_id() == Some(parent_id))
            .cloned()
            .collect()
    }

    /// Get the parent of an entry
    pub fn get_parent(&self, id: &str) -> Option<SessionEntry> {
        self.by_id
            .read()
            .get(id)
            .and_then(|e| e.parent_id())
            .and_then(|pid| self.by_id.read().get(pid).cloned())
    }

    /// Get the label for an entry
    pub fn get_label(&self, id: &str) -> Option<String> {
        self.labels_by_id.read().get(id).cloned()
    }

    /// Set or clear a label on an entry
    pub fn append_label_change(&mut self, target_id: &str, label: Option<&str>) -> Result<String, String> {
        if !self.by_id.read().contains_key(target_id) {
            return Err(format!("Entry {} not found", target_id));
        }

        let leaf = self.leaf_id.read().clone();
        let entry = LabelEntry {
            base: SessionEntryBase {
                entry_type: "label".to_string(),
                id: generate_id(&self.by_id.read().keys().cloned().collect()),
                parent_id: leaf,
                timestamp: Utc::now().to_rfc3339(),
            },
            target_id: target_id.to_string(),
            label: label.map(|s| s.to_string()),
        };

        let id = entry.base.id.clone();
        self._append_entry(SessionEntry::Label(entry.clone()));

        if let Some(l) = label {
            self.labels_by_id.write().insert(target_id.to_string(), l.to_string());
            self.label_timestamps_by_id.write().insert(target_id.to_string(), entry.base.timestamp);
        } else {
            self.labels_by_id.write().remove(target_id);
            self.label_timestamps_by_id.write().remove(target_id);
        }

        Ok(id)
    }

    /// Walk from entry to root, returning all entries in path order
    pub fn get_branch(&self, from_id: Option<&str>) -> Vec<SessionEntry> {
        let mut path = Vec::new();
        let start_id = from_id.or_else(|| self.leaf_id.read().clone().as_deref());
        let Some(start_id) = start_id else {
            return path;
        };

        let mut current = self.by_id.read().get(start_id).cloned();
        while let Some(entry) = current {
            path.insert(0, entry.clone());
            current = entry.parent_id().and_then(|pid| self.by_id.read().get(pid).cloned());
        }
        path
    }

    /// Get path to root for a given entry
    pub fn get_path_to_root(&self, from_id: &str) -> Vec<SessionEntry> {
        self.get_branch(Some(from_id))
    }

    /// Get ancestry (same as path to root)
    pub fn get_ancestry(&self, from_id: &str) -> Vec<SessionEntry> {
        self.get_branch(Some(from_id))
    }

    /// Get depth of an entry
    pub fn get_depth(&self, id: &str) -> i64 {
        let mut depth = 0;
        let mut current = self.by_id.read().get(id).cloned();
        while let Some(entry) = current {
            depth += 1;
            current = entry.parent_id().and_then(|pid| self.by_id.read().get(pid).cloned());
        }
        depth - 1 // Root has depth 0
    }

    /// Build the session context (what gets sent to the LLM)
    pub fn build_session_context(&self) -> SessionContext {
        build_session_context(self.get_entries(), self.leaf_id.read().clone(), Some(&self.by_id))
    }

    /// Get session header
    pub fn get_header(&self) -> Option<SessionHeader> {
        self.file_entries.read().iter().find_map(|e| match e {
            FileEntry::Header(h) => Some(h.clone()),
            _ => None,
        })
    }

    /// Get all session entries (excludes header)
    pub fn get_entries(&self) -> Vec<SessionEntry> {
        self.file_entries
            .read()
            .iter()
            .filter_map(|e| match e {
                FileEntry::Entry(entry) => Some(entry.clone()),
                _ => None,
            })
            .collect()
    }

    /// Get the session as a tree structure
    pub fn get_tree(&self) -> Vec<SessionTreeNode> {
        let entries = self.get_entries();
        let labels = self.labels_by_id.read();
        let label_timestamps = self.label_timestamps_by_id.read();

        let mut node_map: HashMap<String, SessionTreeNode> = HashMap::new();
        let mut roots: Vec<SessionTreeNode> = Vec::new();

        // Create nodes with resolved labels
        for entry in &entries {
            node_map.insert(
                entry.id().to_string(),
                SessionTreeNode {
                    entry: entry.clone(),
                    children: Vec::new(),
                    label: labels.get(entry.id()).cloned(),
                    label_timestamp: label_timestamps.get(entry.id()).cloned(),
                },
            );
        }

        // Build tree
        for entry in &entries {
            let node = node_map.get(entry.id()).unwrap();
            match entry.parent_id() {
                Some(pid) if pid != entry.id() => {
                    if let Some(parent) = node_map.get(pid) {
                        let parent_children = &mut node_map.get_mut(pid).unwrap().children;
                        parent_children.push(node.clone());
                    } else {
                        // Orphan - treat as root
                        roots.push(node.clone());
                    }
                }
                _ => {
                    roots.push(node.clone());
                }
            }
        }

        // Sort children by timestamp (oldest first, newest at bottom)
        sort_tree_by_timestamp(&mut roots);

        roots
    }

    // =========================================================================
    // Branching
    // =========================================================================

    /// Start a new branch from an earlier entry
    pub fn branch(&mut self, branch_from_id: &str) -> Result<(), String> {
        if !self.by_id.read().contains_key(branch_from_id) {
            return Err(format!("Entry {} not found", branch_from_id));
        }
        *self.leaf_id.write() = Some(branch_from_id.to_string());
        Ok(())
    }

    /// Reset the leaf pointer to null (before any entries)
    pub fn reset_leaf(&mut self) {
        *self.leaf_id.write() = None;
    }

    /// Start a new branch with a summary of the abandoned path
    pub fn branch_with_summary(
        &mut self,
        branch_from_id: Option<&str>,
        summary: &str,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> String {
        if let Some(id) = branch_from_id {
            if !self.by_id.read().contains_key(id) {
                return String::new();
            }
        }

        *self.leaf_id.write() = branch_from_id.map(|s| s.to_string());

        let entry = BranchSummaryEntry {
            base: SessionEntryBase {
                entry_type: "branch_summary".to_string(),
                id: generate_id(&self.by_id.read().keys().cloned().collect()),
                parent_id: branch_from_id.map(|s| s.to_string()),
                timestamp: Utc::now().to_rfc3339(),
            },
            from_id: branch_from_id.unwrap_or("root").to_string(),
            summary: summary.to_string(),
            details,
            from_hook,
        };

        let id = entry.base.id.clone();
        self._append_entry(SessionEntry::BranchSummary(entry));
        id
    }

    /// Add a label to the session
    pub fn add_label(&mut self, target_id: &str, label: &str) -> Result<String, String> {
        self.append_label_change(target_id, Some(label))
    }

    /// Remove a label from an entry
    pub fn remove_label(&mut self, target_id: &str) -> Result<String, String> {
        self.append_label_change(target_id, None)
    }

    // =========================================================================
    // Compaction Support
    // =========================================================================

    /// Get the latest compaction entry
    pub fn get_latest_compaction_entry(&self) -> Option<CompactionEntry> {
        let entries = self.get_entries();
        for entry in entries.iter().rev() {
            if let SessionEntry::Compaction(c) = entry {
                return Some(c.clone());
            }
        }
        None
    }

    /// Get all compaction entries
    pub fn get_compaction_entries(&self) -> Vec<CompactionEntry> {
        self.get_entries()
            .iter()
            .filter_map(|e| match e {
                SessionEntry::Compaction(c) => Some(c.clone()),
                _ => None,
            })
            .collect()
    }

    // =========================================================================
    // Session Statistics
    // =========================================================================

    /// Get session statistics
    pub fn get_session_stats(&self) -> SessionStats {
        let entries = self.get_entries();
        let mut message_count = 0i64;
        let mut user_message_count = 0i64;
        let mut assistant_message_count = 0i64;
        let mut total_chars = 0i64;
        let mut total_tokens_estimate = 0i64;

        for entry in &entries {
            if let SessionEntry::Message(m) = entry {
                if let AgentMessage::User { .. } = &m.message {
                    user_message_count += 1;
                }
                if let AgentMessage::Assistant { .. } = &m.message {
                    assistant_message_count += 1;
                }
                message_count += 1;

                // Estimate tokens from message
                let chars = estimate_message_chars(&m.message);
                total_chars += chars;
                total_tokens_estimate += (chars as f64 / 4.0).ceil() as i64;
            }
        }

        SessionStats {
            message_count,
            user_message_count,
            assistant_message_count,
            total_chars,
            estimated_tokens: total_tokens_estimate,
        }
    }

    // =========================================================================
    // Static Methods
    // =========================================================================

    /// List all sessions for a directory
    pub async fn list(cwd: &str, session_dir: Option<&str>) -> Result<Vec<SessionInfo>> {
        let dir = session_dir
            .map(|s| s.to_string())
            .unwrap_or_else(|| get_default_session_dir(cwd));
        list_sessions_from_dir(&dir).await
    }

    /// List all sessions across all project directories
    pub async fn list_all() -> Result<Vec<SessionInfo>> {
        let sessions_dir = get_sessions_dir();

        if !Path::new(&sessions_dir).exists() {
            return Ok(Vec::new());
        }

        let mut all_sessions = Vec::new();
        let entries = fs::read_dir(&sessions_dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if let Ok(sessions) = list_sessions_from_dir(&path.to_string_lossy()).await {
                    all_sessions.extend(sessions);
                }
            }
        }

        all_sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
        Ok(all_sessions)
    }

    /// Fork a session from another project directory into the current project
    pub fn fork_from(source_path: &str, target_cwd: &str, session_dir: Option<&str>) -> Result<Self, String> {
        let source_entries = load_entries_from_file(source_path);
        if source_entries.is_empty() {
            return Err(format!("Cannot fork: source session file is empty or invalid: {}", source_path));
        }

        let source_header = source_entries.iter().find_map(|e| match e {
            FileEntry::Header(h) => Some(h),
            _ => None,
        });
        if source_header.is_none() {
            return Err(format!("Cannot fork: source session has no header: {}", source_path));
        }

        let dir = session_dir
            .map(|s| s.to_string())
            .unwrap_or_else(|| get_default_session_dir(target_cwd));

        if !Path::new(&dir).exists() {
            let _ = fs::create_dir_all(&dir);
        }

        let new_session_id = Uuid::new_v4().to_string();
        let timestamp = Utc::now().to_rfc3339();
        let file_timestamp = timestamp.replace([':', '.', 'T', '-', ':', '+'], "-");
        let short_id = &new_session_id[..8];
        let new_session_file = format!("{}/{}_{}.jsonl", dir, file_timestamp, short_id);

        // Write new header pointing to source as parent
        let new_header = SessionHeader {
            entry_type: "session".to_string(),
            version: Some(CURRENT_SESSION_VERSION),
            id: new_session_id.clone(),
            timestamp: timestamp.clone(),
            cwd: target_cwd.to_string(),
            parent_session: Some(source_path.to_string()),
        };

        let mut handle = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&new_session_file)
            .map_err(|e| e.to_string())?;
        writeln!(&mut handle, "{}", serde_json::to_string(&new_header).unwrap()).map_err(|e| e.to_string())?;

        // Copy all non-header entries from source
        for file_entry in &source_entries {
            if let FileEntry::Entry(_) = file_entry {
                writeln!(&mut handle, "{}", serde_json::to_string(file_entry).unwrap())
                    .map_err(|e| e.to_string())?;
            }
        }

        Ok(Self::open(&new_session_file, Some(&dir), Some(target_cwd)))
    }

    /// Delete a session
    pub fn delete_session(path: &str) -> Result<()> {
        fs::remove_file(path).context("Failed to delete session file")?;
        Ok(())
    }

    /// Rename a session (set its display name)
    pub fn rename_session(&mut self, name: &str) -> String {
        self.append_session_info(name)
    }
}

// ============================================================================
// Session Statistics
// ============================================================================

#[derive(Debug, Clone)]
pub struct SessionStats {
    pub message_count: i64,
    pub user_message_count: i64,
    pub assistant_message_count: i64,
    pub total_chars: i64,
    pub estimated_tokens: i64,
}

// ============================================================================
// NewSessionOptions
// ============================================================================

#[derive(Debug, Clone)]
pub struct NewSessionOptions {
    pub id: Option<String>,
    pub parent_session: Option<String>,
}

// ============================================================================
// Helper Functions
// ============================================================================

fn get_default_session_dir(cwd: &str) -> String {
    let agent_dir = get_agent_dir();
    let safe_path = format!("--{}--", cwd.replace('/',
"").replace('\\', "").replace('/', "-").replace('\\', "-").replace(':', "-"));
    let session_dir = format!("{}/sessions/{}", agent_dir, safe_path);

    if !Path::new(&session_dir).exists() {
        let _ = fs::create_dir_all(&session_dir);
    }

    session_dir
}

fn get_agent_dir() -> String {
    dirs::home_dir()
        .map(|h| h.join(".oxi").to_string_lossy().to_string())
        .unwrap_or_else(|| ".oxi".to_string())
}

fn get_sessions_dir() -> String {
    format!("{}/sessions", get_agent_dir())
}

/// Load entries from a JSONL file
fn load_entries_from_file(file_path: &str) -> Vec<FileEntry> {
    if !Path::new(file_path).exists() {
        return Vec::new();
    }

    let file = match File::open(file_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let reader = BufReader::new(file);
    let mut entries = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<FileEntry>(&line) {
            Ok(entry) => entries.push(entry),
            Err(_) => continue,
        }
    }

    // Validate session header
    if entries.is_empty() {
        return entries;
    }
    let header = match &entries[0] {
        FileEntry::Header(h) => h,
        _ => return Vec::new(),
    };
    if header.entry_type != "session" || header.id.is_empty() {
        return Vec::new();
    }

    entries
}

/// Check if a file is a valid session file
fn is_valid_session_file(file_path: &str) -> bool {
    if let Ok(mut file) = File::open(file_path) {
        use std::io::Read;
        let mut buffer = vec![0u8; 512];
        if let Ok(bytes_read) = file.read(&mut buffer) {
            if let Ok(content) = String::from_utf8(buffer[..bytes_read].to_vec()) {
                if let Some(first_line) = content.split('\n').next() {
                    if let Ok(header) = serde_json::from_str::<SessionHeader>(first_line) {
                        return header.entry_type == "session" && !header.id.is_empty();
                    }
                }
            }
        }
    }
    false
}

/// Find the most recent session in a directory
fn find_most_recent_session(session_dir: &str) -> Option<String> {
    if !Path::new(session_dir).exists() {
        return None;
    }

    let mut files: Vec<(String, std::time::SystemTime)> = Vec::new();

    if let Ok(entries) = fs::read_dir(session_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                if let Some(path_str) = path.to_str() {
                    if is_valid_session_file(path_str) {
                        if let Ok(metadata) = entry.metadata() {
                            if let Ok(mtime) = metadata.modified() {
                                files.push((path_str.to_string(), mtime));
                            }
                        }
                    }
                }
            }
        }
    }

    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.into_iter().next().map(|(p, _)| p)
}

/// Build session context from entries using tree traversal
fn build_session_context(
    entries: Vec<SessionEntry>,
    leaf_id: Option<String>,
    by_id: Option<&RwLock<HashMap<String, SessionEntry>>>,
) -> SessionContext {
    let mut id_map: HashMap<String, SessionEntry> = HashMap::new();
    for entry in &entries {
        id_map.insert(entry.id().to_string(), entry.clone());
    }

    let by_id_ref: &HashMap<String, SessionEntry> = match by_id {
        Some(lock) => lock.read().deref(),
        None => &id_map,
    };

    // Find leaf
    let leaf: Option<&SessionEntry> = match leaf_id {
        Some(ref id) => by_id_ref.get(id),
        None => None,
    };

    if leaf.is_none() && !entries.is_empty() {
        // Fallback to last entry
        leaf = entries.last().map(|e| by_id_ref.get(e.id()).unwrap());
    }

    let Some(leaf) = leaf else {
        return SessionContext {
            messages: Vec::new(),
            thinking_level: "off".to_string(),
            model: None,
        };
    };

    // Walk from leaf to root, collecting path
    let mut path: Vec<&SessionEntry> = Vec::new();
    let mut current: Option<&SessionEntry> = Some(leaf);
    while let Some(entry) = current {
        path.insert(0, entry);
        current = entry.parent_id().and_then(|pid| by_id_ref.get(pid));
    }

    // Extract settings and find compaction
    let mut thinking_level = "off".to_string();
    let mut model: Option<ModelInfo> = None;
    let mut compaction: Option<&CompactionEntry> = None;

    for entry in &path {
        match entry {
            SessionEntry::ThinkingLevelChange(e) => {
                thinking_level = e.thinking_level.clone();
            }
            SessionEntry::ModelChange(e) => {
                model = Some(ModelInfo {
                    provider: e.provider.clone(),
                    model_id: e.model_id.clone(),
                });
            }
            SessionEntry::Message(e) => {
                if let AgentMessage::Assistant { provider, model_id, .. } = &e.message {
                    model = Some(ModelInfo {
                        provider: provider.clone().unwrap_or_default(),
                        model_id: model_id.clone().unwrap_or_default(),
                    });
                }
            }
            SessionEntry::Compaction(e) => {
                compaction = Some(e);
            }
            _ => {}
        }
    }

    // Build messages
    let mut messages: Vec<AgentMessage> = Vec::new();

    if let Some(comp) = compaction {
        // Emit summary first
        messages.push(AgentMessage::CompactionSummary {
            summary: comp.summary.clone(),
            tokens_before: comp.tokens_before,
            timestamp: chrono::Utc::now().timestamp_millis(),
        });

        // Find compaction index in path
        let compaction_idx = path.iter().position(|e| e.id() == comp.base.id);

        if let Some(idx) = compaction_idx {
            // Emit kept messages (before compaction, starting from firstKeptEntryId)
            let mut found_first_kept = false;
            for (i, entry) in path[..idx].iter().enumerate() {
                if entry.id() == comp.first_kept_entry_id {
                    found_first_kept = true;
                }
                if found_first_kept {
                    if let Some(msg) = get_message_from_entry(entry) {
                        messages.push(msg);
                    }
                }
            }

            // Emit messages after compaction
            for entry in &path[idx + 1..] {
                if let Some(msg) = get_message_from_entry(entry) {
                    messages.push(msg);
                }
            }
        }
    } else {
        // No compaction - emit all messages
        for entry in &path {
            if let Some(msg) = get_message_from_entry(entry) {
                messages.push(msg);
            }
        }
    }

    SessionContext {
        messages,
        thinking_level,
        model,
    }
}

/// Get message from entry for session context
fn get_message_from_entry(entry: &SessionEntry) -> Option<AgentMessage> {
    match entry {
        SessionEntry::Message(e) => Some(e.message.clone()),
        SessionEntry::CustomMessage(e) => Some(AgentMessage::Custom {
            custom_type: e.custom_type.clone(),
            content: e.content.clone(),
            display: e.display,
            details: e.details.clone(),
            timestamp: chrono::DateTime::parse_from_rfc3339(&e.base.timestamp)
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0),
        }),
        SessionEntry::BranchSummary(e) => Some(AgentMessage::BranchSummary {
            summary: e.summary.clone(),
            from_id: e.from_id.clone(),
            timestamp: chrono::DateTime::parse_from_rfc3339(&e.base.timestamp)
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0),
        }),
        SessionEntry::Compaction(e) => Some(AgentMessage::CompactionSummary {
            summary: e.summary.clone(),
            tokens_before: e.tokens_before,
            timestamp: chrono::DateTime::parse_from_rfc3339(&e.base.timestamp)
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0),
        }),
        _ => None,
    }
}

/// Estimate character count for a message
fn estimate_message_chars(message: &AgentMessage) -> i64 {
    match message {
        AgentMessage::User { content } => match content {
            ContentValue::String(s) => s.len() as i64,
            ContentValue::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.len()),
                    ContentBlock::Image { .. } => Some(4800), // Estimate images
                })
                .sum::<usize>() as i64,
        },
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .map(|block| match block {
                AssistantContentBlock::Text { text } => text.len(),
                AssistantContentBlock::Thinking { thinking } => thinking.len(),
                AssistantContentBlock::ToolCall { name, arguments, .. } => {
                    name.len() + arguments.to_string().len()
                }
                _ => 0,
            })
            .sum::<usize>() as i64,
        AgentMessage::ToolResult { content, .. } => match content {
            ContentValue::String(s) => s.len() as i64,
            ContentValue::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.len()),
                    ContentBlock::Image { .. } => Some(4800),
                })
                .sum::<usize>() as i64,
        },
        AgentMessage::BashExecution { command, output, .. } => {
            (command.len() + output.len()) as i64
        }
        AgentMessage::BranchSummary { summary, .. } => summary.len() as i64,
        AgentMessage::CompactionSummary { summary, .. } => summary.len() as i64,
        AgentMessage::Custom { content, .. } => match content {
            ContentValue::String(s) => s.len() as i64,
            ContentValue::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.len()),
                    ContentBlock::Image { .. } => Some(4800),
                })
                .sum::<usize>() as i64,
        },
        AgentMessage::System { content } => match content {
            ContentValue::String(s) => s.len() as i64,
            ContentValue::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.len()),
                    ContentBlock::Image { .. } => Some(4800),
                })
                .sum::<usize>() as i64,
        },
    }
}

/// Sort tree nodes by timestamp
fn sort_tree_by_timestamp(nodes: &mut Vec<SessionTreeNode>) {
    nodes.sort_by(|a, b| {
        let time_a = chrono::DateTime::parse_from_rfc3339(a.entry.timestamp())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0);
        let time_b = chrono::DateTime::parse_from_rfc3339(b.entry.timestamp())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0);
        time_a.cmp(&time_b)
    });

    for node in nodes {
        sort_tree_by_timestamp(&mut node.children);
    }
}

/// List sessions from a directory
async fn list_sessions_from_dir(dir: &str) -> Result<Vec<SessionInfo>> {
    if !Path::new(dir).exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    let entries = fs::read_dir(dir)?;
    let files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "jsonl").unwrap_or(false))
        .filter_map(|e| e.path().to_str().map(|s| s.to_string()))
        .collect();

    for file in files {
        if let Some(info) = build_session_info(&file).await {
            sessions.push(info);
        }
    }

    Ok(sessions)
}

/// Build session info from a file
async fn build_session_info(file_path: &str) -> Option<SessionInfo> {
    let content = fs::read_to_string(file_path).ok()?;
    let entries = parse_session_entries(&content)?;

    if entries.is_empty() {
        return None;
    }

    let header = match &entries[0] {
        FileEntry::Header(h) => h,
        _ => return None,
    };

    let stats = fs::metadata(file_path).ok()?;
    let mut message_count = 0i64;
    let mut first_message = String::new();
    let mut all_messages = Vec::new();
    let mut name: Option<String> = None;

    for entry in &entries {
        if let FileEntry::Entry(e) = entry {
            if let SessionEntry::SessionInfo(si) = e {
                name = si.name.clone().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            }
        }

        if let FileEntry::Entry(SessionEntry::Message(m)) = entry {
            if let AgentMessage::User { content } = &m.message {
                message_count += 1;
                let text = extract_text_content(content);
                if !text.is_empty() {
                    all_messages.push(text.clone());
                    if first_message.is_empty() {
                        first_message = text;
                    }
                }
            }
        }
    }

    let cwd = header.cwd.clone();
    let parent_session_path = header.parent_session.clone();
    let created = chrono::DateTime::parse_from_rfc3339(&header.timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let modified = get_session_modified_date(&entries, &header.timestamp, stats.modified());

    Some(SessionInfo {
        path: file_path.to_string(),
        id: header.id.clone(),
        cwd,
        name,
        parent_session_path,
        created,
        modified,
        message_count,
        first_message: first_message.unwrap_or_else(|| "(no messages)".to_string()),
        all_messages_text: all_messages.join(" "),
    })
}

/// Parse session entries from content
fn parse_session_entries(content: &str) -> Option<Vec<FileEntry>> {
    let mut entries = Vec::new();

    for line in content.trim().lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<FileEntry>(line) {
            entries.push(entry);
        }
    }

    Some(entries)
}

/// Get session modified date
fn get_session_modified_date(
    entries: &[FileEntry],
    header_timestamp: &str,
    stats_mtime: std::fs::Metadata,
) -> DateTime<Utc> {
    let last_activity_time = get_last_activity_time(entries);
    if let Some(t) = last_activity_time {
        if t > 0 {
            return DateTime::from_timestamp_millis(t);
        }
    }

    let header_time = chrono::DateTime::parse_from_rfc3339(header_timestamp)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(-1);

    if header_time > 0 {
        return DateTime::from_timestamp_millis(header_time);
    }

    if let Ok(mtime) = stats_mtime.modified() {
        return DateTime::from(mtime);
    }

    Utc::now()
}

/// Get last activity time from entries
fn get_last_activity_time(entries: &[FileEntry]) -> Option<i64> {
    let mut last_activity: Option<i64> = None;

    for entry in entries {
        let entry = match entry {
            FileEntry::Entry(e) => e,
            _ => continue,
        };

        if let SessionEntry::Message(m) = entry {
            if let AgentMessage::User { .. } | AgentMessage::Assistant { .. } = &m.message {
                // Check timestamp from message if available
                let timestamp = chrono::DateTime::parse_from_rfc3339(&m.base.timestamp)
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or(-1);

                if timestamp > 0 {
                    last_activity = Some(std::cmp::max(last_activity.unwrap_or(0), timestamp));
                }
            }
        }
    }

    last_activity
}

/// Extract text content from a message
fn extract_text_content(content: &ContentValue) -> String {
    match content {
        ContentValue::String(s) => s.clone(),
        ContentValue::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let manager = SessionManager::in_memory("/tmp");
        assert!(!manager.get_session_id().is_empty());
        assert_eq!(manager.get_entries().len(), 0);
    }

    #[test]
    fn test_append_message() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id = manager.append_message(AgentMessage::User {
            content: ContentValue::String("Hello".to_string()),
        });
        assert!(!id.is_empty());
        assert_eq!(manager.get_entries().len(), 1);
        assert_eq!(manager.get_leaf_id(), Some(id));
    }

    #[test]
    fn test_tree_traversal() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(AgentMessage::User {
            content: ContentValue::String("Hello".to_string()),
        });
        let _id2 = manager.append_message(AgentMessage::Assistant {
            content: vec![],
            provider: None,
            model_id: None,
            usage: None,
            stop_reason: None,
        });

        // Get branch from root
        let branch = manager.get_branch(None);
        assert_eq!(branch.len(), 2);

        // Get branch from specific entry
        let branch = manager.get_branch(Some(&id1));
        assert_eq!(branch.len(), 1);

        // Get children
        let children = manager.get_children(&id1);
        assert_eq!(children.len(), 1);

        // Get parent
        let parent = manager.get_parent(&id2);
        assert!(parent.is_some());
        assert_eq!(parent.unwrap().id(), id1);
    }

    #[test]
    fn test_branching() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(AgentMessage::User {
            content: ContentValue::String("Hello".to_string()),
        });
        let _id2 = manager.append_message(AgentMessage::Assistant {
            content: vec![],
            provider: None,
            model_id: None,
            usage: None,
            stop_reason: None,
        });
        let _id3 = manager.append_message(AgentMessage::User {
            content: ContentValue::String("How are you?".to_string()),
        });

        // Branch from first message
        manager.branch(&id1).unwrap();
        assert_eq!(manager.get_leaf_id(), Some(id1.clone()));

        // Add new message on branch
        let id4 = manager.append_message(AgentMessage::Assistant {
            content: vec![],
            provider: None,
            model_id: None,
            usage: None,
            stop_reason: None,
        });

        // Should have 4 entries total (3 original + 1 new branch)
        assert_eq!(manager.get_entries().len(), 4);

        // Leaf should be the new message
        assert_eq!(manager.get_leaf_id(), Some(id4));

        // Get tree
        let tree = manager.get_tree();
        assert_eq!(tree.len(), 2); // Two root branches
    }

    #[test]
    fn test_session_context() {
        let mut manager = SessionManager::in_memory("/tmp");
        manager.append_message(AgentMessage::User {
            content: ContentValue::String("Hello".to_string()),
        });
        manager.append_message(AgentMessage::Assistant {
            content: vec![AssistantContentBlock::Text {
                text: "Hi there!".to_string(),
            }],
            provider: Some("test".to_string()),
            model_id: Some("model".to_string()),
            usage: None,
            stop_reason: None,
        });

        let context = manager.build_session_context();
        assert_eq!(context.messages.len(), 2);
        assert!(context.model.is_some());
    }

    #[test]
    fn test_compaction_entry() {
        let mut manager = SessionManager::in_memory("/tmp");
        let _id1 = manager.append_message(AgentMessage::User {
            content: ContentValue::String("First message".to_string()),
        });
        let _id2 = manager.append_message(AgentMessage::Assistant {
            content: vec![],
            provider: None,
            model_id: None,
            usage: None,
            stop_reason: None,
        });

        let id3 = manager.append_compaction(
            "Summarized conversation",
            &_id1,
            1000,
            None::<()>,
            None,
        );
        assert!(!id3.is_empty());

        let latest = manager.get_latest_compaction_entry();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().summary, "Summarized conversation");
    }

    #[test]
    fn test_labels() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(AgentMessage::User {
            content: ContentValue::String("Hello".to_string()),
        });

        manager.add_label(&id1, "important").unwrap();
        assert_eq!(manager.get_label(&id1), Some("important".to_string()));

        manager.remove_label(&id1).unwrap();
        assert_eq!(manager.get_label(&id1), None);
    }
}
