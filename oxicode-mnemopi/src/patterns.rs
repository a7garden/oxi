//! Patterns — dictionary-based compression and pattern detection.
//!
//! Ported from omp `core/patterns.ts`. Provides:
//! - `MemoryCompressor`: dict / RLE / semantic compression for memory content.
//! - `PatternDetector`: temporal, content, and sequence pattern detection.
//!
//! MIT — attribution: adapted from [omp](https://github.com/earendil-works/pi)
//! `packages/mnemopi/src/core/patterns.ts`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

// ── Compression stats ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompressionStats {
    pub original_size: usize,
    pub compressed_size: usize,
    pub ratio: f64,
    pub method: String,
    pub patterns_found: usize,
    pub memories_compressed: usize,
}

impl CompressionStats {
    pub fn savings_percent(&self) -> f64 {
        if self.original_size == 0 {
            return 0.0;
        }
        (1.0 - self.compressed_size as f64 / self.original_size as f64) * 100.0
    }
}

fn utf8_size(s: &str) -> usize {
    s.len()
}

// ── Memory record ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub content: String,
    pub source: String,
    pub timestamp: Option<String>,
    pub created_at: Option<String>,
}

fn timestamp_of(mem: &MemoryRecord) -> Option<&str> {
    if let Some(ref ts) = mem.timestamp
        && !ts.is_empty()
    {
        return Some(ts);
    }
    if let Some(ref ts) = mem.created_at
        && !ts.is_empty()
    {
        return Some(ts);
    }
    None
}

// ── Memory compressor ───────────────────────────────────────────────────

/// Dictionary-based, RLE, and semantic memory compressor.
pub struct MemoryCompressor {
    dictionary: Vec<(String, String)>, // (phrase, token) — ordered for deterministic compress
}

impl Default for MemoryCompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryCompressor {
    pub fn new() -> Self {
        Self {
            dictionary: Self::default_dict(),
        }
    }

    pub fn with_dictionary(dict: Vec<(String, String)>) -> Self {
        Self { dictionary: dict }
    }

    fn default_dict() -> Vec<(String, String)> {
        vec![
            ("remember that ".into(), "".into()),
            ("the user said ".into(), "".into()),
            ("the user asked ".into(), "".into()),
            ("the user wants ".into(), "".into()),
            ("conversation about ".into(), "".into()),
            ("please note that ".into(), "".into()),
            ("important: ".into(), "".into()),
            ("user preference: ".into(), "".into()),
            ("project context: ".into(), "\t".into()),
            ("api key ".into(), "\n".into()),
            ("token ".into(), "\x0b".into()),
            ("session ".into(), "\x0c".into()),
            ("mnemopi ".into(), "\r".into()),
        ]
    }

    /// Compress a single string using the specified method.
    ///
    /// Methods: `"dict"` (default), `"rle"`, `"semantic"`, `"auto"`
    /// (tries dict first, falls back to RLE if savings < 5%).
    pub fn compress(&self, content: &str, method: &str) -> (String, CompressionStats) {
        let original_size = utf8_size(content);
        match method {
            "auto" => {
                let (compressed, stats) = self.dict_compress(content);
                if stats.savings_percent() < 5.0 {
                    return self.rle_compress(content);
                }
                (compressed, stats)
            }
            "dict" => self.dict_compress(content),
            "rle" => self.rle_compress(content),
            "semantic" => self.semantic_compress_single(content),
            _ => (
                content.to_string(),
                CompressionStats {
                    original_size,
                    compressed_size: original_size,
                    ratio: 1.0,
                    method: "none".into(),
                    ..Default::default()
                },
            ),
        }
    }

    fn dict_compress(&self, content: &str) -> (String, CompressionStats) {
        let original_size = utf8_size(content);
        let mut compressed = content.to_string();
        for (phrase, token) in &self.dictionary {
            compressed = compressed.replace(phrase, token);
        }
        let compressed_size = utf8_size(&compressed);
        let ratio = if original_size > 0 {
            compressed_size as f64 / original_size as f64
        } else {
            1.0
        };
        (
            compressed,
            CompressionStats {
                original_size,
                compressed_size,
                ratio,
                method: "dict".into(),
                ..Default::default()
            },
        )
    }

    fn rle_compress(&self, content: &str) -> (String, CompressionStats) {
        let original_size = utf8_size(content);
        if content.is_empty() {
            return (
                content.to_string(),
                CompressionStats {
                    original_size: 0,
                    compressed_size: 0,
                    ratio: 1.0,
                    method: "rle".into(),
                    ..Default::default()
                },
            );
        }

        let chars: Vec<char> = content.chars().collect();
        let mut compressed_parts: Vec<String> = Vec::new();
        let mut count = 1;

        for i in 1..chars.len() {
            if chars[i] == chars[i - 1] && count < 255 {
                count += 1;
            } else {
                let prev = chars[i - 1];
                if count > 3 {
                    compressed_parts.push(format!("[{prev}*{count}]"));
                } else {
                    let run: String = chars[i - count..i].iter().collect();
                    compressed_parts.push(run);
                }
                count = 1;
            }
        }
        let last = chars[chars.len() - 1];
        if count > 3 {
            compressed_parts.push(format!("[{last}*{count}]"));
        } else {
            let run: String = chars[chars.len() - count..].iter().collect();
            compressed_parts.push(run);
        }

        let compressed_string = compressed_parts.join("");
        let compressed_size = utf8_size(&compressed_string);
        let ratio = if original_size > 0 {
            compressed_size as f64 / original_size as f64
        } else {
            1.0
        };
        (
            compressed_string,
            CompressionStats {
                original_size,
                compressed_size,
                ratio,
                method: "rle".into(),
                ..Default::default()
            },
        )
    }

    fn semantic_compress_single(&self, content: &str) -> (String, CompressionStats) {
        let original_size = utf8_size(content);
        let compressed = if original_size > 500 {
            let first_250: String = content.chars().take(250).collect();
            let last_100: String = content
                .chars()
                .rev()
                .take(100)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{first_250} [...] {last_100}")
        } else {
            content.to_string()
        };
        let compressed_size = utf8_size(&compressed);
        let ratio = if original_size > 0 {
            compressed_size as f64 / original_size as f64
        } else {
            1.0
        };
        (
            compressed,
            CompressionStats {
                original_size,
                compressed_size,
                ratio,
                method: "semantic".into(),
                ..Default::default()
            },
        )
    }

    /// Compress a batch of memory records.
    pub fn compress_batch(
        &self,
        memories: &[MemoryRecord],
        method: &str,
    ) -> (Vec<MemoryRecord>, CompressionStats) {
        let mut total_original = 0;
        let mut total_compressed = 0;
        let mut compressed_memories = Vec::with_capacity(memories.len());

        for mem in memories {
            let (compressed, stats) = self.compress(&mem.content, method);
            total_original += stats.original_size;
            total_compressed += stats.compressed_size;
            compressed_memories.push(MemoryRecord {
                content: compressed,
                ..mem.clone()
            });
        }

        let ratio = if total_original > 0 {
            total_compressed as f64 / total_original as f64
        } else {
            1.0
        };
        (
            compressed_memories,
            CompressionStats {
                original_size: total_original,
                compressed_size: total_compressed,
                ratio,
                method: method.into(),
                memories_compressed: memories.len(),
                ..Default::default()
            },
        )
    }

    /// Decompress content using the specified method.
    pub fn decompress(&self, content: &str, method: &str) -> String {
        match method {
            "dict" => {
                let mut result = content.to_string();
                for (phrase, token) in &self.dictionary {
                    if token.is_empty() {
                        continue;
                    }
                    result = result.replace(token.as_str(), phrase.as_str());
                }
                result
            }
            _ => content.to_string(),
        }
    }
}

// ── Pattern detection ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedPattern {
    pub pattern_type: String,
    pub description: String,
    pub confidence: f64,
    pub samples: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

const CONTENT_STOPWORDS: &[&str] = &[
    "about", "after", "before", "being", "could", "doing", "every", "having", "might", "other",
    "should", "their", "there", "these", "those", "through", "under", "where", "which", "while",
    "would", "mnemopi", "memory", "memories",
];

fn extract_words(text: &str, min_len: usize) -> Vec<String> {
    let stopwords: HashSet<&str> = CONTENT_STOPWORDS.iter().copied().collect();
    let lowered = text.to_lowercase();
    lowered
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| w.len() >= min_len && !stopwords.contains(*w))
        .map(String::from)
        .collect()
}

fn most_common<K: std::hash::Hash + Eq + Clone>(
    counter: &HashMap<K, usize>,
    limit: usize,
) -> Vec<(K, usize)> {
    let mut entries: Vec<(K, usize)> = counter.iter().map(|(k, v)| (k.clone(), *v)).collect();
    entries.sort_by_key(|b| std::cmp::Reverse(b.1));
    entries.truncate(limit);
    entries
}

/// Pattern detector for temporal, content, and sequence patterns.
pub struct PatternDetector {
    pub min_confidence: f64,
}

impl Default for PatternDetector {
    fn default() -> Self {
        Self {
            min_confidence: 0.6,
        }
    }
}

impl PatternDetector {
    pub fn new(min_confidence: f64) -> Self {
        Self { min_confidence }
    }

    /// Detect temporal patterns (hour-of-day and day-of-week frequency).
    pub fn detect_temporal(&self, memories: &[MemoryRecord]) -> Vec<DetectedPattern> {
        let mut patterns = Vec::new();
        let mut timestamps: Vec<chrono::DateTime<chrono::Utc>> = Vec::new();

        for mem in memories {
            if let Some(ts) = timestamp_of(mem)
                && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts)
            {
                timestamps.push(dt.with_timezone(&chrono::Utc));
            }
        }

        if timestamps.len() < 3 {
            return patterns;
        }
        let total = timestamps.len();

        // Hour-of-day patterns
        let mut hour_counts: HashMap<u32, usize> = HashMap::new();
        for ts in &timestamps {
            *hour_counts
                .entry(ts.format("%H").to_string().parse::<u32>().unwrap_or(0))
                .or_default() += 1;
        }
        for (hour, count) in most_common(&hour_counts, 3) {
            let confidence = count as f64 / total as f64;
            if confidence >= self.min_confidence {
                patterns.push(DetectedPattern {
                    pattern_type: "temporal".into(),
                    description: format!(
                        "Memories frequently created at {hour:02}:00 ({count}/{total} times)"
                    ),
                    confidence,
                    samples: timestamps
                        .iter()
                        .filter(|ts| {
                            ts.format("%H").to_string().parse::<u32>().unwrap_or(0) == hour
                        })
                        .take(3)
                        .map(|ts| ts.to_rfc3339())
                        .collect(),
                    metadata: HashMap::from([
                        ("hour".into(), serde_json::json!(hour)),
                        ("count".into(), serde_json::json!(count)),
                        ("total".into(), serde_json::json!(total)),
                    ]),
                });
            }
        }

        // Day-of-week patterns
        let day_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let mut day_counts: HashMap<usize, usize> = HashMap::new();
        for ts in &timestamps {
            let dow = ts.weekday().num_days_from_monday() as usize;
            *day_counts.entry(dow).or_default() += 1;
        }
        for (day, count) in most_common(&day_counts, 2) {
            let confidence = count as f64 / total as f64;
            if confidence >= self.min_confidence
                && let Some(&name) = day_names.get(day)
            {
                patterns.push(DetectedPattern {
                    pattern_type: "temporal".into(),
                    description: format!(
                        "Memories frequently created on {name} ({count}/{total} times)"
                    ),
                    confidence,
                    samples: timestamps
                        .iter()
                        .filter(|ts| ts.weekday().num_days_from_monday() as usize == day)
                        .take(3)
                        .map(|ts| ts.to_rfc3339())
                        .collect(),
                    metadata: HashMap::from([
                        ("day".into(), serde_json::json!(name)),
                        ("count".into(), serde_json::json!(count)),
                        ("total".into(), serde_json::json!(total)),
                    ]),
                });
            }
        }

        patterns
    }

    /// Detect content patterns (frequent words and co-occurring topics).
    pub fn detect_content(&self, memories: &[MemoryRecord]) -> Vec<DetectedPattern> {
        let mut patterns = Vec::new();
        let all_text = memories
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let words = extract_words(&all_text, 5);
        let mut word_counts: HashMap<String, usize> = HashMap::new();
        for word in &words {
            *word_counts.entry(word.clone()).or_default() += 1;
        }
        let total_words = words.len();

        for (word, count) in most_common(&word_counts, 5) {
            let confidence = (count as f64 / (3.0_f64).max(total_words as f64 * 0.05)).min(1.0);
            if count >= 2 && confidence >= self.min_confidence {
                let word_lower = word.to_lowercase();
                patterns.push(DetectedPattern {
                    pattern_type: "content".into(),
                    description: format!("Frequent topic: '{word}' appears {count} times"),
                    confidence,
                    samples: memories
                        .iter()
                        .filter(|m| m.content.to_lowercase().contains(&word_lower))
                        .take(3)
                        .map(|m| m.content.clone())
                        .collect(),
                    metadata: HashMap::from([
                        ("word".into(), serde_json::json!(word)),
                        ("count".into(), serde_json::json!(count)),
                    ]),
                });
            }
        }

        // Co-occurrence patterns
        if memories.len() >= 3 {
            let mut cooccurrence: HashMap<String, usize> = HashMap::new();
            let mut pair_words: HashMap<String, (String, String)> = HashMap::new();

            for mem in memories {
                let mem_words: Vec<String> = extract_words(&mem.content.to_lowercase(), 5);
                let unique: HashSet<&str> = mem_words.iter().map(|s| s.as_str()).collect();
                let sorted: Vec<&str> = {
                    let mut s: Vec<&str> = unique.into_iter().collect();
                    s.sort();
                    s
                };
                for i in 0..sorted.len() {
                    for j in (i + 1)..sorted.len() {
                        let key = format!("{}\0{}", sorted[i], sorted[j]);
                        pair_words.insert(key.clone(), (sorted[i].into(), sorted[j].into()));
                        *cooccurrence.entry(key).or_default() += 1;
                    }
                }
            }

            for (key, count) in most_common(&cooccurrence, 3) {
                if let Some((w1, w2)) = pair_words.get(&key) {
                    let confidence = (count as f64 / memories.len() as f64).min(1.0);
                    if count >= 2 && confidence >= self.min_confidence {
                        patterns.push(DetectedPattern {
                            pattern_type: "content".into(),
                            description: format!(
                                "Co-occurring topics: '{w1}' + '{w2}' appear together {count} times"
                            ),
                            confidence,
                            samples: memories
                                .iter()
                                .filter(|m| {
                                    let c = m.content.to_lowercase();
                                    c.contains(w1) && c.contains(w2)
                                })
                                .take(3)
                                .map(|m| m.content.clone())
                                .collect(),
                            metadata: HashMap::from([
                                ("word1".into(), serde_json::json!(w1)),
                                ("word2".into(), serde_json::json!(w2)),
                                ("count".into(), serde_json::json!(count)),
                            ]),
                        });
                    }
                }
            }
        }

        patterns
    }

    /// Detect sequence patterns (source A often followed by source B).
    pub fn detect_sequence(&self, memories: &[MemoryRecord]) -> Vec<DetectedPattern> {
        let mut patterns = Vec::new();
        if memories.len() < 3 {
            return patterns;
        }

        let mut sorted: Vec<&MemoryRecord> = memories
            .iter()
            .filter(|m| timestamp_of(m).is_some())
            .collect();
        sorted.sort_by(|a, b| {
            timestamp_of(a)
                .unwrap_or("")
                .cmp(timestamp_of(b).unwrap_or(""))
        });

        let sources: Vec<&str> = sorted.iter().map(|m| m.source.as_str()).collect();
        let mut pair_counts: HashMap<String, usize> = HashMap::new();
        let mut pair_sources: HashMap<String, (String, String)> = HashMap::new();

        for i in 0..sources.len().saturating_sub(1) {
            let key = format!("{}\0{}", sources[i], sources[i + 1]);
            pair_sources.insert(key.clone(), (sources[i].into(), sources[i + 1].into()));
            *pair_counts.entry(key).or_default() += 1;
        }

        for (key, count) in most_common(&pair_counts, 3) {
            if let Some((s1, s2)) = pair_sources.get(&key) {
                let confidence =
                    (count as f64 / (2.0_f64).max(sources.len() as f64 - 1.0)).min(1.0);
                if count >= 2 && confidence >= self.min_confidence {
                    let mut samples = Vec::new();
                    for i in 0..sources.len().saturating_sub(1) {
                        if sources[i] == s1.as_str() && sources[i + 1] == s2.as_str() {
                            if let (Some(first), Some(second)) = (sorted.get(i), sorted.get(i + 1))
                            {
                                let f: String = first.content.chars().take(50).collect();
                                let s: String = second.content.chars().take(50).collect();
                                samples.push(format!("{f}... -> {s}..."));
                            }
                            if samples.len() >= 2 {
                                break;
                            }
                        }
                    }
                    patterns.push(DetectedPattern {
                        pattern_type: "sequence".into(),
                        description: format!(
                            "Sequence pattern: '{s1}' often followed by '{s2}' ({count} times)"
                        ),
                        confidence,
                        samples,
                        metadata: HashMap::from([
                            ("source1".into(), serde_json::json!(s1)),
                            ("source2".into(), serde_json::json!(s2)),
                            ("count".into(), serde_json::json!(count)),
                        ]),
                    });
                }
            }
        }

        patterns
    }

    /// Detect all pattern types, sorted by confidence descending.
    pub fn detect_all(&self, memories: &[MemoryRecord]) -> Vec<DetectedPattern> {
        let mut patterns = Vec::new();
        patterns.extend(self.detect_temporal(memories));
        patterns.extend(self.detect_content(memories));
        patterns.extend(self.detect_sequence(memories));
        patterns.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        patterns
    }

    /// Summarize detected patterns as a JSON-like map.
    pub fn summarize_patterns(&self, memories: &[MemoryRecord]) -> serde_json::Value {
        let patterns = self.detect_all(memories);
        let top = patterns.first();
        serde_json::json!({
            "total_memories": memories.len(),
            "patterns_found": patterns.len(),
            "temporal_patterns": patterns.iter().filter(|p| p.pattern_type == "temporal").map(|p| serde_json::to_value(p).unwrap_or_default()).collect::<Vec<_>>(),
            "content_patterns": patterns.iter().filter(|p| p.pattern_type == "content").map(|p| serde_json::to_value(p).unwrap_or_default()).collect::<Vec<_>>(),
            "sequence_patterns": patterns.iter().filter(|p| p.pattern_type == "sequence").map(|p| serde_json::to_value(p).unwrap_or_default()).collect::<Vec<_>>(),
            "top_pattern": top.map(|p| serde_json::to_value(p).unwrap_or_default()).unwrap_or(serde_json::Value::Null),
        })
    }
}

use chrono::Datelike;

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compressor_dict() {
        let comp = MemoryCompressor::new();
        let input = "remember that the user wants dark theme";
        let (compressed, stats) = comp.compress(input, "dict");
        assert!(compressed_size_le(&compressed, input) || stats.ratio <= 1.0);
        assert_eq!(stats.method, "dict");
    }

    fn compressed_size_le(a: &str, b: &str) -> bool {
        a.len() <= b.len()
    }

    #[test]
    fn test_compressor_rle() {
        let comp = MemoryCompressor::new();
        let input = "aaaaaaaabbbbbbcccc";
        let (compressed, _stats) = comp.compress(input, "rle");
        // RLE should compress repeated chars
        assert!(compressed.contains("[a*8]") || compressed.len() < input.len());
    }

    #[test]
    fn test_compressor_semantic() {
        let comp = MemoryCompressor::new();
        let long = "x".repeat(600);
        let (compressed, stats) = comp.compress(&long, "semantic");
        assert_eq!(stats.method, "semantic");
        assert!(compressed.len() < long.len());
        assert!(compressed.contains("[...]"));
    }

    #[test]
    fn test_compressor_decompress() {
        let comp = MemoryCompressor::new();
        let input = "project context: testing";
        let (compressed, _) = comp.compress(input, "dict");
        let decompressed = comp.decompress(&compressed, "dict");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_compressor_batch() {
        let comp = MemoryCompressor::new();
        let memories = vec![
            MemoryRecord {
                content: "remember that we deployed".into(),
                source: "deploy".into(),
                ..Default::default()
            },
            MemoryRecord {
                content: "remember that we tested".into(),
                source: "test".into(),
                ..Default::default()
            },
        ];
        let (compressed, stats) = comp.compress_batch(&memories, "dict");
        assert_eq!(compressed.len(), 2);
        assert_eq!(stats.memories_compressed, 2);
    }

    #[test]
    fn test_pattern_detector_content() {
        let detector = PatternDetector::new(0.5);
        let memories: Vec<MemoryRecord> = (0..5)
            .map(|i| MemoryRecord {
                content: format!("The deployment strategy for project number {i}"),
                source: "test".into(),
                ..Default::default()
            })
            .collect();
        let patterns = detector.detect_content(&memories);
        // "deployment" and "strategy" should appear
        assert!(!patterns.is_empty());
    }

    #[test]
    fn test_pattern_detector_sequence() {
        let detector = PatternDetector::new(0.5);
        let memories = vec![
            MemoryRecord {
                content: "First step".into(),
                source: "deploy".into(),
                timestamp: Some("2026-01-01T10:00:00Z".into()),
                ..Default::default()
            },
            MemoryRecord {
                content: "Second step".into(),
                source: "test".into(),
                timestamp: Some("2026-01-01T11:00:00Z".into()),
                ..Default::default()
            },
            MemoryRecord {
                content: "Third step".into(),
                source: "deploy".into(),
                timestamp: Some("2026-01-01T12:00:00Z".into()),
                ..Default::default()
            },
            MemoryRecord {
                content: "Fourth step".into(),
                source: "test".into(),
                timestamp: Some("2026-01-01T13:00:00Z".into()),
                ..Default::default()
            },
        ];
        let patterns = detector.detect_sequence(&memories);
        // deploy → test should be detected as a sequence
        assert!(
            patterns
                .iter()
                .any(|p| p.description.contains("deploy") && p.description.contains("test"))
        );
    }

    #[test]
    fn test_pattern_detector_insufficient_data() {
        let detector = PatternDetector::new(0.6);
        let memories = vec![MemoryRecord {
            content: "single memory".into(),
            ..Default::default()
        }];
        assert!(detector.detect_temporal(&memories).is_empty());
        assert!(detector.detect_content(&memories).is_empty());
        assert!(detector.detect_sequence(&memories).is_empty());
    }

    #[test]
    fn test_summarize_patterns() {
        let detector = PatternDetector::new(0.4);
        let memories: Vec<MemoryRecord> = (0..4)
            .map(|i| MemoryRecord {
                content: format!("The infrastructure deployment was successful number {i}"),
                source: if i % 2 == 0 { "deploy" } else { "test" }.into(),
                timestamp: Some(format!("2026-01-0{}T10:00:00Z", i + 1)),
                ..Default::default()
            })
            .collect();
        let summary = detector.summarize_patterns(&memories);
        assert_eq!(summary["total_memories"], 4);
    }
}
