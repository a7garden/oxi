//! Banks — ported from omp `banks.ts`.
//!
//! Each bank is a separate SQLite database. Banks allow isolating memory
//! stores by project, team, or any other dimension. The default bank is
//! "default".

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{MnemopiError, Result};

const DB_FILENAME: &str = "mnemopi.db";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BankStats {
    pub name: String,
    pub exists: bool,
    pub db_path: String,
    pub db_size_bytes: u64,
}

/// Manages memory banks (separate SQLite databases).
pub struct BankManager {
    pub data_dir: PathBuf,
    pub banks_dir: PathBuf,
}

impl BankManager {
    /// Create a BankManager rooted at the given data directory.
    /// Defaults to `~/.oxicode/mnemopi/data`.
    pub fn new(data_dir: Option<&Path>) -> Self {
        let data_dir = data_dir.map(PathBuf::from).unwrap_or_else(default_data_dir);
        let banks_dir = data_dir.join("banks");
        fs::create_dir_all(&banks_dir).ok();
        Self {
            data_dir,
            banks_dir,
        }
    }

    fn validate_name(&self, name: &str) -> Result<()> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name.contains("..")
            || name.contains('\0')
        {
            return Err(MnemopiError::InvalidInput(format!(
                "invalid bank name: {name:?}"
            )));
        }
        Ok(())
    }

    /// Create a new bank. Returns the DB path.
    pub fn create_bank(&self, name: &str) -> Result<PathBuf> {
        self.validate_name(name)?;
        let dir = self.banks_dir.join(name);
        fs::create_dir_all(&dir)?;
        let db_path = dir.join(DB_FILENAME);
        // Touch the DB file so `bank_exists` returns true immediately.
        // SQLite initializes on first open, so an empty file is valid.
        fs::File::create(&db_path)?;
        Ok(db_path)
    }

    /// Delete a bank. If `force`, removes the directory even if the DB
    /// file doesn't exist.
    pub fn delete_bank(&self, name: &str, force: bool) -> Result<bool> {
        self.validate_name(name)?;
        let dir = self.banks_dir.join(name);
        let db_path = dir.join(DB_FILENAME);
        if !db_path.exists() && !force {
            return Ok(false);
        }
        fs::remove_dir_all(&dir)?;
        Ok(true)
    }

    /// List all bank names.
    pub fn list_banks(&self) -> Result<Vec<String>> {
        let mut banks = Vec::new();
        if !self.banks_dir.exists() {
            return Ok(banks);
        }
        for entry in fs::read_dir(&self.banks_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                banks.push(name.to_string());
            }
        }
        banks.sort();
        Ok(banks)
    }

    /// Check if a bank exists.
    pub fn bank_exists(&self, name: &str) -> bool {
        self.banks_dir.join(name).join(DB_FILENAME).exists()
    }

    /// Get the DB path for a bank.
    pub fn get_bank_db_path(&self, name: &str) -> PathBuf {
        self.banks_dir.join(name).join(DB_FILENAME)
    }

    /// Rename a bank.
    pub fn rename_bank(&self, old_name: &str, new_name: &str) -> Result<PathBuf> {
        self.validate_name(old_name)?;
        self.validate_name(new_name)?;
        let old_dir = self.banks_dir.join(old_name);
        let new_dir = self.banks_dir.join(new_name);
        fs::rename(&old_dir, &new_dir)?;
        Ok(new_dir.join(DB_FILENAME))
    }

    /// Get stats for a bank.
    pub fn get_bank_stats(&self, name: &str) -> Result<BankStats> {
        let db_path = self.get_bank_db_path(name);
        let db_size_bytes = if db_path.exists() {
            fs::metadata(&db_path)?.len()
        } else {
            0
        };
        Ok(BankStats {
            name: name.to_string(),
            exists: db_path.exists(),
            db_path: db_path.to_string_lossy().to_string(),
            db_size_bytes,
        })
    }
}

/// Default data directory: `~/.oxicode/mnemopi/data`.
pub fn default_data_dir() -> PathBuf {
    dirs_or_home().join(".oxicode").join("mnemopi").join("data")
}

fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // Fallback: use dirs crate if available, otherwise temp
            std::env::temp_dir()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bank_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = BankManager::new(Some(tmp.path()));

        // Create
        let path = mgr.create_bank("project-a").unwrap();
        assert!(path.ends_with(DB_FILENAME));

        // Exists
        assert!(mgr.bank_exists("project-a"));

        // List
        let banks = mgr.list_banks().unwrap();
        assert!(banks.contains(&"project-a".to_string()));

        // Stats
        let stats = mgr.get_bank_stats("project-a").unwrap();
        assert_eq!(stats.name, "project-a");
        assert!(stats.exists);

        // Rename
        mgr.rename_bank("project-a", "project-b").unwrap();
        assert!(!mgr.bank_exists("project-a"));
        assert!(mgr.bank_exists("project-b"));

        // Delete
        assert!(mgr.delete_bank("project-b", false).unwrap());
        assert!(!mgr.bank_exists("project-b"));
    }

    #[test]
    fn test_validate_name_rejects_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = BankManager::new(Some(tmp.path()));
        assert!(mgr.create_bank("../escape").is_err());
        assert!(mgr.create_bank("a/b").is_err());
        assert!(mgr.create_bank("").is_err());
    }
}
