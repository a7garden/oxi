//! File path security validation.

use std::path::{Path, PathBuf};

/// Path security error
#[derive(Debug)]
pub enum PathSecurityError {
    /// Path not found
    NotFound(PathBuf),
    /// Path traversal detected
    Traversal(PathBuf),
    /// Path outside workspace
    OutsideWorkspace(PathBuf),
}

impl std::fmt::Display for PathSecurityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "Path not found: {}", p.display()),
            Self::Traversal(p) => write!(f, "Path traversal detected: {}", p.display()),
            Self::OutsideWorkspace(p) => write!(f, "Path outside workspace: {}", p.display()),
        }
    }
}

impl std::error::Error for PathSecurityError {}

/// Security validation utility for file access.
pub struct PathGuard {
    root: PathBuf,
}

impl PathGuard {
    /// Creates a new instance based on the current working directory.
    pub fn new(cwd: &Path) -> Self {
        let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        Self { root }
    }

    /// Checks whether the path is within the workspace.
    pub fn validate(&self, path: &Path) -> Result<PathBuf, PathSecurityError> {
        // 1. Prevent traversal
        if path.components().any(|c| c.as_os_str() == "..") {
            return Err(PathSecurityError::Traversal(path.to_path_buf()));
        }

        // 2. For existing paths, canonicalize to resolve the real path
        if path.exists() {
            let canonical = path
                .canonicalize()
                .map_err(|_| PathSecurityError::NotFound(path.to_path_buf()))?;

            // 3. Verify it is inside the root
            if !canonical.starts_with(&self.root) {
                return Err(PathSecurityError::OutsideWorkspace(canonical));
            }

            Ok(canonical)
        } else {
            // For non-existing paths, only check traversal
            Ok(path.to_path_buf())
        }
    }

    /// Check traversal only (no workspace boundary enforcement).
    ///
    /// Use this for tools that may legitimately access paths outside the
    /// workspace root (e.g. reading system config, writing to temp dirs).
    /// Blocks `..` components and canonicalizes existing paths but does
    /// NOT reject absolute paths outside the workspace.
    pub fn validate_traversal(&self, path: &Path) -> Result<PathBuf, PathSecurityError> {
        // 1. Prevent traversal
        if path.components().any(|c| c.as_os_str() == "..") {
            return Err(PathSecurityError::Traversal(path.to_path_buf()));
        }

        // 2. For existing paths, canonicalize to get the real path
        if path.exists() {
            let canonical = path
                .canonicalize()
                .map_err(|_| PathSecurityError::NotFound(path.to_path_buf()))?;
            Ok(canonical)
        } else {
            Ok(path.to_path_buf())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reject_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(tmp.path());
        let result = guard.validate(Path::new("../../../etc/passwd"));
        assert!(result.is_err());
    }

    #[test]
    fn accept_valid_path() {
        let tmp = tempfile::tempdir().unwrap();
        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "hello").unwrap();
        let guard = PathGuard::new(tmp.path());
        let result = guard.validate(&test_file);
        assert!(result.is_ok());
    }

    #[test]
    fn reject_absolute_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(tmp.path());
        let result = guard.validate(Path::new("/etc/passwd"));
        assert!(result.is_err());
    }
}
