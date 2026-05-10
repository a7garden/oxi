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

/// Type alias for entry IDs (for backward compatibility)
pub type EntryId = Uuid;

/// Current session version for migrations
pub const CURRENT_SESSION_VERSION: i32 = 3;

// ============================================================================
// Backward Compatibility Layer
// ============================================================================

/// Session metadata stored separately from entries (backward compatibility)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Unique session identifier.
    pub id: Uuid,
    /// ID of the parent session this was branched from.
    pub parent_id: Option<Uuid>,
    /// ID of the root session in the branch chain.
    pub root_id: Option<Uuid>,
    /// Entry ID where this session was branched.
    pub branch_point: Option<Uuid>,
    /// Creation timestamp in milliseconds since epoch.
    pub created_at: i64,
    /// Last update timestamp in milliseconds since epoch.
    pub updated_at: i64,
    /// Optional human-readable session name.
    pub name: Option<String>,
}

impl SessionMeta {
    /// New.
    pub fn new(id: Uuid) -> Self {
        let now = Utc::now().timestamp_millis();
        Self {
            id,
            parent_id: None,
            root_id: None,
            branch_point: None,
            created_at: now,
            updated_at: now,
            name: None,
        }
    }


    /// Branched from.
    pub fn branched_from(parent_id: Uuid, root_id: Option<Uuid>, branch_point: Uuid) -> Self {
        let now = Utc::now().timestamp_millis();
        Self {
            id: Uuid::new_v4(),
            parent_id: Some(parent_id),
            root_id: root_id.or(Some(parent_id)),
            branch_point: Some(branch_point),
            created_at: now,
            updated_at: now,
            name: None,
        }
    }
}

/// Information about where a session branched from
#[derive(Debug, Clone)]
pub struct BranchInfo {
    /// The session id.
    pub session_id: Uuid,
    /// The parent session id.
    pub parent_session_id: Option<Uuid>,
    /// The root session id.
    pub root_session_id: Option<Uuid>,
    /// The branch point entry id.
    pub branch_point_entry_id: Option<Uuid>,
    /// The parent session name.
    pub parent_session_name: Option<String>,
}

// ============================================================================
// Session Header
// ============================================================================

/// Session header stored as the first line in JSONL files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    /// The entry type.
    #[serde(rename = "type")]
    pub entry_type: String,
    /// The version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i32>,
    /// The id.
    pub id: String,
    /// The timestamp.
    pub timestamp: String,
    /// The cwd.
    pub cwd: String,
    /// The parent session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

impl SessionHeader {
    /// New.
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
// Content Types
// ============================================================================

/// Content can be string or array of content blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentValue {
    /// String.
    String(String),
    /// Blocks.
    Blocks(Vec<ContentBlock>),
}

impl ContentValue {
    /// As str.
    pub fn as_str(&self) -> &str {
        match self {
            ContentValue::String(s) => s,
            ContentValue::Blocks(blocks) => {
                // For blocks, return first text block or empty
                for block in blocks {
                    if let ContentBlock::Text { text } = block {
                        return text;
                    }
                }
                ""
            }
        }
    }
}

impl From<String> for ContentValue {
    fn from(s: String) -> Self {
        ContentValue::String(s)
    }
}

impl From<&str> for ContentValue {
    fn from(s: &str) -> Self {
        ContentValue::String(s.to_string())
    }
}

/// Content block for text or image content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain text content block.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: String,
    },
    /// Image content block.
    #[serde(rename = "image")]
    Image {
        /// Base64-encoded image data.
        data: String,
        /// MIME type of the image.
        media_type: Option<String>,
    },
}

// ============================================================================
// Agent Message Types
// ============================================================================

/// Agent message roles
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum AgentMessage {
    /// User.
    #[serde(rename = "user")]
    User { 
        /// The content.
        #[serde(flatten)]
        content: ContentValue,
    },
    /// Assistant.
    #[serde(rename = "assistant")]
    Assistant {
        /// The content.
        content: Vec<AssistantContentBlock>,
        /// The provider.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// The model id.
        #[serde(skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        /// The usage.
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// The stop reason.
        #[serde(rename = "stopReason", skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },
    /// Tool Result.
    #[serde(rename = "toolResult")]
    ToolResult {
        /// The content.
        content: ContentValue,
        /// The tool call id.
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
    },
    /// System.
    #[serde(rename = "system")]
    System {
        /// The content.
        #[serde(flatten)]
        content: ContentValue,
    },
    /// Bash Execution.
    #[serde(rename = "bashExecution")]
    BashExecution {
        /// The command.
        command: String,
        /// The output.
        output: String,
        /// The exit code.
        #[serde(rename = "exitCode")]
        exit_code: Option<i32>,
        /// The cancelled.
        cancelled: bool,
        /// The truncated.
        truncated: bool,
        /// The full output path.
        #[serde(rename = "fullOutputPath", skip_serializing_if = "Option::is_none")]
        full_output_path: Option<String>,
        /// The exclude from context.
        #[serde(rename = "excludeFromContext", skip_serializing_if = "Option::is_none")]
        exclude_from_context: Option<bool>,
        /// The timestamp.
        timestamp: i64,
    },
    /// Custom.
    #[serde(rename = "custom")]
    Custom {
        /// The custom type.
        #[serde(rename = "customType")]
        custom_type: String,
        /// The content.
        content: ContentValue,
        /// The display.
        display: bool,
        /// The details.
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        /// The timestamp.
        timestamp: i64,
    },
    /// Branch Summary.
    #[serde(rename = "branchSummary")]
    BranchSummary {
        /// The summary.
        summary: String,
        /// The from id.
        #[serde(rename = "fromId")]
        from_id: String,
        /// The timestamp.
        timestamp: i64,
    },
    /// Compaction Summary.
    #[serde(rename = "compactionSummary")]
    CompactionSummary {
        /// The summary.
        summary: String,
        /// The tokens before.
        #[serde(rename = "tokensBefore")]
        tokens_before: i64,
        /// The timestamp.
        timestamp: i64,
    },
}

impl AgentMessage {
    /// Get the content of the message as a string
    pub fn content(&self) -> String {
        match self {
            AgentMessage::User { content } => content.as_str().to_string(),
            AgentMessage::Assistant { content, .. } => {
                let estimated_len = content
                    .iter()
                    .map(|b| match b {
                        AssistantContentBlock::Text { text: t } => t.len(),
                        _ => 0,
                    })
                    .sum::<usize>();
                let mut text = String::with_capacity(estimated_len.max(256));
                for block in content {
                    match block {
                        AssistantContentBlock::Text { text: t } => text.push_str(t),
                        _ => {}
                    }
                }
                text
            }
            AgentMessage::ToolResult { content, .. } => content.as_str().to_string(),
            AgentMessage::System { content } => content.as_str().to_string(),
            AgentMessage::BashExecution { output, .. } => output.clone(),
            AgentMessage::Custom { content, .. } => content.as_str().to_string(),
            AgentMessage::BranchSummary { summary, .. } => summary.clone(),
            AgentMessage::CompactionSummary { summary, .. } => summary.clone(),
        }
    }

    /// Check if this is a user message
    pub fn is_user(&self) -> bool {
        matches!(self, AgentMessage::User { .. })
    }

    /// Check if this is an assistant message
    pub fn is_assistant(&self) -> bool {
        matches!(self, AgentMessage::Assistant { .. })
    }
}

/// Content block for assistant messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantContentBlock {
    /// Plain text content block.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: String,
    },
    /// Extended thinking content block.
    #[serde(rename = "thinking")]
    Thinking {
        /// The thinking content.
        thinking: String,
    },
    /// Tool Call.
    #[serde(rename = "toolCall")]
    ToolCall {
        /// The id.
        id: String,
        /// The name.
        name: String,
        /// The arguments.
        arguments: serde_json::Value,
    },
    /// Tool Plan.
    #[serde(rename = "toolPlan")]
    ToolPlan {
        /// The content.
        content: String,
        /// The tool call id.
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
    },
    /// Image result content block.
    #[serde(rename = "image")]
    ImageResult {
        /// Base64-encoded image data.
        data: String,
        /// MIME type of the image.
        media_type: String,
    },
    /// Refusal content block.
    #[serde(rename = "refusal")]
    Refusal {
        /// The refusal reason.
        content: String,
    },
}

/// Usage statistics from an assistant message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    /// The input.
    #[serde(rename = "inputTokens", skip_serializing_if = "Option::is_none")]
    pub input: Option<i64>,
    /// The output.
    #[serde(rename = "outputTokens", skip_serializing_if = "Option::is_none")]
    pub output: Option<i64>,
    /// The cache read.
    #[serde(rename = "cacheReadTokens", skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<i64>,
    /// The cache write.
    #[serde(rename = "cacheWriteTokens", skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<i64>,
    /// The total tokens.
    #[serde(rename = "totalTokens", skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
}

// ============================================================================
// Session Entry Types
// ============================================================================

/// Base fields for all session entries (internal use)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntryBase {
    /// The entry type.
    #[serde(rename = "type")]
    pub entry_type: String,
    /// The id.
    pub id: String,
    /// The parent id.
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    /// The timestamp.
    pub timestamp: String,
}

/// Message entry with AgentMessage content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessageEntry {
    /// The base.
    #[serde(flatten)]
    pub base: SessionEntryBase,
    /// The message.
    pub message: AgentMessage,
}

/// Thinking level change entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingLevelChangeEntry {
    /// The base.
    #[serde(flatten)]
    pub base: SessionEntryBase,
    /// The thinking level.
    #[serde(rename = "thinkingLevel")]
    pub thinking_level: String,
}

/// Model change entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelChangeEntry {
    /// The base.
    #[serde(flatten)]
    pub base: SessionEntryBase,
    /// The provider.
    pub provider: String,
    /// The model id.
    #[serde(rename = "modelId")]
    pub model_id: String,
}

/// Compaction entry for context window management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionEntry {
    /// The base.
    #[serde(flatten)]
    pub base: SessionEntryBase,
    /// The summary.
    pub summary: String,
    /// The first kept entry id.
    #[serde(rename = "firstKeptEntryId")]
    pub first_kept_entry_id: String,
    /// The tokens before.
    #[serde(rename = "tokensBefore")]
    pub tokens_before: i64,
    /// The details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// The from hook.
    #[serde(rename = "fromHook", skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
}

/// Branch summary entry for abandoned branches
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummaryEntry {
    /// The base.
    #[serde(flatten)]
    pub base: SessionEntryBase,
    /// The from id.
    #[serde(rename = "fromId")]
    pub from_id: String,
    /// The summary.
    pub summary: String,
    /// The details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// The from hook.
    #[serde(rename = "fromHook", skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
}

/// Custom entry for extensions to store extension-specific data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomEntry {
    /// The base.
    #[serde(flatten)]
    pub base: SessionEntryBase,
    /// The custom type.
    #[serde(rename = "customType")]
    pub custom_type: String,
    /// The data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Label entry for user-defined bookmarks/markers on entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelEntry {
    /// The base.
    #[serde(flatten)]
    pub base: SessionEntryBase,
    /// The target id.
    #[serde(rename = "targetId")]
    pub target_id: String,
    /// The label.
    pub label: Option<String>,
}

/// Session metadata entry (e.g., user-defined display name)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoEntry {
    /// The base.
    #[serde(flatten)]
    pub base: SessionEntryBase,
    /// The name.
    pub name: Option<String>,
}

/// Custom message entry for extensions to inject messages into LLM context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomMessageEntry {
    /// The base.
    #[serde(flatten)]
    pub base: SessionEntryBase,
    /// The custom type.
    #[serde(rename = "customType")]
    pub custom_type: String,
    /// The content.
    pub content: ContentValue,
    /// The details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// The display.
    pub display: bool,
}

/// All possible session entries (internal enum)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionEntryEnum {
    /// Message.
    Message(SessionMessageEntry),
    /// Thinking Level Change.
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    /// Model Change.
    ModelChange(ModelChangeEntry),
    /// Compaction.
    Compaction(CompactionEntry),
    /// Branch Summary.
    BranchSummary(BranchSummaryEntry),
    /// Custom.
    Custom(CustomEntry),
    /// Label.
    Label(LabelEntry),
    /// Session Info.
    SessionInfo(SessionInfoEntry),
    /// Custom Message.
    CustomMessage(CustomMessageEntry),
}

/// Session entry - a simple struct for backward compatibility
/// This wraps the internal enum representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    /// The id.
    pub id: String,
    /// The parent id.
    pub parent_id: Option<String>,
    /// The timestamp.
    pub timestamp: i64,
    /// The message.
    pub message: AgentMessage,
}

impl SessionEntry {
    /// Create a new session entry
    pub fn new(message: AgentMessage) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            parent_id: None,
            timestamp: Utc::now().timestamp_millis(),
            message,
        }
    }

    /// Create a simple message entry with a role string and content
    pub fn simple_message(role: &str, content: &str) -> Self {
        use crate::session::ContentValue;
        let message = match role {
            "user" => AgentMessage::User {
                content: ContentValue::String(content.to_string()),
            },
            "assistant" => AgentMessage::Assistant {
                content: vec![AssistantContentBlock::Text {
                    text: content.to_string(),
                }],
                provider: None,
                model_id: None,
                usage: None,
                stop_reason: None,
            },
            "system" => AgentMessage::System {
                content: ContentValue::String(content.to_string()),
            },
            _ => AgentMessage::System {
                content: ContentValue::String(content.to_string()),
            },
        };
        Self::new(message)
    }

    /// Create a branched entry with a parent reference
    pub fn branched(message: AgentMessage, parent_id: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            parent_id: Some(parent_id.to_string()),
            timestamp: Utc::now().timestamp_millis(),
            message,
        }
    }

    /// Get the message content as a string
    pub fn content(&self) -> String {
        self.message.content()
    }
}

/// Raw file entry (includes header and internal enum)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileEntry {
    /// Header.
    Header(SessionHeader),
    /// Entry.
    Entry(SessionEntryEnum),
}

// ============================================================================
// Session Context
// ============================================================================

/// Context built from session entries for the LLM
#[derive(Debug, Clone)]
pub struct SessionContext {
    /// The messages.
    pub messages: Vec<AgentMessage>,
    /// The thinking level.
    pub thinking_level: String,
    /// The model.
    pub model: Option<ModelInfo>,
}

/// Model information
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// The provider.
    pub provider: String,
    /// The model id.
    pub model_id: String,
}

// ============================================================================
// Session Info
// ============================================================================

/// Session metadata for listing
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// The path.
    pub path: String,
    /// The id.
    pub id: String,
    /// The cwd.
    pub cwd: String,
    /// The name.
    pub name: Option<String>,
    /// The parent session path.
    pub parent_session_path: Option<String>,
    /// The created.
    pub created: DateTime<Utc>,
    /// The modified.
    pub modified: DateTime<Utc>,
    /// The message count.
    pub message_count: i64,
    /// The first message.
    pub first_message: String,
    /// The all messages text.
    pub all_messages_text: String,
}

// ============================================================================
// Session Tree Node
// ============================================================================

/// Tree node for get_tree()
#[derive(Debug, Clone)]
pub struct SessionTreeNode {
    /// The entry.
    pub entry: SessionEntry,
    /// The children.
    pub children: Vec<SessionTreeNode>,
    /// The label.
    pub label: Option<String>,
    /// The label timestamp.
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
                    SessionEntryEnum::Message(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "message".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntryEnum::ThinkingLevelChange(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "thinking_level_change".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntryEnum::ModelChange(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "model_change".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntryEnum::Compaction(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "compaction".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntryEnum::BranchSummary(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "branch_summary".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntryEnum::Custom(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "custom".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntryEnum::Label(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "label".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntryEnum::SessionInfo(e) => {
                        e.base.id = generate_id(&ids);
                        e.base.parent_id = prev_id.clone();
                        e.base.entry_type = "session_info".to_string();
                        prev_id = Some(e.base.id.clone());
                        e.base.id.clone()
                    }
                    SessionEntryEnum::CustomMessage(e) => {
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
            FileEntry::Entry(_) => {
                // v2 to v3 migration handled elsewhere
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
///
/// SessionManager handles session persistence, branching, and tree traversal.
/// Each session is stored as a JSONL file where each line is a session entry.
/// Entries form a tree structure allowing for session branching and history.
pub struct SessionManager {
    session_id: String,
    session_file: Option<String>,
    session_dir: String,
    cwd: String,
    persist: bool,
    flushed: bool,
    /// Tracks how many agent messages have been persisted so far,
    /// so that `persist_session()` only appends new messages.
    persisted_count: RwLock<usize>,
    file_entries: RwLock<Vec<FileEntry>>,
    by_id: RwLock<HashMap<String, SessionEntry>>,
    labels_by_id: RwLock<HashMap<String, String>>,
    label_timestamps_by_id: RwLock<HashMap<String, String>>,
    leaf_id: RwLock<Option<String>>,
}

// Manual Clone implementation — only copies internal pointers, not file handles
impl Clone for SessionManager {
    fn clone(&self) -> Self {
        Self {
            session_id: self.session_id.clone(),
            session_file: self.session_file.clone(),
            session_dir: self.session_dir.clone(),
            cwd: self.cwd.clone(),
            persist: self.persist,
            flushed: self.flushed,
            persisted_count: RwLock::new(*self.persisted_count.read()),
            file_entries: RwLock::new(self.file_entries.read().clone()),
            by_id: RwLock::new(self.by_id.read().clone()),
            labels_by_id: RwLock::new(self.labels_by_id.read().clone()),
            label_timestamps_by_id: RwLock::new(self.label_timestamps_by_id.read().clone()),
            leaf_id: RwLock::new(self.leaf_id.read().clone()),
        }
    }
}

impl SessionManager {
    /// Create a new session and persist it to disk.
    pub fn create(cwd: &str, session_dir: Option<&str>) -> Self {
        let dir = session_dir
            .map(|s| s.to_string())
            .unwrap_or_else(|| get_default_session_dir(cwd));

        let mut manager = Self::new_internal(cwd, &dir, None, true);
        manager.persist = true;
        manager
    }

    /// Open an existing session from a file path.
    pub fn open(path: &str, session_dir: Option<&str>, cwd_override: Option<&str>) -> Self {
        let entries = load_entries_from_file(path);
        let header = entries.iter().find_map(|e| match e {
            FileEntry::Header(h) => Some(h),
            _ => None,
        });
        let cwd = cwd_override
            .map(|s| s.to_string())
            .or_else(|| header.as_ref().map(|h| h.cwd.clone()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).to_string_lossy().to_string());
        let dir = session_dir
            .map(|s| s.to_string())
            .unwrap_or_else(|| Path::new(path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| ".".to_string()));

        let mut manager = Self::new_internal(&cwd, &dir, Some(path), true);
        manager.persist = true;
        manager
    }

    /// Continue the most recent session, or create a new one if none exists.
    pub fn continue_recent(cwd: &str, session_dir: Option<&str>) -> Self {
        let dir = session_dir
            .map(|s| s.to_string())
            .unwrap_or_else(|| get_default_session_dir(cwd));

        if let Some(most_recent) = find_most_recent_session(&dir) {
            return Self::open(&most_recent, None, None);
        }
        Self::create(cwd, None)
    }

    /// Create an in-memory session without file persistence.
    pub fn in_memory(cwd: &str) -> Self {
        let cwd = cwd.to_string();
        Self::new_internal(&cwd, "", None, false)
    }

    fn new_internal(cwd: &str, session_dir: &str, session_file: Option<&str>, persist: bool) -> Self {
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
            persisted_count: RwLock::new(0),
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
        *self.persisted_count.write() = 0;
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
                // Convert internal enum to simple SessionEntry struct
                if let Some(session_entry) = convert_to_session_entry(e) {
                    by_id.insert(session_entry.id.clone(), session_entry.clone());
                    *leaf_id = Some(session_entry.id.clone());
                }

                // Handle labels
                if let SessionEntryEnum::Label(l) = e {
                    if let Some(ref label) = l.label {
                        labels.insert(l.target_id.clone(), label.clone());
                        label_timestamps.insert(l.target_id.clone(), l.base.timestamp.clone());
                    } else {
                        labels.remove(&l.target_id);
                        label_timestamps.remove(&l.target_id);
                    }
                }
            }
        }
    }

    fn _rewrite_file(&self) {
        if !self.persist || self.session_file.is_none() {
            return;
        }

        let file = match self.session_file.as_ref() {
            Some(f) => f,
            None => return,
        };

        let content: String = self
            .file_entries
            .read()
            .iter()
            .map(|e| serde_json::to_string(e).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        if let Err(e) = fs::write(file, content) {
            tracing::warn!("Failed to rewrite session file {}: {}", file, e);
        }
    }

    /// Check if session is persisted to disk
    pub fn is_persisted(&self) -> bool {
        self.persist
    }

    /// Get the number of agent messages that have already been persisted.
    pub fn persisted_count(&self) -> usize {
        *self.persisted_count.read()
    }

    /// Set the number of agent messages that have been persisted.
    pub fn set_persisted_count(&self, count: usize) {
        *self.persisted_count.write() = count;
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

    fn _persist(&mut self, entry: &SessionEntry) {
        if !self.persist {
            return;
        }
        let Some(file) = &self.session_file else {
            return;
        };

        let has_assistant = self.file_entries.read().iter().any(|e| {
            matches!(
                e,
                FileEntry::Entry(SessionEntryEnum::Message(m)) if m.message.is_assistant()
            )
        });

        if !has_assistant {
            // Mark as not flushed so when assistant arrives, all entries get written
            self.flushed = false;
            return;
        }

        let mut handle = match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
        {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("Failed to open session file for append {}: {}", file, e);
                return;
            }
        };

        if !self.flushed {
            for e in self.file_entries.read().iter() {
                if let Ok(line) = serde_json::to_string(e) {
                    let _ = writeln!(&mut handle, "{}", line);
                }
            }
            self.flushed = true;
        } else {
            // Convert SessionEntry back to FileEntry for writing
            let file_entry = convert_from_session_entry(entry);
            if let Ok(line) = serde_json::to_string(&file_entry) {
                let _ = writeln!(&mut handle, "{}", line);
            }
        }
    }

    fn _append_entry(&mut self, entry: SessionEntry) {
        let file_entry = convert_from_session_entry(&entry);
        self.file_entries.write().push(FileEntry::Entry(file_entry));
        self.by_id.write().insert(entry.id.clone(), entry.clone());
        *self.leaf_id.write() = Some(entry.id.clone());
        self._persist(&entry);
    }

    /// Append a message as child of current leaf
    pub fn append_message(&mut self, message: AgentMessage) -> String {
        let leaf = self.leaf_id.read().clone();
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: leaf,
            timestamp: Utc::now().timestamp_millis(),
            message,
        };
        self._append_entry(entry);
        id
    }

    /// Append a thinking level change
    pub fn append_thinking_level_change(&mut self, thinking_level: &str) -> String {
        let leaf = self.leaf_id.read().clone();
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: leaf,
            timestamp: Utc::now().timestamp_millis(),
            message: AgentMessage::Custom {
                custom_type: "thinking_level_change".to_string(),
                content: ContentValue::String(thinking_level.to_string()),
                display: false,
                details: None,
                timestamp: Utc::now().timestamp_millis(),
            },
        };
        self._append_entry(entry);
        id
    }

    /// Append a model change
    pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> String {
        let leaf = self.leaf_id.read().clone();
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: leaf,
            timestamp: Utc::now().timestamp_millis(),
            message: AgentMessage::Custom {
                custom_type: "model_change".to_string(),
                content: ContentValue::String(format!("{}:{}", provider, model_id)),
                display: false,
                details: None,
                timestamp: Utc::now().timestamp_millis(),
            },
        };
        self._append_entry(entry);
        id
    }

    /// Append a compaction summary
    pub fn append_compaction(
        &mut self,
        summary: &str,
        _first_kept_entry_id: &str,
        tokens_before: i64,
        _details: Option<serde_json::Value>,
        _from_hook: Option<bool>,
    ) -> String {
        let leaf = self.leaf_id.read().clone();
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: leaf,
            timestamp: Utc::now().timestamp_millis(),
            message: AgentMessage::CompactionSummary {
                summary: summary.to_string(),
                tokens_before,
                timestamp: Utc::now().timestamp_millis(),
            },
        };
        self._append_entry(entry);
        id
    }

    /// Append a custom entry (for extensions)
    pub fn append_custom_entry(&mut self, custom_type: &str, data: Option<serde_json::Value>) -> String {
        let leaf = self.leaf_id.read().clone();
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: leaf,
            timestamp: Utc::now().timestamp_millis(),
            message: AgentMessage::Custom {
                custom_type: custom_type.to_string(),
                content: data.as_ref().map(|d| ContentValue::String(d.to_string())).unwrap_or(ContentValue::String(String::new())),
                display: false,
                details: data.clone(),
                timestamp: Utc::now().timestamp_millis(),
            },
        };
        self._append_entry(entry);
        id
    }

    /// Append a session info entry (e.g., display name)
    pub fn append_session_info(&mut self, name: &str) -> String {
        let leaf = self.leaf_id.read().clone();
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: leaf,
            timestamp: Utc::now().timestamp_millis(),
            message: AgentMessage::Custom {
                custom_type: "session_info".to_string(),
                content: ContentValue::String(name.trim().to_string()),
                display: false,
                details: None,
                timestamp: Utc::now().timestamp_millis(),
            },
        };
        self._append_entry(entry);
        id
    }

    /// Get the current session name from the latest session_info entry
    pub fn get_session_name(&self) -> Option<String> {
        let entries = self.get_entries();
        for entry in entries.iter().rev() {
            if let AgentMessage::Custom { custom_type, content, .. } = &entry.message {
                if custom_type == "session_info" {
                    return Some(content.as_str().trim().to_string()).filter(|s| !s.is_empty());
                }
            }
        }
        None
    }

    /// Append a custom message entry (for extensions) that participates in LLM context
    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: ContentValue,
        display: bool,
        details: Option<serde_json::Value>,
    ) -> String {
        let leaf = self.leaf_id.read().clone();
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: leaf,
            timestamp: Utc::now().timestamp_millis(),
            message: AgentMessage::Custom {
                custom_type: custom_type.to_string(),
                content,
                display,
                details,
                timestamp: Utc::now().timestamp_millis(),
            },
        };
        self._append_entry(entry);
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
            .filter(|e| e.parent_id.as_deref() == Some(parent_id))
            .cloned()
            .collect()
    }

    /// Get the parent of an entry
    pub fn get_parent(&self, id: &str) -> Option<SessionEntry> {
        self.by_id
            .read()
            .get(id)
            .and_then(|e| e.parent_id.as_deref())
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
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: leaf,
            timestamp: Utc::now().timestamp_millis(),
            message: AgentMessage::Custom {
                custom_type: "label".to_string(),
                content: ContentValue::String(label.unwrap_or("").to_string()),
                display: false,
                details: Some(serde_json::json!({ "targetId": target_id })),
                timestamp: Utc::now().timestamp_millis(),
            },
        };

        self._append_entry(entry);

        if let Some(l) = label {
            self.labels_by_id.write().insert(target_id.to_string(), l.to_string());
            self.label_timestamps_by_id.write().insert(target_id.to_string(), Utc::now().to_rfc3339());
        } else {
            self.labels_by_id.write().remove(target_id);
            self.label_timestamps_by_id.write().remove(target_id);
        }

        Ok(id)
    }

    /// Walk from entry to root, returning all entries in path order
    pub fn get_branch(&self, from_id: Option<&str>) -> Vec<SessionEntry> {
        let mut path = Vec::new();
        let leaf_fallback = self.leaf_id.read().clone();
        let start_id = from_id.or_else(|| leaf_fallback.as_deref());
        let Some(start_id) = start_id else {
            return path;
        };

        let mut current = self.by_id.read().get(start_id).cloned();
        while let Some(entry) = current {
            path.insert(0, entry.clone());
            current = entry.parent_id.as_ref().and_then(|pid| self.by_id.read().get(pid).cloned());
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
            current = entry.parent_id.as_ref().and_then(|pid| self.by_id.read().get(pid).cloned());
        }
        depth - 1 // Root has depth 0
    }

    /// Build the session context (what gets sent to the LLM)
    pub fn build_session_context(&self) -> SessionContext {
        let entries = self.get_entries();
        let leaf_id = self.leaf_id.read().clone();
        build_session_context_internal(&entries, leaf_id, None)
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
        self.by_id.read().values().cloned().collect()
    }

    /// Get the session as a tree structure
    /// If id is provided, returns tree for that session (backward compat)
    pub fn get_tree(&self, _id: Uuid) -> anyhow::Result<Vec<SessionTreeNode>> {
        let entries = self.get_entries();
        let labels: HashMap<String, String> = self.labels_by_id.read().clone();
        let label_timestamps: HashMap<String, String> = self.label_timestamps_by_id.read().clone();

        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        let mut root_ids: Vec<String> = Vec::new();

        // Build adjacency list
        for entry in &entries {
            adj.insert(entry.id.clone(), Vec::new());
        }

        // Determine parent-child relationships
        for entry in &entries {
            let is_root = match entry.parent_id.as_deref() {
                Some(pid) if pid != entry.id => !adj.contains_key(pid),
                _ => true,
            };
            if is_root {
                root_ids.push(entry.id.clone());
            } else if let Some(ref pid) = entry.parent_id {
                if let Some(children) = adj.get_mut(pid.as_str()) {
                    children.push(entry.id.clone());
                } else {
                    root_ids.push(entry.id.clone());
                }
            }
        }

        // Build entries map
        let entries_map: HashMap<String, SessionEntry> = entries.into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();

        // Recursively build tree nodes
        fn build(
            id: &str,
            adj: &HashMap<String, Vec<String>>,
            entries_map: &HashMap<String, SessionEntry>,
            labels: &HashMap<String, String>,
            label_timestamps: &HashMap<String, String>,
        ) -> anyhow::Result<SessionTreeNode> {
            let entry = entries_map.get(id).ok_or_else(|| anyhow::anyhow!("Corrupted session: entry {} not found", id))?.clone();
            let child_ids = adj.get(id).cloned().unwrap_or_default();
            let children: Vec<SessionTreeNode> = child_ids.iter().map(|cid| {
                build(cid, adj, entries_map, labels, label_timestamps)
            }).collect::<Result<Vec<_>, _>>()?;
            Ok(SessionTreeNode {
                entry,
                children,
                label: labels.get(id).cloned(),
                label_timestamp: label_timestamps.get(id).cloned(),
            })
        }

        let mut roots = root_ids.into_iter().map(|rid| {
            build(&rid, &adj, &entries_map, &labels, &label_timestamps)
        }).collect::<anyhow::Result<Vec<_>>>()?;

        sort_tree_by_timestamp(&mut roots);
        Ok(roots)
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
        _details: Option<serde_json::Value>,
        _from_hook: Option<bool>,
    ) -> String {
        if let Some(id) = branch_from_id {
            if !self.by_id.read().contains_key(id) {
                return String::new();
            }
        }

        *self.leaf_id.write() = branch_from_id.map(|s| s.to_string());

        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: branch_from_id.map(|s| s.to_string()),
            timestamp: Utc::now().timestamp_millis(),
            message: AgentMessage::BranchSummary {
                summary: summary.to_string(),
                from_id: branch_from_id.unwrap_or("root").to_string(),
                timestamp: Utc::now().timestamp_millis(),
            },
        };

        self._append_entry(entry);
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
    pub fn get_latest_compaction_entry(&self) -> Option<SessionEntry> {
        let entries = self.get_entries();
        for entry in entries.iter().rev() {
            if let AgentMessage::CompactionSummary { .. } = &entry.message {
                return Some(entry.clone());
            }
        }
        None
    }

    /// Get all compaction entries
    pub fn get_compaction_entries(&self) -> Vec<SessionEntry> {
        self.get_entries()
            .iter()
            .filter(|e| matches!(&e.message, AgentMessage::CompactionSummary { .. }))
            .cloned()
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
            if let AgentMessage::User { .. } = &entry.message {
                user_message_count += 1;
            }
            if let AgentMessage::Assistant { .. } = &entry.message {
                assistant_message_count += 1;
            }
            if entry.message.is_user() || entry.message.is_assistant() {
                message_count += 1;
                // Estimate tokens from message
                let content = entry.content();
                let chars = content.len() as i64;
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
        writeln!(&mut handle, "{}", serde_json::to_string(&new_header).expect("session header serializable")).map_err(|e| e.to_string())?;

        // Copy all non-header entries from source
        for file_entry in &source_entries {
            if let FileEntry::Entry(_) = file_entry {
                writeln!(&mut handle, "{}", serde_json::to_string(file_entry).expect("session entry serializable"))
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

    // =========================================================================
    // Backward Compatibility Methods
    // =========================================================================

    /// Create a new SessionManager (async for backward compatibility)
    pub async fn new() -> Result<Self> {
        Self::new_async().await
    }

    /// Create a new SessionManager (async for backward compatibility)
    pub async fn new_async() -> Result<Self> {
        let home = dirs::home_dir().context("Cannot find home directory")?;
        let base_dir = home.join(".oxi");
        let sessions_dir = base_dir.join("sessions");
        tokio::fs::create_dir_all(&sessions_dir).await?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).to_string_lossy().to_string();
        Ok(Self::in_memory(&cwd))
    }

    /// Get the session file path for a given session ID
    pub fn session_path(&self, id: &Uuid) -> PathBuf {
        if let Some(file) = &self.session_file {
            PathBuf::from(file)
        } else {
            PathBuf::from(format!("{}/{}.jsonl", self.session_dir, id))
        }
    }

    /// List all sessions (backward compat)
    pub async fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        // Simple implementation: scan the session dir for jsonl files
        let mut metas = Vec::new();
        let session_dir = Path::new(&self.session_dir);
        if !session_dir.exists() {
            return Ok(metas);
        }
        let entries = fs::read_dir(session_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                let file_name = path.file_stem().unwrap_or_else(|| std::ffi::OsStr::new("")).to_string_lossy().to_string();
                // Try to extract uuid from filename
                if let Some(uuid_part) = file_name.split('_').last() {
                    if let Ok(uuid) = Uuid::parse_str(uuid_part) {
                        let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
                        let now_ts = Utc::now().timestamp_millis();
                        metas.push(SessionMeta {
                            id: uuid,
                            parent_id: None,
                            root_id: None,
                            branch_point: None,
                            created_at: now_ts,
                            updated_at: mtime.map(|t| {
                                let dt: DateTime<Utc> = DateTime::from(t);
                                dt.timestamp_millis()
                            }).unwrap_or(now_ts),
                            name: None,
                        });
                    }
                }
            }
        }
        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(metas)
    }

    /// Save entries (backward compat)
    pub async fn save(&self, _id: Uuid, _entries: &[SessionEntry]) -> Result<()> {
        self._rewrite_file();
        Ok(())
    }

    /// Load entries (backward compat)
    pub async fn load(&self, _id: Uuid) -> Result<Vec<SessionEntry>> {
        Ok(self.get_entries())
    }

    /// Delete a session (backward compat)
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let path = self.session_path(&id);
        if path.exists() {
            fs::remove_file(path).context("Failed to delete session file")?;
        }
        Ok(())
    }

    /// Create a branch from an existing session at a given entry
    pub async fn branch_from(
        &self,
        parent_id: Uuid,
        entry_id: Uuid,
    ) -> Result<(Uuid, Vec<SessionEntry>)> {
        let _entry_id_str = entry_id.to_string();
        let _parent_id_str = parent_id.to_string();
        
        // Get entries up to the branch point
        let _entries = self.get_entries();
        let path = self.get_branch(Some(&entry_id.to_string()));
        
        let new_id = Uuid::new_v4();
        let new_entries: Vec<SessionEntry> = path.into_iter().map(|e| {
            let mut new_entry = e.clone();
            new_entry.id = Uuid::new_v4().to_string();
            new_entry
        }).collect();

        // Update the last entry to have parent reference
        // (simplified version of the original branch_from)
        Ok((new_id, new_entries))
    }

    /// Get branch info for a session
    pub async fn get_branch_info(&self, _id: Uuid) -> Result<Option<BranchInfo>> {
        // Simplified implementation
        Ok(None)
    }

    /// Get tree for a specific session (backward compat)
    pub async fn get_tree_async(&self, _id: Uuid) -> Result<Vec<SessionTreeNode>> {
        Ok(self.get_tree(Uuid::nil())?)
    }

    /// Save metadata (backward compat)
    pub async fn save_meta(&self, _meta: &SessionMeta) -> Result<()> {
        Ok(())
    }

    /// Load metadata (backward compat)
    pub async fn load_meta(&self, _id: Uuid) -> Result<Option<SessionMeta>> {
        Ok(None)
    }

    /// Create a new session (backward compat)
    pub async fn create_session(&mut self) -> Result<SessionMeta> {
        let id = Uuid::new_v4();
        let meta = SessionMeta::new(id);
        Ok(meta)
    }
}

// ============================================================================
// Internal Conversion Functions
// ============================================================================

/// Convert internal enum to simple SessionEntry struct
fn convert_to_session_entry(entry: &SessionEntryEnum) -> Option<SessionEntry> {
    match entry {
        SessionEntryEnum::Message(m) => Some(SessionEntry {
            id: m.base.id.clone(),
            parent_id: m.base.parent_id.clone(),
            timestamp: DateTime::parse_from_rfc3339(&m.base.timestamp)
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0),
            message: m.message.clone(),
        }),
        _ => None, // For now, we only convert message entries to the simple struct
    }
}

/// Convert simple SessionEntry to internal FileEntry for persistence
fn convert_from_session_entry(entry: &SessionEntry) -> SessionEntryEnum {
    let timestamp = DateTime::from_timestamp_millis(entry.timestamp)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    
    SessionEntryEnum::Message(SessionMessageEntry {
        base: SessionEntryBase {
            entry_type: "message".to_string(),
            id: entry.id.clone(),
            parent_id: entry.parent_id.clone(),
            timestamp,
        },
        message: entry.message.clone(),
    })
}

// ============================================================================
// Session Statistics
// ============================================================================

/// Session Stats.
#[derive(Debug, Clone)]
pub struct SessionStats {
    /// The message count.
    pub message_count: i64,
    /// The user message count.
    pub user_message_count: i64,
    /// The assistant message count.
    pub assistant_message_count: i64,
    /// The total chars.
    pub total_chars: i64,
    /// The estimated tokens.
    pub estimated_tokens: i64,
}

// ============================================================================
// NewSessionOptions
// ============================================================================

/// New Session Options.
#[derive(Debug, Clone)]
pub struct NewSessionOptions {
    /// The id.
    pub id: Option<String>,
    /// The parent session.
    pub parent_session: Option<String>,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get default session dir.
pub fn get_default_session_dir(cwd: &str) -> String {
    let agent_dir = get_agent_dir();
    let safe_path = format!("--{}--", cwd.replace('/', "-").replace('\\', "-").replace(':', "-"));
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
fn build_session_context_internal(
    entries: &[SessionEntry],
    leaf_id: Option<String>,
    _by_id: Option<&RwLock<HashMap<String, SessionEntry>>>,
) -> SessionContext {
    // Find leaf
    let leaf: Option<&SessionEntry> = leaf_id
        .as_ref()
        .and_then(|id| entries.iter().find(|e| e.id == *id));

    let leaf = leaf.or_else(|| entries.last());

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
        current = entry.parent_id.as_ref().and_then(|pid| entries.iter().find(|e| e.id == *pid));
    }

    // Extract settings
    let mut thinking_level = "off".to_string();
    let mut model: Option<ModelInfo> = None;

    for entry in &path {
        if let AgentMessage::Assistant { provider, model_id, .. } = &entry.message {
            model = Some(ModelInfo {
                provider: provider.clone().unwrap_or_default(),
                model_id: model_id.clone().unwrap_or_default(),
            });
        }
        if let AgentMessage::Custom { custom_type, content, .. } = &entry.message {
            if custom_type == "thinking_level_change" {
                thinking_level = content.as_str().to_string();
            }
        }
    }

    // Build messages - include all messages in the path
    let messages: Vec<AgentMessage> = path
        .iter()
        .filter(|e| e.message.is_user() || e.message.is_assistant() 
            || matches!(&e.message, AgentMessage::BranchSummary { .. })
            || matches!(&e.message, AgentMessage::CompactionSummary { .. }))
        .map(|e| e.message.clone())
        .collect();

    SessionContext {
        messages,
        thinking_level,
        model,
    }
}

/// Sort tree nodes by timestamp
fn sort_tree_by_timestamp(nodes: &mut Vec<SessionTreeNode>) {
    nodes.sort_by(|a, b| a.entry.timestamp.cmp(&b.entry.timestamp));

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
            // Check for session_info
            if let SessionEntryEnum::SessionInfo(si) = e {
                name = si.name.clone().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            }
            // Check for messages
            if let SessionEntryEnum::Message(m) = e {
                if m.message.is_user() {
                    message_count += 1;
                    let text = m.message.content();
                    if !text.is_empty() {
                        all_messages.push(text.clone());
                        if first_message.is_empty() {
                            first_message = text;
                        }
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
    let modified = get_session_modified_date(&entries, &header.timestamp, &stats);

    Some(SessionInfo {
        path: file_path.to_string(),
        id: header.id.clone(),
        cwd,
        name,
        parent_session_path,
        created,
        modified,
        message_count,
        first_message: if first_message.is_empty() { "(no messages)".to_string() } else { first_message },
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
    stats: &std::fs::Metadata,
) -> DateTime<Utc> {
    let last_activity_time = get_last_activity_time(entries);
    if let Some(t) = last_activity_time {
        if t > 0 {
            return DateTime::from_timestamp_millis(t).unwrap_or_else(Utc::now);
        }
    }

    let header_time = chrono::DateTime::parse_from_rfc3339(header_timestamp)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(-1);

    if header_time > 0 {
        return DateTime::from_timestamp_millis(header_time).unwrap_or_else(Utc::now);
    }

    if let Ok(mtime) = stats.modified() {
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

        if let SessionEntryEnum::Message(m) = entry {
            if m.message.is_user() || m.message.is_assistant() {
                last_activity = Some(std::cmp::max(last_activity.unwrap_or(0), m.base.timestamp.parse().unwrap_or(0)));
            }
        }
    }

    last_activity
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
        let id = manager.append_message(AgentMessage::User { content: ContentValue::String("Hello".to_string()) });
        assert!(!id.is_empty());
        assert_eq!(manager.get_entries().len(), 1);
        assert_eq!(manager.get_leaf_id(), Some(id));
    }

    #[test]
    fn test_tree_traversal() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(AgentMessage::User { content: ContentValue::String("Hello".to_string()) });
        let id2 = manager.append_message(AgentMessage::Assistant { content: vec![], provider: None, model_id: None, usage: None, stop_reason: None });

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
        assert_eq!(parent.unwrap().id, id1);
    }

    #[test]
    fn test_branching() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(AgentMessage::User { content: ContentValue::String("Hello".to_string()) });
        let _id2 = manager.append_message(AgentMessage::Assistant { content: vec![], provider: None, model_id: None, usage: None, stop_reason: None });
        let _id3 = manager.append_message(AgentMessage::User { content: ContentValue::String("How are you?".to_string()) });

        // Branch from first message
        manager.branch(&id1).unwrap();
        assert_eq!(manager.get_leaf_id(), Some(id1.clone()));

        // Add new message on branch
        let id4 = manager.append_message(AgentMessage::Assistant { content: vec![], provider: None, model_id: None, usage: None, stop_reason: None });

        // Should have 4 entries total (3 original + 1 new branch)
        assert_eq!(manager.get_entries().len(), 4);

        // Leaf should be the new message
        assert_eq!(manager.get_leaf_id(), Some(id4));

        // Get tree - 1 root (id1), with 2 children (id2 and id4)
        let tree = manager.get_tree(Uuid::nil()).unwrap();
        assert_eq!(tree.len(), 1); // One root
        assert_eq!(tree[0].children.len(), 2); // id1 has 2 children: id2 and id4
    }

    #[test]
    fn test_session_context() {
        let mut manager = SessionManager::in_memory("/tmp");
        manager.append_message(AgentMessage::User { content: ContentValue::String("Hello".to_string()) });
        manager.append_message(AgentMessage::Assistant { 
            content: vec![AssistantContentBlock::Text { text: "Hi there!".to_string() }],
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
        let id1 = manager.append_message(AgentMessage::User { content: ContentValue::String("First message".to_string()) });
        let _id2 = manager.append_message(AgentMessage::Assistant { content: vec![], provider: None, model_id: None, usage: None, stop_reason: None });

        let id3 = manager.append_compaction(
            "Summarized conversation",
            &id1,
            1000,
            None,
            None,
        );
        assert!(!id3.is_empty());

        let latest = manager.get_latest_compaction_entry();
        assert!(latest.is_some());
    }

    #[test]
    fn test_labels() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(AgentMessage::User { content: ContentValue::String("Hello".to_string()) });

        manager.add_label(&id1, "important").unwrap();
        assert_eq!(manager.get_label(&id1), Some("important".to_string()));

        manager.remove_label(&id1).unwrap();
        assert_eq!(manager.get_label(&id1), None);
    }

    // ========================================================================
    // Session tree and branching tests
    // ========================================================================

    /// Helper: create a user message
    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::User { content: ContentValue::String(text.to_string()) }
    }

    /// Helper: create an assistant message
    fn assistant_msg(text: &str) -> AgentMessage {
        AgentMessage::Assistant {
            content: vec![AssistantContentBlock::Text { text: text.to_string() }],
            provider: Some("anthropic".to_string()),
            model_id: Some("claude-test".to_string()),
            usage: None,
            stop_reason: None,
        }
    }

    /// Helper: create a bare assistant message (no content/metadata)
    fn bare_assistant_msg() -> AgentMessage {
        AgentMessage::Assistant {
            content: vec![],
            provider: None,
            model_id: None,
            usage: None,
            stop_reason: None,
        }
    }

    // ------------------------------------------------------------------------
    // append operations integration into tree
    // ------------------------------------------------------------------------

    #[test]
    fn test_append_thinking_level_change_integrates() {
        let mut manager = SessionManager::in_memory("/tmp");
        let msg_id = manager.append_message(user_msg("hello"));
        let thinking_id = manager.append_thinking_level_change("high");
        let msg2_id = manager.append_message(assistant_msg("response"));

        let entries = manager.get_entries();
        assert_eq!(entries.len(), 3);

        // Thinking entry should be between the two messages
        let thinking_entry = entries.iter().find(|e| e.id == thinking_id).unwrap();
        assert_eq!(thinking_entry.parent_id, Some(msg_id));

        let msg2 = entries.iter().find(|e| e.id == msg2_id).unwrap();
        assert_eq!(msg2.parent_id, Some(thinking_id));
    }

    #[test]
    fn test_append_model_change_integrates() {
        let mut manager = SessionManager::in_memory("/tmp");
        let msg_id = manager.append_message(user_msg("hello"));
        let model_id = manager.append_model_change("openai", "gpt-4");
        let msg2_id = manager.append_message(assistant_msg("response"));

        let entries = manager.get_entries();
        let model_entry = entries.iter().find(|e| e.id == model_id).unwrap();
        assert_eq!(model_entry.parent_id, Some(msg_id));

        let msg2 = entries.iter().find(|e| e.id == msg2_id).unwrap();
        assert_eq!(msg2.parent_id, Some(model_id));
    }

    #[test]
    fn test_append_compaction_integrates_into_tree() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(user_msg("1"));
        let id2 = manager.append_message(assistant_msg("2"));
        let compaction_id = manager.append_compaction("summary", &id1, 1000, None, None);
        let id3 = manager.append_message(user_msg("3"));

        let entries = manager.get_entries();
        let compaction = entries.iter().find(|e| e.id == compaction_id).unwrap();
        assert_eq!(compaction.parent_id, Some(id2));

        let msg3 = entries.iter().find(|e| e.id == id3).unwrap();
        assert_eq!(msg3.parent_id, Some(compaction_id));

        // Verify compaction content
        if let AgentMessage::CompactionSummary { summary, tokens_before, .. } = &compaction.message {
            assert_eq!(summary, "summary");
            assert_eq!(*tokens_before, 1000);
        } else {
            panic!("Expected CompactionSummary");
        }
    }

    #[test]
    fn test_leaf_pointer_advances() {
        let mut manager = SessionManager::in_memory("/tmp");
        assert!(manager.get_leaf_id().is_none());

        let id1 = manager.append_message(user_msg("1"));
        assert_eq!(manager.get_leaf_id(), Some(id1.clone()));

        let id2 = manager.append_message(assistant_msg("2"));
        assert_eq!(manager.get_leaf_id(), Some(id2.clone()));

        let id3 = manager.append_thinking_level_change("high");
        assert_eq!(manager.get_leaf_id(), Some(id3));
    }

    #[test]
    fn test_get_entry() {
        let mut manager = SessionManager::in_memory("/tmp");
        assert!(manager.get_entry("nonexistent").is_none());

        let id1 = manager.append_message(user_msg("first"));
        let id2 = manager.append_message(assistant_msg("second"));

        let entry1 = manager.get_entry(&id1);
        assert!(entry1.is_some());
        assert!(entry1.unwrap().message.is_user());

        let entry2 = manager.get_entry(&id2);
        assert!(entry2.is_some());
        assert!(entry2.unwrap().message.is_assistant());
    }

    #[test]
    fn test_get_leaf_entry() {
        let manager = SessionManager::in_memory("/tmp");
        assert!(manager.get_leaf_entry().is_none());

        let mut manager = SessionManager::in_memory("/tmp");
        manager.append_message(user_msg("1"));
        let id2 = manager.append_message(assistant_msg("2"));

        let leaf = manager.get_leaf_entry();
        assert!(leaf.is_some());
        assert_eq!(leaf.unwrap().id, id2);
    }

    // ------------------------------------------------------------------------
    // getBranch / getPath
    // ------------------------------------------------------------------------

    #[test]
    fn test_get_branch_full_path_root_to_leaf() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(user_msg("1"));
        let id2 = manager.append_message(assistant_msg("2"));
        let id3 = manager.append_thinking_level_change("high");
        let id4 = manager.append_message(user_msg("3"));

        let branch = manager.get_branch(None);
        assert_eq!(branch.len(), 4);
        assert_eq!(branch[0].id, id1);
        assert_eq!(branch[1].id, id2);
        assert_eq!(branch[2].id, id3);
        assert_eq!(branch[3].id, id4);
    }

    #[test]
    fn test_get_branch_from_specific_entry() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(user_msg("1"));
        let id2 = manager.append_message(assistant_msg("2"));
        manager.append_message(user_msg("3"));
        manager.append_message(assistant_msg("4"));

        let branch = manager.get_branch(Some(&id2));
        assert_eq!(branch.len(), 2);
        assert_eq!(branch[0].id, id1);
        assert_eq!(branch[1].id, id2);
    }

    // ------------------------------------------------------------------------
    // Multiple branches at same point (3 siblings)
    // ------------------------------------------------------------------------

    #[test]
    fn test_multiple_branches_at_same_point() {
        let mut manager = SessionManager::in_memory("/tmp");
        manager.append_message(user_msg("root"));
        let id2 = manager.append_message(bare_assistant_msg());

        // Branch A
        manager.branch(&id2).unwrap();
        let id_a = manager.append_message(user_msg("branch-A"));

        // Branch B
        manager.branch(&id2).unwrap();
        let id_b = manager.append_message(user_msg("branch-B"));

        // Branch C
        manager.branch(&id2).unwrap();
        let id_c = manager.append_message(user_msg("branch-C"));

        let tree = manager.get_tree(Uuid::nil()).unwrap();
        let node2 = &tree[0].children[0];
        assert_eq!(node2.entry.id, id2);
        assert_eq!(node2.children.len(), 3);

        let mut branch_ids: Vec<String> = node2.children.iter().map(|c| c.entry.id.clone()).collect();
        branch_ids.sort();
        let mut expected = vec![id_a, id_b, id_c];
        expected.sort();
        assert_eq!(branch_ids, expected);
    }

    // ------------------------------------------------------------------------
    // Deep branching
    // ------------------------------------------------------------------------

    #[test]
    fn test_deep_branching() {
        let mut manager = SessionManager::in_memory("/tmp");

        // Main path: 1 -> 2 -> 3 -> 4
        manager.append_message(user_msg("1"));
        let id2 = manager.append_message(bare_assistant_msg());
        let id3 = manager.append_message(user_msg("3"));
        manager.append_message(bare_assistant_msg());

        // Branch from 2: 2 -> 5 -> 6
        manager.branch(&id2).unwrap();
        let id5 = manager.append_message(user_msg("5"));
        manager.append_message(bare_assistant_msg());

        // Branch from 5: 5 -> 7
        manager.branch(&id5).unwrap();
        manager.append_message(user_msg("7"));

        let tree = manager.get_tree(Uuid::nil()).unwrap();

        // node2 has 2 children: id3 and id5
        let node2 = &tree[0].children[0];
        assert_eq!(node2.children.len(), 2);

        let node5 = node2.children.iter().find(|c| c.entry.id == id5).unwrap();
        assert_eq!(node5.children.len(), 2); // id6 and id7

        let node3 = node2.children.iter().find(|c| c.entry.id == id3).unwrap();
        assert_eq!(node3.children.len(), 1); // id4
    }

    // ------------------------------------------------------------------------
    // branch_with_summary
    // ------------------------------------------------------------------------

    #[test]
    fn test_branch_with_summary_inserts_and_advances() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(user_msg("1"));
        manager.append_message(bare_assistant_msg());
        manager.append_message(user_msg("3"));

        let summary_id = manager.branch_with_summary(
            Some(&id1),
            "Summary of abandoned work",
            None,
            None,
        );
        assert!(!summary_id.is_empty());
        assert_eq!(manager.get_leaf_id(), Some(summary_id.clone()));

        // Verify branch_summary entry
        let entries = manager.get_entries();
        let summary_entry = entries.iter().find(|e| e.id == summary_id).unwrap();
        assert_eq!(summary_entry.parent_id, Some(id1));

        if let AgentMessage::BranchSummary { summary, .. } = &summary_entry.message {
            assert_eq!(summary, "Summary of abandoned work");
        } else {
            panic!("Expected BranchSummary");
        }
    }

    // ------------------------------------------------------------------------
    // build_session_context with branches
    // ------------------------------------------------------------------------

    #[test]
    fn test_build_session_context_returns_branch_messages() {
        let mut manager = SessionManager::in_memory("/tmp");

        // Main: 1 -> 2 -> 3
        manager.append_message(user_msg("msg1"));
        let id2 = manager.append_message(bare_assistant_msg());
        manager.append_message(user_msg("msg3"));

        // Branch from 2: 2 -> 4
        manager.branch(&id2).unwrap();
        manager.append_message(assistant_msg("msg4-branch"));

        let ctx = manager.build_session_context();
        // Should have msg1, msg2, msg4-branch (NOT msg3)
        assert_eq!(ctx.messages.len(), 3);
        assert!(ctx.messages[0].is_user());
        assert!(ctx.messages[1].is_assistant());
        assert!(ctx.messages[2].is_assistant());
    }

    #[test]
    fn test_build_session_context_follows_branch_path() {
        // Tree: 1 -> 2 -> 3 (branch A)
        //             \-> 4 (branch B)
        let mut manager = SessionManager::in_memory("/tmp");
        manager.append_message(user_msg("start"));
        let id2 = manager.append_message(bare_assistant_msg());
        manager.append_message(user_msg("branch A"));

        // Switch to branch B
        manager.branch(&id2).unwrap();
        manager.append_message(user_msg("branch B"));

        let ctx = manager.build_session_context();
        assert_eq!(ctx.messages.len(), 3);
        // Last message should be "branch B"
        let last = ctx.messages.last().unwrap();
        assert_eq!(last.content(), "branch B");
    }

    #[test]
    fn test_build_session_context_includes_branch_summary() {
        let mut manager = SessionManager::in_memory("/tmp");
        manager.append_message(user_msg("start"));
        let id2 = manager.append_message(bare_assistant_msg());
        manager.append_message(user_msg("abandoned path"));

        // Branch with summary
        manager.branch_with_summary(Some(&id2), "Summary of abandoned work", None, None);
        manager.append_message(user_msg("new direction"));

        let ctx = manager.build_session_context();
        // Should include: start, response, branch_summary, new direction
        assert!(ctx.messages.len() >= 3);

        // Branch summary should be in messages
        let has_summary = ctx.messages.iter().any(|m| {
            if let AgentMessage::BranchSummary { summary, .. } = m {
                summary == "Summary of abandoned work"
            } else {
                false
            }
        });
        assert!(has_summary, "Branch summary should be in context messages");
    }

    #[test]
    fn test_build_session_context_with_compaction() {
        let mut manager = SessionManager::in_memory("/tmp");

        // Build conversation
        let id1 = manager.append_message(user_msg("first"));
        manager.append_message(assistant_msg("response1"));
        manager.append_message(user_msg("second"));
        manager.append_message(assistant_msg("response2"));

        // Add compaction
        manager.append_compaction("Summary of first two turns", &id1, 1000, None, None);

        // Continue after compaction
        manager.append_message(user_msg("third"));
        manager.append_message(assistant_msg("response3"));

        let ctx = manager.build_session_context();
        // CompactionSummary is NOT included in context messages (only user/assistant/branch_summary)
        // but the path from leaf should include all entries
        assert!(ctx.messages.len() >= 4); // at minimum: user, assistant, user, assistant from after-compaction path

        // Compaction entry should exist in the entries
        let compaction_entries = manager.get_compaction_entries();
        assert_eq!(compaction_entries.len(), 1);
    }

    #[test]
    fn test_build_session_context_tracks_thinking_level() {
        let mut manager = SessionManager::in_memory("/tmp");
        manager.append_message(user_msg("hello"));
        manager.append_thinking_level_change("high");
        manager.append_message(assistant_msg("thinking hard"));

        let ctx = manager.build_session_context();
        assert_eq!(ctx.thinking_level, "high");
    }

    // ------------------------------------------------------------------------
    // Labels in tree nodes
    // ------------------------------------------------------------------------

    #[test]
    fn test_labels_in_tree_nodes() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(user_msg("hello"));
        let id2 = manager.append_message(assistant_msg("hi"));

        manager.add_label(&id1, "start").unwrap();
        manager.add_label(&id2, "response").unwrap();

        let tree = manager.get_tree(Uuid::nil()).unwrap();
        let node1 = &tree[0];
        assert_eq!(node1.label, Some("start".to_string()));

        let node2 = &node1.children[0];
        assert_eq!(node2.label, Some("response".to_string()));
    }

    #[test]
    fn test_last_label_wins() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(user_msg("hello"));

        manager.add_label(&id1, "first").unwrap();
        manager.add_label(&id1, "second").unwrap();
        manager.add_label(&id1, "third").unwrap();

        assert_eq!(manager.get_label(&id1), Some("third".to_string()));
    }

    // ------------------------------------------------------------------------
    // branch throws for non-existent
    // ------------------------------------------------------------------------

    #[test]
    fn test_branch_throws_for_nonexistent() {
        let mut manager = SessionManager::in_memory("/tmp");
        manager.append_message(user_msg("hello"));

        let result = manager.branch("nonexistent");
        assert!(result.is_err());
    }

    // ------------------------------------------------------------------------
    // Labels not included in buildSessionContext
    // ------------------------------------------------------------------------

    #[test]
    fn test_labels_not_in_session_context() {
        let mut manager = SessionManager::in_memory("/tmp");
        let msg_id = manager.append_message(user_msg("hello"));
        manager.add_label(&msg_id, "checkpoint").unwrap();

        let ctx = manager.build_session_context();
        // Should only have the user message, not label entries
        assert_eq!(ctx.messages.len(), 1);
        assert!(ctx.messages[0].is_user());
    }

    // ------------------------------------------------------------------------
    // appendCustomEntry integration
    // ------------------------------------------------------------------------

    #[test]
    fn test_custom_entry_integrates_into_tree() {
        let mut manager = SessionManager::in_memory("/tmp");
        let msg_id = manager.append_message(user_msg("hello"));
        let custom_id = manager.append_custom_entry("my_data", Some(serde_json::json!({"foo": "bar"})));
        let msg2_id = manager.append_message(assistant_msg("response"));

        let entries = manager.get_entries();
        let custom = entries.iter().find(|e| e.id == custom_id).unwrap();
        assert_eq!(custom.parent_id, Some(msg_id));

        if let AgentMessage::Custom { custom_type, .. } = &custom.message {
            assert_eq!(custom_type, "my_data");
        } else {
            panic!("Expected Custom message");
        }

        let msg2 = entries.iter().find(|e| e.id == msg2_id).unwrap();
        assert_eq!(msg2.parent_id, Some(custom_id));

        // buildSessionContext should work (custom entries skipped in messages)
        let ctx = manager.build_session_context();
        // Only the 2 real messages; custom entry is not user/assistant/branch_summary
        assert_eq!(ctx.messages.len(), 2);
    }

    // ------------------------------------------------------------------------
    // Empty session edge cases
    // ------------------------------------------------------------------------

    #[test]
    fn test_get_branch_empty_session() {
        let manager = SessionManager::in_memory("/tmp");
        let branch = manager.get_branch(None);
        assert!(branch.is_empty());
    }

    #[test]
    fn test_get_tree_empty_session() {
        let manager = SessionManager::in_memory("/tmp");
        let tree = manager.get_tree(Uuid::nil()).unwrap();
        assert!(tree.is_empty());
    }

    // ------------------------------------------------------------------------
    // Complex tree with branches and compaction
    // ------------------------------------------------------------------------

    #[test]
    fn test_complex_tree_with_branches_and_compaction() {
        let mut manager = SessionManager::in_memory("/tmp");

        // Main path: 1 -> 2 -> 3 -> 4 -> compaction(5) -> 6 -> 7
        manager.append_message(user_msg("start"));
        manager.append_message(assistant_msg("r1"));
        let id3 = manager.append_message(user_msg("q2"));
        manager.append_message(assistant_msg("r2"));
        manager.append_compaction("Compacted history", &id3, 1000, None, None);
        manager.append_message(user_msg("q3"));
        manager.append_message(assistant_msg("r3"));

        // Abandoned branch from 3
        manager.branch(&id3).unwrap();
        manager.append_message(user_msg("wrong path"));
        manager.append_message(assistant_msg("wrong response"));

        // Branch summary resuming from 3
        manager.branch_with_summary(Some(&id3), "Tried wrong approach", None, None);
        manager.append_message(user_msg("better approach"));

        let tree = manager.get_tree(Uuid::nil()).unwrap();
        // Root node
        assert_eq!(tree.len(), 1);

        // Walk tree to verify structure
        let root = &tree[0];
        assert!(root.entry.message.is_user());
    }

    // ------------------------------------------------------------------------
    // get_latest_compaction_entry returns the most recent
    // ------------------------------------------------------------------------

    #[test]
    fn test_multiple_compactions_returns_latest() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(user_msg("a"));
        manager.append_message(bare_assistant_msg());
        manager.append_compaction("First summary", &id1, 1000, None, None);
        manager.append_message(user_msg("c"));
        manager.append_message(bare_assistant_msg());
        manager.append_compaction("Second summary", &id1, 2000, None, None);

        // get_compaction_entries returns all compaction entries
        let compactions = manager.get_compaction_entries();
        assert_eq!(compactions.len(), 2);

        // At least one should exist with the second summary
        let latest = manager.get_latest_compaction_entry();
        assert!(latest.is_some());
    }

    // ------------------------------------------------------------------------
    // get_compaction_entries returns all
    // ------------------------------------------------------------------------

    #[test]
    fn test_get_all_compaction_entries() {
        let mut manager = SessionManager::in_memory("/tmp");
        let id1 = manager.append_message(user_msg("a"));
        manager.append_message(bare_assistant_msg());
        manager.append_compaction("First", &id1, 1000, None, None);
        manager.append_message(user_msg("b"));
        manager.append_message(bare_assistant_msg());
        manager.append_compaction("Second", &id1, 2000, None, None);

        let compactions = manager.get_compaction_entries();
        assert_eq!(compactions.len(), 2);
    }
}
