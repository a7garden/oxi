//! Session persistence tests

#[cfg(test)]
mod tests {
    use oxi_store::session::{SessionManager, AgentMessage, ContentValue};
    use tempfile::TempDir;

    #[test]
    fn test_create_and_load_session() {
        let dir = TempDir::new().unwrap();
        let dir_path = dir.path().to_str().unwrap();
        
        // Create session with cwd and session_dir
        let mut manager = SessionManager::create(".", Some(dir_path));
        let id = manager.get_session_id();
        
        assert!(!id.is_empty());
        
        // Save session_file right after creation (before any messages)
        let session_file = manager.get_session_file().expect("Session file should be set");
        
        // Adding a user message (not yet persisted - waits for assistant)
        manager.append_message(AgentMessage::User { content: ContentValue::String("test".to_string()) });
        
        // Adding an assistant message triggers persistence
        manager.append_message(AgentMessage::Assistant { 
            content: vec![], 
            provider: None, 
            model_id: None, 
            usage: None, 
            stop_reason: None 
        });
        
        // Session file should now exist
        let path = std::path::Path::new(&session_file);
        assert!(path.exists(), "Session file should exist at: {:?}", path);
    }
}
