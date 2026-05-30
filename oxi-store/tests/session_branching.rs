//! Session branching and tree navigation tests

#[cfg(test)]
mod tests {
    use oxi_store::session::{
        AgentMessage, AssistantContentBlock, ContentValue, SessionEntry, SessionManager,
    };

    fn msg(role: &str, content: &str) -> AgentMessage {
        match role {
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
            _ => AgentMessage::System {
                content: ContentValue::String(content.to_string()),
            },
        }
    }

    fn entry(role: &str, content: &str) -> SessionEntry {
        SessionEntry::simple_message(role, content)
    }

    fn entry_with_parent(role: &str, content: &str, parent_id: &str) -> SessionEntry {
        let e = msg(role, content);
        SessionEntry::branched(e, parent_id)
    }

    #[test]
    fn test_session_create_and_read() {
        let mut mgr = SessionManager::in_memory("/tmp/test");
        let message = msg("user", "Hello");

        let id = mgr.append_message(message.clone());
        assert!(!id.is_empty(), "append_message should return an ID");

        let retrieved = mgr.get_entry(&id);
        assert!(retrieved.is_some(), "Should retrieve the entry");
        assert_eq!(retrieved.unwrap().id, id);
    }

    #[test]
    fn test_session_multiple_entries() {
        let mut mgr = SessionManager::in_memory("/tmp/test");

        let e1 = entry("user", "Hello");
        let e1_id = mgr.append_message(e1.message.clone());

        let e2 = entry_with_parent("assistant", "Hi", &e1_id);
        let e2_id = mgr.append_message(e2.message.clone());

        let e3 = entry_with_parent("user", "How are you?", &e2_id);
        let _e3_id = mgr.append_message(e3.message.clone());

        // All entries should be retrievable
        assert!(mgr.get_entry(&e1_id).is_some());
        assert!(mgr.get_entry(&e2_id).is_some());
    }

    #[test]
    fn test_session_fork() {
        let mut mgr = SessionManager::in_memory("/tmp/test");

        let e1 = entry("user", "Start");
        let e1_id = mgr.append_message(e1.message.clone());

        let e2 = entry_with_parent("assistant", "Response", &e1_id);
        let e2_id = mgr.append_message(e2.message.clone());

        // Fork from e2: Branch A
        let a = entry_with_parent("user", "Branch A question", &e2_id);
        let a_id = mgr.append_message(a.message.clone());

        // Fork from e2: Branch B
        let b = entry_with_parent("user", "Branch B question", &e2_id);
        let _b_id = mgr.append_message(b.message.clone());

        // Both should have e2 as parent
        assert_eq!(mgr.get_entry(&a_id).unwrap().parent_id, Some(e2_id));
    }

    #[test]
    fn test_session_branch_independence() {
        let mut mgr = SessionManager::in_memory("/tmp/test");

        let base_id = mgr.append_message(msg("assistant", "Base"));

        let a_id = mgr.append_message(msg("user", "Branch A"));
        let b_id = mgr.append_message(msg("user", "Branch B"));

        // Each branch has independent content
        let a_content = mgr.get_entry(&a_id).unwrap().content();
        let b_content = mgr.get_entry(&b_id).unwrap().content();
        assert_ne!(a_content, b_content);
    }

    #[test]
    fn test_validate_session_id_valid() {
        assert!(SessionManager::validate_session_id(
            &uuid::Uuid::new_v4().to_string()
        ));
        assert!(SessionManager::validate_session_id(
            "00000000-0000-0000-0000-000000000000"
        ));
    }

    #[test]
    fn test_validate_session_id_invalid() {
        assert!(!SessionManager::validate_session_id(""));
        assert!(!SessionManager::validate_session_id(
            "550e8400-e29b-41d4-a716"
        ));
        assert!(!SessionManager::validate_session_id("not-a-uuid-at-all"));
        assert!(!SessionManager::validate_session_id(
            "550e8400-e29b-41d4-a716-446655440000-extra"
        ));
    }

    #[test]
    fn test_parent_chain() {
        let mut mgr = SessionManager::in_memory("/tmp/test");

        // Create a 3-entry chain
        let e1 = entry("user", "Root");
        let e1_id = mgr.append_message(e1.message.clone());

        let e2 = entry_with_parent("assistant", "Middle", &e1_id);
        let e2_id = mgr.append_message(e2.message.clone());

        let e3 = entry_with_parent("user", "Leaf", &e2_id);
        let _e3_id = mgr.append_message(e3.message.clone());

        // Verify chain
        assert!(mgr.get_entry(&e2_id).unwrap().parent_id == Some(e1_id));
    }

    #[test]
    fn test_session_get_nonexistent() {
        let mgr = SessionManager::in_memory("/tmp/test");
        assert!(mgr.get_entry("nonexistent-id").is_none());
    }

    #[test]
    fn test_session_empty_manager() {
        let mgr = SessionManager::in_memory("/tmp/test");
        assert!(mgr.get_entry("anything").is_none());
    }
}
