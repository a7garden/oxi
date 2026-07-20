//! oxi-sandbox — PR-D1 스켈레톤.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Sandbox 정책 종류.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxProfile {
    /// 파일 읽기만 허용.
    ReadOnly,
    /// workspace 내부 읽기/쓰기 허용.
    WorkspaceWrite,
    /// workspace 접근 + 화이트리스트 호스트만 네트워크 허용.
    NetworkRestricted,
    /// 사용자 정의 규칙.
    Custom(PolicyRules),
}

impl SandboxProfile {
    /// ReadOnly 프로파일 여부.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::ReadOnly)
    }

    /// WorkspaceWrite 프로파일 여부.
    pub fn is_workspace_write(&self) -> bool {
        matches!(self, Self::WorkspaceWrite)
    }

    /// NetworkRestricted 프로파일 여부.
    pub fn is_network_restricted(&self) -> bool {
        matches!(self, Self::NetworkRestricted)
    }
}

/// Sandbox 정책 상세.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRules {
    /// 읽기 허용 경로.
    pub allowed_read_paths: Vec<PathBuf>,
    /// 쓰기 허용 경로.
    pub allowed_write_paths: Vec<PathBuf>,
    /// 네트워크 허용 호스트.
    pub allowed_network_hosts: Vec<String>,
    /// 차단 env var (예: `LD_PRELOAD`). 비어있으면 차단 안 함.
    pub blocked_env_vars: Vec<String>,
}

/// Sandbox 오류.
#[derive(Debug, Error)]
pub enum SandboxError {
    /// 플랫폼이 feature flag 미활성화.
    #[error("sandbox backend not enabled for this build (feature flag missing)")]
    BackendDisabled,
    /// 워크스페이스 경로 부재 / 잘못됨.
    #[error("invalid workspace path: {0}")]
    InvalidWorkspace(PathBuf),
    /// 정책 위반.
    #[error("sandbox policy violation: target={target}, op={op}")]
    PolicyViolation { target: String, op: String },
    /// 내부 I/O 오류.
    #[error("sandbox io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Sandbox 매니저.
///
/// PR-D1 에서는 타입 / 상태만 보유. 실제 wrap 호출은 PR-D2 부터.
pub struct SandboxManager {
    profile: SandboxProfile,
    workspace_root: PathBuf,
    /// 위반 카운터 — PR-D1 단순 카운트. 추후 Prometheus 등으로 확장.
    violation_count: parking_lot::Mutex<u64>,
}

impl std::fmt::Debug for SandboxManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxManager")
            .field("profile", &self.profile)
            .field("workspace_root", &self.workspace_root)
            .field("violation_count", &*self.violation_count.lock())
            .finish()
    }
}

impl SandboxManager {
    /// 새 매니저 생성.
    pub fn new(profile: SandboxProfile, workspace_root: PathBuf) -> Self {
        Self {
            profile,
            workspace_root,
            violation_count: parking_lot::Mutex::new(0),
        }
    }

    /// 워크스페이스 경로.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// 활성 프로파일.
    pub fn profile(&self) -> &SandboxProfile {
        &self.profile
    }

    /// 정책 위반 카운트.
    pub fn violation_count(&self) -> u64 {
        *self.violation_count.lock()
    }

    /// 정책 위반 로깅.
    pub fn log_violation(&self, target: &str, op: &str) {
        *self.violation_count.lock() += 1;
        tracing::warn!(target = target, op = op, "sandbox policy violation");
    }

    /// 정책 적용 — PR-D1 stub. PR-D2 부터 bwrap/sandbox-exec 호출.
    pub fn apply(&self) -> Result<(), SandboxError> {
        if !self.workspace_root.as_os_str().is_empty() {
            Ok(())
        } else {
            Err(SandboxError::InvalidWorkspace(self.workspace_root.clone()))
        }
    }

    /// 경로가 workspace 내부인지 검사.
    pub fn is_within_workspace(&self, path: &Path) -> bool {
        path.starts_with(&self.workspace_root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_manager_with_profile_and_workspace() {
        let mgr = SandboxManager::new(SandboxProfile::WorkspaceWrite, PathBuf::from("/tmp/work"));
        assert_eq!(mgr.profile(), &SandboxProfile::WorkspaceWrite);
        assert_eq!(mgr.workspace_root(), Path::new("/tmp/work"));
        assert_eq!(mgr.violation_count(), 0);
    }

    #[test]
    fn profile_is_predicates() {
        let r = SandboxProfile::ReadOnly;
        assert!(r.is_read_only());
        assert!(!r.is_workspace_write());
        assert!(!r.is_network_restricted());

        let w = SandboxProfile::WorkspaceWrite;
        assert!(!w.is_read_only());
        assert!(w.is_workspace_write());
        assert!(!w.is_network_restricted());

        let n = SandboxProfile::NetworkRestricted;
        assert!(!n.is_read_only());
        assert!(!n.is_workspace_write());
        assert!(n.is_network_restricted());
    }

    #[test]
    fn custom_profile_carries_policy_rules() {
        let policy = PolicyRules {
            allowed_read_paths: vec![PathBuf::from("/usr")],
            allowed_write_paths: vec![PathBuf::from("/workspace")],
            allowed_network_hosts: vec!["api.example.com".into()],
            blocked_env_vars: vec!["LD_PRELOAD".into()],
        };
        let p = SandboxProfile::Custom(policy.clone());
        match p {
            SandboxProfile::Custom(rules) => assert_eq!(rules, policy),
            _ => panic!("expected Custom variant"),
        }
    }

    #[test]
    fn log_violation_increments_counter() {
        let mgr = SandboxManager::new(SandboxProfile::ReadOnly, PathBuf::from("/tmp/work"));
        mgr.log_violation("/etc/passwd", "read");
        mgr.log_violation("/etc/shadow", "read");
        assert_eq!(mgr.violation_count(), 2);
    }

    #[test]
    fn apply_returns_ok_when_workspace_set() {
        let mgr = SandboxManager::new(SandboxProfile::WorkspaceWrite, PathBuf::from("/tmp/work"));
        assert!(mgr.apply().is_ok());
    }

    #[test]
    fn apply_returns_err_when_workspace_empty() {
        let mgr = SandboxManager::new(SandboxProfile::ReadOnly, PathBuf::new());
        match mgr.apply() {
            Err(SandboxError::InvalidWorkspace(_)) => {}
            other => panic!("expected InvalidWorkspace, got {other:?}"),
        }
    }

    #[test]
    fn is_within_workspace_checks_path_prefix() {
        let mgr = SandboxManager::new(SandboxProfile::WorkspaceWrite, PathBuf::from("/tmp/work"));
        assert!(mgr.is_within_workspace(Path::new("/tmp/work/foo")));
        assert!(mgr.is_within_workspace(Path::new("/tmp/work")));
        assert!(!mgr.is_within_workspace(Path::new("/etc")));
        assert!(!mgr.is_within_workspace(Path::new("/tmp/other")));
    }

    #[test]
    fn policy_rules_default_is_empty() {
        let r = PolicyRules::default();
        assert!(r.allowed_read_paths.is_empty());
        assert!(r.allowed_write_paths.is_empty());
        assert!(r.allowed_network_hosts.is_empty());
        assert!(r.blocked_env_vars.is_empty());
    }
}
