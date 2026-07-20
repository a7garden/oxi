// 12-frame spinner for status bar.

/// Get the frame for the given phase (0-based).
pub fn spinner_frame(phase: u8) -> &'static str {
    FRAMES[(phase as usize) % FRAMES.len()]
}

const FRAMES: &[&str] = &[
    "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "⠟", "⠻",
];

/// ASCII fallback (for future use).
#[allow(dead_code)]
const FRAMES_ASCII: &[&str] = &["|", "/", "-", "\\"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_cycles_12_frames() {
        for i in 0..12 {
            let f = spinner_frame(i as u8);
            assert!(!f.is_empty(), "frame {i} should not be empty");
        }
    }

    #[test]
    fn spinner_wraps_after_12() {
        assert_eq!(spinner_frame(0), spinner_frame(12));
    }
}
