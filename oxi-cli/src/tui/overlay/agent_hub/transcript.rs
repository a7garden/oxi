#![allow(dead_code)] // consumed by Tasks 6+; this task only ships + tests.
//! Mtime-based JSONL transcript reader for Agent Hub.
//!
//! Two formats supported:
//! - advisor: `{"ts":N,"messages":["…","…"]}`
//! - subagent / session: `{"type":"message","id":...,"parentId":...,"timestamp":<RFC3339>,"message":{"role":...,"content":...}}`
//!   (plus a leading `{"type":"session",...}` header line and other entry
//!   variants that we skip silently).
//!
//! Format is detected on the first non-empty line and cached for the file's
//! lifetime. Session-format parsing reuses `crate::store::session` types
//! (`SessionEntryBase` + `AgentMessage`) so we never drift from what the
//! `SessionManager` actually writes.

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use chrono::DateTime;
use serde::Deserialize;

use crate::store::session::{AgentMessage, SessionEntryBase};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptLine {
    pub timestamp_ms: u64,
    pub role: String,
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_status: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct AdvisorLine {
    ts: u64,
    messages: Vec<String>,
}

#[derive(Debug)]
pub struct TranscriptReader {
    path: PathBuf,
    last_mtime: Option<SystemTime>,
    last_size: u64,
    lines: Vec<TranscriptLine>,
    format: TranscriptFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptFormat {
    Unknown,
    Advisor,
    Session,
}

impl TranscriptReader {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_mtime: None,
            last_size: 0,
            lines: Vec::new(),
            format: TranscriptFormat::Unknown,
        }
    }

    /// Re-read file if mtime or size changed. Returns true if lines were
    /// (re-)parsed. Cheap on no-op: 1 stat call.
    pub fn refresh(&mut self) -> bool {
        let Ok(meta) = fs::metadata(&self.path) else {
            self.lines.clear();
            return false;
        };
        let mtime = meta.modified().ok();
        let size = meta.len();
        if mtime == self.last_mtime && size == self.last_size {
            return false;
        }
        self.last_mtime = mtime;
        self.last_size = size;
        let Ok(content) = fs::read_to_string(&self.path) else {
            self.lines.clear();
            return false;
        };
        self.parse(&content);
        true
    }

    fn parse(&mut self, content: &str) {
        self.lines.clear();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if self.format == TranscriptFormat::Unknown {
                self.format = if line.contains("\"ts\"") && line.contains("\"messages\"") {
                    TranscriptFormat::Advisor
                } else {
                    TranscriptFormat::Session
                };
            }
            match self.format {
                TranscriptFormat::Advisor => self.parse_advisor(line),
                TranscriptFormat::Session => self.parse_session(line),
                TranscriptFormat::Unknown => {}
            }
        }
    }

    fn parse_advisor(&mut self, line: &str) {
        let Ok(a) = serde_json::from_str::<AdvisorLine>(line) else {
            return;
        };
        for m in a.messages {
            self.lines.push(TranscriptLine {
                timestamp_ms: a.ts,
                role: "assistant".into(),
                text: m,
                tool_name: None,
                tool_status: None,
            });
        }
    }

    fn parse_session(&mut self, line: &str) {
        // Skip non-message lines (header, custom entries) — they fail to
        // deserialize as a `SessionMessageEntry` (which carries an
        // `AgentMessage` body) and we just drop them.
        #[derive(Deserialize)]
        struct Entry {
            #[serde(flatten)]
            base: SessionEntryBase,
            message: AgentMessage,
        }
        let Ok(entry) = serde_json::from_str::<Entry>(line) else {
            return;
        };
        let timestamp_ms = parse_rfc3339_ms(&entry.base.timestamp).unwrap_or(0);
        let role = role_tag(&entry.message);
        let text = entry.message.content();
        self.lines.push(TranscriptLine {
            timestamp_ms,
            role,
            text,
            tool_name: None,
            tool_status: None,
        });
    }

    #[must_use]
    pub fn lines(&self) -> &[TranscriptLine] {
        &self.lines
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Map `AgentMessage` variants back to their `#[serde(rename = "…")]` tag.
fn role_tag(msg: &AgentMessage) -> String {
    match msg {
        AgentMessage::User { .. } => "user".into(),
        AgentMessage::Assistant { .. } => "assistant".into(),
        AgentMessage::ToolResult { .. } => "toolResult".into(),
        AgentMessage::System { .. } => "system".into(),
        AgentMessage::BashExecution { .. } => "bashExecution".into(),
        AgentMessage::Custom { .. } => "custom".into(),
        AgentMessage::BranchSummary { .. } => "branchSummary".into(),
        AgentMessage::CompactionSummary { .. } => "compactionSummary".into(),
    }
}

fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis().max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    fn write_jsonl(path: &Path, lines: &[&str]) {
        let mut f = std::fs::File::create(path).unwrap();
        for l in lines {
            writeln!(f, "{}", l).unwrap();
        }
    }

    #[test]
    fn parses_advisor_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("__advisor.jsonl");
        write_jsonl(
            &path,
            &[
                r#"{"ts":1000,"messages":["review carefully"]}"#,
                r#"{"ts":2000,"messages":["ok proceed","ack"]}"#,
            ],
        );
        let mut r = TranscriptReader::new(path);
        assert!(r.refresh());
        let lines = r.lines();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "review carefully");
        assert_eq!(lines[2].text, "ack");
    }

    #[test]
    fn parses_session_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub.jsonl");
        // Real SessionManager JSONL: leading header line, then user +
        // assistant entries with the actual schema (assistant.content is an
        // ARRAY of AssistantContentBlock, timestamp is RFC3339).
        write_jsonl(
            &path,
            &[
                r#"{"type":"session","version":3,"id":"56f86b58-3c4d-490c-83c6-17a1492032e8","timestamp":"2026-07-23T14:48:53.588+00:00","cwd":"/tmp"}"#,
                r#"{"type":"message","id":"057fc4fa-2a1c-41fe-93ae-ff6729831708","parentId":null,"timestamp":"2026-07-23T14:48:53.589+00:00","message":{"role":"user","content":"do the thing"}}"#,
                r#"{"type":"message","id":"c8ba8b80-0a9e-4b47-93d3-9a0a9f23c1ea","parentId":"057fc4fa-2a1c-41fe-93ae-ff6729831708","timestamp":"2026-07-23T14:48:53.590+00:00","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
            ],
        );
        let mut r = TranscriptReader::new(path);
        assert!(r.refresh());
        let lines = r.lines();
        // header line skipped; 2 message lines kept.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].role, "user");
        assert_eq!(lines[0].text, "do the thing");
        assert_eq!(lines[1].role, "assistant");
        assert_eq!(lines[1].text, "done");
        assert!(lines[0].timestamp_ms > 0);
    }

    #[test]
    fn refresh_skips_when_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        write_jsonl(&path, &[r#"{"ts":1,"messages":["x"]}"#]);
        let mut r = TranscriptReader::new(path);
        assert!(r.refresh());
        assert!(
            !r.refresh(),
            "second refresh with no change must return false"
        );
    }

    #[test]
    fn refresh_reruns_on_size_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        write_jsonl(&path, &[r#"{"ts":1,"messages":["x"]}"#]);
        let mut r = TranscriptReader::new(path.clone());
        r.refresh();
        write_jsonl(
            &path,
            &[
                r#"{"ts":1,"messages":["x"]}"#,
                r#"{"ts":2,"messages":["y"]}"#,
            ],
        );
        assert!(r.refresh());
        assert_eq!(r.lines().len(), 2);
    }

    #[test]
    fn missing_file_yields_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = TranscriptReader::new(dir.path().join("missing.jsonl"));
        assert!(!r.refresh());
        assert!(r.is_empty());
    }
}
