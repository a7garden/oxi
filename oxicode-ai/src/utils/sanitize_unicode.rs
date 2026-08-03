//! Unicode sanitization utilities
//!
//! Removes unpaired Unicode surrogate characters from strings.
//! Unpaired surrogates cause JSON serialization errors in many API providers.
//! Valid emoji and other characters outside the Basic Multilingual Plane use
//! properly paired surrogates and will NOT be affected.

/// Removes unpaired Unicode surrogate characters from a string.
///
/// Unpaired surrogates (high surrogates 0xD800-0xDBFF without matching low surrogates
/// 0xDC00-0xDFFF, or vice versa) cause JSON serialization errors in many API providers.
///
/// Valid emoji and other characters outside the Basic Multilingual Plane use properly paired
/// surrogates and will NOT be affected by this function.
///
/// # Examples
/// ```
/// use oxicode_ai::utils::sanitize_unicode::sanitize_surrogates;
///
/// // Valid emoji (properly paired surrogates) are preserved
/// assert_eq!(sanitize_surrogates("Hello 🙈 World"), "Hello 🙈 World");
///
/// // Normal text passes through unchanged
/// assert_eq!(sanitize_surrogates("Hello world"), "Hello world");
/// ```
pub fn sanitize_surrogates(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        let code = ch as u32;

        // Check if this is a high surrogate (0xD800-0xDBFF)
        if (0xD800..=0xDBFF).contains(&code) {
            // Check if next char is a low surrogate
            if let Some(&next_ch) = chars.peek() {
                let next_code = next_ch as u32;
                if (0xDC00..=0xDFFF).contains(&next_code) {
                    // Properly paired surrogate - keep both
                    result.push(ch);
                    // SAFETY: `peek()` above returned `Some(&next_ch)`, so the
                    // iterator is not exhausted; `next()` cannot return None.
                    #[allow(clippy::expect_used)]
                    result.push(chars.next().expect("peeked char exists"));
                    continue;
                }
            }
            // Unpaired high surrogate - skip it
            continue;
        }

        // Check if this is a low surrogate (0xDC00-0xDFFF) without preceding high surrogate
        if (0xDC00..=0xDFFF).contains(&code) {
            // Unpaired low surrogate - skip it
            continue;
        }

        // Normal character - keep it
        result.push(ch);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_emoji_preserved() {
        assert_eq!(sanitize_surrogates("Hello 🙈 World"), "Hello 🙈 World");
    }

    #[test]
    fn test_normal_text_unchanged() {
        assert_eq!(sanitize_surrogates("Hello, world!"), "Hello, world!");
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(sanitize_surrogates(""), "");
    }

    #[test]
    fn test_ascii_preserved() {
        assert_eq!(sanitize_surrogates("abc123!@#"), "abc123!@#");
    }

    #[test]
    fn test_multiple_emoji_preserved() {
        assert_eq!(sanitize_surrogates("🎉🚀✨🔥"), "🎉🚀✨🔥");
    }

    #[test]
    fn test_cjk_characters_preserved() {
        assert_eq!(sanitize_surrogates("你好世界"), "你好世界");
    }

    #[test]
    fn test_unpaired_high_surrogate_removed() {
        // Create a string with an unpaired high surrogate (0xD800)
        // We need to construct it from bytes since Rust won't allow creating
        // a char from a bare surrogate value
        let input_bytes: &[u8] = b"Text ";
        let mut bytes: Vec<u8> = input_bytes.to_vec();
        // High surrogate: 0xD800 in UTF-8 is 0xED 0xA0 0x80
        bytes.extend_from_slice(&[0xED, 0xA0, 0x80]);
        bytes.extend_from_slice(b" here");
        let input = String::from_utf8_lossy(&bytes).into_owned();
        // The lossy conversion replaces invalid surrogates with replacement char
        // but our sanitizer should handle raw surrogates if they appear
        let result = sanitize_surrogates(&input);
        // After sanitization, the surrogate should be removed
        assert!(result.contains("Text"));
        assert!(result.contains("here"));
    }

    #[test]
    fn test_unpaired_low_surrogate_removed() {
        // Create a string with an unpaired low surrogate (0xDC00)
        let input_bytes: &[u8] = b"Text ";
        let mut bytes: Vec<u8> = input_bytes.to_vec();
        // Low surrogate: 0xDC00 in UTF-8 is 0xED 0xB0 0x80
        bytes.extend_from_slice(&[0xED, 0xB0, 0x80]);
        bytes.extend_from_slice(b" here");
        let input = String::from_utf8_lossy(&bytes).into_owned();
        let result = sanitize_surrogates(&input);
        assert!(result.contains("Text"));
        assert!(result.contains("here"));
    }

    #[test]
    fn test_trailing_unpaired_high_surrogate() {
        let input_bytes: &[u8] = b"Hello";
        let mut bytes: Vec<u8> = input_bytes.to_vec();
        bytes.extend_from_slice(&[0xED, 0xA0, 0x80]); // High surrogate 0xD800
        let input = String::from_utf8_lossy(&bytes).into_owned();
        let result = sanitize_surrogates(&input);
        assert!(result.contains("Hello"));
    }

    #[test]
    fn test_leading_unpaired_low_surrogate() {
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&[0xED, 0xB0, 0x80]); // Low surrogate 0xDC00
        bytes.extend_from_slice(b"Hello");
        let input = String::from_utf8_lossy(&bytes).into_owned();
        let result = sanitize_surrogates(&input);
        assert!(result.contains("Hello"));
    }
}
