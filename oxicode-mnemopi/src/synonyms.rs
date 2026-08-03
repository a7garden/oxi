//! Synonym expansion and query normalization — ported from omp
//! `core/synonyms.ts`.
//!
//! Provides a static synonym table for common technical terms, a stop-word
//! list, and query normalization for FTS search.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Stop words filtered from queries — ported from omp `STOP_WORDS`.
pub static STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "have", "has", "had", "do",
        "does", "did", "will", "would", "could", "should", "may", "might", "can", "shall", "must",
        "i", "you", "he", "she", "it", "we", "they", "me", "him", "her", "us", "them", "my",
        "your", "his", "its", "our", "their", "mine", "yours", "hers", "ours", "theirs", "what",
        "which", "who", "whom", "where", "when", "why", "how", "this", "that", "these", "those",
        "of", "in", "on", "at", "to", "for", "with", "by", "from", "as", "into", "through",
        "during", "before", "after", "above", "below", "up", "down", "out", "off", "over", "under",
    ]
    .into_iter()
    .collect()
});

/// Synonym groups — ported from omp `SYNONYM_GROUPS`.
///
/// Each canonical word maps to a list of synonyms. Both directions are
/// indexed for lookup.
static SYNONYM_MAP: LazyLock<HashMap<&'static str, Vec<&'static str>>> = LazyLock::new(|| {
    let groups: &[(&str, &[&str])] = &[
        ("database", &["db", "datastore", "data_store"]),
        (
            "password",
            &["pass", "pwd", "passwd", "credential", "secret", "token"],
        ),
        ("config", &["configuration", "settings", "cfg", "setup"]),
        (
            "error",
            &["bug", "issue", "fault", "failure", "crash", "exception"],
        ),
        (
            "fix",
            &["repair", "resolve", "solve", "patch", "correct", "address"],
        ),
        (
            "deploy",
            &["deployment", "release", "ship", "push", "rollout"],
        ),
        ("server", &["host", "machine", "vm", "instance", "node"]),
        ("api", &["endpoint", "interface", "service"]),
        ("key", &["token", "credential", "secret", "api_key"]),
        ("user", &["account", "profile", "identity", "person"]),
        (
            "model",
            &["llm", "ai", "provider", "gpt", "claude", "gemini"],
        ),
        (
            "speed",
            &["fast", "quick", "performance", "latency", "throughput"],
        ),
        ("memory", &["recall", "remember", "storage", "retention"]),
        ("search", &["find", "lookup", "query", "retrieve", "locate"]),
        ("file", &["document", "doc", "text", "note"]),
        ("code", &["script", "program", "source", "implementation"]),
        ("test", &["verify", "check", "validate", "probe", "examine"]),
        ("backup", &["snapshot", "copy", "save", "archive"]),
        ("install", &["setup", "configure", "bootstrap", "init"]),
        ("update", &["upgrade", "refresh", "renew", "sync"]),
        (
            "delete",
            &["remove", "destroy", "purge", "clean", "wipe", "erase"],
        ),
        ("list", &["show", "display", "enumerate", "catalog"]),
        ("time", &["date", "when", "timestamp", "schedule"]),
        ("url", &["link", "address", "uri", "path"]),
        ("health", &["status", "check", "pulse", "alive", "up"]),
        ("service", &["daemon", "process", "systemd", "worker"]),
        ("port", &["socket", "bind", "listen"]),
        (
            "network",
            &["internet", "connection", "connectivity", "dns"],
        ),
        ("ssh", &["terminal", "shell", "remote", "connect"]),
        (
            "git",
            &["commit", "push", "pull", "repo", "repository", "branch"],
        ),
        ("log", &["output", "stdout", "stderr", "trace", "debug"]),
        ("cron", &["schedule", "job", "task", "timer", "periodic"]),
        ("email", &["mail", "message", "inbox", "smtp"]),
        ("image", &["picture", "photo", "screenshot", "graphic"]),
        ("browser", &["web", "page", "site", "navigate", "chrome"]),
        ("monitor", &["watch", "observe", "track", "survey"]),
        ("alert", &["notify", "notification", "warning", "ping"]),
        ("migrate", &["transfer", "move", "relocate", "port"]),
        ("compare", &["diff", "versus", "vs", "contrast"]),
        ("save", &["store", "persist", "preserve", "keep"]),
    ];

    let mut map = HashMap::new();
    for (canonical, syns) in groups {
        // canonical → syns
        map.insert(*canonical, syns.to_vec());
        // each synonym → canonical + other syns
        for &syn in *syns {
            let mut reverse = vec![*canonical];
            for &other in *syns {
                if other != syn {
                    reverse.push(other);
                }
            }
            map.insert(syn, reverse);
        }
    }
    map
});

/// Normalize a query: lowercase, split into tokens, remove stop words.
pub fn normalize_query(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|t| t.len() >= 2 && !STOP_WORDS.contains(t.as_str()))
        .collect()
}

/// Get synonyms for a word (if any).
///
/// Returns the canonical form plus all known synonyms. If the word has no
/// synonyms, returns a single-element vec containing the word itself.
pub fn get_synonyms(word: &str) -> Vec<String> {
    let lower = word.to_lowercase();
    if let Some(syns) = SYNONYM_MAP.get(lower.as_str()) {
        let mut result: Vec<String> = syns.iter().map(|s| s.to_string()).collect();
        if !result.contains(&lower) {
            result.push(lower);
        }
        result
    } else {
        vec![lower]
    }
}

/// Expand a query token for FTS search, including synonyms.
///
/// For each token, returns the token plus its synonyms (for OR-matching).
pub fn expand_token(word: &str) -> Vec<String> {
    get_synonyms(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synonym_lookup() {
        let syns = get_synonyms("db");
        assert!(syns.contains(&"database".to_string()));
        assert!(syns.contains(&"db".to_string()));
    }

    #[test]
    fn synonym_reverse() {
        let syns = get_synonyms("database");
        assert!(syns.contains(&"db".to_string()));
    }

    #[test]
    fn no_synonym_passthrough() {
        let syns = get_synonyms("banana");
        assert_eq!(syns, vec!["banana"]);
    }

    #[test]
    fn normalize_removes_stop_words() {
        let tokens = normalize_query("the quick brown fox");
        assert_eq!(tokens, vec!["quick", "brown", "fox"]);
    }

    #[test]
    fn normalize_empty_for_all_stop_words() {
        let tokens = normalize_query("the is a of");
        assert!(tokens.is_empty());
    }
}
