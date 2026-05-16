//! Fuzzy string matching for autocomplete and filtering.
//!
//! Provides character-by-character fuzzy matching with gap penalties,
//! case-insensitive comparison, and score-based ranking.

/// Result of a fuzzy match, containing a relevance score and the positions
/// of matched characters in the original text.
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyResult {
    /// Match quality score (higher is better).
    pub score: f64,
    /// Indices of matched characters in the original text.
    pub positions: Vec<usize>,
}

/// Perform a fuzzy match of `pattern` against `text`.
///
/// Returns `Some(FuzzyResult)` if all pattern characters appear in order
/// within the text (case-insensitive), or `None` otherwise.
///
/// Scoring rules:
/// - Each matched character contributes a base score.
/// - Consecutive matches receive a bonus that increases with streak length.
/// - A gap between matched characters incurs a penalty proportional to gap size.
/// - Matching at the start of the text receives a bonus.
/// - Matching after a word boundary (`_`, `-`, `.`, ` `, `/`) receives a bonus.
/// - Shorter overall texts receive a small length bonus (prefer compact matches).
pub fn fuzzy_match(pattern: &str, text: &str) -> Option<FuzzyResult> {
    if pattern.is_empty() {
        return Some(FuzzyResult {
            score: 1.0,
            positions: Vec::new(),
        });
    }

    let pattern_chars: Vec<char> = pattern.to_lowercase().chars().collect();
    let text_chars: Vec<char> = text.to_lowercase().chars().collect();

    if pattern_chars.len() > text_chars.len() {
        return None;
    }

    let mut positions = Vec::with_capacity(pattern_chars.len());
    let mut score: f64 = 0.0;
    let mut consecutive = 0u32;
    let mut last_match_idx: Option<usize> = None;

    let mut pi = 0;
    for (ti, &tc) in text_chars.iter().enumerate() {
        if pi >= pattern_chars.len() {
            break;
        }
        if tc == pattern_chars[pi] {
            // Base score for a match
            score += 1.0;

            // Consecutive match bonus
            if let Some(last) = last_match_idx {
                if last + 1 == ti {
                    consecutive += 1;
                    score += 0.5 * consecutive as f64;
                } else {
                    // Gap penalty: larger gaps hurt more
                    let gap = (ti - last) as f64;
                    score -= gap * 0.1;
                    consecutive = 0;
                }
            } else {
                consecutive = 1;
            }

            // Start-of-text bonus
            if ti == 0 {
                score += 1.0;
            }

            // Word-boundary bonus
            if ti > 0 && is_word_boundary(text_chars[ti - 1]) {
                score += 0.8;
            }

            positions.push(ti);
            last_match_idx = Some(ti);
            pi += 1;
        }
    }

    if pi == pattern_chars.len() {
        // Length bonus: prefer shorter texts for the same pattern
        let text_len = text_chars.len() as f64;
        score += 1.0 / (1.0 + text_len * 0.05);

        Some(FuzzyResult { score, positions })
    } else {
        None
    }
}

/// Check if a character is considered a word boundary separator.
fn is_word_boundary(c: char) -> bool {
    c == '_' || c == '-' || c == '.' || c == ' ' || c == '/'
}

/// Rank a slice of strings by fuzzy matching against a pattern.
///
/// Returns a sorted vector of `(FuzzyResult, &str)` pairs, best matches first.
/// Items that do not match are excluded.
pub fn fuzzy_rank<'a>(pattern: &str, candidates: &'a [String]) -> Vec<(FuzzyResult, &'a str)> {
    let mut results: Vec<(FuzzyResult, &'a str)> = candidates
        .iter()
        .filter_map(|c| fuzzy_match(pattern, c).map(|r| (r, c.as_str())))
        .collect();

    results.sort_by(|a, b| {
        b.0.score
            .partial_cmp(&a.0.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let result = fuzzy_match("abc", "abc").unwrap();
        assert!(!result.positions.is_empty());
        assert_eq!(result.positions, vec![0, 1, 2]);
        assert!(result.score > 0.0);
    }

    #[test]
    fn test_case_insensitive() {
        let result = fuzzy_match("ABC", "abc").unwrap();
        assert_eq!(result.positions.len(), 3);

        let result2 = fuzzy_match("abc", "ABC").unwrap();
        assert_eq!(result2.positions.len(), 3);
    }

    #[test]
    fn test_no_match() {
        assert!(fuzzy_match("xyz", "abc").is_none());
    }

    #[test]
    fn test_empty_pattern_matches_everything() {
        let result = fuzzy_match("", "anything").unwrap();
        assert!(result.positions.is_empty());
        assert!(result.score > 0.0);
    }

    #[test]
    fn test_subsequence_match_with_positions() {
        // "sr" should match "s" at 0 and "r" at 1 in "src"
        let result = fuzzy_match("sr", "src").unwrap();
        assert_eq!(result.positions, vec![0, 1]);
    }

    #[test]
    fn test_gap_penalty() {
        // Matching consecutive chars should score higher than scattered chars
        let tight = fuzzy_match("abc", "abc").unwrap().score;
        let loose = fuzzy_match("abc", "a_x_b_c").unwrap().score;
        assert!(
            tight > loose,
            "tight score {tight} should exceed loose score {loose}"
        );
    }

    #[test]
    fn test_word_boundary_bonus() {
        // Matching at a word boundary should score higher
        let with_boundary = fuzzy_match("bc", "a_bc").unwrap().score;
        let without_boundary = fuzzy_match("bc", "aabc").unwrap().score;
        assert!(
            with_boundary > without_boundary,
            "boundary score {with_boundary} should exceed non-boundary {without_boundary}"
        );
    }

    #[test]
    fn test_fuzzy_rank_sorts_by_score() {
        let candidates = vec![
            "src/main.rs".to_string(),
            "some/really/long/path/src/main.rs".to_string(),
            "src.rs".to_string(),
        ];
        let ranked = fuzzy_rank("src", &candidates);
        assert!(ranked.len() >= 2);
        // Scores should be descending
        for window in ranked.windows(2) {
            assert!(window[0].0.score >= window[1].0.score);
        }
    }

    #[test]
    fn test_pattern_longer_than_text() {
        assert!(fuzzy_match("abcdef", "abc").is_none());
    }

    // ===== Additional edge-case tests =====

    #[test]
    fn test_start_of_text_bonus() {
        // Matching at start should score higher than matching in middle
        let at_start = fuzzy_match("ab", "abcd").unwrap().score;
        let in_middle = fuzzy_match("ab", "xaby").unwrap().score;
        assert!(
            at_start > in_middle,
            "start score {at_start} should exceed middle score {in_middle}"
        );
    }

    #[test]
    fn test_consecutive_match_bonus() {
        let consecutive = fuzzy_match("abc", "abcd").unwrap().score;
        let scattered = fuzzy_match("abc", "aXbXc").unwrap().score;
        assert!(
            consecutive > scattered,
            "consecutive {consecutive} should exceed scattered {scattered}"
        );
    }

    #[test]
    fn test_fuzzy_rank_excludes_non_matches() {
        let candidates = vec!["hello".to_string(), "world".to_string()];
        let ranked = fuzzy_rank("xyz", &candidates);
        assert!(ranked.is_empty());
    }

    #[test]
    fn test_fuzzy_rank_empty_pattern() {
        let candidates = vec!["hello".to_string(), "world".to_string()];
        let ranked = fuzzy_rank("", &candidates);
        // Empty pattern matches everything
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn test_fuzzy_rank_empty_candidates() {
        let candidates: Vec<String> = vec![];
        let ranked = fuzzy_rank("abc", &candidates);
        assert!(ranked.is_empty());
    }

    #[test]
    fn test_is_word_boundary() {
        assert!(is_word_boundary('_'));
        assert!(is_word_boundary('-'));
        assert!(is_word_boundary('.'));
        assert!(is_word_boundary(' '));
        assert!(is_word_boundary('/'));
        assert!(!is_word_boundary('a'));
        assert!(!is_word_boundary('1'));
    }

    #[test]
    fn test_single_char_pattern() {
        let result = fuzzy_match("a", "abc").unwrap();
        assert_eq!(result.positions, vec![0]);
    }

    #[test]
    fn test_single_char_text_match() {
        let result = fuzzy_match("a", "a").unwrap();
        assert_eq!(result.positions, vec![0]);
    }

    #[test]
    fn test_single_char_text_no_match() {
        assert!(fuzzy_match("b", "a").is_none());
    }

    #[test]
    fn test_unicode_match() {
        let result = fuzzy_match("한", "한글").unwrap();
        assert_eq!(result.positions.len(), 1);
    }

    #[test]
    fn test_length_bonus() {
        // Shorter texts should score slightly higher for same pattern
        let short = fuzzy_match("ab", "ab").unwrap().score;
        let long = fuzzy_match("ab", "ab_extra_padding").unwrap().score;
        assert!(
            short > long,
            "short text score {short} should exceed long text score {long}"
        );
    }
}
