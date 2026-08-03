//! Auto-discovery of resources inside an installed package directory.
//!
//! When a `PackageManifest` has no explicit resource lists, the manager
//! walks the install dir and collects `DiscoveredResource`s by extension
//! or by directory shape (`SKILL.md` for skills, `prompts/*.md` for
//! prompts, `themes/*.json` for themes). Hidden dirs and
//! `node_modules` are skipped.

use super::types::{DiscoveredResource, ResourceKind};
use std::fs;
use std::path::Path;

/// Discover extension files in a directory.
pub(super) fn discover_extensions(dir: &Path) -> Vec<DiscoveredResource> {
    let mut results = Vec::new();
    discover_extensions_recursive(dir, dir, &mut results);
    results
}

pub(super) fn discover_extensions_recursive(
    base: &Path,
    current: &Path,
    results: &mut Vec<DiscoveredResource>,
) {
    if !current.exists() {
        return;
    }

    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            // Check for index.ts / index.js in subdirectory
            for index in &["index.ts", "index.js"] {
                let index_path = path.join(index);
                if index_path.exists() {
                    let rel = path.strip_prefix(base).unwrap_or(&path);
                    results.push(DiscoveredResource {
                        kind: ResourceKind::Extension,
                        path: index_path,
                        relative_path: rel.join(index).to_string_lossy().to_string(),
                    });
                }
            }
        } else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "so" | "dylib" | "dll" | "ts" | "js") {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                results.push(DiscoveredResource {
                    kind: ResourceKind::Extension,
                    path: path.clone(),
                    relative_path: rel.to_string_lossy().to_string(),
                });
            }
        }
    }
}

/// Discover skill directories containing SKILL.md
pub(super) fn discover_skills(dir: &Path) -> Vec<DiscoveredResource> {
    let mut results = Vec::new();
    discover_skills_recursive(dir, dir, &mut results);
    results
}

pub(super) fn discover_skills_recursive(
    base: &Path,
    current: &Path,
    results: &mut Vec<DiscoveredResource>,
) {
    if !current.exists() {
        return;
    }

    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            let skill_file = path.join("SKILL.md");
            if skill_file.exists() {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                results.push(DiscoveredResource {
                    kind: ResourceKind::Skill,
                    path: skill_file,
                    relative_path: rel.join("SKILL.md").to_string_lossy().to_string(),
                });
            }
            discover_skills_recursive(base, &path, results);
        }
    }
}

/// Discover prompt template files (.md in prompts/ subdirectory)
pub(super) fn discover_prompts(dir: &Path) -> Vec<DiscoveredResource> {
    let prompts_dir = dir.join("prompts");
    discover_files_by_ext(
        if prompts_dir.exists() {
            &prompts_dir
        } else {
            dir
        },
        "md",
        ResourceKind::Prompt,
    )
}

/// Discover theme files (.json in themes/ subdirectory)
pub(super) fn discover_themes(dir: &Path) -> Vec<DiscoveredResource> {
    let themes_dir = dir.join("themes");
    discover_files_by_ext(
        if themes_dir.exists() {
            &themes_dir
        } else {
            dir
        },
        "json",
        ResourceKind::Theme,
    )
}

/// Recursively find files with a given extension
pub(super) fn discover_files_by_ext(
    dir: &Path,
    ext: &str,
    kind: ResourceKind,
) -> Vec<DiscoveredResource> {
    let mut results = Vec::new();
    discover_files_recursive(dir, dir, ext, kind, &mut results);
    results
}

pub(super) fn discover_files_recursive(
    base: &Path,
    current: &Path,
    ext: &str,
    kind: ResourceKind,
    results: &mut Vec<DiscoveredResource>,
) {
    if !current.exists() {
        return;
    }

    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            discover_files_recursive(base, &path, ext, kind, results);
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            let rel = path.strip_prefix(base).unwrap_or(&path);
            results.push(DiscoveredResource {
                kind,
                path: path.clone(),
                relative_path: rel.to_string_lossy().to_string(),
            });
        }
    }
}
