//! Session persistence tests

#[cfg(test)]
mod tests {
    use oxi::session::SessionManager;
    use tempfile::TempDir;

    #[test]
    fn test_create_and_load_session() {
        let dir = TempDir::new().unwrap();
        let dir_path = dir.path().to_str().unwrap();
        
        // Create session with cwd and session_dir
        let manager = SessionManager::create(".", Some(dir_path));
        let id = manager.get_session_id();
        
        assert!(!id.is_empty());
        
        // Session file exists
        let session_path = dir.path().join(format!("{}.jsonl", id));
        assert!(session_path.exists());
    }
}
