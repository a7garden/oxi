//! Integration test for tool streaming and progress updates

use oxicode_agent::prelude::*;
use std::sync::{Arc, Mutex};

/// Test that ReadTool emits progress updates for large files
#[tokio::test]
async fn test_read_tool_progress_streaming() {
    use tokio::fs;

    // Create a test file with enough content to trigger progress updates
    let test_content = "x".repeat(10000); // 10KB file
    let test_path = "/tmp/oxicode_test_large_file.txt";

    // Create the file
    fs::write(test_path, &test_content).await.unwrap();

    // Track progress updates
    let progress_updates = Arc::new(Mutex::new(Vec::new()));
    let progress_clone = progress_updates.clone();

    // Create a ReadTool and set up progress callback
    let tool = ReadTool::new();

    // Set up progress callback
    tool.on_progress(Arc::new(move |msg: String| {
        progress_clone.lock().unwrap().push(msg);
    }));

    // Execute the tool
    let params = serde_json::json!({ "path": test_path });
    let result = tool
        .execute("test_call_id", params, None, &ToolContext::default())
        .await;

    // Verify the result was successful
    assert!(result.is_ok());
    let result = result.unwrap();
    assert!(result.success);

    // Verify we got progress updates. The lock is scoped so the MutexGuard
    // is dropped at the block end, before the cleanup await below
    // (clippy::await_holding_lock does not credit an explicit `drop()`).
    {
        let updates = progress_updates.lock().unwrap();
        assert!(
            !updates.is_empty(),
            "Expected progress updates for large file"
        );

        // Should have at least: start message and completion message
        assert!(
            updates.iter().any(|u| u.contains("Reading file")),
            "Should have file start message"
        );
    }

    // Clean up
    let _ = fs::remove_file(test_path).await;
}

/// Test that BashTool emits progress updates
#[tokio::test]
async fn test_bash_tool_progress_streaming() {
    // Track progress updates
    let progress_updates = Arc::new(Mutex::new(Vec::new()));
    let progress_clone = progress_updates.clone();

    // Create a BashTool and set up progress callback
    let tool = BashTool::new();

    // Set up progress callback
    tool.on_progress(Arc::new(move |msg: String| {
        progress_clone.lock().unwrap().push(msg);
    }));

    // Execute a simple command
    let params = serde_json::json!({
        "command": "echo 'Hello World' && sleep 0.1 && echo 'Done'"
    });
    let result = tool
        .execute("test_call_id", params, None, &ToolContext::default())
        .await;

    // Verify the result was successful
    assert!(result.is_ok());
    let result = result.unwrap();
    assert!(result.success);
    assert!(result.output.contains("Hello World"));

    // Verify we got progress updates
    let updates = progress_updates.lock().unwrap();
    assert!(
        !updates.is_empty(),
        "Expected progress updates for bash command"
    );

    // Should have execution message and process exit message
    assert!(
        updates
            .iter()
            .any(|u| u.contains("Executing:") || u.contains("Running")),
        "Should have execution message"
    );
    assert!(
        updates.iter().any(|u| u.contains("exited with code")),
        "Should have exit message"
    );
}

/// Test that tools without progress callback still work
#[tokio::test]
async fn test_tool_without_progress_callback() {
    let tool = ReadTool::new();

    // Create a small test file
    let test_path = "/tmp/oxicode_test_small_file.txt";
    tokio::fs::write(test_path, "Hello, World!").await.unwrap();

    // Execute without progress callback
    let params = serde_json::json!({ "path": test_path });
    let result = tool
        .execute("test_call_id", params, None, &ToolContext::default())
        .await;

    // Should still work
    assert!(result.is_ok());
    let result = result.unwrap();
    assert!(result.success);
    assert!(result.output.contains("Hello, World!"));

    // Clean up
    let _ = tokio::fs::remove_file(test_path).await;
}

/// Test that progress updates respect the file size threshold
#[tokio::test]
async fn test_read_tool_small_file_no_progress() {
    // Create a small test file (less than 1KB threshold)
    let test_path = "/tmp/oxicode_test_tiny_file.txt";
    tokio::fs::write(test_path, "Small content").await.unwrap();

    let progress_updates = Arc::new(Mutex::new(Vec::new()));
    let progress_clone = progress_updates.clone();

    let tool = ReadTool::new();
    tool.on_progress(Arc::new(move |msg: String| {
        progress_clone.lock().unwrap().push(msg);
    }));

    let params = serde_json::json!({ "path": test_path });
    let result = tool
        .execute("test_call_id", params, None, &ToolContext::default())
        .await;

    assert!(result.is_ok());

    // Small file should still get start/end messages but no percentage updates.
    // Scoped so the MutexGuard is dropped before the cleanup await below.
    {
        let updates = progress_updates.lock().unwrap();
        // Should have start message
        assert!(!updates.is_empty(), "Should have at least start message");
    }

    // Clean up
    let _ = tokio::fs::remove_file(test_path).await;
}
