// Minimal preview module stub for oxicode-vtui-compat
pub fn truncate_with_ellipsis(_s: &str, _max_len: usize) -> String {
    _s.to_string()
}

pub struct Preview {
    _private: (),
}

impl Preview {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for Preview {
    fn default() -> Self {
        Self::new()
    }
}
