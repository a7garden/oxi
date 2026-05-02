use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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
        }
    }
}

pub struct SessionManager {
    sessions_dir: PathBuf,
    meta_dir: PathBuf,
}

impl SessionManager {
    pub async fn new() -> Result<Self> {
        let home = dirs::home_dir().context("Cannot find home directory")?;
        let base_dir = home.join(".oxi");
        let sessions_dir = base_dir.join("sessions");
        let meta_dir = base_dir.join("meta");
        tokio::fs::create_dir_all(&sessions_dir).await?;
        tokio::fs::create_dir_all(&meta_dir).await?;
        Ok(Self { sessions_dir, meta_dir })
    }

    pub async fn save(&self, id: Uuid, entries: &[SessionEntry]) -> Result<()> {
        let path = self.session_path(&id);
        let json = serde_json::to_string_pretty(entries)?;
        tokio::fs::write(&path, json).await?;
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

    /// List all session IDs
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

    /// Delete a session
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        tokio::fs::remove_file(self.session_path(&id)).await.ok();
        tokio::fs::remove_file(self.meta_path(&id)).await.ok();
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