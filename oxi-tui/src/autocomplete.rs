//! Autocomplete module - provides fuzzy matching and completion logic.
//!
//! This module provides utilities for text completion including fuzzy
//! matching for intelligent suggestions.

/// FuzzyMatcher provides approximate string matching for autocomplete.
/// It scores candidates based on character matching with support for
/// non-contiguous matches and partial path matching.

#[derive(Debug, Clone, Default)]
pub struct FuzzyMatcher {
    // Configuration could be added here (case sensitivity, etc.)
}

impl FuzzyMatcher {
    /// Create a new fuzzy matcher with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a pattern matches a candidate with a score.
    /// Returns the match score (higher = better) if matched, None otherwise.
    ///
    /// Matching rules:
    /// - All pattern characters must appear in order in the candidate (non-consecutive)
    /// - Consecutive matches score higher
    /// - Matching at word boundaries scores higher
    /// - Case insensitive by default
    /// - '_' or ' ' in pattern acts as wildcard matching any character
    pub fn matches(&self, pattern: &str, candidate: &str) -> Option<usize> {
        if pattern.is_empty() {
            return Some(100); // Empty pattern always matches
        }

        let pattern_lower: Vec<char> = pattern.to_lowercase().chars().collect();
        let candidate_lower: Vec<char> = candidate.to_lowercase().chars().collect();

        let mut pattern_idx = 0;
        let mut score: usize = 0;
        let mut last_match_pos: Option<usize> = None;
        let mut consecutive_bonus = 0;

        for (i, c) in candidate_lower.iter().enumerate() {
            if pattern_idx >= pattern_lower.len() {
                break;
            }

            let pchar = pattern_lower[pattern_idx];
            
            // Check if pattern char matches or is wildcard
            let is_wildcard = pchar == '_' || pchar == ' ';
            let matches = is_wildcard || *c == pchar;
            
            if matches {
                // Base score for match
                score += 10;
                
                // Bonus for consecutive matches
                if !is_wildcard {
                    if let Some(last) = last_match_pos {
                        if last + 1 == i {
                            consecutive_bonus += 5;
                            score += consecutive_bonus;
                        } else {
                            consecutive_bonus = 0;
                        }
                    }
                }

                // Bonus for matching at start
                if i == 0 {
                    score += 15;
                }

                // Bonus for matching after path separator
                if i > 0 && Self::is_path_separator(candidate, i) {
                    score += 10;
                }

                last_match_pos = Some(i);
                pattern_idx += 1;
            }
        }

        // All pattern characters must be found
        if pattern_idx == pattern_lower.len() {
            // Bonus for shorter candidates (more exact matches)
            score += (50 as usize).saturating_sub(candidate.len().min(50));
            
            Some(score)
        } else {
            None
        }
    }

    /// Check if a character at position is a path separator.
    fn is_path_separator(candidate: &str, pos: usize) -> bool {
        candidate.chars().nth(pos).map_or(false, |c| c == '/' || c == '\\')
    }

    /// Match a pattern against multiple candidates and return sorted results.
    /// Returns vector of (score, candidate) pairs sorted by score descending.
    pub fn match_many(&self, pattern: &str, candidates: &[String]) -> Vec<(usize, String)> {
        let mut results: Vec<(usize, String)> = candidates
            .iter()
            .filter_map(|c| {
                self.matches(pattern, c)
                    .map(|score| (score, c.clone()))
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| b.0.cmp(&a.0));
        
        results
    }

    /// Check if pattern matches the start of candidate.
    pub fn starts_with(&self, pattern: &str, candidate: &str) -> bool {
        candidate.to_lowercase().starts_with(&pattern.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let matcher = FuzzyMatcher::new();
        assert!(matcher.matches("foo", "foo").is_some());
    }

    #[test]
    fn test_prefix_match() {
        let matcher = FuzzyMatcher::new();
        let result = matcher.matches("sr", "src/main.rs");
        assert!(result.is_some());
    }

    #[test]
    fn test_fuzzy_match() {
        let matcher = FuzzyMatcher::new();
        // s_m should match s-r-c by skipping the hyphen as wildcard
        // But 'm' must appear after 's' - our non-consecutive matching handles this
        let result = matcher.matches("s_m", "src/main.rs");
        assert!(result.is_some());
    }

    #[test]
    fn test_no_match() {
        let matcher = FuzzyMatcher::new();
        assert!(matcher.matches("xyz", "abc").is_none());
    }

    #[test]
    fn test_empty_pattern() {
        let matcher = FuzzyMatcher::new();
        assert!(matcher.matches("", "anything").is_some());
    }

    #[test]
    fn test_case_insensitive() {
        let matcher = FuzzyMatcher::new();
        assert!(matcher.matches("FOO", "foo").is_some());
    }

    #[test]
    fn test_match_many() {
        let matcher = FuzzyMatcher::new();
        let candidates = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "tests/test.rs".to_string(),
        ];
        let results = matcher.match_many("src", &candidates);
        assert!(!results.is_empty());
        // src/main.rs and src/lib.rs should match
        assert!(results.iter().any(|(_, s)| s.contains("src")));
    }
}