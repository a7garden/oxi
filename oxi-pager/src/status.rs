// StatusState — footer status, spinner, token tracking.
pub struct StatusState {
    pub spinner_phase: u8,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost: f64,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub last_error: Option<String>,
}

impl Default for StatusState {
    fn default() -> Self {
        Self {
            spinner_phase: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            model: None,
            session_id: None,
            last_error: None,
        }
    }
}

impl StatusState {
    /// Advance the spinner by one tick (12-frame cycle).
    pub fn tick(&mut self) {
        self.spinner_phase = (self.spinner_phase + 1) % 12;
    }

    /// Set an error to display once.
    pub fn set_error(&mut self, msg: String) {
        self.last_error = Some(msg);
    }

    /// Clear the one-shot error.
    pub fn clear_error(&mut self) {
        self.last_error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_advances_phase() {
        let mut s = StatusState::default();
        assert_eq!(s.spinner_phase, 0);
        s.tick();
        assert_eq!(s.spinner_phase, 1);
    }

    #[test]
    fn tick_wraps_at_12() {
        let mut s = StatusState::default();
        for _ in 0..12 {
            s.tick();
        }
        assert_eq!(s.spinner_phase, 0);
    }
}
