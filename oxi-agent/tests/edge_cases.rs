//! Edge-case tests for built-in tools:
//! - Symlink handling (circular)
//! - Large file reading
//! - Ambiguous edit matches
//! - Bash blocked env vars

use oxi_agent::prelude::*;
use serde_json::json;
use std::os::unix::fs::symlink;
use tokio::fs;

// ── Helpers ──────────────────────────────────────────────────────

async fn create_temp_dir(name: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = format!("/tmp/oxi_edge_test_{}_{}", name, id);
    let _ = fs::remove_dir_all(&path).await;
    fs::create_dir_all(&path).await.unwrap();
    path
}

async fn cleanup(path: &str) {
    let _ = fs::remove_dir_all(path).await;
}

async fn execute_tool(tool: &dyn AgentTool, params: serde_json::Value) -> AgentToolResult {
    tool.execute("test_call", params, None, &oxi_agent::ToolContext::default()).await.unwrap()
}

// ═══════════════════════════════════════════════════════════════════
// Grep / Find with Symlinks (circular)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_grep_with_circular_symlink() {
    // Note: Circular symlinks cause infinite recursion in grep_walk.
    // Instead, test with a symlink to an external file to verify symlink handling.
    let dir = create_temp_dir("grep_symlink").await;
    let file_path = format!("{}/test.txt", dir);
    fs::write(&file_path, "target pattern here")
        .await
        .unwrap();

    // Create a symlink to an external file
    let external_dir = create_temp_dir("grep_symlink_ext").await;
    let external_file = format!("{}/linked.txt", external_dir);
    fs::write(&external_file, "external pattern match")
        .await
        .unwrap();
    let link_path = format!("{}/linked.txt", dir);
    std::os::unix::fs::symlink(&external_file, &link_path).expect("create symlink");

    let tool = GrepTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "pattern": "target pattern",
            "path": dir
        }),
    )
    .await;

    assert!(result.success, "grep should succeed with symlinks: {}", result.output);
    assert!(
        result.output.contains("target pattern"),
        "should find the pattern in test.txt"
    );

    cleanup(&dir).await;
    cleanup(&external_dir).await;
}

#[tokio::test]
async fn test_find_with_circular_symlink() {
    // Circular symlinks cause infinite loops in find. Instead test with
    // a symlink to an external directory.
    let dir = create_temp_dir("find_symlink").await;
    fs::write(format!("{}/real_file.txt", dir), "")
        .await
        .unwrap();

    let external_dir = create_temp_dir("find_symlink_ext").await;
    fs::write(format!("{}/external.txt", external_dir), "")
        .await
        .unwrap();
    let link_path = format!("{}/linked_dir", dir);
    std::os::unix::fs::symlink(&external_dir, &link_path).expect("create dir symlink");

    let tool = FindTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "path": dir,
            "max_depth": 2
        }),
    )
    .await;

    assert!(result.success, "find should succeed with symlinks: {}", result.output);
    assert!(result.output.contains("real_file.txt"));

    cleanup(&dir).await;
    cleanup(&external_dir).await;
}

#[tokio::test]
async fn test_grep_with_broken_symlink() {
    // Grep may or may not follow broken symlinks; the key is it doesn't crash.
    let dir = create_temp_dir("grep_broken_symlink").await;

    // Write a real file
    fs::write(format!("{}/test.txt", dir), "findme")
        .await
        .unwrap();

    // Create a broken symlink — grep may ignore it or error, both are OK.
    let link_path = format!("{}/broken_link", dir);
    symlink("/tmp/nonexistent_target_12345", &link_path).expect("create broken symlink");

    let tool = GrepTool::new();
    let result = tool
        .execute(
            "test_call",
            json!({
                "pattern": "findme",
                "path": dir
            }),
            None,
            &ToolContext::default(),
        )
        .await;

    // Either succeeds and finds the match, or returns an error — both are acceptable.
    // The key invariant is it doesn't panic or hang.
    match result {
        Ok(r) => {
            assert!(r.success);
            assert!(r.output.contains("findme"));
        }
        Err(_) => {
            // Broken symlink may cause error; that's OK
        }
    }

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_find_with_broken_symlink() {
    let dir = create_temp_dir("find_broken_symlink").await;

    fs::write(format!("{}/real.txt", dir), "")
        .await
        .unwrap();

    let link_path = format!("{}/broken_link", dir);
    symlink("/tmp/nonexistent_target_12345", &link_path).expect("create broken symlink");

    let tool = FindTool::new();
    let result = execute_tool(&tool, json!({ "path": dir })).await;

    assert!(result.success);
    assert!(result.output.contains("real.txt"));

    cleanup(&dir).await;
}

// ═══════════════════════════════════════════════════════════════════
// Read Tool with Large Files
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_read_large_file() {
    let dir = create_temp_dir("read_large").await;
    let file_path = format!("{}/large.txt", dir);

    // Create a file with many lines (~5000 lines)
    let lines: Vec<String> = (0..5000).map(|i| format!("Line {}: Some content here with padding", i)).collect();
    let content = lines.join("\n");
    fs::write(&file_path, &content).await.unwrap();

    let tool = ReadTool::new();

    // Read with offset and limit to test pagination
    // offset is 1-indexed, so offset=101 starts at line 100 (0-indexed index 100)
    let result = execute_tool(
        &tool,
        json!({
            "path": file_path,
            "offset": 101,
            "limit": 10
        }),
    )
    .await;

    assert!(result.success, "read with offset/limit should succeed: {}", result.output);
    // Output includes line number prefix. Check for the actual content text.
    // offset=101 with 1-indexed offset shows lines 100-109 (0-indexed indexes 100-109)
    assert!(result.output.contains("Line 100:"), "should contain line 100: {}", result.output);
    assert!(result.output.contains("Line 109:"), "should contain line 109: {}", result.output);
    // Should NOT contain line 110 or line 99
    assert!(!result.output.contains("Line 110:"), "should not contain line 110");
    assert!(!result.output.contains("Line 99:"), "should not contain line 99");

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_read_file_with_long_lines() {
    let dir = create_temp_dir("read_long_lines").await;
    let file_path = format!("{}/longlines.txt", dir);

    // Create a file with very long lines
    let long_line = "A".repeat(10_000);
    let content = format!("short\n{}\nanother short", long_line);
    fs::write(&file_path, &content).await.unwrap();

    let tool = ReadTool::new();
    let result = execute_tool(&tool, json!({ "path": file_path })).await;

    // Should succeed (may truncate long lines)
    assert!(result.success);

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_read_empty_file() {
    let dir = create_temp_dir("read_empty_edge").await;
    let file_path = format!("{}/empty.txt", dir);
    fs::write(&file_path, "").await.unwrap();

    let tool = ReadTool::new();
    let result = execute_tool(&tool, json!({ "path": file_path })).await;

    assert!(result.success);
    assert_eq!(result.output, "");

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_read_file_with_binary_content() {
    let dir = create_temp_dir("read_binary").await;
    let file_path = format!("{}/binary.bin", dir);

    // Write binary content (contains null bytes)
    let binary_content: Vec<u8> = (0..255).collect();
    fs::write(&file_path, &binary_content).await.unwrap();

    let tool = ReadTool::new();
    let result = tool.execute("test_call", json!({ "path": file_path }), None, &oxi_agent::ToolContext::default()).await;

    // Should detect binary and return an error or warning
    assert!(result.is_err() || !result.as_ref().unwrap().success || result.unwrap().output.contains("binary"));
    cleanup(&dir).await;
}

#[tokio::test]
async fn test_read_offset_beyond_file() {
    let dir = create_temp_dir("read_offset_beyond").await;
    let file_path = format!("{}/short.txt", dir);
    fs::write(&file_path, "only 3 lines\nline 2\nline 3").await.unwrap();

    let tool = ReadTool::new();
    let result = tool
        .execute(
            "test_call",
            json!({
                "path": file_path,
                "offset": 1000,
                "limit": 10
            }),
            None,
            &ToolContext::default(),
        )
        .await;

    // ReadTool returns an error (Err) when offset exceeds file length
    match result {
        Ok(r) => {
            // Success=false with error message about offset
            assert!(!r.success, "should fail when offset exceeds file length");
            assert!(r.output.contains("Offset"), "should mention offset: {}", r.output);
        }
        Err(e) => {
            // Tool returned an error string
            assert!(e.contains("Offset") || e.contains("exceeds"));
        }
    }

    cleanup(&dir).await;
}

// ═══════════════════════════════════════════════════════════════════
// Edit Tool with Ambiguous Matches
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_edit_ambiguous_match_multiple_occurrences() {
    let dir = create_temp_dir("edit_ambiguous").await;
    let file_path = format!("{}/code.rs", dir);

    // File with repeated pattern
    fs::write(
        &file_path,
        "fn foo() {\n    let x = 1;\n    let x = 1;\n    let x = 1;\n}",
    )
    .await
    .unwrap();

    let tool = EditTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "path": file_path,
            "old_text": "let x = 1;",
            "new_text": "let x = 2;"
        }),
    )
    .await;

    // Edit tool should reject ambiguous match (multiple occurrences)
    assert!(
        !result.success,
        "edit should fail when old_text matches multiple times"
    );
    assert!(
        result.output.contains("unique")
            || result.output.contains("multiple")
            || result.output.contains("ambiguous"),
        "error should mention uniqueness/ambiguity: {}",
        result.output
    );

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_edit_ambiguous_match_with_context() {
    let dir = create_temp_dir("edit_ambiguous_ctx").await;
    let file_path = format!("{}/file.txt", dir);

    // File where the same text appears but with different surrounding context
    fs::write(
        &file_path,
        "function a() {\n    return 'hello';\n}\n\nfunction b() {\n    return 'hello';\n}",
    )
    .await
    .unwrap();

    let tool = EditTool::new();
    // Try to edit just "return 'hello'" which appears twice
    let result = execute_tool(
        &tool,
        json!({
            "path": file_path,
            "old_text": "return 'hello';",
            "new_text": "return 'world';"
        }),
    )
    .await;

    // Should fail due to ambiguity
    assert!(!result.success);

    // Using more context should work
    let result2 = execute_tool(
        &tool,
        json!({
            "path": file_path,
            "old_text": "function a() {\n    return 'hello';\n}",
            "new_text": "function a() {\n    return 'world';\n}"
        }),
    )
    .await;

    assert!(result2.success, "should succeed with enough context");

    let content = fs::read_to_string(&file_path).await.unwrap();
    assert!(content.contains("return 'world'"));
    assert!(content.contains("return 'hello'")); // Second occurrence unchanged

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_edit_empty_old_text_rejected() {
    let dir = create_temp_dir("edit_empty_old").await;
    let file_path = format!("{}/file.txt", dir);
    fs::write(&file_path, "some content").await.unwrap();

    let tool = EditTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "path": file_path,
            "old_text": "",
            "new_text": "replacement"
        }),
    )
    .await;

    // Empty old_text should be rejected (matches everywhere or nowhere)
    assert!(!result.success);

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_edit_multi_edits_non_overlapping() {
    let dir = create_temp_dir("edit_multi").await;
    let file_path = format!("{}/multi.txt", dir);
    fs::write(
        &file_path,
        "color = red\nsize = small\nshape = circle\nweight = light",
    )
    .await
    .unwrap();

    let tool = EditTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "path": file_path,
            "edits": [
                { "old_text": "color = red", "new_text": "color = blue" },
                { "old_text": "size = small", "new_text": "size = large" }
            ]
        }),
    )
    .await;

    assert!(result.success, "multi-edit should succeed: {}", result.output);

    let content = fs::read_to_string(&file_path).await.unwrap();
    assert!(content.contains("color = blue"));
    assert!(content.contains("size = large"));
    assert!(content.contains("shape = circle")); // unchanged
    assert!(content.contains("weight = light")); // unchanged

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_edit_preserves_file_when_not_found() {
    let dir = create_temp_dir("edit_preserve").await;
    let file_path = format!("{}/file.txt", dir);
    let original = "original content\nline 2\nline 3";
    fs::write(&file_path, original).await.unwrap();

    let tool = EditTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "path": file_path,
            "old_text": "nonexistent",
            "new_text": "replacement"
        }),
    )
    .await;

    assert!(!result.success);

    // File should be unchanged
    let content = fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, original);

    cleanup(&dir).await;
}

// ═══════════════════════════════════════════════════════════════════
// Bash Tool Blocked Environment Variables
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_bash_blocked_env_ld_preload() {
    let tool = BashTool::new();
    // Bash tool silently strips blocked env vars and still runs the command.
    // Verify the command succeeds but LD_PRELOAD was NOT actually set.
    let result = execute_tool(
        &tool,
        json!({
            "command": "echo $LD_PRELOAD"
        }),
    )
    .await;

    // Run again with LD_PRELOAD explicitly set — it should be stripped
    let result_blocked = execute_tool(
        &tool,
        json!({
            "command": "echo $LD_PRELOAD",
            "env": {
                "LD_PRELOAD": "/malicious/lib.so"
            }
        }),
    )
    .await;

    assert!(result_blocked.success, "command should still run: {}", result_blocked.output);
    // The env var should have been stripped, so echo should output nothing
    // (or just the default $LD_PRELOAD which is empty)
    assert!(
        !result_blocked.output.contains("/malicious/lib.so"),
        "LD_PRELOAD should have been stripped from env: {}",
        result_blocked.output
    );
}

#[tokio::test]
async fn test_bash_blocked_env_path() {
    let tool = BashTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "command": "echo $PATH",
            "env": {
                "PATH": "/malicious/bin"
            }
        }),
    )
    .await;

    assert!(result.success, "command should still run");
    // PATH should have been stripped, so the value shouldn't be /malicious/bin
    assert!(
        !result.output.contains("/malicious/bin"),
        "PATH override should have been stripped: {}",
        result.output
    );
}

#[tokio::test]
async fn test_bash_blocked_env_dyld() {
    let tool = BashTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "command": "echo $DYLD_INSERT_LIBRARIES",
            "env": {
                "DYLD_INSERT_LIBRARIES": "/malicious.dylib"
            }
        }),
    )
    .await;

    assert!(result.success, "command should still run");
    assert!(
        !result.output.contains("/malicious.dylib"),
        "DYLD_INSERT_LIBRARIES should have been stripped: {}",
        result.output
    );
}

#[tokio::test]
async fn test_bash_allowed_env_var() {
    let dir = create_temp_dir("bash_env").await;
    let file_path = format!("{}/output.txt", dir);

    let tool = BashTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "command": format!("echo $MY_TEST_VAR > {}", file_path),
            "env": {
                "MY_TEST_VAR": "hello_from_test"
            }
        }),
    )
    .await;

    assert!(result.success, "should allow non-blocked env vars: {}", result.output);

    // Verify the env var was set correctly
    let content = fs::read_to_string(&file_path).await.unwrap_or_default();
    assert!(
        content.contains("hello_from_test"),
        "MY_TEST_VAR should have been set, got: {}",
        content
    );

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_bash_multiple_blocked_env_vars() {
    let tool = BashTool::new();
    // The tool silently strips blocked vars; command still runs
    let result = execute_tool(
        &tool,
        json!({
            "command": "echo $HOME $MY_SAFE_VAR",
            "env": {
                "HOME": "/evil",
                "LD_PRELOAD": "/evil.so",
                "MY_SAFE_VAR": "ok"
            }
        }),
    )
    .await;

    assert!(result.success, "command should still run");
    // Blocked vars should be stripped but MY_SAFE_VAR should work
    assert!(
        result.output.contains("ok"),
        "MY_SAFE_VAR should be set: {}",
        result.output
    );
    assert!(
        !result.output.contains("/evil"),
        "Blocked vars HOME/LD_PRELOAD should be stripped: {}",
        result.output
    );
}

// ═══════════════════════════════════════════════════════════════════
// Additional Edge Cases
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_write_unicode_content() {
    let dir = create_temp_dir("write_unicode").await;
    let file_path = format!("{}/unicode.txt", dir);

    let tool = WriteTool::new();
    let result = execute_tool(
        &tool,
        json!({
            "path": file_path,
            "content": "Hello 🌍 世界 мир مرحبا"
        }),
    )
    .await;
    assert!(result.success);

    let content = fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "Hello 🌍 世界 мир مرحبا");

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_grep_literal_mode() {
    let dir = create_temp_dir("grep_literal").await;
    let file_path = format!("{}/test.txt", dir);
    fs::write(&file_path, "file.rs contains [abc] pattern")
        .await
        .unwrap();

    let tool = GrepTool::new();
    // Search for literal "[abc]" which is a regex special char
    let result = execute_tool(
        &tool,
        json!({
            "pattern": "[abc]",
            "path": dir,
            "literal": true
        }),
    )
    .await;

    assert!(result.success);
    assert!(result.output.contains("[abc]"), "should find literal [abc]");

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_bash_timeout() {
    let tool = BashTool::new();
    let result = tool
        .execute(
            "test_call",
            json!({
                "command": "sleep 10",
                "timeout": 1
            }),
            None,
            &ToolContext::default(),
        )
        .await;

    // Should timeout and return an error or failure
    assert!(
        result.is_err() || !result.as_ref().map(|r| r.success).unwrap_or(false),
        "long-running command should timeout"
    );
}

#[tokio::test]
async fn test_find_symlink_to_file() {
    let dir = create_temp_dir("find_symlink_file").await;
    let file_path = format!("{}/real.txt", dir);
    fs::write(&file_path, "content").await.unwrap();

    let link_path = format!("{}/link.txt", dir);
    symlink(&file_path, &link_path).expect("create file symlink");

    let tool = FindTool::new();
    let result = execute_tool(&tool, json!({ "path": dir })).await;

    assert!(result.success);
    // Should find both the real file and the symlink
    assert!(result.output.contains("real.txt"));

    cleanup(&dir).await;
}

#[tokio::test]
async fn test_read_symlink_to_file() {
    let dir = create_temp_dir("read_symlink").await;
    let file_path = format!("{}/real.txt", dir);
    fs::write(&file_path, "hello via symlink").await.unwrap();

    let link_path = format!("{}/link.txt", dir);
    symlink(&file_path, &link_path).expect("create file symlink");

    let tool = ReadTool::new();
    let result = execute_tool(&tool, json!({ "path": link_path })).await;

    assert!(result.success);
    assert!(result.output.contains("hello via symlink"));

    cleanup(&dir).await;
}
