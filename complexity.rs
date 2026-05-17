/// Task complexity level for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Complexity {
    /// Simple, single-step tasks (e.g., "translate this text")
    Trivial,
    /// Routine tasks needing moderate reasoning (e.g., "write a function")
    Simple,
    /// Tasks requiring multi-step reasoning (e.g., "architect a service")
    Moderate,
    /// Complex tasks needing deep analysis (e.g., "write a full codebase")
    #[default]
    Complex,
    /// Research-grade tasks needing the best models
    Research,
}

impl Complexity {
    /// Returns the relative cost tier (0=cheapest, 4=most expensive) for routing
    pub fn cost_tier(&self) -> u8 {
        match self {
            Self::Trivial => 0,
            Self::Simple => 1,
            Self::Moderate => 2,
            Self::Complex => 3,
            Self::Research => 4,
        }
    }
}
