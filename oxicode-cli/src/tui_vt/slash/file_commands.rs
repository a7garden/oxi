//! User-defined slash commands loaded from `.md` files.
//!
//! Each command file lives in `.oxicode/commands/<name>.md` (project) or
//! `~/.oxicode/commands/<name>.md` (user). The filename stem (minus `.md`)
//! is the command name. Optional YAML-like frontmatter provides `description`
//! and `aliases`. The body is a prompt template expanded at dispatch time.

use std::collections::HashSet;
use std::path::Path;

/// A user-defined slash command loaded from a `.md` file.
#[derive(Debug, Clone)]
pub struct FileCommand {
    /// Command name (filename stem, no `.md`, no leading `/`).
    pub name: String,
    /// Description from frontmatter, or first body line.
    pub description: String,
    /// Alternative names from frontmatter `aliases` (comma-separated).
    pub aliases: Vec<String>,
    /// Template body (frontmatter stripped). Contains `$ARGUMENTS`, `$@`,
    /// `$1`, `$2`, ... placeholders expanded at dispatch time.
    body: String,
}

impl FileCommand {
    /// Parse a command from its filename stem and file content.
    ///
    /// Frontmatter is optional YAML-like `key: value` lines between `---`
    /// delimiters. Supported keys: `description`, `aliases`.
    /// Without frontmatter, the first non-empty body line is the description.
    pub fn parse(name: &str, content: &str) -> Self {
        let (front, body) = split_frontmatter(content);
        let mut description = String::new();
        let mut aliases = Vec::new();

        if let Some(ref front) = front {
            for line in front.lines() {
                if let Some(val) = line.strip_prefix("description:") {
                    description = val.trim().to_string();
                } else if let Some(val) = line.strip_prefix("aliases:") {
                    aliases = val
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
            }
        }

        // Fallback: first non-empty body line as description.
        if description.is_empty() {
            description = body
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .chars()
                .take(60)
                .collect();
            if body.lines().any(|l| !l.trim().is_empty()) && description.len() == 60 {
                description.push_str("...");
            }
        }

        FileCommand {
            name: name.to_string(),
            description,
            aliases,
            body,
        }
    }

    /// True if `token` matches the canonical name or any alias.
    pub fn matches(&self, token: &str) -> bool {
        self.name == token || self.aliases.iter().any(|a| a == token)
    }

    /// Expand the template body with `args`, replacing `$ARGUMENTS`/`$@`
    /// (full arg string) and `$1`, `$2`, ... (positional).
    pub fn expand(&self, args: &str) -> String {
        expand_template(&self.body, args)
    }
}

/// Split `---\n...\n---` frontmatter from the body.
/// Returns `(frontmatter_without_delimiters, body_without_frontmatter)`.
fn split_frontmatter(content: &str) -> (Option<String>, String) {
    if let Some(body) = content.strip_prefix("---\n")
        && let Some(end) = body.find("\n---")
    {
        let front = body[..end].to_string();
        let rest = body[end + 4..].trim_start_matches('\n').to_string();
        return (Some(front), rest);
    }
    (None, content.trim_start_matches('\n').to_string())
}

/// Expand template placeholders: `$ARGUMENTS`/`$@` (all args), `$1`, `$2`, ...
///
/// Scans the template body left-to-right in a single pass and never re-scans
/// substituted content, so user argument text containing `$N`, `$@`, or
/// `$ARGUMENTS` is emitted verbatim.
pub fn expand_template(body: &str, args: &str) -> String {
    let positional = split_args(args);
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            // Peek at the slice after `$` so `strip_prefix` keeps the remainder
            // available for further inspection.
            let rest = &body[i + 1..];
            if rest.starts_with("ARGUMENTS") {
                out.push_str(args);
                i += 1 + "ARGUMENTS".len();
                continue;
            }
            if rest.starts_with('@') {
                out.push_str(args);
                i += 2;
                continue;
            }
            // Count consecutive ASCII digits immediately after `$`.
            let digit_len = rest
                .as_bytes()
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digit_len > 0 {
                // `$0` is never a positional (positionals are 1-indexed); emit
                // it literally so a template that mentions `$0` is preserved.
                if digit_len == 1 && rest.as_bytes()[0] == b'0' {
                    out.push('$');
                    out.push('0');
                    i += 2;
                    continue;
                }
                let digits = &rest[..digit_len];
                match digits.parse::<usize>() {
                    Ok(idx) if idx >= 1 && idx <= positional.len() => {
                        out.push_str(positional[idx - 1].as_str());
                    }
                    Ok(_) => {
                        // Missing positional becomes empty.
                    }
                    Err(_) => {
                        // Unreachable: digits are all ASCII so parse cannot fail.
                        out.push('$');
                        for c in digits.chars() {
                            out.push(c);
                        }
                    }
                }
                i += 1 + digit_len;
                continue;
            }
            // `$` not followed by a known placeholder: emit literally.
            out.push('$');
            i += 1;
            continue;
        }
        // Copy one full UTF-8 codepoint starting at byte `i`. `i` always
        // points at a char boundary because we only advance by full codepoint
        // lengths in the non-`$` branch and by ASCII lengths in the `$` branch.
        let ch = body[i..].chars().next().unwrap_or('\0');
        if ch == '\0' {
            break;
        }
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Simple quote-aware argument split (omp `parseCommandArgs` parity).
/// Supports `'single'` and `"double"` quoting. No backslash escaping.
fn split_args(args: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    for ch in args.chars() {
        match in_quote {
            Some(q) => {
                if ch == q {
                    in_quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => {
                if ch == '\'' || ch == '"' {
                    in_quote = Some(ch);
                } else if ch.is_whitespace() {
                    if !current.is_empty() {
                        result.push(std::mem::take(&mut current));
                    }
                } else {
                    current.push(ch);
                }
            }
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Scan one commands directory. Appends discovered commands to `out`,
/// skipping names already in `seen` (first-wins collision resolution).
fn scan_dir(dir: &Path, out: &mut Vec<FileCommand>, seen: &mut HashSet<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| {
        entry
            .path()
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned)
    });
    for entry in entries {
        let path = entry.path();
        // Only non-hidden `.md` files.
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if stem.starts_with('.') {
            continue;
        }
        if seen.contains(&stem) {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        seen.insert(stem.clone());
        out.push(FileCommand::parse(&stem, &content));
    }
}

/// Load all file-based commands from project (`.oxicode/commands/`) and user
/// (canonical home `commands/`, with legacy `~/.oxicode/commands/` read-only
/// fallback) directories. Project entries take precedence on name collision
/// (scanned first).
pub fn load_file_commands(cwd: &Path) -> Vec<FileCommand> {
    let mut commands = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. Project: .oxicode/commands/ (higher precedence)
    let project_dir = cwd.join(".oxicode").join("commands");
    scan_dir(&project_dir, &mut commands, &mut seen);

    // 2. User: canonical commands dir (legacy read-only fallback).
    if let Some(user_dir) = oxicode_catalog::oxi_home::read_path(Path::new("commands")) {
        scan_dir(&user_dir, &mut commands, &mut seen);
    }

    commands
}
/// Try to match and expand a file-based slash command.
/// `input` is the full prompt text starting with `/` (e.g. `"/review src/main.rs"`).
/// Returns the expanded prompt text if a file command matched, or `None`.
pub fn try_expand(commands: &[FileCommand], input: &str) -> Option<String> {
    let trimmed = input.trim();
    let after_slash = trimmed.strip_prefix('/')?;
    let (token, args) = match after_slash.find(' ') {
        Some(space) => (&after_slash[..space], after_slash[space + 1..].trim()),
        None => (after_slash, ""),
    };
    for cmd in commands {
        if cmd.matches(token) {
            return Some(cmd.expand(args));
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static HOME_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct HomeGuard(String);

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: tests that mutate HOME hold HOME_ENV_LOCK until after this guard drops.
            unsafe { std::env::set_var("HOME", &self.0) };
        }
    }

    #[test]
    fn parse_with_frontmatter() {
        let content = "---\ndescription: My review command\naliases: cr, review-code\n---\nReview this code:\n\n$ARGUMENTS\n";
        let cmd = FileCommand::parse("review", content);
        assert_eq!(cmd.name, "review");
        assert_eq!(cmd.description, "My review command");
        assert_eq!(cmd.aliases, vec!["cr", "review-code"]);
        assert!(cmd.body.contains("Review this code:"));
        assert!(cmd.body.contains("$ARGUMENTS"));
    }

    #[test]
    fn parse_without_frontmatter_uses_body_and_first_line_description() {
        let content = "Just do the thing\nWith more detail\n";
        let cmd = FileCommand::parse("thing", content);
        assert_eq!(cmd.name, "thing");
        assert_eq!(cmd.description, "Just do the thing");
        assert!(cmd.body.contains("Just do the thing"));
    }

    #[test]
    fn parse_frontmatter_with_no_aliases() {
        let content = "---\ndescription: A simple command\n---\nBody text\n";
        let cmd = FileCommand::parse("simple", content);
        assert!(cmd.aliases.is_empty());
    }

    #[test]
    fn expand_arguments_placeholder() {
        let body = "Review: $ARGUMENTS";
        assert_eq!(expand_template(body, "src/main.rs"), "Review: src/main.rs");
    }

    #[test]
    fn expand_at_placeholder() {
        let body = "Check $@ now";
        assert_eq!(expand_template(body, "foo bar"), "Check foo bar now");
    }

    #[test]
    fn expand_positional() {
        let body = "From $1 to $2";
        assert_eq!(expand_template(body, "alpha beta"), "From alpha to beta");
    }

    #[test]
    fn expand_missing_positional_becomes_empty() {
        let body = "Only $1 and $2";
        assert_eq!(expand_template(body, "alpha"), "Only alpha and ");
    }

    #[test]
    fn expand_preserves_dollar_digit_in_args() {
        assert_eq!(
            expand_template("Summarize: $ARGUMENTS", "The $9 variable"),
            "Summarize: The $9 variable"
        );
    }

    #[test]
    fn expand_no_placeholders_returns_body_unchanged() {
        let body = "Static prompt text";
        assert_eq!(expand_template(body, "ignored"), "Static prompt text");
    }

    #[test]
    fn expand_empty_args() {
        let body = "Do stuff with $ARGUMENTS";
        assert_eq!(expand_template(body, ""), "Do stuff with ");
    }

    #[test]
    fn matches_canonical_name() {
        let cmd = FileCommand::parse("review", "---\ndescription: x\n---\nbody");
        assert!(cmd.matches("review"));
        assert!(!cmd.matches("other"));
    }

    #[test]
    fn matches_alias() {
        let cmd = FileCommand::parse("review", "---\ndescription: x\naliases: cr, rv\n---\nbody");
        assert!(cmd.matches("cr"));
        assert!(cmd.matches("rv"));
    }

    #[test]
    fn expand_method_combines_template_and_args() {
        let cmd = FileCommand::parse(
            "test",
            "---\ndescription: x\n---\nRun $1 tests for $ARGUMENTS",
        );
        let expanded = cmd.expand("unit src/");
        assert_eq!(expanded, "Run unit tests for unit src/");
    }

    #[test]
    fn split_args_basic() {
        assert_eq!(split_args("foo bar baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn split_args_quoted() {
        assert_eq!(
            split_args("foo \"bar baz\" qux"),
            vec!["foo", "bar baz", "qux"]
        );
    }

    #[test]
    fn load_from_project_dir() {
        let _home_lock = HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = TempDir::new().unwrap();
        let tmp_home = TempDir::new().unwrap();
        let old_home = std::env::var("HOME").unwrap_or_default();
        let _home_guard = HomeGuard(old_home);
        // SAFETY: HOME_ENV_LOCK serializes these process-wide test mutations.
        unsafe { std::env::set_var("HOME", tmp_home.path()) };

        let cmds_dir = tmp.path().join(".oxicode").join("commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(
            cmds_dir.join("review.md"),
            "---\ndescription: proj review\n---\nReview $ARGUMENTS",
        )
        .unwrap();

        let cmds = load_file_commands(tmp.path());
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].name, "review");
        assert_eq!(cmds[0].description, "proj review");
    }

    #[test]
    fn scan_dir_orders_commands_alphabetically() {
        let tmp = TempDir::new().unwrap();
        let cmds_dir = tmp.path().join(".oxicode").join("commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(cmds_dir.join("zulu.md"), "zulu body").unwrap();
        fs::write(cmds_dir.join("alpha.md"), "alpha body").unwrap();

        let mut cmds = Vec::new();
        let mut seen = HashSet::new();
        scan_dir(&cmds_dir, &mut cmds, &mut seen);

        let names: Vec<&str> = cmds.iter().map(|cmd| cmd.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zulu"]);
    }

    #[test]
    fn project_shadows_user_on_name_collision() {
        let tmp_proj = TempDir::new().unwrap();
        let tmp_user = TempDir::new().unwrap();

        // Create a fake user home
        let user_oxicode = tmp_user.path().join(".oxicode").join("commands");
        fs::create_dir_all(&user_oxicode).unwrap();
        fs::write(
            user_oxicode.join("shared.md"),
            "---\ndescription: USER version\n---\nuser body",
        )
        .unwrap();

        // Project version
        let proj_cmds = tmp_proj.path().join(".oxicode").join("commands");
        fs::create_dir_all(&proj_cmds).unwrap();
        fs::write(
            proj_cmds.join("shared.md"),
            "---\ndescription: PROJECT version\n---\nproj body",
        )
        .unwrap();

        // We can't easily mock dirs::home_dir, so test the internal
        // scan logic directly.
        let mut cmds = Vec::new();
        let mut seen = HashSet::new();
        scan_dir(&proj_cmds, &mut cmds, &mut seen);
        scan_dir(&user_oxicode, &mut cmds, &mut seen);

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].description, "PROJECT version");
    }

    #[test]
    fn load_ignores_non_md_files() {
        let tmp = TempDir::new().unwrap();
        let cmds_dir = tmp.path().join(".oxicode").join("commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(cmds_dir.join("valid.md"), "---\ndescription: x\n---\nbody").unwrap();
        fs::write(cmds_dir.join("readme.txt"), "not a command").unwrap();
        fs::write(
            cmds_dir.join(".hidden.md"),
            "---\ndescription: hidden\n---\nbody",
        )
        .unwrap();

        let mut cmds = Vec::new();
        let mut seen = HashSet::new();
        scan_dir(&cmds_dir, &mut cmds, &mut seen);

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].name, "valid");
    }

    #[test]
    fn load_handles_missing_dir_gracefully() {
        let _home_lock = HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = TempDir::new().unwrap();
        let tmp_home = TempDir::new().unwrap();
        let old_home = std::env::var("HOME").unwrap_or_default();
        let _home_guard = HomeGuard(old_home);
        // SAFETY: HOME_ENV_LOCK serializes these process-wide test mutations.
        unsafe { std::env::set_var("HOME", tmp_home.path()) };

        // No .oxicode/commands/ exists — should return empty, not error.
        let cmds = load_file_commands(tmp.path());
        assert!(cmds.is_empty());
    }

    #[test]
    fn split_args_empty() {
        assert!(split_args("").is_empty());
    }

    #[test]
    fn try_expand_matches_canonical_name() {
        let cmds = vec![FileCommand::parse(
            "review",
            "---\ndescription: x\n---\nReview $ARGUMENTS",
        )];
        let result = try_expand(&cmds, "/review src/main.rs");
        assert_eq!(result.as_deref(), Some("Review src/main.rs"));
    }

    #[test]
    fn try_expand_matches_alias() {
        let cmds = vec![FileCommand::parse(
            "review",
            "---\ndescription: x\naliases: cr\n---\nCode review: $ARGUMENTS",
        )];
        let result = try_expand(&cmds, "/cr src/lib.rs");
        assert_eq!(result.as_deref(), Some("Code review: src/lib.rs"));
    }

    #[test]
    fn try_expand_no_args() {
        let cmds = vec![FileCommand::parse(
            "deploy",
            "---\ndescription: x\n---\nDeploy everything now",
        )];
        let result = try_expand(&cmds, "/deploy");
        assert_eq!(result.as_deref(), Some("Deploy everything now"));
    }

    #[test]
    fn try_expand_no_match_returns_none() {
        let cmds = vec![FileCommand::parse(
            "review",
            "---\ndescription: x\n---\nbody",
        )];
        assert!(try_expand(&cmds, "/nonexistent").is_none());
    }

    #[test]
    fn try_expand_empty_commands_returns_none() {
        assert!(try_expand(&[], "/anything").is_none());
    }
}
