//! 파일 접근 경로 보안 검증.

use std::path::{Path, PathBuf};

/// 경로 보안 오류
#[derive(Debug)]
pub enum PathSecurityError {
    /// 경로를 찾을 수 없음
    NotFound(PathBuf),
    /// 경로 순회 감지
    Traversal(PathBuf),
    /// 작업 공간 밖 경로
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

/// 파일 접근 시 보안 검증 유틸.
pub struct PathGuard {
    root: PathBuf,
}

impl PathGuard {
    /// 작업 디렉토리 기반으로 생성.
    pub fn new(cwd: &Path) -> Self {
        let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        Self { root }
    }

    /// 경로가 작업 공간 내에 있는지 확인.
    pub fn validate(&self, path: &Path) -> Result<PathBuf, PathSecurityError> {
        // 1. 순회 방지
        if path.components().any(|c| c.as_os_str() == "..") {
            return Err(PathSecurityError::Traversal(path.to_path_buf()));
        }

        // 2. 존재하는 경로면 canonicalize로 실제 경로 확인
        if path.exists() {
            let canonical = path
                .canonicalize()
                .map_err(|_| PathSecurityError::NotFound(path.to_path_buf()))?;

            // 3. 루트 내부인지 확인
            if !canonical.starts_with(&self.root) {
                return Err(PathSecurityError::OutsideWorkspace(canonical));
            }

            Ok(canonical)
        } else {
            // 존재하지 않는 경로는 순회만 확인
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
        // 1. 순회 방지
        if path.components().any(|c| c.as_os_str() == "..") {
            return Err(PathSecurityError::Traversal(path.to_path_buf()));
        }

        // 2. 존재하는 경로면 canonicalize로 실제 경로 얻기
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
