//! Thinking-loop detector — ported from omp
//! `packages/ai/src/utils/thinking-loop.ts`.
//!
//! MIT — attribution: adapted from
//! [omp](https://github.com/can1357/oh-my-pi) (Can Berk Güder, earendil-works).
//!
//! ## Purpose
//!
//! Reasoning models (Gemini, DeepSeek-R1, …) sometimes enter a degenerate
//! loop in their thinking stream: the same paragraph reshuffled over and
//! over, a short token repeated back-to-back, or filler that recycles the
//! recent vocabulary without ever naming a new concrete reference. The
//! model bills tokens indefinitely; the user never sees an answer.
//!
//! This module provides [`ThinkingLoopDetector`] — a stateful detector
//! fed streamed thinking deltas. It recognises three loop shapes:
//!
//! 1. **Verbatim tail repetition** — a short unit repeated back-to-back
//!    at the tail of the rolling window (e.g. `🌊 🌊 🌊 …`).
//! 2. **Near-duplicate segment cluster** — paragraphs whose word-trigram
//!    fingerprints overlap above a Jaccard threshold (cosmetic rewording
//!    of the same paragraph).
//! 3. **Progress-lexicon stall** — paragraphs that recycle the recent
//!    vocabulary (low novelty) and introduce no new concrete reference
//!    (no new code span / path / identifier).

use std::collections::HashSet;

/// Rolling tail (chars) inspected for verbatim back-to-back repetition.
const VERBATIM_TAIL_WINDOW: usize = 250;
/// Minimum total repeated chars before a verbatim run counts as a loop.
const VERBATIM_MIN_REPEATED_CHARS: usize = 180;
/// Longest unit length probed for a verbatim repeat.
const VERBATIM_MAX_UNIT: usize = 60;
/// Minimum consecutive repeats for a verbatim loop.
const VERBATIM_MIN_COUNT: usize = 4;

/// Char cap for an unterminated segment; forces a flush so a wall-of-text
/// loop (no blank lines / headings) still segments.
const SEGMENT_CHAR_CAP: usize = 700;
/// Normalized-length floor below which a segment is ignored.
const SEGMENT_MIN_NORM_CHARS: usize = 60;
/// How many recent substantial segments are kept for similarity
/// comparison.
const SEGMENT_WINDOW: usize = 16;
/// Word-trigram Jaccard at/above which two segments count as
/// near-duplicates.
const SEGMENT_SIMILARITY: f64 = 0.8;
/// Substantial segments required before detection may fire (warm-up).
const SEGMENT_MIN_COUNT: usize = 8;
/// Near-duplicate cluster size (current + matches) that trips the loop.
const SEGMENT_MIN_CLUSTER: usize = 4;

/// Recent segments whose pooled unigram vocabulary is the novelty
/// baseline for progress-lexicon stall detection.
const LEX_NOVELTY_WINDOW: usize = 8;
/// Novelty (fraction of a segment's content words unseen across the
/// recent window) at/below which a segment counts as recycling earlier
/// wording.
const LEX_STALL_NOVELTY_FLOOR: f64 = 0.2;
/// Consecutive low-information segments that trip a progress-lexicon
/// stall.
const LEX_STALL_MIN_RUN: usize = 8;

/// Stable lead phrase of the detector's reason strings. Used by upstream
/// retry classifiers to recognise the failure as a transient stream
/// stall.
pub const THINKING_LOOP_MARKER: &str = "thinking loop detected";

/// Stateful detector fed streamed thinking deltas.
#[derive(Debug, Default)]
pub struct ThinkingLoopDetector {
    tail: String,
    pending: String,
    window: Vec<HashSet<String>>,
    count: usize,
    word_window: Vec<HashSet<String>>,
    lex_stall_run: usize,
    anchor_window: Vec<HashSet<String>>,
    fired: bool,
}

impl ThinkingLoopDetector {
    /// Construct a fresh detector with empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one streamed thinking delta.
    ///
    /// Returns `Some(reason)` the first time a loop is recognised. The
    /// caller should stop the stream and surface the reason upstream.
    /// Empty deltas are no-ops.
    pub fn push(&mut self, delta: &str) -> Option<String> {
        if self.fired || delta.is_empty() {
            return None;
        }

        // 1. Verbatim back-to-back repetition over the rolling tail.
        self.tail.push_str(delta);
        let tail_chars = self.tail.chars().count();
        if tail_chars > VERBATIM_TAIL_WINDOW {
            let skip = tail_chars - VERBATIM_TAIL_WINDOW;
            self.tail = self.tail.chars().skip(skip).collect();
        }
        if let Some((unit, times)) = detect_verbatim_repetition(&self.tail) {
            self.fired = true;
            let trimmed = unit.trim();
            return Some(format!(
                "{THINKING_LOOP_MARKER}: repeated \"{trimmed}\" {times}× back-to-back"
            ));
        }

        // 2. Near-duplicate paragraph loop.
        self.pending.push_str(delta);
        loop {
            let boundary = find_blank_line(&self.pending);
            let raw: String;
            match boundary {
                Some(range) => {
                    // Drain the buffer through the end of the blank-line
                    // run, but keep only the segment text (chars before
                    // the run).
                    let consumed: String = self.pending.drain(..range.end_byte).collect();
                    raw = consumed
                        .char_indices()
                        .take(range.start_char)
                        .map(|(_, c)| c)
                        .collect();
                }
                None => {
                    let pending_chars = self.pending.chars().count();
                    if pending_chars > SEGMENT_CHAR_CAP {
                        let split_at = self
                            .pending
                            .char_indices()
                            .nth(SEGMENT_CHAR_CAP)
                            .map(|(b, _)| b)
                            .unwrap_or(self.pending.len());
                        raw = self.pending.drain(..split_at).collect();
                    } else {
                        return None;
                    }
                }
            }
            let mut rest = raw;
            while !rest.is_empty() {
                let chunk_len = rest.chars().count().min(SEGMENT_CHAR_CAP);
                let split_at = rest
                    .char_indices()
                    .nth(chunk_len)
                    .map(|(b, _)| b)
                    .unwrap_or(rest.len());
                let chunk: String = rest.drain(..split_at).collect();
                if let Some(hit) = self.consume_segment(&chunk) {
                    self.fired = true;
                    return Some(hit);
                }
            }
        }
    }

    /// Process the buffered trailing paragraph. Called when the thinking
    /// block ends so the final segment is not dropped.
    pub fn flush(&mut self) -> Option<String> {
        if self.fired || self.pending.is_empty() {
            return None;
        }
        let mut rest = std::mem::take(&mut self.pending);
        while !rest.is_empty() {
            let chunk_len = rest.chars().count().min(SEGMENT_CHAR_CAP);
            let split_at = rest
                .char_indices()
                .nth(chunk_len)
                .map(|(b, _)| b)
                .unwrap_or(rest.len());
            let chunk: String = rest.drain(..split_at).collect();
            if let Some(hit) = self.consume_segment(&chunk) {
                self.fired = true;
                return Some(hit);
            }
        }
        None
    }

    /// Reset to initial state.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// True once a loop has been recognised (sticky until `reset`).
    pub fn fired(&self) -> bool {
        self.fired
    }

    fn consume_segment(&mut self, raw: &str) -> Option<String> {
        let stripped = strip_summary_headers(raw);
        let normalized = normalize_segment(&stripped);
        if normalized.chars().count() < SEGMENT_MIN_NORM_CHARS {
            return None;
        }

        // (a) Near-duplicate trigram cluster.
        let fingerprint = trigram_shingles(&normalized);
        let mut cluster = 1usize;
        for prev in &self.window {
            if jaccard(&fingerprint, prev) >= SEGMENT_SIMILARITY {
                cluster += 1;
            }
        }

        // (b) Progress-lexicon stall.
        let words: HashSet<String> = normalized
            .split_whitespace()
            .filter(|w| !w.is_empty())
            .map(|w| w.to_string())
            .collect();
        let mut prior_vocab: HashSet<String> = HashSet::new();
        for set in &self.word_window {
            for w in set {
                prior_vocab.insert(w.clone());
            }
        }
        let unseen = words.iter().filter(|w| !prior_vocab.contains(*w)).count();
        let novelty = if prior_vocab.is_empty() {
            1.0
        } else {
            unseen as f64 / words.len().max(1) as f64
        };

        let anchors = extract_concrete_anchors(&stripped);
        let mut new_anchor = false;
        for anchor in &anchors {
            let is_new = self.anchor_window.iter().all(|seen| !seen.contains(anchor));
            if is_new {
                new_anchor = true;
                break;
            }
        }

        if novelty <= LEX_STALL_NOVELTY_FLOOR && !new_anchor {
            self.lex_stall_run += 1;
        } else {
            self.lex_stall_run = 0;
        }

        self.window.push(fingerprint);
        if self.window.len() > SEGMENT_WINDOW {
            self.window.remove(0);
        }
        self.word_window.push(words);
        if self.word_window.len() > LEX_NOVELTY_WINDOW {
            self.word_window.remove(0);
        }
        self.anchor_window.push(anchors);
        if self.anchor_window.len() > LEX_NOVELTY_WINDOW {
            self.anchor_window.remove(0);
        }
        self.count += 1;

        if self.count >= SEGMENT_MIN_COUNT {
            if cluster >= SEGMENT_MIN_CLUSTER {
                return Some(format!(
                    "{THINKING_LOOP_MARKER}: {cluster} near-identical segments within the last {SEGMENT_WINDOW}"
                ));
            }
            if self.lex_stall_run >= LEX_STALL_MIN_RUN {
                return Some(format!(
                    "{THINKING_LOOP_MARKER}: {} low-information segments recycling recent wording",
                    self.lex_stall_run
                ));
            }
        }
        None
    }
}

/// A found blank-line boundary — character index where the boundary
/// starts and the byte offset just past the end of the consumed run.
struct CharRange {
    start_char: usize,
    end_byte: usize,
}

/// Find the first `\n\s*\n` boundary in `pending`. Returns the
/// character index of the boundary and the byte offset just past the
/// consumed run.
fn find_blank_line(pending: &str) -> Option<CharRange> {
    let chars: Vec<char> = pending.chars().collect();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == '\n' {
            let mut j = i + 1;
            while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t' || chars[j] == '\n') {
                j += 1;
            }
            if j > i + 1 {
                let mut bytes_consumed = 0usize;
                for (k, c) in chars.iter().enumerate() {
                    if k >= j {
                        break;
                    }
                    bytes_consumed += c.len_utf8();
                }
                return Some(CharRange {
                    start_char: i,
                    end_byte: bytes_consumed,
                });
            }
        }
        i += 1;
    }
    None
}

/// Detect a short unit repeated back-to-back at the tail (verbatim
/// loop). Only a unit carrying a letter or pictographic emoji counts.
fn detect_verbatim_repetition(text: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < VERBATIM_MIN_REPEATED_CHARS {
        return None;
    }
    let window_size = chars.len().min(VERBATIM_TAIL_WINDOW);
    let search_space = &chars[chars.len() - window_size..];

    for len in 2..=VERBATIM_MAX_UNIT {
        if search_space.len() < len * 4 {
            continue;
        }
        let unit: String = search_space[search_space.len() - len..].iter().collect();
        if !unit
            .chars()
            .any(|c| c.is_alphabetic() || is_pictographic(c))
        {
            continue;
        }

        let mut count = 0usize;
        let mut pos = search_space.len();
        while pos >= len {
            let slice = &search_space[pos - len..pos];
            let candidate: String = slice.iter().collect();
            if candidate == unit {
                count += 1;
                pos -= len;
            } else {
                break;
            }
        }
        if count >= VERBATIM_MIN_COUNT && len * count >= VERBATIM_MIN_REPEATED_CHARS {
            return Some((unit, count));
        }
    }
    None
}

/// Conservative pictographic check (covers common emoji ranges without
/// pulling in `unicode-properties` or regex).
fn is_pictographic(c: char) -> bool {
    let cp = c as u32;
    matches!(
        cp,
        0x1F300..=0x1F5FF
            | 0x1F600..=0x1F64F
            | 0x1F680..=0x1F6FF
            | 0x1F700..=0x1F77F
            | 0x1F780..=0x1F7FF
            | 0x1F800..=0x1F8FF
            | 0x1F900..=0x1F9FF
            | 0x1FA00..=0x1FA6F
            | 0x1FA70..=0x1FAFF
            | 0x2600..=0x26FF
            | 0x2700..=0x27BF
    )
}

/// Strip reasoning-summarizer titles ("## Heading", "**bold title**").
fn strip_summary_headers(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let mut hash_count = 0;
            for c in trimmed.chars() {
                if c == '#' {
                    hash_count += 1;
                } else {
                    break;
                }
            }
            if (1..=6).contains(&hash_count) {
                let after = &trimmed[hash_count..];
                if after.starts_with(' ') || after.starts_with('\t') {
                    continue;
                }
            }
        }
        if trimmed.starts_with("**")
            && trimmed.len() >= 4
            && trimmed[2..].trim_end().ends_with("**")
        {
            continue;
        }
        if trimmed.starts_with("***")
            && trimmed.len() >= 6
            && trimmed[3..].trim_end().ends_with("***")
        {
            continue;
        }
        let _ = writeln!(out, "{line}");
    }
    out
}

/// Lowercase and tokenize prose plus code/path payloads, dropping pure
/// numbers.
fn normalize_segment(segment: &str) -> String {
    use std::fmt::Write;
    let lower: String = segment.to_lowercase();
    let unbackticked = lower.replace('`', " ");
    let mut out = String::with_capacity(unbackticked.len());
    let mut prev_space = true;
    for c in unbackticked.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    let mut filtered = String::with_capacity(out.len());
    for token in out.split_whitespace() {
        if token.chars().any(|c| c.is_ascii_lowercase()) {
            let _ = write!(filtered, "{token} ");
        }
    }
    filtered.trim_end().to_string()
}

/// Word-trigram shingle set of a normalized segment.
fn trigram_shingles(normalized: &str) -> HashSet<String> {
    let words: Vec<&str> = normalized.split_whitespace().collect();
    let mut shingles = HashSet::new();
    if words.len() < 3 {
        if !words.is_empty() {
            shingles.insert(words.join(" "));
        }
        return shingles;
    }
    for i in 0..=words.len() - 3 {
        shingles.insert(format!("{} {} {}", words[i], words[i + 1], words[i + 2]));
    }
    shingles
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (small, large) = if a.len() < b.len() { (a, b) } else { (b, a) };
    let mut intersection = 0usize;
    for x in small {
        if large.contains(x) {
            intersection += 1;
        }
    }
    let union = a.len() + b.len() - intersection;
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Extract concrete references the model is reasoning about: code spans
/// (backticks), multi-segment paths, snake/camel/Pascal identifiers.
fn extract_concrete_anchors(segment: &str) -> HashSet<String> {
    let mut out = HashSet::new();

    // Backtick code spans.
    let mut cur = String::new();
    let mut in_backtick = false;
    for c in segment.chars() {
        if c == '`' {
            if in_backtick {
                let trimmed = cur.trim();
                if !trimmed.is_empty() {
                    out.insert(trimmed.to_lowercase());
                }
                cur.clear();
                in_backtick = false;
            } else {
                in_backtick = true;
            }
            continue;
        }
        if in_backtick {
            cur.push(c);
        }
    }
    if in_backtick {
        let trimmed = cur.trim();
        if !trimmed.is_empty() {
            out.insert(trimmed.to_lowercase());
        }
    }

    // Multi-segment paths.
    for token in segment.split_whitespace() {
        if token.contains('/') && token.chars().any(|c| c.is_alphabetic()) {
            out.insert(token.to_lowercase());
        }
    }

    // snake_case and CamelCase / PascalCase identifiers.
    for raw_token in segment.split_whitespace() {
        let token: String = raw_token
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_string();
        if token.is_empty() {
            continue;
        }
        let only_word = token.chars().all(|c| c.is_alphanumeric() || c == '_');
        if !only_word {
            continue;
        }
        let has_snake = token.contains('_');
        let has_camel = token
            .chars()
            .enumerate()
            .any(|(i, c)| i > 0 && c.is_ascii_uppercase());
        if has_snake || has_camel {
            out.insert(token.to_lowercase());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_repetition_short_unit_loop() {
        let unit = "🌊".repeat(200);
        let hit = detect_verbatim_repetition(&unit);
        assert!(hit.is_some(), "expected verbatim detection");
        let (u, n) = hit.unwrap();
        assert!(u.contains('🌊'));
        assert!(n >= 4);
    }

    #[test]
    fn verbatim_repetition_too_short_no_loop() {
        assert!(detect_verbatim_repetition("ababab").is_none());
    }

    #[test]
    fn verbatim_repetition_punctuation_only_no_loop() {
        let s = "----".repeat(60);
        assert!(detect_verbatim_repetition(&s).is_none());
    }

    #[test]
    fn detector_push_fires_on_verbatim_loop() {
        let mut det = ThinkingLoopDetector::new();
        let unit = "ab ".repeat(120);
        let hit = det.push(&unit);
        assert!(hit.is_some(), "verbatim loop should fire");
        let reason = hit.unwrap();
        assert!(reason.contains(THINKING_LOOP_MARKER));
        assert!(reason.contains("back-to-back"));
    }

    #[test]
    fn detector_fires_once_then_sticky() {
        let mut det = ThinkingLoopDetector::new();
        let unit = "ab ".repeat(120);
        let first = det.push(&unit);
        let second = det.push(&unit);
        assert!(first.is_some());
        assert!(second.is_none(), "after firing, pushes are no-ops");
        assert!(det.fired());
    }

    #[test]
    fn detector_reset_clears_state() {
        let mut det = ThinkingLoopDetector::new();
        let _ = det.push(&"ab ".repeat(120));
        assert!(det.fired());
        det.reset();
        assert!(!det.fired());
    }

    #[test]
    fn detector_empty_push_is_noop() {
        let mut det = ThinkingLoopDetector::new();
        assert!(det.push("").is_none());
    }

    #[test]
    fn normalize_segment_lowercases_and_drops_numbers() {
        let n = normalize_segment("The QUICK brown 123 fox jumps over `code`");
        assert!(n.contains("the"));
        assert!(n.contains("quick"));
        assert!(n.contains("brown"));
        assert!(!n.contains("123"));
    }

    #[test]
    fn trigram_shingles_three_words() {
        let s = trigram_shingles("the quick brown");
        assert_eq!(s.len(), 1);
        assert!(s.contains("the quick brown"));
    }

    #[test]
    fn trigram_shingles_short_input_passthrough() {
        let s = trigram_shingles("hello");
        assert_eq!(s.len(), 1);
        assert!(s.contains("hello"));
    }

    #[test]
    fn jaccard_identical_sets() {
        let a: HashSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let b = a.clone();
        assert!((jaccard(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn jaccard_disjoint_sets() {
        let a: HashSet<String> = ["a"].iter().map(|s| s.to_string()).collect();
        let b: HashSet<String> = ["b"].iter().map(|s| s.to_string()).collect();
        assert!((jaccard(&a, &b) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn jaccard_empty_set_zero() {
        let a: HashSet<String> = HashSet::new();
        let b: HashSet<String> = ["a"].iter().map(|s| s.to_string()).collect();
        assert!((jaccard(&a, &b) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn strip_summary_headers_drops_atx_and_bold() {
        let input = "## Heading\n**bold title**\nactual content";
        let stripped = strip_summary_headers(input);
        assert!(!stripped.contains("Heading"));
        assert!(!stripped.contains("bold title"));
        assert!(stripped.contains("actual content"));
    }

    #[test]
    fn extract_concrete_anchors_catches_code_spans() {
        let s = "Look at `oxi_ai::Message` and `lib.rs` for details";
        let anchors = extract_concrete_anchors(s);
        assert!(anchors.contains("oxi_ai::message"));
        assert!(anchors.contains("lib.rs"));
    }

    #[test]
    fn extract_concrete_anchors_catches_paths() {
        let s = "see src/main.rs and crates/foo/Cargo.toml";
        let anchors = extract_concrete_anchors(s);
        assert!(anchors.iter().any(|a| a.contains("src/main.rs")));
        assert!(anchors.iter().any(|a| a.contains("crates/foo/cargo.toml")));
    }

    #[test]
    fn extract_concrete_anchors_catches_snake_case() {
        let s = "Consider the variable my_var_name.";
        let anchors = extract_concrete_anchors(s);
        assert!(anchors.contains("my_var_name"));
    }

    #[test]
    fn extract_concrete_anchors_catches_camel_case() {
        let s = "Use the MyCoolClass implementation.";
        let anchors = extract_concrete_anchors(s);
        assert!(anchors.contains("mycoolclass"));
    }
}
