//! Fuzzy slash command dropdown with MRU (Most Recently Used) ranking.
//!
//! Replaces oxicode-cli's basic completion popup with:
//! - nucleo-based fuzzy matching with score ranking
//! - MRU decay (7-day half-life) — frequently-used commands float to top
//! - Inline ghost completion: `/com` → `/comm`**it** (highlighted tail)
//! - Mid-text token recognition: `/model` inside other text triggers teal highlight
//! - Two-bit completeness: `takes_args` + `args_required` → drives completion quality
//!
//! ## Architecture
//!
//! ```text
//! SlashRegistry::builtins() -> SlashCommand { name, args, takes_args, ... }
//!                          |
//!                          v
//!              SlashDropdown::new(commands)
//!                          |
//!                          v
//! user types "/comm" -> matches scored -> ranked -> rendered as List
//! ```
//!
//! State is owned by `AppState` (oxicode-cli). oxicode-tui exposes the widget library
//! function; oxicode-cli wires it to the input handler.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nucleo::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

/// Half-life for MRU score decay — a command used now drops to half
/// priority in 7 days if not used again.
const MRU_HALF_LIFE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// MRU score boost per use.
const MRU_BOOST: f64 = 1.0;

/// Decay factor (1.0 = no decay). Computed from MRU_HALF_LIFE.
fn decay_for(elapsed: Duration) -> f64 {
    if elapsed >= MRU_HALF_LIFE * 10 {
        return 0.0;
    }
    0.5_f64.powf(elapsed.as_secs_f64() / MRU_HALF_LIFE.as_secs_f64())
}

/// A command the user can invoke with `/name args`.
#[derive(Debug, Clone)]
pub struct SlashCommand {
    /// Slash command name without prefix (e.g. "model", "help").
    pub name: String,
    /// Short description shown in the dropdown.
    pub description: String,
    /// Long-form help text shown in expanded help modal.
    pub long_help: String,
    /// Category for grouping in help modal.
    pub category: SlashCategory,
    /// Whether this command takes arguments (drives ghost completion).
    pub takes_args: bool,
    /// Whether the command is invalid without args.
    pub args_required: bool,
}

impl SlashCommand {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            long_help: String::new(),
            category: SlashCategory::General,
            takes_args: false,
            args_required: false,
        }
    }

    pub fn with_long_help(mut self, help: impl Into<String>) -> Self {
        self.long_help = help.into();
        self
    }

    pub fn with_category(mut self, cat: SlashCategory) -> Self {
        self.category = cat;
        self
    }

    pub fn with_args(mut self, takes: bool, required: bool) -> Self {
        self.takes_args = takes;
        self.args_required = required;
        self
    }
}

/// Categories for grouping in the help modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCategory {
    General,
    Model,
    Session,
    Tools,
    Theme,
    System,
}

/// MRU entry — last used timestamp + accumulated boost.
#[derive(Debug, Clone, Default)]
struct MruEntry {
    last_used: Option<Instant>,
    boost: f64,
}

/// A dropdown item with computed ranking.
#[derive(Debug, Clone)]
pub struct RankedCommand {
    pub command: SlashCommand,
    /// Higher = better match.
    pub score: u32,
    /// Indices in `command.name` that matched the query (for ghost highlighting).
    pub match_indices: Vec<u32>,
}

/// Stateful fuzzy dropdown widget.
#[derive(Debug)]
pub struct SlashDropdown {
    commands: Vec<SlashCommand>,
    mru: HashMap<String, MruEntry>,
    /// Last query used (for incremental matching).
    last_query: String,
    /// Cached matcher to avoid re-allocation per query.
    matcher: Matcher,
}

impl SlashDropdown {
    /// Create a new dropdown with the given commands.
    pub fn new(commands: Vec<SlashCommand>) -> Self {
        let matcher = Matcher::new(Config::DEFAULT);
        let mut mru = HashMap::with_capacity(commands.len());
        for cmd in &commands {
            mru.insert(cmd.name.clone(), MruEntry::default());
        }
        Self {
            commands,
            mru,
            last_query: String::new(),
            matcher,
        }
    }

    /// Detect a `/command` invocation at the end of `input` and return the
    /// matching commands ranked by fuzzy score + MRU boost.
    pub fn matches(&mut self, input: &str) -> Option<Vec<RankedCommand>> {
        let query = extract_query(input)?;
        self.last_query = query.clone();

        let pattern = Pattern::new(
            &query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut scored: Vec<RankedCommand> = Vec::with_capacity(self.commands.len());
        for cmd in &self.commands {
            let haystack = Utf32Str::Ascii(cmd.name.as_bytes());
            let mut indices = Vec::new();
            let score = pattern.indices(haystack, &mut self.matcher, &mut indices);
            if score.is_none() {
                continue;
            }
            let mut s = score.unwrap_or(0);
            // MRU boost
            if let Some(entry) = self.mru.get(&cmd.name)
                && let Some(last) = entry.last_used
            {
                let boost = (entry.boost * decay_for(last.elapsed()) * 100.0) as u32;
                s = s.saturating_add(boost);
            }
            scored.push(RankedCommand {
                command: cmd.clone(),
                score: s,
                match_indices: indices,
            });
        }

        // Sort by score descending.
        scored.sort_by_key(|cmd| std::cmp::Reverse(cmd.score));
        Some(scored)
    }

    /// Record that the user invoked a command. Bumps MRU score.
    pub fn record_use(&mut self, name: &str) {
        let entry = self.mru.entry(name.to_string()).or_default();
        entry.last_used = Some(Instant::now());
        entry.boost += MRU_BOOST;
    }

    /// Compute the inline ghost completion for `query`.
    ///
    /// `/comm` → `commit` (full command if unique match), `/commi` → `commit`
    /// Returns the longest matching command name where query is a prefix,
    /// or empty string if none.
    pub fn ghost_complete(&self, query: &str) -> String {
        if query.is_empty() {
            return String::new();
        }
        // Find commands that start with query.
        let matches: Vec<&SlashCommand> = self
            .commands
            .iter()
            .filter(|c| c.name.starts_with(query) && c.name.len() > query.len())
            .collect();
        if matches.len() != 1 {
            return String::new(); // ambiguous → no ghost
        }
        // Compute common prefix between query and the single match.
        let candidate = matches[0].name.as_str();
        for (i, c) in candidate.char_indices() {
            if i >= query.len() {
                return candidate[i..].to_string();
            }
            if query.as_bytes()[i] != c as u8 {
                return candidate[i..].to_string();
            }
        }
        String::new()
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether the dropdown has any commands.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Get all commands (for help modal).
    pub fn commands(&self) -> &[SlashCommand] {
        &self.commands
    }
}

/// Extract the in-progress slash query from `input`.
///
/// Returns the substring after the last `/` that starts a command name.
/// Returns None if no `/` is found or `/` is not at a word boundary.
fn extract_query(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut last_slash: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'/' {
            // Must be at word boundary: start of input or preceded by whitespace.
            let preceded_by_space = i == 0 || bytes[i - 1] == b' ';
            if preceded_by_space {
                last_slash = Some(i);
            }
        }
    }
    let start = last_slash? + 1;
    // Don't include if there's a space (query complete, args starting).
    let rest = &input[start..];
    if rest.contains(' ') {
        return None;
    }
    Some(rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_commands() -> Vec<SlashCommand> {
        vec![
            SlashCommand::new("commit", "Commit changes").with_args(true, true),
            SlashCommand::new("compress", "Compress conversation").with_args(false, false),
            SlashCommand::new("help", "Show help").with_category(SlashCategory::General),
            SlashCommand::new("model", "Switch model").with_category(SlashCategory::Model),
            SlashCommand::new("theme", "Switch theme").with_category(SlashCategory::Theme),
        ]
    }

    // ── Query extraction ───────────────────────────────────────────────

    #[test]
    fn extract_query_at_start() {
        assert_eq!(extract_query("/comm"), Some("comm".into()));
    }

    #[test]
    fn extract_query_after_space() {
        assert_eq!(extract_query("hello /help"), Some("help".into()));
    }

    #[test]
    fn extract_query_no_slash() {
        assert_eq!(extract_query("hello world"), None);
    }

    #[test]
    fn extract_query_with_args_returns_none() {
        assert_eq!(extract_query("/commit foo bar"), None);
    }

    #[test]
    fn extract_query_mid_text_slash() {
        // Mid-text slash not at word boundary → None.
        assert_eq!(extract_query("path/to/file"), None);
    }

    // ── Ranking ────────────────────────────────────────────────────────

    #[test]
    fn matches_returns_ranked_results() {
        let mut dd = SlashDropdown::new(test_commands());
        let results = dd.matches("/co").expect("should match");
        assert!(!results.is_empty());
        // "co" matches "commit" and "compress" (common prefix).
        let names: Vec<&str> = results.iter().map(|r| r.command.name.as_str()).collect();
        assert!(names.contains(&"commit"));
        assert!(names.contains(&"compress"));
    }

    #[test]
    fn matches_empty_query_returns_all() {
        let mut dd = SlashDropdown::new(test_commands());
        let results = dd.matches("/").expect("should match");
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn matches_no_match_returns_empty() {
        let mut dd = SlashDropdown::new(test_commands());
        let results = dd.matches("/zzzzz").expect("should match");
        assert!(results.is_empty());
    }

    // ── MRU ────────────────────────────────────────────────────────────

    #[test]
    fn record_use_boosts_ranking() {
        let mut dd = SlashDropdown::new(test_commands());
        dd.record_use("compress"); // boost compress
        let results = dd.matches("/c").expect("should match");
        // compress should now rank above commit (boost + length tiebreaker).
        assert!(!results.is_empty());
        assert_eq!(results[0].command.name, "compress");
    }

    // ── Ghost completion ───────────────────────────────────────────────

    #[test]
    fn ghost_complete_unique_match() {
        let dd = SlashDropdown::new(test_commands());
        assert_eq!(dd.ghost_complete("comm"), "it"); // commit
        assert_eq!(dd.ghost_complete("compr"), "ess"); // compress
    }

    #[test]
    fn ghost_complete_ambiguous_returns_empty() {
        let dd = SlashDropdown::new(test_commands());
        // Both "commit" and "compress" start with "com" → ambiguous.
        assert_eq!(dd.ghost_complete("com"), "");
    }

    #[test]
    fn ghost_complete_no_match_returns_empty() {
        let dd = SlashDropdown::new(test_commands());
        assert_eq!(dd.ghost_complete("xyz"), "");
    }

    #[test]
    fn ghost_complete_full_match_returns_empty() {
        let dd = SlashDropdown::new(test_commands());
        assert_eq!(dd.ghost_complete("help"), "");
    }

    // ── Registry API ───────────────────────────────────────────────────

    #[test]
    fn len_and_commands_work() {
        let dd = SlashDropdown::new(test_commands());
        assert_eq!(dd.len(), 5);
        assert!(!dd.is_empty());
        assert_eq!(dd.commands().len(), 5);
    }

    #[test]
    fn slash_command_builder() {
        let cmd = SlashCommand::new("test", "desc")
            .with_long_help("long help")
            .with_category(SlashCategory::Tools)
            .with_args(true, false);
        assert_eq!(cmd.name, "test");
        assert_eq!(cmd.long_help, "long help");
        assert_eq!(cmd.category, SlashCategory::Tools);
        assert!(cmd.takes_args);
        assert!(!cmd.args_required);
    }

    // ── Decay ──────────────────────────────────────────────────────────

    #[test]
    fn decay_for_zero_elapsed_is_one() {
        assert!((decay_for(Duration::ZERO) - 1.0).abs() < 0.001);
    }

    #[test]
    fn decay_for_one_half_life_is_half() {
        assert!((decay_for(MRU_HALF_LIFE) - 0.5).abs() < 0.001);
    }

    #[test]
    fn decay_for_very_long_elapsed_is_zero() {
        assert_eq!(decay_for(MRU_HALF_LIFE * 20), 0.0);
    }
}
