//! Minimal text-shape normalization: line-ending detection / round-trip and
//! BOM stripping. The patcher uses these to canonicalize text to LF before
//! applying edits and to restore the original shape on write-back.
//!
//! Ported from omp `packages/hashline/src/normalize.ts`.

/// Line ending style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Crlf,
    Lf,
}

/// Detect the first line ending style in `content`. Defaults to LF when
/// neither is present.
pub fn detect_line_ending(content: &str) -> LineEnding {
    let crlf_idx = content.find("\r\n");
    let lf_idx = content.find('\n');
    match (crlf_idx, lf_idx) {
        (None, None) | (_, None) => LineEnding::Lf,
        (None, Some(_)) => LineEnding::Lf,
        (Some(c), Some(l)) => {
            if c < l {
                LineEnding::Crlf
            } else {
                LineEnding::Lf
            }
        }
    }
}

/// Normalize every line ending to LF.
pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Re-encode LF text with the requested line ending.
pub fn restore_line_endings(text: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Crlf => text.replace('\n', "\r\n"),
        LineEnding::Lf => text.to_string(),
    }
}

/// BOM strip result.
pub struct BomResult<'a> {
    /// Either empty or the BOM sequence.
    pub bom: &'a str,
    /// Text with any leading BOM removed.
    pub text: &'a str,
}

/// Strip a UTF-8 BOM if present.
pub fn strip_bom(content: &str) -> BomResult<'_> {
    if let Some(rest) = content.strip_prefix('\u{feff}') {
        BomResult {
            bom: "\u{feff}",
            text: rest,
        }
    } else {
        BomResult {
            bom: "",
            text: content,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_endings() {
        assert_eq!(detect_line_ending("a\r\nb"), LineEnding::Crlf);
        assert_eq!(detect_line_ending("a\nb"), LineEnding::Lf);
        assert_eq!(detect_line_ending("no newlines"), LineEnding::Lf);
        assert_eq!(detect_line_ending("a\nb\r\nc"), LineEnding::Lf);
    }

    #[test]
    fn normalize_crlf() {
        assert_eq!(normalize_to_lf("a\r\nb\r\nc"), "a\nb\nc");
        assert_eq!(normalize_to_lf("a\rb"), "a\nb");
    }

    #[test]
    fn restore_crlf() {
        assert_eq!(restore_line_endings("a\nb", LineEnding::Crlf), "a\r\nb");
        assert_eq!(restore_line_endings("a\nb", LineEnding::Lf), "a\nb");
    }

    #[test]
    fn strip_utf8_bom() {
        let r = strip_bom("\u{feff}hello");
        assert_eq!(r.bom, "\u{feff}");
        assert_eq!(r.text, "hello");

        let r = strip_bom("hello");
        assert_eq!(r.bom, "");
        assert_eq!(r.text, "hello");
    }
}
