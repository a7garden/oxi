//! Chat normalization — ported from omp `core/chat-normalize.ts`.
//!
//! Provides [`normalize_chat`], [`normalize_batch`], and [`extraction_rate`].
//! Strips chat-style filler acronyms (lol, idk, tbh, …), expands contractions
//! (`u` → `you`, `wanna` → `want to`, …), collapses repeated characters,
//! replaces non-ASCII runs with spaces, and decides whether a message is
//! substantive enough to keep.
//!
//! ## Port notes
//!
//! This is a faithful port of the .ts source rather than a literal
//! re-implementation of the original written spec — the spec listed
//! conjunctions (and / but / so / …) as fragment starters and prose
//! discourse markers (um / uh / like / …) as fillers, whereas the .ts
//! uses gerund fragment starters (`going`, `thinking`, `fixing`, …) and
//! chat acronyms (lol, idk, tbh, …). The rest of ported mnemopi
//! (extraction, recall) depends on the .ts semantics, so we follow them
//! here and note the divergence. The [`normalize_chat`] signature
//! (`add_implicit_subjects: bool`) and the other public surface still
//! match the spec; the *thresholds / table contents* match the .ts.
//!
//! ## Subject prefix
//!
//! The implicit-subject prefix (`"i am "`) is only added for the
//! 2-word fragment-starter case — the .ts defines "starts with a
//! subject" implicitly via the `FRAGMENT_STARTERS` set (gerund
//! fragments the caller is implicitly performing).
//!
//! MIT — adapted from
//! [omp](https://github.com/can1357/oh-my-pi) `packages/mnemopi/src/core/chat-normalize.ts`.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

// ── Public types ────────────────────────────────────────────────────────

/// Extraction-rate summary returned by [`extraction_rate`].
///
/// `rate` is `survived / total` rounded to three decimal places (matching
/// the .ts implementation); `dropped_samples` is capped at five entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionRate {
    pub total: usize,
    pub survived: usize,
    pub dropped: usize,
    pub rate: f64,
    pub dropped_samples: Vec<String>,
}

// ── Contractions ────────────────────────────────────────────────────────

/// Contraction table — ported from omp `CONTRACTIONS`.
///
/// Order matters: longest-first (`ur` before `u`, `u're` before `u`) so
/// the regex alternation picks the longer match.
const CONTRACTIONS: &[(&str, &str)] = &[
    ("u're", "you are"),
    ("ur", "your"),
    ("u", "you"),
    ("r", "are"),
    ("y", "why"),
    ("b4", "before"),
    ("bc", "because"),
    ("cuz", "because"),
    ("gonna", "going to"),
    ("wanna", "want to"),
    ("gotta", "got to"),
    ("kinda", "kind of"),
    ("sorta", "sort of"),
    ("dunno", "don't know"),
    ("lemme", "let me"),
    ("gimme", "give me"),
    ("outta", "out of"),
    ("hafta", "have to"),
    ("shoulda", "should have"),
    ("woulda", "would have"),
    ("coulda", "could have"),
];

static CONTRACTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Alternation order matches CONTRACTIONS (longest-first) so
    // `\bur\b` wins over `\bu\b`.
    let alts = CONTRACTIONS
        .iter()
        .map(|(from, _)| regex::escape(from))
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&format!(r"(?i)\b({alts})\b")).expect("valid contraction regex")
});

// ── Filler words (chat acronyms) ────────────────────────────────────────

/// Chat-acronym filler words to strip — ported from omp `FILLER_WORDS`.
const FILLER_WORDS: &[&str] = &[
    "afaik", "brb", "fr", "fwiw", "idc", "idk", "iirc", "ikr", "imho", "imo", "irl", "istg",
    "lmao", "lmaoo", "lmfao", "lol", "ngl", "nvm", "omg", "omgg", "omggg", "rofl", "smh", "tbh",
    "tldr", "w", "wdym", "wtf",
];
static EDGE_PUNCT_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Non-raw string so we can escape the embedded quotes.
    Regex::new("^[.,!?;:'\"]+|[.,!?;:\"]+$").expect("valid edge punct regex")
});

fn is_filler_word(word: &str) -> bool {
    let stripped = EDGE_PUNCT_RE.replace_all(word, "").into_owned();
    FILLER_WORDS
        .iter()
        .any(|f| f.eq_ignore_ascii_case(&stripped))
}

// ── Fragment starters (gerunds → implicit "i am") ──────────────────────

/// Gerund fragment starters — ported from omp `FRAGMENT_STARTERS`.
const FRAGMENT_STARTERS: &[&str] = &[
    "building",
    "checking",
    "coming",
    "deploying",
    "feeling",
    "fixing",
    "going",
    "hoping",
    "looking",
    "planning",
    "running",
    "testing",
    "thinking",
    "trying",
    "wondering",
    "working",
];

static FRAGMENT_STARTER_SET: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| FRAGMENT_STARTERS.iter().copied().collect());

// ── Repetition collapse ────────────────────────────────────────────────
// The .ts uses /(.)\1{2,}/g (a JS backreference) to collapse runs of any
// single character repeated 3+ times to a single instance. The Rust
// `regex` crate does not support backreferences, so we walk the string
// and collapse runs manually. The output is identical for ASCII; for
// multi-byte chars each `char` is treated as one unit, matching the JS
// behaviour on UTF-16 code units (where surrogate pairs still appear as
// one logical character to the regex).
fn collapse_repeated_chars(value: &str) -> String {
    // Mirrors the .ts regex /(.)\1{2,}/g which collapses runs of 3+ of the
    // same character down to 1. The Rust `regex` crate has no
    // backreferences, so we walk the string manually.
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i + 1;
        while j < chars.len() && chars[j] == c {
            j += 1;
        }
        let run_len = j - i;
        // Keep the first char always; only suppress the rest when 3+.
        out.push(c);
        if run_len < 3 {
            for k in 1..run_len {
                out.push(chars[i + k]);
            }
        }
        i = j;
    }
    out
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Replace runs of non-ASCII characters with a single space (one space
/// per run, not per character) — ported from omp `replaceNonAsciiRuns`.
fn replace_non_ascii_runs(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_non_ascii_run = false;
    for ch in value.chars() {
        if (ch as u32) > 0x7f {
            if !in_non_ascii_run {
                out.push(' ');
                in_non_ascii_run = true;
            }
        } else {
            out.push(ch);
            in_non_ascii_run = false;
        }
    }
    out
}

// ── Public API ──────────────────────────────────────────────────────────

/// Normalize a chat-style message.
///
/// Pipeline (ported from `normalizeChat`):
/// 1. Trim; return `None` if empty after trim.
/// 2. Lowercase + trim.
/// 3. Expand contractions via the regex table.
/// 4. Split on whitespace, drop filler words (chat acronyms).
/// 5. Join back, collapse `(.) \1{2,}` runs to a single char.
/// 6. Replace non-ASCII runs with spaces, collapse whitespace.
/// 7. If fewer than 2 words remain, keep only a single word with length
///    > 5 (otherwise drop).
/// 8. If `add_implicit_subjects` and the result is exactly 2 words whose
///    first word is a `FRAGMENT_STARTERS` entry, prepend `"i am ".
///
/// `add_implicit_subjects` is required (matching the spec signature);
/// pass `false` to disable implicit subject insertion (the .ts default
/// is `true`, so most callers should pass `true`).
pub fn normalize_chat(text: &str, add_implicit_subjects: bool) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }

    let mut normalized = text.to_lowercase();
    normalized = normalized.trim().to_string();

    normalized = CONTRACTION_RE
        .replace_all(&normalized, |caps: &regex::Captures<'_>| {
            let hit = caps.get(1).map_or("", |m| m.as_str()).to_lowercase();
            CONTRACTIONS
                .iter()
                .find(|(from, _)| from.eq_ignore_ascii_case(&hit))
                .map_or("", |(_, to)| *to)
                .to_string()
        })
        .into_owned();

    let meaningful: Vec<&str> = normalized
        .split_whitespace()
        .filter(|word| !is_filler_word(word))
        .collect();

    if meaningful.is_empty() {
        return None;
    }

    normalized = meaningful.join(" ");
    normalized = collapse_repeated_chars(&normalized);
    normalized = replace_non_ascii_runs(&normalized);
    normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");

    let words: Vec<&str> = if normalized.is_empty() {
        Vec::new()
    } else {
        normalized.split(' ').collect()
    };
    let word_count = words.len();

    if word_count < 2 {
        if word_count == 1 && words.first().is_some_and(|w| w.len() > 5) {
            return Some(normalized);
        }
        return None;
    }

    if add_implicit_subjects && word_count == 2 {
        let first_word = words.first().copied().unwrap_or("");
        if FRAGMENT_STARTER_SET.contains(first_word) {
            normalized = format!("i am {normalized}");
        }
    }

    Some(normalized)
}

/// Batch-normalize a slice of messages. Each message is normalized
/// independently via [`normalize_chat`] with `add_implicit_subjects = true`
/// (matching the .ts `normalizeBatch` default).
pub fn normalize_batch(messages: &[String]) -> Vec<Option<String>> {
    messages.iter().map(|m| normalize_chat(m, true)).collect()
}

/// Compute the fraction of messages that survive normalization.
///
/// `dropped_samples` is capped at five entries to mirror the .ts.
pub fn extraction_rate(messages: &[String]) -> ExtractionRate {
    let normalized = normalize_batch(messages);
    let mut survived = 0;
    let mut dropped_samples: Vec<String> = Vec::new();

    for (i, msg) in messages.iter().enumerate() {
        if normalized[i].is_some() {
            survived += 1;
        } else if dropped_samples.len() < 5 {
            dropped_samples.push(msg.clone());
        }
    }

    let total = messages.len();
    let dropped = total - survived;
    let rate = if total == 0 {
        0.0
    } else {
        let r = (survived as f64) / (total as f64);
        (r * 1000.0).round() / 1000.0
    };

    ExtractionRate {
        total,
        survived,
        dropped,
        rate,
        dropped_samples,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_contractions() {
        // "u" → "you", "ur" → "your", "b4" → "before"
        let out = normalize_chat("u forgot ur keys b4 the meeting", true).unwrap();
        assert!(
            out.contains("you forgot your keys before the meeting"),
            "got: {out}"
        );
        // Word boundaries matter: "but" must NOT become "byout".
        let out2 = normalize_chat("but the data is good", true).unwrap();
        assert!(out2.contains("but"), "got: {out2}");
        // Longest match wins: "ur" before "u".
        let out3 = normalize_chat("ur amazing", true).unwrap();
        assert!(out3.contains("your amazing"), "got: {out3}");
        // Multi-word contractions.
        let out4 = normalize_chat("i wanna go home", true).unwrap();
        assert!(out4.contains("want to"), "got: {out4}");
    }

    #[test]
    fn removes_filler_acronyms() {
        // "lol", "idk", "tbh" are fillers in the .ts table.
        let out = normalize_chat("lol i think idk the answer tbh", true).unwrap();
        assert!(!out.contains("lol"), "got: {out}");
        assert!(!out.contains("idk"), "got: {out}");
        assert!(!out.contains("tbh"), "got: {out}");
        assert!(out.contains("think"));
        assert!(out.contains("answer"));
    }

    #[test]
    fn fragment_starter_gets_implicit_subject() {
        // The .ts prepends "i am" only for the 2-word fragment-starter case.
        let out = normalize_chat("going home", true).unwrap();
        assert_eq!(out, "i am going home");
        // Disabled → no prefix even in the 2-word case.
        let out2 = normalize_chat("going home", false).unwrap();
        assert_eq!(out2, "going home");
        // 3-word case is kept verbatim — the .ts gate is exact-2.
        let out3 = normalize_chat("going to deploy tomorrow", true).unwrap();
        assert_eq!(out3, "going to deploy tomorrow");
    }

    #[test]
    fn collapses_repeated_chars_and_whitespace() {
        let out = normalize_chat("heyyyy   thereeee", true).unwrap();
        // "heyyyy" → "hey", "thereeee" → "there"
        assert!(out.contains("hey there"), "got: {out}");
        // Non-ASCII run → single space.
        let out2 = normalize_chat("hello \u{4e2d}\u{6587} world", true).unwrap();
        assert!(out2.contains("hello"), "got: {out2}");
        assert!(out2.contains("world"), "got: {out2}");
        assert!(!out2.contains('\u{4e2d}'), "got: {out2}");
    }

    #[test]
    fn preserves_two_letter_runs_only_collapses_three_plus() {
        // 3+ repeats → 1. 2 repeats must be preserved (see advisory).
        let out = normalize_chat("good soooo book", true).unwrap();
        assert!(out.contains("good"), "double-letter run collapsed: {out}");
        assert!(out.contains("so"), "quad-run not collapsed to 1: {out}");
        assert!(out.contains("book"), "double-letter run collapsed: {out}");
    }

    #[test]
    fn rejects_short_messages() {
        // Empty after trim → None.
        assert_eq!(normalize_chat("   ", true), None);
        // Pure filler → None.
        assert_eq!(normalize_chat("lol omg idk", true), None);
        // Single short word → None.
        assert_eq!(normalize_chat("hi", true), None);
        // Single long word → kept.
        assert_eq!(
            normalize_chat("deployment", true),
            Some("deployment".to_string())
        );
        // Two normal words without gerund starter → kept as-is.
        assert_eq!(
            normalize_chat("hello world", true),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn batch_normalize_returns_per_message_options() {
        let msgs = vec![
            "hello world".to_string(),
            "lol".to_string(),
            "going home".to_string(),
            String::new(),
        ];
        let out = normalize_batch(&msgs);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], Some("hello world".to_string()));
        assert_eq!(out[1], None);
        assert_eq!(out[2], Some("i am going home".to_string()));
    }

    #[test]
    fn extraction_rate_counts_correctly() {
        let msgs = vec![
            "this is a real message".to_string(),
            "lol".to_string(),
            "another valid sentence here".to_string(),
            "omg".to_string(),
            "going to ship it tomorrow".to_string(),
        ];
        let r = extraction_rate(&msgs);
        assert_eq!(r.total, 5);
        assert_eq!(r.survived, 3);
        assert_eq!(r.dropped, 2);
        // rate = 3/5 = 0.6
        assert!((r.rate - 0.6).abs() < 1e-9, "rate = {}", r.rate);
        assert_eq!(r.dropped_samples.len(), 2);
        assert!(r.dropped_samples.contains(&"lol".to_string()));
        assert!(r.dropped_samples.contains(&"omg".to_string()));
    }

    #[test]
    fn extraction_rate_empty_input() {
        let r = extraction_rate(&[]);
        assert_eq!(r.total, 0);
        assert_eq!(r.survived, 0);
        assert_eq!(r.dropped, 0);
        assert_eq!(r.rate, 0.0);
        assert!(r.dropped_samples.is_empty());
    }

    #[test]
    fn dropped_samples_capped_at_five() {
        let msgs: Vec<String> = (0..10).map(|_| "lol".to_string()).collect();
        let r = extraction_rate(&msgs);
        assert_eq!(r.total, 10);
        assert_eq!(r.survived, 0);
        assert_eq!(r.dropped, 10);
        assert_eq!(r.dropped_samples.len(), 5);
    }
}
