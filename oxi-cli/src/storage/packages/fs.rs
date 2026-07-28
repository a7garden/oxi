//! Filesystem helpers for the package system.
//!
//! `copy_dir_recursive` is used during install to materialise a package
//! into the packages dir. `find_single_subdir` handles the common case
//! of an extracted archive that nests the package under a single
//! root directory. `prune_empty_parents` keeps the git/npm scoped
//! install dirs tidy after removal.

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Recursively copy a directory
pub(super) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// Find the single subdirectory inside an extracted archive
pub(super) fn find_single_subdir(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<_> = fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).collect();
    if entries.len() == 1 && entries[0].path().is_dir() {
        Some(entries[0].path())
    } else {
        None
    }
}

/// Remove empty parent directories up to a root
pub(super) fn prune_empty_parents(target: &Path, root: &Path) {
    let mut current = target.parent();
    while let Some(dir) = current {
        if dir == root || !dir.starts_with(root) {
            break;
        }
        if dir.exists() {
            let is_empty = fs::read_dir(dir)
                .map(|mut rd| rd.next().is_none())
                .unwrap_or(false);
            if is_empty {
                let _ = fs::remove_dir(dir);
            } else {
                break;
            }
        }
        current = dir.parent();
    }
}
