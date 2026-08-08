# User-Defined Slash Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users create custom TUI slash commands from `.md` files in `.oxicode/commands/` (project) and `~/.oxicode/commands/` (user), with frontmatter metadata and `$ARGUMENTS`/`$1`/`$2` template expansion — omp-compatible format.

**Architecture:** A new `file_commands.rs` module under `tui_vt/slash/` owns command-file parsing, template expansion, and directory discovery. File commands are loaded once at TUI startup into `RenderState.file_commands`. Dispatch is integrated into the existing `SlashOutcome::NotHandled` branch in `handle_inline_event` — builtins always win; file commands are the fallback before "Unknown command". Autocomplete (`slash_filter`) is extended to include file commands in the popup.

**Tech Stack:** Rust 2024 edition, `std::fs` for discovery, existing `dirs::home_dir()` for user root, existing `parking_lot::Mutex<RenderState>` for state sharing between input thread and event loop.

## Global Constraints

- **Builtins always win.** File commands cannot shadow builtins. Dispatch checks builtins first (`SlashRegistry::dispatch`); file commands are only consulted in the `NotHandled` branch.
- **Project beats user on collision.** `.oxicode/commands/review.md` shadows `~/.oxicode/commands/review.md` (first-wins by name).
- **omp-compatible format.** Frontmatter keys (`description`, `aliases`) and template placeholders (`$ARGUMENTS`/`$@`, `$1`, `$2`, ...) match omp's `expandSlashCommand` semantics.
- **No file watcher.** Commands load once at startup. Restart to pick up new/changed files (same as omp's lifecycle).
- **Expanded text is sent directly to `prompt_tx`, never re-dispatched.** Prevents `/`-prefixed template content from causing recursive slash dispatch.
- **Library crate lint rules apply** — no `unwrap()` in non-test code (use `?` / `unwrap_or_default`); `cargo clippy --workspace --all-targets -- -D warnings` must pass clean.
- **Test runner:** `cargo nextest run -p oxicode-cli` for all test verification steps.

## File Structure

| File | Responsibility |
|---|---|
| **Create:** `oxicode-cli/src/tui_vt/slash/file_commands.rs` | `FileCommand` struct, frontmatter parser, template expander, directory loader, `try_expand` dispatch helper. Self-contained pure logic + `std::fs` discovery. |
| **Modify:** `oxicode-cli/src/tui_vt/slash/mod.rs` | Add `pub mod file_commands;` declaration. |
| **Modify:** `oxicode-cli/src/tui_vt/main_loop.rs` | Add `file_commands: Vec<FileCommand>` to `RenderState`; load at startup in `run_tui`; extend `slash_filter` to include file commands; check file commands in `NotHandled` branch of `handle_inline_event`. |

---
> **Execution order:** Tasks are numbered by logical layer, but for every task to
> commit compilable code, execute in this order: **1 → 2 → 5 → 3 → 4 → 6**.
> (Task 5 adds the `RenderState.file_commands` field + import that Tasks 3 and 4
> reference; doing it first avoids intermediate non-compiling commits.)

### Task 1: FileCommand Core — Struct, Frontmatter Parser, Template Expander

**Files:**
- Create: `oxicode-cli/src/tui_vt/slash/file_commands.rs`
- Modify: `oxicode-cli/src/tui_vt/slash/mod.rs:1-2`

**Interfaces:**
- Produces: `FileCommand` struct, `FileCommand::parse(name, content)`, `FileCommand::matches(token)`, `FileCommand::expand(args)`, `split_args(args)`, `expand_template(body, args)`

- [ ] **Step 1: Add module declaration**

`oxicode-cli/src/tui_vt/slash/mod.rs` — add after line 2:
```rust
pub mod file_commands;
```

- [ ] **Step 2: Write failing tests for frontmatter parsing and template expansion**

Create `oxicode-cli/src/tui_vt/slash/file_commands.rs` with test module only:

```rust
//! User-defined slash commands loaded from `.md` files.
//!
//! Each command file lives in `.oxicode/commands/<name>.md` (project) or
//! `~/.oxicode/commands/<name>.md` (user). The filename stem (minus `.md`)
//! is the command name. Optional YAML-like frontmatter provides `description`
//! and `aliases`. The body is a prompt template expanded at dispatch time.

#[cfg(test)]
mod tests {
    use super::*;

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
        let cmd = FileCommand::parse("test", "---\ndescription: x\n---\nRun $1 tests for $ARGUMENTS");
        let expanded = cmd.expand("unit src/");
        assert_eq!(expanded, "Run unit tests for unit src/");
    }

    #[test]
    fn split_args_basic() {
        assert_eq!(split_args("foo bar baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn split_args_quoted() {
        assert_eq!(split_args("foo \"bar baz\" qux"), vec!["foo", "bar baz", "qux"]);
    }

    #[test]
    fn split_args_empty() {
        assert!(split_args("").is_empty());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo nextest run -p oxicode-cli file_commands`
Expected: FAIL — `FileCommand` not defined.

- [ ] **Step 4: Implement FileCommand struct, parser, and expander**

Add to `oxicode-cli/src/tui_vt/slash/file_commands.rs` (above the test module):

```rust
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
            if body.lines().any(|l| !l.trim().is_empty())
                && description.len() == 60
            {
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
    if let Some(body) = content.strip_prefix("---\n") {
        if let Some(end) = body.find("\n---") {
            let front = body[..end].to_string();
            let rest = body[end + 4..].trim_start_matches('\n').to_string();
            return (Some(front), rest);
        }
    }
    (None, content.trim_start_matches('\n').to_string())
}

/// Expand template placeholders: `$ARGUMENTS`/`$@` (all args), `$1`, `$2`, ...
pub fn expand_template(body: &str, args: &str) -> String {
    let positional = split_args(args);
    let mut result = body.replace("$ARGUMENTS", args);
    result = result.replace("$@", args);
    for (i, arg) in positional.iter().enumerate() {
        result = result.replace(&format!("${}", i + 1), arg);
    }
    result
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p oxicode-cli file_commands`
Expected: PASS — all 14 tests.

- [ ] **Step 6: Commit**

```bash
git add oxicode-cli/src/tui_vt/slash/file_commands.rs oxicode-cli/src/tui_vt/slash/mod.rs
git commit -m "feat(tui): add FileCommand parser and template expander for user-defined slash commands"
```

---

### Task 2: File Discovery Loader

**Files:**
- Modify: `oxicode-cli/src/tui_vt/slash/file_commands.rs` (append loader + tests)

**Interfaces:**
- Consumes: `FileCommand::parse(name, content)` from Task 1.
- Produces: `load_file_commands(cwd: &Path) -> Vec<FileCommand>` — scans project then user directories, project-wins on collision.

- [ ] **Step 1: Write failing tests for discovery**

Append to the `tests` module in `file_commands.rs`:

```rust
    #[test]
    fn load_from_project_dir() {
        let tmp = TempDir::new().unwrap();
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
        let mut seen = std::collections::HashSet::new();
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
        fs::write(cmds_dir.join(".hidden.md"), "---\ndescription: hidden\n---\nbody").unwrap();

        let mut cmds = Vec::new();
        let mut seen = std::collections::HashSet::new();
        scan_dir(&cmds_dir, &mut cmds, &mut seen);

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].name, "valid");
    }

    #[test]
    fn load_handles_missing_dir_gracefully() {
        let tmp = TempDir::new().unwrap();
        // No .oxicode/commands/ exists — should return empty, not error.
        let cmds = load_file_commands(tmp.path());
        assert!(cmds.is_empty());
    }
```

Add `use std::fs;` and `use tempfile::TempDir;` at the top of the test module. Check `tempfile` is available — it is already a dev-dependency (used in persona.rs tests).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p oxicode-cli file_commands`
Expected: FAIL — `load_file_commands` and `scan_dir` not defined.

- [ ] **Step 3: Implement the loader**

Append to `file_commands.rs` (above the test module):

```rust
use std::collections::HashSet;
use std::path::Path;

/// Scan one commands directory. Appends discovered commands to `out`,
/// skipping names already in `seen` (first-wins collision resolution).
fn scan_dir(dir: &Path, out: &mut Vec<FileCommand>, seen: &mut HashSet<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
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

/// Load all file-based commands from project (`.oxicode/commands/`) and
/// user (`~/.oxicode/commands/`) directories. Project entries take
/// precedence on name collision (scanned first).
pub fn load_file_commands(cwd: &Path) -> Vec<FileCommand> {
    let mut commands = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. Project: .oxicode/commands/ (higher precedence)
    let project_dir = cwd.join(".oxicode").join("commands");
    scan_dir(&project_dir, &mut commands, &mut seen);

    // 2. User: ~/.oxicode/commands/
    if let Some(home) = dirs::home_dir() {
        let user_dir = home.join(".oxicode").join("commands");
        scan_dir(&user_dir, &mut commands, &mut seen);
    }

    commands
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p oxicode-cli file_commands`
Expected: PASS — all tests including discovery.

- [ ] **Step 5: Commit**

```bash
git add oxicode-cli/src/tui_vt/slash/file_commands.rs
git commit -m "feat(tui): add file-based slash command discovery from .oxicode/commands/"
```

---

### Task 3: Dispatch Integration — `try_expand` + `NotHandled` Branch

**Files:**
- Modify: `oxicode-cli/src/tui_vt/slash/file_commands.rs` (add `try_expand`)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs:1229-1239` (extend `NotHandled` arm)

**Interfaces:**
- Consumes: `FileCommand::matches`, `FileCommand::expand` from Task 1; `RenderState.file_commands` (added in Task 5).
- Produces: `try_expand(commands, input) -> Option<String>` — returns expanded prompt text if a file command matches.

- [ ] **Step 1: Write failing test for `try_expand`**

Append to the `tests` module in `file_commands.rs`:

```rust
    #[test]
    fn try_expand_matches_canonical_name() {
        let cmds = vec![FileCommand::parse("review", "---\ndescription: x\n---\nReview $ARGUMENTS")];
        let result = try_expand(&cmds, "/review src/main.rs");
        assert_eq!(result.as_deref(), Some("Review src/main.rs"));
    }

    #[test]
    fn try_expand_matches_alias() {
        let cmds = vec![FileCommand::parse("review", "---\ndescription: x\naliases: cr\n---\nCode review: $ARGUMENTS")];
        let result = try_expand(&cmds, "/cr src/lib.rs");
        assert_eq!(result.as_deref(), Some("Code review: src/lib.rs"));
    }

    #[test]
    fn try_expand_no_args() {
        let cmds = vec![FileCommand::parse("deploy", "---\ndescription: x\n---\nDeploy everything now")];
        let result = try_expand(&cmds, "/deploy");
        assert_eq!(result.as_deref(), Some("Deploy everything now"));
    }

    #[test]
    fn try_expand_no_match_returns_none() {
        let cmds = vec![FileCommand::parse("review", "---\ndescription: x\n---\nbody")];
        assert!(try_expand(&cmds, "/nonexistent").is_none());
    }

    #[test]
    fn try_expand_empty_commands_returns_none() {
        assert!(try_expand(&[], "/anything").is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p oxicode-cli file_commands`
Expected: FAIL — `try_expand` not defined.

- [ ] **Step 3: Implement `try_expand`**

Append to `file_commands.rs`:

```rust
/// Try to match and expand a file-based slash command.
///
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p oxicode-cli file_commands`
Expected: PASS — all tests.

- [ ] **Step 5: Wire `try_expand` into the `NotHandled` branch of `handle_inline_event`**

In `oxicode-cli/src/tui_vt/main_loop.rs`, the current `NotHandled` arm (around line 1232) is:

```rust
                    SlashOutcome::NotHandled => {
                        ctx.reply(
                            InlineMessageKind::Error,
                            format!("Unknown command: {}", prompt.trim()),
                        );
                        LoopOutcome::Continue
                    }
```

Replace with:

```rust
                    SlashOutcome::NotHandled => {
                        // File-based commands: try before erroring.
                        if let Some(expanded) = crate::tui_vt::slash::file_commands::try_expand(
                            &ctx.state.file_commands,
                            &prompt,
                        ) {
                            // Send expanded text directly to the agent worker.
                            // The original `/cmd args` is already echoed above.
                            let _ = prompt_tx.send(expanded);
                            LoopOutcome::Continue
                        } else {
                            ctx.reply(
                                InlineMessageKind::Error,
                                format!("Unknown command: {}", prompt.trim()),
                            );
                            LoopOutcome::Continue
                        }
                    }
```

Note: `prompt_tx` is already a parameter of `handle_inline_event` (line 1204). `ctx.state` gives access to `RenderState` including the new `file_commands` field (added in Task 5).

- [ ] **Step 6: Build to verify compilation** (this will fail until Task 5 adds the `file_commands` field — that is expected; verify only that the logic is syntactically correct)

Run: `cargo build -p oxicode-cli 2>&1 | head -20`
Expected: FAIL on `no field 'file_commands' on type 'RenderState'` — this is resolved in Task 5.

- [ ] **Step 7: Commit** (the build error will be fixed by Task 5; commit the logic now)

```bash
git add oxicode-cli/src/tui_vt/slash/file_commands.rs oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(tui): integrate file commands into slash dispatch NotHandled branch"
```

---

### Task 4: Autocomplete Integration — `slash_filter` + Popup

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs:3895-3915` (`slash_filter` signature + body)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs:3931` (`refresh_slash_popup` call site)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs:4122-4145` (existing `slash_filter` tests)

**Interfaces:**
- Consumes: `FileCommand` from Task 1; `RenderState.file_commands` (added in Task 5).
- Produces: `slash_filter(token, file_commands)` — extended to include file commands (excluding names shadowed by builtins).

- [ ] **Step 1: Update `slash_filter` to accept and include file commands**

Current function (line 3895):
```rust
fn slash_filter(token: &str) -> Vec<SlashPopupItem> {
    SlashRegistry::builtin_commands()
        .into_iter()
        .filter(|(name, _, aliases)| {
            token.is_empty()
                || name.starts_with(token)
                || aliases.iter().any(|a| a.starts_with(token))
        })
        .map(|(name, desc, aliases)| {
            let mut label = format!("/{name}");
            for a in &aliases {
                label.push_str(&format!(", /{a}"));
            }
            SlashPopupItem {
                label,
                description: desc.to_string(),
                name: name.to_string(),
            }
        })
        .collect()
}
```

Replace with:
```rust
fn slash_filter(token: &str, file_commands: &[FileCommand]) -> Vec<SlashPopupItem> {
    let builtins = SlashRegistry::builtin_commands();
    let builtin_names: std::collections::HashSet<&str> =
        builtins.iter().map(|(n, _, _)| *n).collect();

    let mut items: Vec<SlashPopupItem> = builtins
        .into_iter()
        .filter(|(name, _, aliases)| {
            token.is_empty()
                || name.starts_with(token)
                || aliases.iter().any(|a| a.starts_with(token))
        })
        .map(|(name, desc, aliases)| {
            let mut label = format!("/{name}");
            for a in &aliases {
                label.push_str(&format!(", /{a}"));
            }
            SlashPopupItem {
                label,
                description: desc.to_string(),
                name: name.to_string(),
            }
        })
        .collect();

    // Append file commands (skip names shadowed by builtins).
    for fc in file_commands {
        if builtin_names.contains(fc.name.as_str()) {
            continue;
        }
        if token.is_empty()
            || fc.name.starts_with(token)
            || fc.aliases.iter().any(|a| a.starts_with(token))
        {
            let mut label = format!("/{}", fc.name);
            for a in &fc.aliases {
                label.push_str(&format!(", /{a}"));
            }
            items.push(SlashPopupItem {
                label,
                description: fc.description.clone(),
                name: fc.name.clone(),
            });
        }
    }

    items
}
```

- [ ] **Step 2: Update `refresh_slash_popup` call site**

Line 3931:
```rust
    let items = slash_filter(token);
```
→
```rust
    let items = slash_filter(token, &state.file_commands);
```

- [ ] **Step 3: Update existing `slash_filter` tests**

Lines 4122-4145 — change all calls to pass `&[]`:
```rust
        let items = slash_filter("", &[]);
```
and:
```rust
        let items = slash_filter("qu", &[]);
```
and:
```rust
        let items = slash_filter("cl", &[]);
```

Add a new test for file command filtering:
```rust
    #[test]
    fn file_commands_appear_in_filter() {
        let fc = crate::tui_vt::slash::file_commands::FileCommand::parse(
            "review",
            "---\ndescription: proj cmd\naliases: cr\n---\nbody",
        );
        let items = slash_filter("", &[fc]);
        assert!(items.iter().any(|i| i.name == "review"));
        assert!(items.iter().any(|i| i.name == "quit")); // builtins still present
    }

    #[test]
    fn file_commands_filtered_by_prefix() {
        let fc = crate::tui_vt::slash::file_commands::FileCommand::parse(
            "review",
            "---\ndescription: x\n---\nbody",
        );
        let items = slash_filter("rev", &[fc]);
        assert!(items.iter().any(|i| i.name == "review"));
    }
```

- [ ] **Step 4: Update render test call sites**

Lines 4265 and 4277 — change `slash_filter("")` to `slash_filter("", &[])`:
```rust
        state.slash_popup.items = slash_filter("", &[]);
```
and:
```rust
        state.slash_popup.items = slash_filter("qu", &[]);
```

- [ ] **Step 5: Run tests to verify they pass** (will fail until Task 5 adds the `file_commands` field — verify syntax only)

Run: `cargo build -p oxicode-cli 2>&1 | head -20`
Expected: FAIL on `no field 'file_commands'` — resolved in Task 5.

- [ ] **Step 6: Commit**

```bash
git add oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(tui): include user-defined commands in slash autocomplete popup"
```

---

### Task 5: RenderState Field + Startup Loading

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs:237-238` (add field to `RenderState`)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs:45` (add import)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs:681-682` (load at startup in `run_tui`)

**Interfaces:**
- Consumes: `load_file_commands` from Task 2; `FileCommand` from Task 1.
- Produces: `RenderState.file_commands: Vec<FileCommand>` — populated once at startup, read by dispatch (Task 3) and autocomplete (Task 4).

- [ ] **Step 1: Add import**

Line 45:
```rust
use crate::tui_vt::slash::registry::{SlashCtx, SlashOutcome, SlashRegistry};
```
→
```rust
use crate::tui_vt::slash::file_commands::FileCommand;
use crate::tui_vt::slash::registry::{SlashCtx, SlashOutcome, SlashRegistry};
```

- [ ] **Step 2: Add field to `RenderState`**

After line 237 (`pub seen_tips: ...`), before the closing `}` on line 238:
```rust
    /// User-defined slash commands loaded once at startup from
    /// `.oxicode/commands/` and `~/.oxicode/commands/`.
    pub file_commands: Vec<FileCommand>,
```

- [ ] **Step 3: Load file commands at startup**

In `run_tui`, after line 682 (`state.lock().catalog = Some(app.catalog());`), add:
```rust
    state.lock().file_commands =
        crate::tui_vt::slash::file_commands::load_file_commands(&cwd);
```

- [ ] **Step 4: Build and verify everything compiles**

Run: `cargo build -p oxicode-cli`
Expected: PASS — all field references from Tasks 3 and 4 now resolve.

- [ ] **Step 5: Run full test suite for the crate**

Run: `cargo nextest run -p oxicode-cli`
Expected: PASS — all tests.

- [ ] **Step 6: Commit**

```bash
git add oxicode-cli/src/tui_vt/main_loop.rs
git commit -m "feat(tui): load user-defined slash commands into RenderState at startup"
```

---

### Task 6: Clippy + Fmt + E2E Smoke Test

**Files:**
- No new files. Verification only.

- [ ] **Step 1: Run clippy clean**

Run: `cargo clippy -p oxicode-cli --all-targets -- -D warnings`
Expected: PASS — no warnings.

If clippy flags `unwrap_used` in non-test code, replace with `?` or `unwrap_or_default()`.

- [ ] **Step 2: Run fmt check**

Run: `cargo fmt --all -- --check`
Expected: PASS. If not, run `cargo fmt --all` and re-check.

- [ ] **Step 3: Manual E2E smoke test**

Create a test command:
```bash
mkdir -p .oxicode/commands
cat > .oxicode/commands/summarize.md << 'EOF'
---
description: Summarize a file concisely
aliases: sum
---
Read and summarize the following file in 3 bullet points:

$ARGUMENTS
EOF
```

Launch the TUI: `cargo run -q --`
Type `/sum` — verify the autocomplete popup shows `summarize`.
Type `/summarize oxicode-cli/src/main.rs` — verify the agent receives the expanded prompt ("Read and summarize ... oxicode-cli/src/main.rs ...").

Clean up:
```bash
rm -rf .oxicode/commands/summarize.md
```

- [ ] **Step 4: Commit if any formatting fixes were applied**

```bash
git add -A
git commit -m "test(tui): verify user-defined slash command E2E flow"
```

---

## Self-Review

**1. Spec coverage:**
- ✓ File-based command discovery from `.oxicode/commands/` + `~/.oxicode/commands/` → Task 2
- ✓ Frontmatter (description, aliases) → Task 1
- ✓ Template expansion ($ARGUMENTS, $@, $1, $2) → Task 1
- ✓ Builtins always win (dispatch checks builtins first, file commands in NotHandled) → Task 3
- ✓ Project beats user on collision → Task 2 (scan order + first-wins seen set)
- ✓ Autocomplete popup includes file commands → Task 4
- ✓ No file watcher, load once at startup → Task 5

**2. Placeholder scan:** No TBD/TODO/"add appropriate". All code blocks are complete.

**3. Type consistency:**
- `FileCommand` fields: `name: String`, `description: String`, `aliases: Vec<String>`, `body: String` — consistent across Tasks 1–4.
- `load_file_commands(cwd: &Path) -> Vec<FileCommand>` — Task 2 defines, Task 5 calls.
- `try_expand(commands: &[FileCommand], input: &str) -> Option<String>` — Task 3 defines, Task 3 step 5 calls.
- `slash_filter(token: &str, file_commands: &[FileCommand])` — Task 4 defines, Task 4 step 2 calls.
- `RenderState.file_commands: Vec<FileCommand>` — Task 5 defines, Tasks 3+4 reference.
