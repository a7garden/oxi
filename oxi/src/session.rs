use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// A single entry in a session conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: Uuid,
    /// Parent session ID for branched sessions (None = root session)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    pub message: AgentMessage,
    /// Optional label for this entry (e.g., for bookmarks)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub timestamp: i64,
}

impl SessionEntry {
    /// Create a new entry
    pub fn new(message: AgentMessage) -> Self {
        Self {
            id: Uuid::new_v4(),
            parent_id: None,
            message,
            label: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create a branched entry with a parent reference
    pub fn branched(message: AgentMessage, parent_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            parent_id: Some(parent_id),
            message,
            label: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentMessage {
    User { content: String },
    Assistant { content: String },
    System { content: String },
}

impl AgentMessage {
    /// Get the content of the message
    pub fn content(&self) -> &str {
        match self {
            AgentMessage::User { content } => content,
            AgentMessage::Assistant { content } => content,
            AgentMessage::System { content } => content,
        }
    }
}

/// Session metadata stored separately from entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,          // Root session that this branched from (if any)
    pub root_id: Option<Uuid>,           // Original root session (for deep branches)
    pub branch_point: Option<Uuid>,       // Entry ID where branching occurred
    pub created_at: i64,
    pub updated_at: i64,
    pub name: Option<String>,
    pub model: Option<String>,
    pub message_count: usize,
    pub cwd: Option<String>,
}

impl SessionMeta {
    pub fn new(id: Uuid) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            id,
            parent_id: None,
            root_id: None,
            branch_point: None,
            created_at: now,
            updated_at: now,
            name: None,
            model: None,
            message_count: 0,
            cwd: None,
        }
    }

    pub fn branched_from(parent_id: Uuid, root_id: Option<Uuid>, branch_point: Uuid) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            id: Uuid::new_v4(),
            parent_id: Some(parent_id),
            root_id: root_id.or(Some(parent_id)),
            branch_point: Some(branch_point),
            created_at: now,
            updated_at: now,
            name: None,
            model: None,
            message_count: 0,
            cwd: None,
        }
    }

    /// Update the updated_at timestamp
    pub fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().timestamp_millis();
    }

    /// Get created_at as DateTime
    pub fn created_at_datetime(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.created_at).unwrap_or_else(Utc::now)
    }

    /// Get updated_at as DateTime
    pub fn updated_at_datetime(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.updated_at).unwrap_or_else(Utc::now)
    }
}

/// Tree node for hierarchical session display
#[derive(Debug, Clone)]
pub struct SessionTreeNode {
    pub meta: SessionMeta,
    pub children: Vec<SessionTreeNode>,
}

/// Flattened session node for display with tree structure info
#[derive(Debug, Clone)]
pub struct FlatSessionNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub depth: usize,
    pub is_last: bool,
    /// For each ancestor level, whether there are more siblings after it
    pub ancestor_continues: Vec<bool>,
    pub created_at: DateTime<Utc>,
    pub model: String,
    pub message_count: usize,
    pub has_children: bool,
}

impl FlatSessionNode {
    /// Build tree prefix string (│, ├, └ characters)
    pub fn tree_prefix(&self) -> String {
        if self.depth == 0 {
            return String::new();
        }

        let mut prefix = String::new();
        for &continues in &self.ancestor_continues {
            if continues {
                prefix.push_str("│  ");
            } else {
                prefix.push_str("   ");
            }
        }
        // Add branch character
        if self.is_last {
            prefix.push_str("└─ ");
        } else {
            prefix.push_str("├─ ");
        }
        prefix
    }
}

/// Sort mode for session list
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    /// Threaded - hierarchical tree view
    Threaded,
    /// Recent - sorted by last updated time
    Recent,
    /// Name - sorted alphabetically by name
    Name,
}

impl Default for SortMode {
    fn default() -> Self {
        SortMode::Threaded
    }
}

/// Session scope filter
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScope {
    /// Only sessions in current directory
    Current,
    /// All sessions
    All,
}

impl Default for SessionScope {
    fn default() -> Self {
        SessionScope::Current
    }
}

pub struct SessionManager {
    sessions_dir: PathBuf,
    meta_dir: PathBuf,
    trash_dir: PathBuf,
}

impl SessionManager {
    pub async fn new() -> Result<Self> {
        let home = dirs::home_dir().context("Cannot find home directory")?;
        let base_dir = home.join(".oxi");
        let sessions_dir = base_dir.join("sessions");
        let meta_dir = base_dir.join("meta");
        let trash_dir = base_dir.join("trash");
        tokio::fs::create_dir_all(&sessions_dir).await?;
        tokio::fs::create_dir_all(&meta_dir).await?;
        tokio::fs::create_dir_all(&trash_dir).await?;
        Ok(Self { sessions_dir, meta_dir, trash_dir })
    }

    pub async fn save(&self, id: Uuid, entries: &[SessionEntry]) -> Result<()> {
        let path = self.session_path(&id);
        let json = serde_json::to_string_pretty(entries)?;
        tokio::fs::write(&path, json).await?;

        // Update message count in metadata
        if let Some(mut meta) = self.load_meta(id).await? {
            meta.message_count = entries.len();
            meta.touch();
            self.save_meta(&meta).await?;
        }

        Ok(())
    }

    pub async fn load(&self, id: Uuid) -> Result<Vec<SessionEntry>> {
        let path = self.session_path(&id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let contents = tokio::fs::read_to_string(&path).await?;
        let entries: Vec<SessionEntry> = serde_json::from_str(&contents)?;
        Ok(entries)
    }

    pub fn session_path(&self, id: &Uuid) -> PathBuf {
        self.sessions_dir.join(format!("{}.json", id))
    }

    /// Get the path for session metadata
    fn meta_path(&self, id: &Uuid) -> PathBuf {
        self.meta_dir.join(format!("{}.json", id))
    }

    /// Get the path for trashed session
    fn trash_path(&self, id: &Uuid) -> PathBuf {
        self.trash_dir.join(format!("{}.json", id))
    }

    /// List all session metadata
    pub async fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        let mut entries = tokio::fs::read_dir(&self.meta_dir).await?;
        let mut metas = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(contents) = tokio::fs::read_to_string(&path).await {
                    if let Ok(meta) = serde_json::from_str::<SessionMeta>(&contents) {
                        metas.push(meta);
                    }
                }
            }
        }

        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(metas)
    }

    /// List sessions in trash
    pub async fn list_trash(&self) -> Result<Vec<SessionMeta>> {
        let mut entries = tokio::fs::read_dir(&self.trash_dir).await?;
        let mut metas = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(contents) = tokio::fs::read_to_string(&path).await {
                    if let Ok(meta) = serde_json::from_str::<SessionMeta>(&contents) {
                        metas.push(meta);
                    }
                }
            }
        }

        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(metas)
    }

    /// Save session metadata
    pub async fn save_meta(&self, meta: &SessionMeta) -> Result<()> {
        let path = self.meta_path(&meta.id);
        let json = serde_json::to_string_pretty(meta)?;
        tokio::fs::write(&path, json).await?;
        Ok(())
    }

    /// Load session metadata
    pub async fn load_meta(&self, id: Uuid) -> Result<Option<SessionMeta>> {
        let path = self.meta_path(&id);
        if !path.exists() {
            return Ok(None);
        }
        let contents = tokio::fs::read_to_string(&path).await?;
        let meta: SessionMeta = serde_json::from_str(&contents)?;
        Ok(Some(meta))
    }

    /// Create a new session
    pub async fn create(&self) -> Result<SessionMeta> {
        let id = Uuid::new_v4();
        let meta = SessionMeta::new(id);
        self.save_meta(&meta).await?;
        Ok(meta)
    }

    /// Create a branch from an existing session at a given entry
    pub async fn branch_from(&self, parent_id: Uuid, entry_id: Uuid) -> Result<(Uuid, Vec<SessionEntry>)> {
        // Load parent entries
        let parent_entries = self.load(parent_id).await?;

        // Find the entry index
        let entry_idx = parent_entries
            .iter()
            .position(|e| e.id == entry_id)
            .with_context(|| format!("Entry {} not found in session {}", entry_id, parent_id))?;

        // Load parent metadata to get root info
        let parent_meta = self.load_meta(parent_id).await?
            .with_context(|| format!("Parent session {} not found", parent_id))?;

        // Create new session
        let new_id = Uuid::new_v4();
        let meta = SessionMeta::branched_from(parent_id, parent_meta.root_id.or(Some(parent_id)), entry_id);

        // Copy entries up to and including the branch point
        let mut new_entries: Vec<SessionEntry> = parent_entries[..=entry_idx]
            .iter()
            .map(|e| {
                let mut new_entry = e.clone();
                new_entry.id = Uuid::new_v4();
                new_entry
            })
            .collect();

        // Update the last entry to have parent reference
        if let Some(last) = new_entries.last_mut() {
            last.parent_id = Some(entry_id);
        }

        // Save the new session
        self.save_meta(&meta).await?;
        self.save(new_id, &new_entries).await?;

        Ok((new_id, new_entries))
    }

    /// Get all entries in a session
    pub async fn get_entries(&self, session_id: Uuid) -> Result<Vec<SessionEntry>> {
        self.load(session_id).await
    }

    /// Get all entries in tree order (depth-first traversal from root to this session)
    pub async fn get_tree(&self, session_id: Uuid) -> Result<Vec<(Uuid, SessionEntry)>> {
        let mut tree = Vec::new();
        let mut current_id = Some(session_id);

        while let Some(id) = current_id {
            let meta = match self.load_meta(id).await? {
                Some(m) => m,
                None => break,
            };

            // Load entries for this session
            let entries = self.load(id).await?;
            for entry in entries {
                tree.push((id, entry));
            }

            // Move to parent
            current_id = meta.parent_id;
        }

        Ok(tree)
    }

    /// Build hierarchical session tree from flat metadata list
    pub fn build_tree(&self, metas: &[SessionMeta]) -> Vec<SessionTreeNode> {
        use std::collections::HashMap;

        let mut nodes: HashMap<Uuid, SessionTreeNode> = HashMap::new();
        let mut children_map: HashMap<Option<Uuid>, Vec<Uuid>> = HashMap::new();

        // First pass: create nodes
        for meta in metas {
            children_map.entry(meta.parent_id).or_default().push(meta.id);
            nodes.insert(meta.id, SessionTreeNode {
                meta: meta.clone(),
                children: Vec::new(),
            });
        }

        // Second pass: build children
        let mut roots: Vec<SessionTreeNode> = Vec::new();
        for (parent_id, child_ids) in children_map {
            for child_id in child_ids {
                if let Some(node) = nodes.remove(&child_id) {
                    if let Some(pid) = parent_id {
                        if let Some(parent) = nodes.get_mut(&pid) {
                            parent.children.push(node);
                        } else {
                            roots.push(node);
                        }
                    } else {
                        roots.push(node);
                    }
                }
            }
        }

        // Sort children by updated_at descending
        fn sort_children(node: &mut SessionTreeNode) {
            node.children.sort_by(|a, b| b.meta.updated_at.cmp(&a.meta.updated_at));
            for child in &mut node.children {
                sort_children(child);
            }
        }

        for node in &mut roots {
            sort_children(node);
        }

        // Sort roots by updated_at descending
        roots.sort_by(|a, b| b.meta.updated_at.cmp(&a.meta.updated_at));
        roots
    }

    /// Flatten tree into display list with tree structure metadata
    pub fn flatten_tree(&self, tree: &[SessionTreeNode], show_all: bool) -> Vec<FlatSessionNode> {
        let mut result = Vec::new();

        fn walk(
            node: &SessionTreeNode,
            depth: usize,
            ancestor_continues: Vec<bool>,
            is_last: bool,
            result: &mut Vec<FlatSessionNode>,
            has_children_map: &std::collections::HashMap<Uuid, bool>,
        ) {
            let name = node.meta.name.clone()
                .unwrap_or_else(|| {
                    // Generate name from first message content
                    let entries = Vec::new(); // We'd need to load to know
                    node.meta.id.to_string()[..8].to_string()
                });

            result.push(FlatSessionNode {
                id: node.meta.id.to_string(),
                name,
                parent_id: node.meta.parent_id.map(|p| p.to_string()),
                depth,
                is_last,
                ancestor_continues: ancestor_continues.clone(),
                created_at: node.meta.created_at_datetime(),
                model: node.meta.model.clone().unwrap_or_default(),
                message_count: node.meta.message_count,
                has_children: *has_children_map.get(&node.meta.id).unwrap_or(&false),
            });

            // Process children
            for (i, child) in node.children.iter().enumerate() {
                let child_is_last = i == node.children.len() - 1;
                // Only show continuation line for non-root ancestors
                let mut child_ancestors = ancestor_continues.clone();
                if depth > 0 {
                    child_ancestors.push(!is_last);
                }
                walk(child, depth + 1, child_ancestors, child_is_last, result, has_children_map);
            }
        }

        // Build has_children map
        let mut has_children_map = std::collections::HashMap::new();
        for node in tree {
            fn check_has_children(node: &SessionTreeNode, map: &mut std::collections::HashMap<Uuid, bool>) {
                let has = !node.children.is_empty();
                map.insert(node.meta.id, has);
                for child in &node.children {
                    check_has_children(child, map);
                }
            }
            check_has_children(node, &mut has_children_map);
        }

        for (i, node) in tree.iter().enumerate() {
            let is_last = i == tree.len() - 1;
            walk(node, 0, Vec::new(), is_last, &mut result, &has_children_map);
        }

        result
    }

    /// Rename a session
    pub async fn rename_session(&mut self, id: &str, name: &str) -> Result<()> {
        let uuid = Uuid::parse_str(id)
            .with_context(|| format!("Invalid session ID: {}", id))?;

        let mut meta = self.load_meta(uuid).await?
            .with_context(|| format!("Session {} not found", id))?;

        meta.name = Some(name.to_string());
        meta.touch();
        self.save_meta(&meta).await?;

        Ok(())
    }

    /// Delete a session permanently
    pub async fn delete_session(&mut self, id: &str) -> Result<()> {
        let uuid = Uuid::parse_str(id)
            .with_context(|| format!("Invalid session ID: {}", id))?;

        tokio::fs::remove_file(self.session_path(&uuid)).await.ok();
        tokio::fs::remove_file(self.meta_path(&uuid)).await.ok();

        Ok(())
    }

    /// Move a session to trash
    pub async fn trash_session(&mut self, id: &str) -> Result<()> {
        let uuid = Uuid::parse_str(id)
            .with_context(|| format!("Invalid session ID: {}", id))?;

        // Load session data
        let entries = self.load(uuid).await?;
        let meta = self.load_meta(uuid).await?
            .with_context(|| format!("Session {} not found", id))?;

        // Save to trash
        let trash_session_path = self.trash_dir.join(format!("{}.json", uuid));
        let json = serde_json::to_string_pretty(&entries)?;
        tokio::fs::write(&trash_session_path, &json).await?;

        let trash_meta_path = self.trash_dir.join(format!("meta_{}.json", uuid));
        let meta_json = serde_json::to_string_pretty(&meta)?;
        tokio::fs::write(&trash_meta_path, &meta_json).await?;

        // Remove from original location
        tokio::fs::remove_file(self.session_path(&uuid)).await.ok();
        tokio::fs::remove_file(self.meta_path(&uuid)).await.ok();

        Ok(())
    }

    /// Restore a session from trash
    pub async fn restore_from_trash(&mut self, id: &str) -> Result<()> {
        let uuid = Uuid::parse_str(id)
            .with_context(|| format!("Invalid session ID: {}", id))?;

        // Load from trash
        let trash_session_path = self.trash_dir.join(format!("{}.json", uuid));
        let trash_meta_path = self.trash_dir.join(format!("meta_{}.json", uuid));

        if !trash_session_path.exists() {
            return Err(anyhow::anyhow!("Session not found in trash"));
        }

        let entries = tokio::fs::read_to_string(&trash_session_path).await?;
        let meta_json = tokio::fs::read_to_string(&trash_meta_path).await?;
        let meta: SessionMeta = serde_json::from_str(&meta_json)?;

        // Restore to original location
        tokio::fs::write(self.session_path(&uuid), &entries).await?;
        tokio::fs::write(self.meta_path(&uuid), &meta_json).await?;

        // Remove from trash
        tokio::fs::remove_file(&trash_session_path).await.ok();
        tokio::fs::remove_file(&trash_meta_path).await.ok();

        Ok(())
    }

    /// Empty trash
    pub async fn empty_trash(&mut self) -> Result<()> {
        let mut entries = tokio::fs::read_dir(&self.trash_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                tokio::fs::remove_file(&path).await.ok();
            }
        }

        Ok(())
    }

    /// Get all direct branches from a given entry across all sessions
    pub async fn get_branches_from_entry(&self, entry_id: Uuid) -> Result<Vec<(Uuid, SessionEntry)>> {
        let mut branches = Vec::new();
        let metas = self.list_sessions().await?;

        for meta in metas {
            // Check if this session branched from the given entry
            if meta.branch_point == Some(entry_id) || meta.parent_id == Some(entry_id) {
                // Get first entry of this branch
                let entries = self.load(meta.id).await?;
                if let Some(first) = entries.first() {
                    branches.push((meta.id, first.clone()));
                }
            }
        }

        Ok(branches)
    }

    /// Get branch point info for a session
    pub async fn get_branch_info(&self, session_id: Uuid) -> Result<Option<BranchInfo>> {
        let meta = match self.load_meta(session_id).await? {
            Some(m) => m,
            None => return Ok(None),
        };

        if meta.parent_id.is_none() {
            return Ok(None);
        }

        let parent_meta = self.load_meta(meta.parent_id.unwrap()).await?;
        Ok(Some(BranchInfo {
            session_id,
            parent_session_id: meta.parent_id,
            root_session_id: meta.root_id,
            branch_point_entry_id: meta.branch_point,
            parent_session_name: parent_meta.as_ref().and_then(|m| m.name.clone()),
        }))
    }

    /// Delete a session permanently (deprecated, use delete_session or trash_session)
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        tokio::fs::remove_file(self.session_path(&id)).await.ok();
        tokio::fs::remove_file(self.meta_path(&id)).await.ok();
        Ok(())
    }

    /// Update session metadata (for tracking model, cwd, etc.)
    pub async fn update_meta(&self, id: Uuid, name: Option<String>, model: Option<String>, cwd: Option<String>) -> Result<()> {
        let mut meta = self.load_meta(id).await?
            .with_context(|| format!("Session {} not found", id))?;

        if let Some(n) = name {
            meta.name = Some(n);
        }
        if let Some(m) = model {
            meta.model = Some(m);
        }
        if let Some(c) = cwd {
            meta.cwd = Some(c);
        }
        meta.touch();
        self.save_meta(&meta).await?;

        Ok(())
    }
}

/// Information about where a session branched from
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub session_id: Uuid,
    pub parent_session_id: Option<Uuid>,
    pub root_session_id: Option<Uuid>,
    pub branch_point_entry_id: Option<Uuid>,
    pub parent_session_name: Option<String>,
}

/// Thread-safe wrapper for SessionManager
pub struct SharedSessionManager {
    inner: Arc<RwLock<SessionManager>>,
}

impl SharedSessionManager {
    pub fn new(manager: SessionManager) -> Self {
        Self {
            inner: Arc::new(RwLock::new(manager)),
        }
    }

    pub async fn rename_session(&self, id: &str, name: &str) -> Result<()> {
        self.inner.write().await.rename_session(id, name).await
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        self.inner.write().await.delete_session(id).await
    }

    pub async fn trash_session(&self, id: &str) -> Result<()> {
        self.inner.write().await.trash_session(id).await
    }

    pub async fn restore_from_trash(&self, id: &str) -> Result<()> {
        self.inner.write().await.restore_from_trash(id).await
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        self.inner.read().await.list_sessions().await
    }

    pub async fn list_trash(&self) -> Result<Vec<SessionMeta>> {
        self.inner.read().await.list_trash().await
    }

    pub async fn empty_trash(&self) -> Result<()> {
        self.inner.write().await.empty_trash().await
    }

    pub async fn get_session_path(&self, id: &str) -> Option<PathBuf> {
        let uuid = Uuid::parse_str(id).ok()?;
        Some(self.inner.read().await.session_path(&uuid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_load_session() {
        let manager = SessionManager::new().await.unwrap();

        // Create session
        let meta = manager.create().await.unwrap();
        let session_id = meta.id;

        // Save entries
        let entries = vec![
            SessionEntry::new(AgentMessage::User { content: "Hello".to_string() }),
            SessionEntry::new(AgentMessage::Assistant { content: "Hi!".to_string() }),
        ];
        manager.save(session_id, &entries).await.unwrap();

        // Load and verify
        let loaded = manager.load(session_id).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].message.content(), "Hello");

        // Load metadata
        let loaded_meta = manager.load_meta(session_id).await.unwrap().unwrap();
        assert_eq!(loaded_meta.message_count, 2);
    }

    #[tokio::test]
    async fn test_branch_session() {
        let manager = SessionManager::new().await.unwrap();

        // Create parent session
        let parent_meta = manager.create().await.unwrap();
        let parent_id = parent_meta.id;

        // Add entries
        let entries = vec![
            SessionEntry::new(AgentMessage::User { content: "Question".to_string() }),
            SessionEntry::new(AgentMessage::Assistant { content: "Answer 1".to_string() }),
            SessionEntry::new(AgentMessage::User { content: "Followup".to_string() }),
            SessionEntry::new(AgentMessage::Assistant { content: "Answer 2".to_string() }),
        ];
        manager.save(parent_id, &entries).await.unwrap();

        // Branch at entry 1
        let (child_id, child_entries) = manager.branch_from(parent_id, entries[1].id).await.unwrap();

        // Verify branch
        assert_eq!(child_entries.len(), 2); // Entries 0 and 1
        assert_eq!(child_entries[0].message.content(), "Question");
        assert_eq!(child_entries[1].message.content(), "Answer 1");

        // Check branch info
        let branch_info = manager.get_branch_info(child_id).await.unwrap().unwrap();
        assert_eq!(branch_info.parent_session_id, Some(parent_id));
        assert_eq!(branch_info.branch_point_entry_id, Some(entries[1].id));
    }

    #[test]
    fn test_flat_session_node_prefix() {
        let node = FlatSessionNode {
            id: "1".to_string(),
            name: "Test".to_string(),
            parent_id: None,
            depth: 0,
            is_last: false,
            ancestor_continues: vec![],
            created_at: Utc::now(),
            model: "gpt-4".to_string(),
            message_count: 5,
            has_children: true,
        };
        assert_eq!(node.tree_prefix(), "");

        let child = FlatSessionNode {
            id: "2".to_string(),
            name: "Child".to_string(),
            parent_id: Some("1".to_string()),
            depth: 1,
            is_last: true,
            ancestor_continues: vec![],
            created_at: Utc::now(),
            model: "gpt-4".to_string(),
            message_count: 3,
            has_children: false,
        };
        assert_eq!(child.tree_prefix(), "└─ ");

        let grandchild = FlatSessionNode {
            id: "3".to_string(),
            name: "Grandchild".to_string(),
            parent_id: Some("2".to_string()),
            depth: 2,
            is_last: false,
            ancestor_continues: vec![true],
            created_at: Utc::now(),
            model: "gpt-4".to_string(),
            message_count: 1,
            has_children: false,
        };
        assert_eq!(grandchild.tree_prefix(), "   ├─ ");
    }

    #[test]
    fn test_sort_mode_default() {
        assert_eq!(SortMode::default(), SortMode::Threaded);
    }

    #[test]
    fn test_session_scope_default() {
        assert_eq!(SessionScope::default(), SessionScope::Current);
    }

    #[tokio::test]
    async fn test_build_tree() {
        let manager = SessionManager::new().await.unwrap();

        // Create sessions with parent-child relationships
        let root = manager.create().await.unwrap();
        let mut root_meta = manager.load_meta(root.id).await.unwrap().unwrap();
        root_meta.name = Some("Root".to_string());
        manager.save_meta(&root_meta).await.unwrap();

        let child1 = manager.branch_from(root.id, Uuid::new_v4()).await.unwrap().0;
        let mut child1_meta = manager.load_meta(child1).await.unwrap().unwrap();
        child1_meta.name = Some("Child 1".to_string());
        manager.save_meta(&child1_meta).await.unwrap();

        let child2 = manager.branch_from(root.id, Uuid::new_v4()).await.unwrap().0;
        let mut child2_meta = manager.load_meta(child2).await.unwrap().unwrap();
        child2_meta.name = Some("Child 2".to_string());
        manager.save_meta(&child2_meta).await.unwrap();

        // List and build tree
        let metas = manager.list_sessions().await.unwrap();
        let tree = manager.build_tree(&metas);

        // Should have one root with children
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].meta.name.as_deref(), Some("Root"));
        assert_eq!(tree[0].children.len(), 2);
    }

    #[tokio::test]
    async fn test_flatten_tree() {
        let manager = SessionManager::new().await.unwrap();

        // Create a simple tree
        let root = manager.create().await.unwrap();
        let (child_id, _) = manager.branch_from(root.id, Uuid::new_v4()).await.unwrap();

        let metas = manager.list_sessions().await.unwrap();
        let tree = manager.build_tree(&metas);
        let flat = manager.flatten_tree(&tree, true);

        // Should have root and child
        assert!(flat.len() >= 2);
        let root_node = flat.iter().find(|n| n.id == root.id.to_string()).unwrap();
        assert_eq!(root_node.depth, 0);

        let child_node = flat.iter().find(|n| n.id == child_id.to_string()).unwrap();
        assert_eq!(child_node.depth, 1);
    }

    #[tokio::test]
    async fn test_rename_session() {
        let manager = SessionManager::new().await.unwrap();

        let meta = manager.create().await.unwrap();
        manager.rename_session(&meta.id.to_string(), "New Name").await.unwrap();

        let loaded = manager.load_meta(meta.id).await.unwrap().unwrap();
        assert_eq!(loaded.name, Some("New Name".to_string()));
    }

    #[tokio::test]
    async fn test_trash_and_restore() {
        let manager = SessionManager::new().await.unwrap();

        let meta = manager.create().await.unwrap();
        let id_str = meta.id.to_string();

        // Trash it
        manager.trash_session(&id_str).await.unwrap();

        // Should not appear in list
        let list = manager.list_sessions().await.unwrap();
        assert!(!list.iter().any(|m| m.id == meta.id));

        // Restore it
        manager.restore_from_trash(&id_str).await.unwrap();

        // Should appear again
        let list = manager.list_sessions().await.unwrap();
        assert!(list.iter().any(|m| m.id == meta.id));
    }

    #[tokio::test]
    async fn test_session_meta_datetime() {
        let meta = SessionMeta::new(Uuid::new_v4());
        assert!(meta.created_at_datetime().timestamp() > 0);
        assert!(meta.updated_at_datetime().timestamp() > 0);

        meta.touch();
        let new_updated = meta.updated_at_datetime();
        assert!(new_updated.timestamp() >= meta.created_at_datetime().timestamp());
    }
}