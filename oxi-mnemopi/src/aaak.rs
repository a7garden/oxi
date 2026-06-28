//! aaak text encoding — lossy compression for episodic consolidation summaries.
//!
//! Ported from omp's `core/aaak.ts`. Replaces long category names, common
//! phrases, and structural connectors with compact tokens, dramatically
//! reducing the byte size of consolidated memories while preserving
//! semantic content.
//!
//! MIT — attribution: adapted from [omp](https://github.com/earendil-works/pi)
//! `packages/mnemopi/src/core/aaak.ts`.

use std::collections::HashMap;

/// Category → short prefix mapping.
pub const CATEGORY_MAP: &[(&str, &str)] = &[
    ("PREFERENCE", "PREF"),
    ("TRAIT", "TRAIT"),
    ("STATUS", "STAT"),
    ("INSTRUCTION", "INST"),
    ("PROJECT", "PROJ"),
    ("LOCATION", "LOC"),
    ("FAMILY", "FAM"),
    ("OCCUPATION", "OCC"),
    ("DECISION", "DEC"),
    ("EVENT", "EVT"),
    ("TOOL", "TOOL"),
    ("FACT", "FACT"),
    ("OPINION", "OPN"),
];

/// Long phrase → shorthand mapping. Applied in order of **descending
/// phrase length** so that longer phrases match before their substrings.
pub const PHRASE_MAP: &[(&str, &str)] = &[
    ("User asked for ", "ASK "),
    ("User requested ", "REQ "),
    ("User prefers ", "PREF "),
    ("User dislikes ", "DISLIKE "),
    ("User voice message ", "VM "),
    ("User stack: ", "STACK|"),
    ("User asked ", "ASK "),
    ("User wants ", "WANT "),
    ("User likes ", "LIKE "),
    ("User built ", "BUILT "),
    ("User email is ", "@"),
    ("Married to ", "MARRIED→"),
    ("Full-stack developer", "FSDEV"),
    ("AI Systems Engineer", "AIENG"),
    ("Software Developer", "SDEV"),
    ("self-hosted", "selfhost"),
    ("transcription", "transc"),
    ("translation", "transl"),
    ("automation", "auto"),
    ("bilingual", "bi"),
    ("Bilingual", "bi"),
    ("real-time", "RT"),
    ("Real-time", "RT"),
    ("Email: ", "@"),
    ("GitHub: ", "GH:"),
    ("Phone: ", "PH:"),
    ("Location: ", "LOC:"),
    ("User is ", "IS "),
    ("User has ", "HAS "),
];

/// Structural connector replacements.
pub const STRUCTURAL_REPLACEMENTS: &[(&str, &str)] = &[
    (" - ", " | "),
    (" -- ", " | "),
    (" | ", " | "),
    (", ", " | "),
    (" and ", "+"),
    (" or ", "/"),
    (" for ", "→"),
    (" to ", "→"),
    (" with ", " w/ "),
    (" over ", ">"),
    (" instead of ", "!>"),
    (" because of ", "∵"),
    (" due to ", "∵"),
    (" using ", "→"),
    (" built ", "→"),
    (" in ", ":"),
    (" at ", "@"),
    (" on ", "@"),
    (" from ", "<-"),
];

/// Build a reverse lookup map (value → key).
fn build_reverse<'a>(map: &'a [(&'a str, &'a str)]) -> HashMap<&'a str, &'a str> {
    map.iter().map(|(k, v)| (*v, *k)).collect()
}

/// Reverse category map (prefix → full category name).
pub fn rev_category() -> HashMap<&'static str, &'static str> {
    build_reverse(CATEGORY_MAP)
}

/// Reverse phrase map (shorthand → original phrase).
pub fn rev_phrase() -> HashMap<&'static str, &'static str> {
    build_reverse(PHRASE_MAP)
}

/// If `text` starts with `"CATEGORY: "`, replace with the short prefix +
/// `"|"`.
pub fn apply_category_prefixes(text: &str) -> String {
    for (full, short) in CATEGORY_MAP {
        let prefix = format!("{full}: ");
        if let Some(rest) = text.strip_prefix(&prefix) {
            return format!("{short}|{rest}");
        }
    }
    text.to_string()
}

/// Apply phrase substitutions in descending key-length order so longer
/// phrases are matched first.
pub fn apply_phrases(text: &str) -> String {
    // PHRASE_MAP is already sorted by descending key length in the const,
    // but let's be safe.
    let mut entries: Vec<(&str, &str)> = PHRASE_MAP.to_vec();
    entries.sort_by_key(|b| std::cmp::Reverse(b.0.len()));

    let mut result = text.to_string();
    for (phrase, shorthand) in &entries {
        result = result.replace(phrase, shorthand);
    }
    result
}

/// Apply structural connector replacements.
pub fn apply_structural(text: &str) -> String {
    let mut result = text.to_string();
    for (pattern, replacement) in STRUCTURAL_REPLACEMENTS {
        result = result.replace(pattern, replacement);
    }
    result
}

/// Compact parenthesised expressions: remove internal whitespace.
pub fn compact_parens(text: &str) -> String {
    text.replace("( ", "(").replace(" )", ")")
}

/// Encode (compress) a text string using the full aaak pipeline:
/// category prefixes → phrases → structural → parens → word replacements.
///
/// Strings that are already compact (contain `|` and ≤ 3 whitespace-separated
/// tokens) are returned unchanged.
pub fn encode(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }

    // Already compact — skip
    if text.contains('|') && text.split_whitespace().count() <= 3 {
        return text.to_string();
    }

    let mut result = text.trim().to_string();
    result = apply_category_prefixes(&result);
    result = apply_phrases(&result);
    result = apply_structural(&result);
    result = compact_parens(&result);
    result = result.replace("working correctly", "OK");
    result = result.replace("working", "OK");
    result = result.replace("complete", "DONE");
    result = result.replace("completed", "DONE");
    result.trim().to_string()
}

/// Convenience alias matching omp's export name.
pub fn aaak_encode(text: &str) -> String {
    encode(text)
}

/// Decode a compressed string back to its approximate original form.
///
/// This is a best-effort reverse: structural replacements and word
/// shortcuts are inverted, then phrases, then category prefixes.
pub fn decode(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }

    let mut result = text.to_string();

    // Reverse structural replacements (invert each pair)
    for (replacement, pattern) in STRUCTURAL_REPLACEMENTS {
        // Skip the `(" | ", " | ")` identity mapping
        if pattern == replacement {
            continue;
        }
        result = result.replace(replacement, pattern);
    }

    // Reverse phrases
    let mut entries: Vec<(&str, &str)> = PHRASE_MAP.to_vec();
    entries.sort_by_key(|b| std::cmp::Reverse(b.1.len())); // sort by shorthand length desc
    for (phrase, shorthand) in &entries {
        result = result.replace(shorthand, phrase);
    }

    // Reverse category prefixes: "PREF|..." → "PREFERENCE: ..."
    for (full, short) in CATEGORY_MAP {
        let prefix = format!("{short}|");
        if let Some(rest) = result.strip_prefix(&prefix) {
            result = format!("{full}: {rest}");
            break;
        }
    }

    // Reverse word shortcuts
    result = result.replace("DONE", "completed");
    result = result.replace("OK", "working");

    result
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_empty() {
        assert_eq!(encode(""), "");
    }

    #[test]
    fn test_encode_already_compact() {
        let compact = "PREF|dark-theme";
        assert_eq!(encode(compact), compact);
    }

    #[test]
    fn test_encode_category_prefix() {
        let input = "PREFERENCE: dark theme";
        let encoded = encode(input);
        assert!(encoded.starts_with("PREF|"));
        assert!(!encoded.contains("PREFERENCE"));
    }

    #[test]
    fn test_encode_phrases() {
        let input = "User asked for help with the database";
        let encoded = encode(input);
        assert!(encoded.contains("ASK "));
        assert!(!encoded.contains("User asked for "));
    }

    #[test]
    fn test_encode_structural() {
        let input = "Deployed the service and configured the proxy";
        let encoded = encode(input);
        // " and " → "+"
        assert!(encoded.contains('+'));
    }

    #[test]
    fn test_encode_word_replacements() {
        assert!(encode("The task is complete").contains("DONE"));
        assert!(encode("working correctly").contains("OK"));
    }

    #[test]
    fn test_decode_roundtrip_approximate() {
        let input = "PREFERENCE: dark theme";
        let encoded = encode(input);
        let decoded = decode(&encoded);
        // Round-trip isn't exact because structural replacements are
        // many-to-one, but category should be restored.
        assert!(decoded.contains("PREFERENCE") || decoded.contains("PREF"));
    }

    #[test]
    fn test_aaak_encode_alias() {
        assert_eq!(aaak_encode("test complete"), encode("test complete"));
    }

    #[test]
    fn test_apply_phrases_longest_first() {
        // "User asked for " should match before "User asked "
        let input = "User asked for a deployment";
        let result = apply_phrases(input);
        assert!(result.starts_with("ASK "));
        assert!(!result.contains("User asked"));
    }

    #[test]
    fn test_reverse_maps() {
        let rev_cat = rev_category();
        assert_eq!(rev_cat.get("PREF").copied(), Some("PREFERENCE"));

        let rev_phr = rev_phrase();
        // "ASK " maps to both "User asked for " and "User asked "
        let ask_val = rev_phr.get("ASK ").copied();
        assert!(ask_val == Some("User asked for ") || ask_val == Some("User asked "));
    }
}
