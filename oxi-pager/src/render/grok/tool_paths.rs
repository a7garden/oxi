//! Read/Edit tool-path resolution and surface formatting.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use unicode_width::UnicodeWidthStr;

use super::line_utils::truncate_str;

/// OXI-CHANGE: inlined replacement for `xai_grok_paths::normalize_lexically`.
/// Lexical normalization — does not touch the filesystem. Strips `.` and
/// resolves `..` purely by component manipulation.
fn normalize_lexically(path: impl AsRef<std::path::Path>) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.as_ref().components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// Read/Edit tool-header path paint surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPathSurface {
    /// Basename only.
    Collapsed,
    /// Relative to session cwd when lexically contained; else normalized.
    Expanded,
    /// Normalized target spelling for the modal preamble.
    Fullscreen,
}

#[derive(Debug, Clone)]
struct ResolvedToolPath {
    display_path: PathBuf,
    relative_to_cwd: Option<String>,
}

fn expand_tilde_with_home(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    use std::path::Component;

    let mut components = path.components();
    let Some(Component::Normal(first)) = components.next() else {
        return Some(path.to_path_buf());
    };
    if first != "~" {
        return Some(path.to_path_buf());
    }

    let mut expanded = home?.to_path_buf();
    for component in components {
        match component {
            Component::Prefix(_) | Component::RootDir => {}
            _ => expanded.push(component.as_os_str()),
        }
    }
    Some(expanded)
}

/// Resolve the path the OS should receive, preserving `.`/`..` and symlink semantics.
pub(crate) fn resolve_tool_path_target_with_home(
    path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    use std::path::Component;

    let target = expand_tilde_with_home(path, home)?;
    if target.is_absolute() || matches!(target.components().next(), Some(Component::Prefix(_))) {
        return Some(target);
    }
    Some(match cwd {
        Some(cwd) => cwd.join(target),
        None => target,
    })
}

fn non_empty_rel(rel: &Path) -> Option<String> {
    let value = rel.to_string_lossy();
    if value.is_empty() {
        None
    } else {
        Some(value.into_owned())
    }
}

fn home_dir() -> Option<&'static Path> {
    static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    HOME.get_or_init(dirs::home_dir).as_deref()
}

/// Resolve the path-native target for OSC8 or background filesystem work.
pub fn resolve_tool_path_target(path: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    resolve_tool_path_target_with_home(Path::new(path), cwd, home_dir())
}

fn resolve_tool_path_with_home(
    path: &str,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> ResolvedToolPath {
    let target = resolve_tool_path_target_with_home(Path::new(path), cwd, home);
    let display_path = target
        .as_deref()
        .map(normalize_lexically)
        .unwrap_or_else(|| PathBuf::from(path));
    let relative_to_cwd = target.as_ref().and_then(|_| {
        let cwd = normalize_lexically(cwd?);
        display_path.strip_prefix(cwd).ok().and_then(non_empty_rel)
    });
    ResolvedToolPath {
        display_path,
        relative_to_cwd,
    }
}

fn resolve_tool_path(path: &str, cwd: Option<&Path>) -> ResolvedToolPath {
    resolve_tool_path_with_home(path, cwd, home_dir())
}

fn path_for_fullscreen_header(path: &str, cwd: Option<&Path>) -> String {
    resolve_tool_path(path, cwd)
        .display_path
        .to_string_lossy()
        .into_owned()
}

fn path_for_expanded_header(path: &str, cwd: Option<&Path>) -> String {
    let resolved = resolve_tool_path(path, cwd);
    resolved
        .relative_to_cwd
        .unwrap_or_else(|| resolved.display_path.to_string_lossy().into_owned())
}

/// Shorten a file path to fit within `budget` display columns using fish-style
/// component shortening.
pub fn shorten_path(path: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    if path.width() <= budget {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 1 {
        return truncate_str(path, budget);
    }

    let mut shortened: Vec<String> = parts.iter().map(|part| part.to_string()).collect();
    let last_idx = shortened.len() - 1;
    for i in 0..last_idx {
        if shortened.iter().map(String::len).sum::<usize>() + shortened.len() - 1 <= budget {
            break;
        }
        if let Some(first) = parts[i].chars().next() {
            shortened[i] = first.to_string();
        }
    }

    let joined = shortened.join("/");
    if joined.width() <= budget {
        return joined;
    }

    let mut tail_start = 0;
    for (i, _) in path.char_indices() {
        if i == 0 {
            continue;
        }
        if path.as_bytes().get(i.wrapping_sub(1)) == Some(&b'/') {
            let candidate = format!("\u{2026}{}", &path[i - 1..]);
            if candidate.width() <= budget {
                tail_start = i - 1;
                break;
            }
        }
    }
    if tail_start > 0 {
        let result = format!("\u{2026}{}", &path[tail_start..]);
        if result.width() <= budget {
            return result;
        }
    }
    truncate_str(path, budget)
}

pub fn path_basename(path: &str, budget: usize) -> String {
    let name = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path);
    truncate_str(name, budget)
}

/// Compatibility formatter: compact basename with `Some(width)`, else stored path.
pub fn path_for_tool_header(path: &str, width: Option<usize>, reserved: usize) -> String {
    match width {
        Some(width) => path_basename(path, width.saturating_sub(reserved)),
        None => path.to_string(),
    }
}

/// Path text for a Read/Edit tool-header surface.
pub fn path_for_tool_surface(
    path: &str,
    surface: ToolPathSurface,
    cwd: Option<&Path>,
    width: Option<usize>,
    reserved: usize,
) -> String {
    match surface {
        ToolPathSurface::Collapsed => {
            let budget = width.unwrap_or(usize::MAX).saturating_sub(reserved);
            path_basename(path, budget)
        }
        ToolPathSurface::Expanded => path_for_expanded_header(path, cwd),
        ToolPathSurface::Fullscreen => path_for_fullscreen_header(path, cwd),
    }
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
