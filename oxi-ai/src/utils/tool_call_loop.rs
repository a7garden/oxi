//! Cross-turn tool-call loop guard — ported from omp
//! `packages/ai/src/utils/tool-call-loop-guard.ts`.
//!
//! MIT — attribution: adapted from
//! [omp](https://github.com/can1357/oh-my-pi) (Can Berk Güder, earendil-works).
//!
//! ## Purpose
//!
//! Models occasionally fixate on a single tool call: the same name and
//! arguments across consecutive turns, with the same (often unhelpful)
//! result. Left unchecked the loop burns the context window and the
//! user's quota indefinitely.
//!
//! [`ToolCallLoopGuard`] records each completed assistant turn and fires
//! when the *same* single-tool call (modulo argument key ordering) hits
//! a configurable threshold. Multi-call turns reset detection.
//!
//! Exempt tools (e.g. `read` for progressive file exploration, `ls` for
//! directory walks) bypass detection — their repetition is legitimate.

use serde_json::Value;

/// Runtime settings for cross-turn tool-call repetition detection.
#[derive(Debug, Clone)]
pub struct ToolCallLoopGuardOptions {
    /// Consecutive identical calls that trip the guard. Clamped to ≥ 1.
    pub threshold: usize,
    /// Tool names exempt from detection (e.g. `read`, `ls`, `grep`).
    pub exempt_tools: Vec<String>,
}

impl Default for ToolCallLoopGuardOptions {
    fn default() -> Self {
        Self {
            threshold: 5,
            exempt_tools: vec!["read".into(), "ls".into(), "grep".into()],
        }
    }
}

/// Details surfaced when a loop is recognised. Hosts can render these
/// into a steering message or abort the agent run.
#[derive(Debug, Clone, PartialEq)]
pub struct RepeatedToolCallDetection {
    /// The tool that was repeated.
    pub tool_name: String,
    /// Consecutive identical calls observed.
    pub count: usize,
    /// Truncated preview of the most recent tool result.
    pub result_summary: String,
    /// Truncated preview of the canonical arguments JSON.
    pub arguments_summary: String,
}

/// One completed assistant turn for the guard to evaluate.
#[derive(Debug, Clone)]
pub struct ToolCallLoopTurn<'a> {
    /// Tool calls in the assistant message. Only single-call turns are
    /// considered for repetition — multi-call turns reset detection.
    pub tool_calls: &'a [ToolCallRef],
    /// Results returned for the call with matching `tool_call_id`, in
    /// any order. Empty if none (the call is still in flight).
    pub tool_results: &'a [ToolResultRef],
}

/// A tool call as seen by the guard. Hosts adapt their provider-native
/// types into this shape.
#[derive(Debug, Clone)]
pub struct ToolCallRef {
    /// Tool-call identifier — used to match results to calls.
    pub id: String,
    /// Tool name (`read`, `write`, `bash`, …).
    pub name: String,
    /// JSON-encoded arguments. Canonicalised before hashing so key
    /// ordering does not matter.
    pub arguments: Value,
}

/// A tool result as seen by the guard.
#[derive(Debug, Clone)]
pub struct ToolResultRef {
    /// Matches the originating [`ToolCallRef::id`].
    pub tool_call_id: String,
    /// Plain-text content for the summary preview. Multi-part results
    /// are concatenated by the host before calling.
    pub content: String,
}

/// Maximum chars included in the per-detection result summary.
const RESULT_SUMMARY_LIMIT: usize = 200;
/// Maximum chars included in the per-detection arguments summary.
const ARGUMENT_SUMMARY_LIMIT: usize = 400;

/// Detects consecutive identical assistant tool calls across model
/// turns.
#[derive(Debug)]
pub struct ToolCallLoopGuard {
    threshold: usize,
    exempt_tools: std::collections::HashSet<String>,
    last_hash: Option<String>,
    count: usize,
}

impl Default for ToolCallLoopGuard {
    fn default() -> Self {
        Self::new(ToolCallLoopGuardOptions::default())
    }
}

impl ToolCallLoopGuard {
    /// Construct from options. `threshold` is clamped to ≥ 1.
    pub fn new(options: ToolCallLoopGuardOptions) -> Self {
        Self {
            threshold: options.threshold.max(1),
            exempt_tools: options.exempt_tools.into_iter().collect(),
            last_hash: None,
            count: 0,
        }
    }

    /// Override the threshold at runtime.
    pub fn with_threshold(mut self, threshold: usize) -> Self {
        self.threshold = threshold.max(1);
        self
    }

    /// Mark a tool as exempt from detection.
    pub fn with_exempt_tool(mut self, tool: impl Into<String>) -> Self {
        self.exempt_tools.insert(tool.into());
        self
    }

    /// Records one completed turn and returns the threshold hit, if any.
    ///
    /// A "completed turn" is one assistant message that contained tool
    /// calls **and** for which the corresponding tool results have been
    /// emitted. Turns with multiple distinct tool calls reset detection.
    pub fn record_turn(&mut self, turn: ToolCallLoopTurn<'_>) -> Option<RepeatedToolCallDetection> {
        // Only single-call turns are considered. Multi-call turns reset
        // detection (the model has clearly moved on to a different
        // request shape).
        if turn.tool_calls.len() != 1 {
            self.last_hash = None;
            self.count = 0;
            return None;
        }
        let tool_call = &turn.tool_calls[0];
        if self.exempt_tools.contains(&tool_call.name) {
            self.last_hash = None;
            self.count = 0;
            return None;
        }

        let canonical_args = canonicalize_json(&tool_call.arguments);
        let canonical_str =
            serde_json::to_string(&canonical_args).unwrap_or_else(|_| "<?>".to_string());
        let hash = format!("{}:{}", tool_call.name, canonical_str);

        if Some(&hash) == self.last_hash.as_ref() {
            self.count += 1;
        } else {
            self.last_hash = Some(hash);
            self.count = 1;
        }

        if self.count != self.threshold {
            return None;
        }

        Some(RepeatedToolCallDetection {
            tool_name: tool_call.name.clone(),
            count: self.count,
            result_summary: summarize_tool_result(turn.tool_results, &tool_call.id),
            arguments_summary: summarize_text(&canonical_str, ARGUMENT_SUMMARY_LIMIT),
        })
    }

    /// Reset state between unrelated runs (e.g. on session boundary).
    pub fn reset(&mut self) {
        self.last_hash = None;
        self.count = 0;
    }
}

/// Recursively sort object keys so JSON equality survives hash-map
/// insertion-order differences. Arrays preserve order.
fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            use serde_json::Map;
            let mut sorted: Vec<(String, Value)> = map
                .iter()
                .map(|(k, v)| (k.clone(), canonicalize_json(v)))
                .collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = Map::new();
            for (k, v) in sorted {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        _ => value.clone(),
    }
}

fn summarize_text(text: &str, limit: usize) -> String {
    let s = text.trim();
    if s.chars().count() <= limit {
        return s.to_string();
    }
    let truncated: String = s.chars().take(limit.saturating_sub(1)).collect();
    format!("{truncated}…")
}

fn summarize_tool_result(results: &[ToolResultRef], tool_call_id: &str) -> String {
    let matching = results
        .iter()
        .find(|r| r.tool_call_id == tool_call_id)
        .map(|r| r.content.clone())
        .unwrap_or_default();
    summarize_text(&matching, RESULT_SUMMARY_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, args: Value) -> ToolCallRef {
        ToolCallRef {
            id: format!("{name}-id"),
            name: name.into(),
            arguments: args,
        }
    }

    fn result_for(id: &str, content: &str) -> ToolResultRef {
        ToolResultRef {
            tool_call_id: id.into(),
            content: content.into(),
        }
    }

    #[test]
    fn default_threshold_is_five_with_read_ls_grep_exempt() {
        let g = ToolCallLoopGuard::default();
        assert_eq!(g.threshold, 5);
        assert!(g.exempt_tools.contains("read"));
        assert!(g.exempt_tools.contains("ls"));
        assert!(g.exempt_tools.contains("grep"));
    }

    #[test]
    fn fires_at_threshold_for_identical_single_call() {
        let mut g = ToolCallLoopGuard::new(ToolCallLoopGuardOptions {
            threshold: 3,
            exempt_tools: vec![],
        });
        let c = call("write", json!({"path": "/a", "content": "x"}));
        let r = result_for(&c.id, "ok");
        let turn = ToolCallLoopTurn {
            tool_calls: std::slice::from_ref(&c),
            tool_results: std::slice::from_ref(&r),
        };
        assert!(g.record_turn(turn.clone()).is_none());
        assert!(g.record_turn(turn.clone()).is_none());
        let hit = g.record_turn(turn).expect("third call should trip");
        assert_eq!(hit.tool_name, "write");
        assert_eq!(hit.count, 3);
        assert_eq!(hit.result_summary, "ok");
        assert!(hit.arguments_summary.contains("\"path\""));
    }

    #[test]
    fn exempt_tool_resets_state() {
        let mut g = ToolCallLoopGuard::new(ToolCallLoopGuardOptions {
            threshold: 2,
            exempt_tools: vec!["read".into()],
        });
        let c = call("read", json!({"path": "/a"}));
        let turn = ToolCallLoopTurn {
            tool_calls: std::slice::from_ref(&c),
            tool_results: &[],
        };
        // Two consecutive reads — exempt, never fires.
        assert!(g.record_turn(turn.clone()).is_none());
        assert!(g.record_turn(turn).is_none());
    }

    #[test]
    fn multi_call_turn_resets_state() {
        let mut g = ToolCallLoopGuard::new(ToolCallLoopGuardOptions {
            threshold: 2,
            exempt_tools: vec![],
        });
        let c1 = call("write", json!({"path": "/a"}));
        let c2 = call("write", json!({"path": "/b"}));
        let multi = ToolCallLoopTurn {
            tool_calls: &[c1, c2],
            tool_results: &[],
        };
        assert!(g.record_turn(multi).is_none());
        // After multi-call, the next single identical call should not
        // immediately fire (count was reset to 0, then incremented to 1).
        let c = call("write", json!({"path": "/a"}));
        let single = ToolCallLoopTurn {
            tool_calls: std::slice::from_ref(&c),
            tool_results: &[],
        };
        assert!(g.record_turn(single).is_none());
    }

    #[test]
    fn different_arguments_reset_state() {
        let mut g = ToolCallLoopGuard::new(ToolCallLoopGuardOptions {
            threshold: 3,
            exempt_tools: vec![],
        });
        let c1 = call("write", json!({"path": "/a"}));
        let c2 = call("write", json!({"path": "/b"}));
        let t1 = ToolCallLoopTurn {
            tool_calls: std::slice::from_ref(&c1),
            tool_results: &[],
        };
        let t2 = ToolCallLoopTurn {
            tool_calls: std::slice::from_ref(&c2),
            tool_results: &[],
        };
        // Two calls with different args: no fire, count stays at 1 after
        // t2.
        assert!(g.record_turn(t1).is_none());
        assert!(g.record_turn(t2).is_none());
    }

    #[test]
    fn argument_key_order_is_canonicalized() {
        let mut g = ToolCallLoopGuard::new(ToolCallLoopGuardOptions {
            threshold: 2,
            exempt_tools: vec![],
        });
        let c1 = call("write", json!({"a": 1, "b": 2}));
        let c2 = call("write", json!({"b": 2, "a": 1}));
        let t1 = ToolCallLoopTurn {
            tool_calls: std::slice::from_ref(&c1),
            tool_results: &[],
        };
        let t2 = ToolCallLoopTurn {
            tool_calls: std::slice::from_ref(&c2),
            tool_results: &[],
        };
        assert!(g.record_turn(t1).is_none());
        let hit = g.record_turn(t2).expect("key order should not matter");
        assert_eq!(hit.count, 2);
    }

    #[test]
    fn reset_clears_state() {
        let mut g = ToolCallLoopGuard::new(ToolCallLoopGuardOptions {
            threshold: 2,
            exempt_tools: vec![],
        });
        let c = call("write", json!({"path": "/a"}));
        let t = ToolCallLoopTurn {
            tool_calls: std::slice::from_ref(&c),
            tool_results: &[],
        };
        g.record_turn(t.clone());
        g.reset();
        assert_eq!(g.count, 0);
        assert!(g.last_hash.is_none());
    }

    #[test]
    fn summarize_text_truncates_with_ellipsis() {
        let s = "x".repeat(100);
        let out = summarize_text(&s, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn summarize_text_short_passthrough() {
        let out = summarize_text("hello", 10);
        assert_eq!(out, "hello");
    }

    #[test]
    fn summarize_tool_result_truncates_and_matches_id() {
        let r = result_for("abc", &"y".repeat(500));
        let out = summarize_tool_result(std::slice::from_ref(&r), "abc");
        assert_eq!(out.chars().count(), RESULT_SUMMARY_LIMIT);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn summarize_tool_result_missing_id_returns_empty() {
        let r = result_for("other", "content");
        let out = summarize_tool_result(std::slice::from_ref(&r), "missing");
        assert!(out.is_empty());
    }

    #[test]
    fn canonicalize_json_sorts_object_keys_recursively() {
        let v = json!({"z": 1, "a": {"y": 2, "b": 3}});
        let c = canonicalize_json(&v);
        let s = serde_json::to_string(&c).unwrap();
        // After sorting, "a" comes before "z".
        assert!(s.find("\"a\"").unwrap() < s.find("\"z\"").unwrap());
        // Nested keys are sorted too.
        let nested_start = s.find("\"y\"").unwrap();
        let nested_b = s.find("\"b\"").unwrap();
        assert!(nested_b < nested_start);
    }
}
