use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    pub message: AgentMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentMessage {
    User { content: String },
    Assistant { content: String },
    System { content: String },
}

pub struct SessionManager {
    sessions_dir: PathBuf,
}

impl SessionManager {
    pub async fn new() -> Result<Self> {
        let home = dirs::home_dir().context("Cannot find home directory")?;
        let sessions_dir = home.join(".oxi").join("sessions");
        tokio::fs::create_dir_all(&sessions_dir).await?;
        Ok(Self { sessions_dir })
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
}