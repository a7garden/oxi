//! Secret obfuscator — ported from omp
//! `packages/coding-agent/src/secrets/obfuscator.ts`.
//!
//! MIT — attribution: adapted from
//! [omp](https://github.com/can1357/oh-my-pi) (Can Berk Güder, earendil-works).
//!
//! ## Purpose
//!
//! When building LLM context (system prompts, tool results, session
//! logs), API keys and other secrets may leak into the text the model
//! sees. This module provides [`SecretObfuscator`] — a bidirectional
//! text scrubber that replaces known secret strings with stable
//! `#XXXX#` placeholders before the text reaches the model, then
//! restores the originals on the way back.
//!
//! ## Modes
//!
//! - **Obfuscate** (default): replaces the secret with a `#XXXX#`
//!   placeholder. The original is recoverable via
//!   [`deobfuscate`](SecretObfuscator::deobfuscate). Use this for
//!   API keys that appear in tool output.
//! - **Replace**: replaces the secret with a deterministic same-length
//!   random-looking string. The original is NOT recoverable. Use this
//!   for secrets that must never be stored (even as a placeholder
//!   mapping).
//!
//! ## Plain mode only (MVP)
//!
//! This implementation handles **plain string** secrets only — exact
//! matches. Regex-based pattern detection (for credit cards, SSNs, etc.)
//! is a future extension; omp's `compileSecretRegex` path is documented
//! in the TODO.

use sha2::{Digest, Sha256};

/// A single secret entry to register with the obfuscator.
#[derive(Debug, Clone)]
pub struct SecretEntry {
    /// The secret string to detect and replace.
    pub content: String,
    /// `"obfuscate"` (default, reversible) or `"replace"` (one-way).
    pub mode: SecretMode,
    /// For `"replace"` mode: the replacement text. If `None`, a
    /// deterministic same-length replacement is generated.
    pub replacement: Option<String>,
}

/// How a secret is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecretMode {
    /// Replace with a `#XXXX#` placeholder. Reversible via
    /// [`SecretObfuscator::deobfuscate`].
    #[default]
    Obfuscate,
    /// Replace with a deterministic string. NOT reversible.
    Replace,
}

/// Placeholder format: `#` + 4 uppercase-hex chars + `#`.
const PLACEHOLDER_LEN: usize = 6; // #XXXX#
const HASH_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Minimum secret length for obfuscation. Secrets shorter than this are
/// skipped to avoid false matches on common short words (e.g. "esp").
const MIN_SECRET_LEN: usize = 8;

/// Characters used for deterministic replacements.
const REPLACEMENT_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Build a deterministic `#XXXX#` placeholder for index `n`.
fn build_placeholder(n: usize) -> String {
    let hash = Sha256::digest(format!("oxi-secret-placeholder-{n}").as_bytes());
    let mut tag = String::with_capacity(PLACEHOLDER_LEN);
    tag.push('#');
    for i in 0..4 {
        let byte = hash[i] as usize;
        tag.push(HASH_CHARS[byte % HASH_CHARS.len()] as char);
    }
    tag.push('#');
    tag
}

/// Generate a deterministic, same-length replacement string from a
/// secret value. The output looks random but is reproducible — the same
/// secret always maps to the same replacement.
fn deterministic_replacement(secret: &str) -> String {
    let hash = Sha256::digest(secret.as_bytes());
    let chars: Vec<char> = secret.chars().collect();
    let mut out = String::with_capacity(secret.len());
    for (i, _) in chars.iter().enumerate() {
        // Mix the hash with the position to produce per-character
        // variation.
        let h = hash[i % hash.len()]
            .wrapping_mul((i as u8).wrapping_add(1))
            .wrapping_add(0x9e);
        out.push(REPLACEMENT_CHARS[h as usize % REPLACEMENT_CHARS.len()] as char);
    }
    out
}

/// Bidirectional secret obfuscator.
///
/// Construct with a list of [`SecretEntry`] values, then call
/// [`obfuscate`](Self::obfuscate) on text before it reaches the LLM and
/// [`deobfuscate`](Self::deobfuscate) on text coming back.
pub struct SecretObfuscator {
    /// Obfuscate-mode: secret → index.
    plain_to_index: std::collections::HashMap<String, usize>,
    /// Obfuscate-mode: index → (secret, placeholder).
    obfuscate_mappings: std::collections::HashMap<usize, (String, String)>,
    /// Reverse lookup: placeholder → secret.
    deobfuscate_map: std::collections::HashMap<String, String>,
    /// Replace-mode: secret → replacement.
    replace_mappings: std::collections::HashMap<String, String>,
    /// Whether any real secrets were configured.
    has_any: bool,
}

impl std::fmt::Debug for SecretObfuscator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretObfuscator")
            .field("secret_count", &self.obfuscate_mappings.len())
            .field("replace_count", &self.replace_mappings.len())
            .field("has_any", &self.has_any)
            .finish_non_exhaustive()
    }
}

impl Default for SecretObfuscator {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl SecretObfuscator {
    /// Construct from a list of secret entries.
    ///
    /// Secrets shorter than 8 chars in obfuscate mode are silently
    /// skipped (false-positive avoidance). Invalid regex entries (future
    /// extension) are also skipped.
    pub fn new(entries: &[SecretEntry]) -> Self {
        let mut ob = Self {
            plain_to_index: std::collections::HashMap::new(),
            obfuscate_mappings: std::collections::HashMap::new(),
            deobfuscate_map: std::collections::HashMap::new(),
            replace_mappings: std::collections::HashMap::new(),
            has_any: false,
        };

        for (index, entry) in entries.iter().enumerate() {
            match entry.mode {
                SecretMode::Obfuscate => {
                    if entry.content.len() < MIN_SECRET_LEN {
                        continue;
                    }
                    let placeholder = build_placeholder(index);
                    ob.plain_to_index.insert(entry.content.clone(), index);
                    ob.obfuscate_mappings
                        .insert(index, (entry.content.clone(), placeholder.clone()));
                    ob.deobfuscate_map
                        .insert(placeholder, entry.content.clone());
                    ob.has_any = true;
                }
                SecretMode::Replace => {
                    let replacement = entry
                        .replacement
                        .clone()
                        .unwrap_or_else(|| deterministic_replacement(&entry.content));
                    ob.replace_mappings
                        .insert(entry.content.clone(), replacement);
                    ob.has_any = true;
                }
            }
        }

        ob
    }

    /// Returns `true` if any secrets were configured.
    pub fn has_secrets(&self) -> bool {
        self.has_any
    }

    /// Obfuscate all known secrets in `text`.
    ///
    /// Obfuscate-mode secrets are replaced with `#XXXX#` placeholders
    /// (reversible via [`deobfuscate`](Self::deobfuscate)). Replace-mode
    /// secrets are replaced with deterministic strings (NOT reversible).
    ///
    /// Processing order: replace-mode first (longest first to handle
    /// prefix overlaps), then obfuscate-mode (longest first).
    pub fn obfuscate(&self, text: &str) -> String {
        if !self.has_any {
            return text.to_string();
        }
        let mut result = text.to_string();

        // Replace-mode: sort by secret length descending so longer
        // secrets are replaced first (prevents partial matches).
        let mut replace_sorted: Vec<(&String, &String)> = self.replace_mappings.iter().collect();
        replace_sorted.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        for (secret, replacement) in replace_sorted {
            if !result.contains(secret.as_str()) {
                continue;
            }
            result = result.replace(secret.as_str(), replacement.as_str());
        }

        // Obfuscate-mode: same longest-first ordering.
        let mut obfuscate_sorted: Vec<(&String, &usize)> = self.plain_to_index.iter().collect();
        obfuscate_sorted.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        for (secret, index) in obfuscate_sorted {
            if !result.contains(secret.as_str()) {
                continue;
            }
            if let Some((_, placeholder)) = self.obfuscate_mappings.get(index) {
                result = result.replace(secret.as_str(), placeholder.as_str());
            }
        }

        result
    }

    /// Deobfuscate `#XXXX#` placeholders back to their original secrets.
    ///
    /// Only obfuscate-mode placeholders are reversed. Replace-mode
    /// replacements are permanent (by design — the original is never
    /// recoverable).
    pub fn deobfuscate(&self, text: &str) -> String {
        if !self.has_any || !text.contains('#') {
            return text.to_string();
        }
        let mut result = text.to_string();
        for (placeholder, secret) in &self.deobfuscate_map {
            if result.contains(placeholder.as_str()) {
                result = result.replace(placeholder.as_str(), secret.as_str());
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(content: &str, mode: SecretMode) -> SecretEntry {
        SecretEntry {
            content: content.to_string(),
            mode,
            replacement: None,
        }
    }

    #[test]
    fn empty_entries_passthrough() {
        let ob = SecretObfuscator::new(&[]);
        assert!(!ob.has_secrets());
        assert_eq!(ob.obfuscate("hello world"), "hello world");
        assert_eq!(ob.deobfuscate("hello world"), "hello world");
    }

    #[test]
    fn obfuscate_replaces_long_secret() {
        let ob =
            SecretObfuscator::new(&[entry("sk-ant-api03-abcdef123456", SecretMode::Obfuscate)]);
        let text = "Bearer sk-ant-api03-abcdef123456";
        let obfuscated = ob.obfuscate(text);
        assert!(obfuscated.contains("Bearer"));
        assert!(!obfuscated.contains("sk-ant-api03"));
        assert!(obfuscated.contains("#"));
    }

    #[test]
    fn deobfuscate_restores_original() {
        let ob =
            SecretObfuscator::new(&[entry("sk-ant-api03-abcdef123456", SecretMode::Obfuscate)]);
        let original = "Bearer sk-ant-api03-abcdef123456";
        let obfuscated = ob.obfuscate(original);
        let restored = ob.deobfuscate(&obfuscated);
        assert_eq!(restored, original);
    }

    #[test]
    fn short_secret_skipped() {
        // Secrets < 8 chars are silently skipped.
        let ob = SecretObfuscator::new(&[entry("short", SecretMode::Obfuscate)]);
        assert!(!ob.has_secrets());
        assert_eq!(ob.obfuscate("has short word"), "has short word");
    }

    #[test]
    fn replace_mode_not_reversible() {
        let ob = SecretObfuscator::new(&[entry("sk-ant-api03-abcdef123456", SecretMode::Replace)]);
        let original = "Bearer sk-ant-api03-abcdef123456";
        let obfuscated = ob.obfuscate(original);
        assert!(!obfuscated.contains("sk-ant-api03"));
        // Deobfuscate is a no-op for replace-mode.
        let restored = ob.deobfuscate(&obfuscated);
        assert_eq!(restored, obfuscated);
    }

    #[test]
    fn replace_mode_with_custom_replacement() {
        let ob = SecretObfuscator::new(&[SecretEntry {
            content: "sk-ant-api03-abcdef123456".into(),
            mode: SecretMode::Replace,
            replacement: Some("[REDACTED]".into()),
        }]);
        let obfuscated = ob.obfuscate("key: sk-ant-api03-abcdef123456");
        assert_eq!(obfuscated, "key: [REDACTED]");
    }

    #[test]
    fn multiple_secrets_all_replaced() {
        let ob = SecretObfuscator::new(&[
            entry("sk-ant-api03-key1abcdef", SecretMode::Obfuscate),
            entry("sk-openai-key2ghijkl", SecretMode::Obfuscate),
        ]);
        let text = "keys: sk-ant-api03-key1abcdef and sk-openai-key2ghijkl";
        let obfuscated = ob.obfuscate(text);
        assert!(!obfuscated.contains("sk-ant-api03"));
        assert!(!obfuscated.contains("sk-openai"));
        let restored = ob.deobfuscate(&obfuscated);
        assert_eq!(restored, text);
    }

    #[test]
    fn longest_first_ordering() {
        // If one secret is a prefix of another, the longer one should be
        // replaced first so the shorter one doesn't partially match.
        let ob = SecretObfuscator::new(&[
            entry("sk-ant-api03-key123456789", SecretMode::Obfuscate),
            entry("sk-ant-api03-key123456789-extra", SecretMode::Obfuscate),
        ]);
        let text = "found sk-ant-api03-key123456789-extra here";
        let obfuscated = ob.obfuscate(text);
        // Both replaced; no partial leak.
        assert!(!obfuscated.contains("sk-ant-api03"));
    }

    #[test]
    fn deterministic_replacement_is_stable() {
        let r1 = deterministic_replacement("sk-ant-api03-key");
        let r2 = deterministic_replacement("sk-ant-api03-key");
        assert_eq!(r1, r2);
        assert_eq!(r1.len(), "sk-ant-api03-key".len());
    }

    #[test]
    fn build_placeholder_format() {
        let p0 = build_placeholder(0);
        let p1 = build_placeholder(1);
        assert!(p0.starts_with('#'));
        assert!(p0.ends_with('#'));
        assert_eq!(p0.len(), PLACEHOLDER_LEN);
        assert_ne!(p0, p1, "different indices produce different placeholders");
    }

    #[test]
    fn deobfuscate_without_placeholders_is_passthrough() {
        let ob =
            SecretObfuscator::new(&[entry("sk-ant-api03-abcdef123456", SecretMode::Obfuscate)]);
        // Text with no placeholders and no '#' chars.
        assert_eq!(ob.deobfuscate("plain text"), "plain text");
    }

    #[test]
    fn debug_does_not_leak_secrets() {
        let ob = SecretObfuscator::new(&[entry(
            "sk-super-secret-key1234567890",
            SecretMode::Obfuscate,
        )]);
        let dbg = format!("{ob:?}");
        assert!(!dbg.contains("sk-super-secret"));
    }
}
