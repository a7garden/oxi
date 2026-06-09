/// Edit file tool - make targeted edits to files
/// Supports:
/// - Multiple non-overlapping edits in one call (`edits[]` array)
/// - Legacy `old_text`/`new_text` single-edit mode
/// - BOM detection and preservation
/// - Line ending normalization (CRLF → LF for matching)
/// - Unified diff output for previews
/// - File mutation queue for concurrent write safety
use super::edit_diff::{
    self, Edit, EditDiffError, detect_line_ending, has_bom, normalize_to_lf, restore_line_endings,
    strip_bom,
};
use super::file_mutation_queue::global_mutation_queue;
use super::path_security::PathGuard;
use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::oneshot;

/// EditTool.
pub struct EditTool {
    root_dir: Option<PathBuf>,
}

impl EditTool {
    /// Create with no explicit root (uses ToolContext.workspace_dir at runtime).
    pub fn new() -> Self {
        Self { root_dir: None }
    }

    /// Create with a specific working directory (overrides ToolContext).
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self {
            root_dir: Some(cwd),
        }
    }

    /// Prepare edit arguments, handling both legacy and multi-edit formats.
    /// Some models send edits as a JSON string instead of an array.
    fn prepare_arguments(params: &Value) -> EditInput {
        let path = params
            .get("path")
            .or(params.get("file_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Try to parse edits array
        let mut edits: Vec<EditEntry> = Vec::new();

        if let Some(edits_val) = params.get("edits") {
            // Some models send edits as a JSON string
            let edits_val = if let Some(s) = edits_val.as_str() {
                serde_json::from_str::<Vec<EditEntry>>(s).unwrap_or_default()
            } else if let Some(arr) = edits_val.as_array() {
                arr.iter()
                    .filter_map(|v| serde_json::from_value::<EditEntry>(v.clone()).ok())
                    .collect()
            } else {
                Vec::new()
            };
            edits = edits_val;
        }

        // Legacy mode: old_text + new_text
        if edits.is_empty()
            && let (Some(old), Some(new)) = (
                params
                    .get("old_text")
                    .or(params.get("oldText"))
                    .and_then(|v| v.as_str()),
                params
                    .get("new_text")
                    .or(params.get("newText"))
                    .and_then(|v| v.as_str()),
            )
        {
            edits.push(EditEntry {
                old_text: old.to_string(),
                new_text: new.to_string(),
            });
        }

        let dry_run = params
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let expected_hash = params
            .get("expected_hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        EditInput {
            path,
            edits,
            dry_run,
            expected_hash,
        }
    }

    /// Apply edits to a file with full BOM/line-ending handling and diff output.
    async fn apply_edits(root_dir: &Path, input: &EditInput) -> Result<EditOutput, ToolError> {
        // Security: validate path with PathGuard
        let guard = PathGuard::new(root_dir);
        let validated_path = guard
            .validate_traversal(Path::new(&input.path))
            .map_err(|e| e.to_string())?;
        let path = validated_path.as_path();

        // Validate edits
        if input.edits.is_empty() {
            return Err(
                "No edits provided. Either use old_text/new_text or edits array.".to_string(),
            );
        }

        // Content-based conflict detection
        // Note: We do a synchronous read for the hash check, then reuse the
        // content below via the async read. The gap is minimal and the
        // file_mutation_queue serializes concurrent edits to the same file.
        if let Some(ref expected) = input.expected_hash {
            let current_content = std::fs::read_to_string(path)
                .map_err(|e| format!("Failed to read file for hash check: {}", e))?;
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            current_content.hash(&mut hasher);
            let current_hash = format!("{:016x}", hasher.finish());
            if current_hash != *expected {
                return Ok(EditOutput {
                    diff: String::new(),
                    first_changed_line: None,
                    applied: false,
                    message: "File has been modified since last read. Re-read the file and retry."
                        .to_string(),
                });
            }
        }

        // Read file content
        let raw_content = fs::read_to_string(path)
            .await
            .map_err(|e| format!("Cannot read file '{}': {}", input.path, e))?;

        // Detect BOM and line endings
        let had_bom = has_bom(&raw_content);
        let line_ending = detect_line_ending(&raw_content);
        let content = normalize_to_lf(strip_bom(&raw_content));

        // Convert to Edit structs with normalized text
        let edits: Vec<Edit> = input
            .edits
            .iter()
            .map(|e| Edit {
                old_text: normalize_to_lf(&e.old_text),
                new_text: normalize_to_lf(&e.new_text),
            })
            .collect();

        // Compute diff for preview
        let diff_result = edit_diff::generate_diff_string(&content, &edits, 4)
            .map_err(|e: EditDiffError| e.message)?;

        if input.dry_run {
            return Ok(EditOutput {
                diff: diff_result.diff,
                first_changed_line: diff_result.first_changed_line,
                applied: false,
                message: "Dry run — no changes applied".to_string(),
            });
        }

        // Apply edits to normalized content
        let modified = edit_diff::apply_edits_to_normalized_content(&content, &edits)
            .map_err(|e: EditDiffError| e.message)?;

        // Restore line endings and BOM
        let mut final_content = restore_line_endings(&modified, line_ending);
        if had_bom {
            final_content = format!("\u{feff}{}", final_content);
        }

        // Write through mutation queue (serializes per-file)
        let final_content_clone = final_content.clone();
        global_mutation_queue()
            .with_queue(path, || async {
                fs::write(&validated_path, &final_content_clone)
                    .await
                    .map_err(|e| format!("Cannot write file '{}': {}", validated_path.display(), e))
            })
            .await
            .map_err(|e: String| e)?;

        Ok(EditOutput {
            diff: diff_result.diff,
            first_changed_line: diff_result.first_changed_line,
            applied: true,
            message: format!("Applied {} edit(s) to {}", edits.len(), input.path),
        })
    }
}

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Parsed edit input
struct EditInput {
    path: String,
    edits: Vec<EditEntry>,
    dry_run: bool,
    /// Hash of file content at last read. If provided, edit will be
    /// rejected if the file has been modified since.
    expected_hash: Option<String>,
}

/// A single edit entry
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EditEntry {
    #[serde(rename = "oldText", alias = "old_text")]
    old_text: String,
    #[serde(rename = "newText", alias = "new_text")]
    new_text: String,
}

/// Result of edit operation
#[derive(Debug)]

struct EditOutput {
    diff: String,
    first_changed_line: Option<usize>,
    #[allow(dead_code)]
    applied: bool,
    message: String,
}

#[async_trait]
impl AgentTool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn label(&self) -> &str {
        "Edit File"
    }

    fn essential(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Make targeted edits to a file. Supports both single edit (old_text/new_text) and multiple edits (edits[] array). \
         Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. \
         If two changes touch the same block or nearby lines, merge them into one edit instead. \
         Use dry_run=true to preview without making changes."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative or absolute)"
                },
                "edits": {
                    "type": "array",
                    "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {
                                "type": "string",
                                "description": "Exact text for one targeted replacement. Must be unique in the original file."
                            },
                            "newText": {
                                "type": "string",
                                "description": "Replacement text for this targeted edit."
                            }
                        },
                        "required": ["oldText", "newText"]
                    }
                },
                "old_text": {
                    "type": "string",
                    "description": "Legacy: exact text to replace (use edits[] instead for new code)"
                },
                "new_text": {
                    "type": "string",
                    "description": "Legacy: replacement text (use edits[] instead for new code)"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "If true, preview the change without applying it",
                    "default": false
                },
                "expected_hash": {
                    "type": "string",
                    "description": "Hash of the file content at last read. If provided, the edit will be rejected if the file was modified since the hash was computed."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let input = Self::prepare_arguments(&params);

            // Use root_dir if set, else ctx.root()
            let root = self.root_dir.as_deref().unwrap_or(ctx.root());

            match Self::apply_edits(root, &input).await {
                Ok(output) => {
                    let mut result =
                        AgentToolResult::success(format!("{}\n\n{}", output.message, output.diff));

                    // Add metadata with first changed line for editor navigation
                    if let Some(line) = output.first_changed_line {
                        result = result.with_metadata(json!({
                            "firstChangedLine": line,
                        }));
                    }

                    Ok(result)
                }
                Err(e) => Ok(AgentToolResult::error(e)),
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prepare_arguments_legacy() {
        let params = json!({
            "path": "/tmp/test.txt",
            "old_text": "hello",
            "new_text": "world"
        });
        let input = EditTool::prepare_arguments(&params);
        assert_eq!(input.path, "/tmp/test.txt");
        assert_eq!(input.edits.len(), 1);
        assert_eq!(input.edits[0].old_text, "hello");
        assert_eq!(input.edits[0].new_text, "world");
        assert!(!input.dry_run);
    }

    #[test]
    fn test_prepare_arguments_multi_edit() {
        let params = json!({
            "path": "/tmp/test.txt",
            "edits": [
                {"oldText": "foo", "newText": "bar"},
                {"oldText": "baz", "newText": "qux"}
            ]
        });
        let input = EditTool::prepare_arguments(&params);
        assert_eq!(input.edits.len(), 2);
    }

    #[test]
    fn test_prepare_arguments_edits_as_string() {
        let params = json!({
            "path": "/tmp/test.txt",
            "edits": "[{\"oldText\":\"a\",\"newText\":\"b\"}]"
        });
        let input = EditTool::prepare_arguments(&params);
        assert_eq!(input.edits.len(), 1);
        assert_eq!(input.edits[0].old_text, "a");
    }

    #[test]
    fn test_prepare_arguments_dry_run() {
        let params = json!({
            "path": "/tmp/test.txt",
            "old_text": "hello",
            "new_text": "world",
            "dry_run": true
        });
        let input = EditTool::prepare_arguments(&params);
        assert!(input.dry_run);
    }

    #[tokio::test]
    async fn test_apply_edits_file_not_found() {
        let input = EditInput {
            path: "/tmp/nonexistent_file_12345.txt".to_string(),
            edits: vec![EditEntry {
                old_text: "foo".to_string(),
                new_text: "bar".to_string(),
            }],
            dry_run: false,
            expected_hash: None,
        };
        let result = EditTool::apply_edits(Path::new("."), &input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot read file"));
    }

    #[tokio::test]
    async fn test_apply_edits_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world\n").await.unwrap();

        let input = EditInput {
            path: file_path.to_str().unwrap().to_string(),
            edits: vec![EditEntry {
                old_text: "hello".to_string(),
                new_text: "goodbye".to_string(),
            }],
            dry_run: true,
            expected_hash: None,
        };
        let output = EditTool::apply_edits(Path::new("."), &input).await.unwrap();
        assert!(!output.applied);
        assert!(output.diff.contains("-hello"));
        assert!(output.diff.contains("+goodbye"));

        // Verify file was not modified
        let content = fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "hello world\n");
    }

    #[tokio::test]
    async fn test_apply_edits_single_edit() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world\nfoo bar\n")
            .await
            .unwrap();

        let input = EditInput {
            path: file_path.to_str().unwrap().to_string(),
            edits: vec![EditEntry {
                old_text: "hello".to_string(),
                new_text: "goodbye".to_string(),
            }],
            dry_run: false,
            expected_hash: None,
        };
        let output = EditTool::apply_edits(Path::new("."), &input).await.unwrap();
        assert!(output.applied);
        assert!(output.message.contains("1 edit(s)"));

        let content = fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "goodbye world\nfoo bar\n");
    }

    #[tokio::test]
    async fn test_apply_edits_multiple_edits() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "aaa\nbbb\nccc\n").await.unwrap();

        let input = EditInput {
            path: file_path.to_str().unwrap().to_string(),
            edits: vec![
                EditEntry {
                    old_text: "aaa".to_string(),
                    new_text: "AAA".to_string(),
                },
                EditEntry {
                    old_text: "ccc".to_string(),
                    new_text: "CCC".to_string(),
                },
            ],
            dry_run: false,
            expected_hash: None,
        };
        let output = EditTool::apply_edits(Path::new("."), &input).await.unwrap();
        assert!(output.applied);
        assert!(output.message.contains("2 edit(s)"));

        let content = fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "AAA\nbbb\nCCC\n");
    }

    #[tokio::test]
    async fn test_apply_edits_crlf_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello\r\nworld\r\n").await.unwrap();

        let input = EditInput {
            path: file_path.to_str().unwrap().to_string(),
            edits: vec![EditEntry {
                old_text: "hello".to_string(),
                new_text: "goodbye".to_string(),
            }],
            dry_run: false,
            expected_hash: None,
        };
        EditTool::apply_edits(Path::new("."), &input).await.unwrap();

        let content = fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "goodbye\r\nworld\r\n");
    }

    #[tokio::test]
    async fn test_apply_edits_bom_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "\u{feff}hello world\n")
            .await
            .unwrap();

        let input = EditInput {
            path: file_path.to_str().unwrap().to_string(),
            edits: vec![EditEntry {
                old_text: "hello".to_string(),
                new_text: "goodbye".to_string(),
            }],
            dry_run: false,
            expected_hash: None,
        };
        EditTool::apply_edits(Path::new("."), &input).await.unwrap();

        let content = fs::read_to_string(&file_path).await.unwrap();
        assert!(content.starts_with('\u{feff}'));
        assert!(content.contains("goodbye"));
    }

    #[test]
    fn test_prepare_arguments_expected_hash() {
        let params = json!({
            "path": "/tmp/test.txt",
            "old_text": "hello",
            "new_text": "world",
            "expected_hash": "abcd1234"
        });
        let input = EditTool::prepare_arguments(&params);
        assert_eq!(input.expected_hash.as_deref(), Some("abcd1234"));
    }

    #[test]
    fn test_prepare_arguments_no_expected_hash() {
        let params = json!({
            "path": "/tmp/test.txt",
            "old_text": "hello",
            "new_text": "world"
        });
        let input = EditTool::prepare_arguments(&params);
        assert!(input.expected_hash.is_none());
    }

    fn compute_hash(content: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    #[tokio::test]
    async fn test_conflict_detection_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world\n").await.unwrap();

        let hash = compute_hash("hello world\n");

        // Modify the file after computing the hash
        fs::write(&file_path, "hello modified world\n")
            .await
            .unwrap();

        let input = EditInput {
            path: file_path.to_str().unwrap().to_string(),
            edits: vec![EditEntry {
                old_text: "hello".to_string(),
                new_text: "goodbye".to_string(),
            }],
            dry_run: false,
            expected_hash: Some(hash),
        };
        let output = EditTool::apply_edits(Path::new("."), &input).await.unwrap();
        assert!(!output.applied);
        assert!(output.message.contains("modified since last read"));

        // Verify file was NOT modified by the edit attempt
        let content = fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "hello modified world\n");
    }

    #[tokio::test]
    async fn test_conflict_detection_hash_match() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world\n").await.unwrap();

        let hash = compute_hash("hello world\n");

        let input = EditInput {
            path: file_path.to_str().unwrap().to_string(),
            edits: vec![EditEntry {
                old_text: "hello".to_string(),
                new_text: "goodbye".to_string(),
            }],
            dry_run: false,
            expected_hash: Some(hash),
        };
        let output = EditTool::apply_edits(Path::new("."), &input).await.unwrap();
        assert!(output.applied);

        let content = fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "goodbye world\n");
    }
}
