//! Session-scoped todo state.
//!
//! `oxicode_agent::tools::TodoStateProvider`의 구현. 에이전트의 `todo` 도구와
//! TUI sticky panel 사이의 단일 진실 소스. `Arc<RwLock<Vec<TodoPhase>>>`로
//! 보유하고, clone은 cheap (Arc 공유).

use std::pin::Pin;
use std::sync::Arc;

use oxicode_agent::tools::todo::{TodoOp, TodoPhase, TodoUpdateResult};
use oxicode_agent::tools::{TodoStateProvider, ToolError};
use parking_lot::RwLock;

/// Session 단위 todo 상태.
///
/// `TodoStateProvider` 특성 구현 — `todo` 도구와 sticky panel이 공유.
/// TUI 채널로 갱신 알림을 보내려면 `notifier: Option<Box<dyn Fn>>`를 추가.
#[derive(Debug)]
pub struct TodoState {
    phases: RwLock<Vec<TodoPhase>>,
}

impl TodoState {
    /// 새 빈 상태 생성.
    pub fn new() -> Self {
        Self {
            phases: RwLock::new(Vec::new()),
        }
    }

    /// 초기 phase들을 지정하여 생성.
    pub fn with_phases(phases: Vec<TodoPhase>) -> Self {
        Self {
            phases: RwLock::new(phases),
        }
    }

    /// 현재 phase 스냅샷 (TUI 매 프레임 호출).
    pub fn get_phases(&self) -> Vec<TodoPhase> {
        self.phases.read().clone()
    }

    /// ops 적용. `apply_ops` 헬퍼를 위임.
    pub fn apply(&self, ops: Vec<TodoOp>) -> TodoUpdateResult {
        let mut phases = self.phases.write();
        let result = oxicode_agent::tools::todo::apply_ops(&mut phases, &ops);
        // 가드는 여기서 drop — async 호출 전.
        drop(phases);
        result
    }

    /// 전체 삭제 (테스트/리셋용).
    #[allow(dead_code)]
    pub fn clear(&self) {
        self.phases.write().clear();
    }
}

impl Default for TodoState {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoStateProvider for TodoState {
    fn get_phases(&self) -> Vec<TodoPhase> {
        TodoState::get_phases(self)
    }

    fn apply_ops<'a>(
        &'a self,
        ops: Vec<TodoOp>,
    ) -> Pin<Box<dyn Future<Output = Result<TodoUpdateResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            // RwLock 가드는 동기 acquire → await 전에 drop
            let mut phases = self.phases.write();
            Ok(oxicode_agent::tools::todo::apply_ops(&mut phases, &ops))
        })
    }
}

/// `Arc<TodoState>`를 그대로 `Arc<dyn TodoStateProvider>`로 변환.
pub fn provider_from_state(state: Arc<TodoState>) -> Arc<dyn TodoStateProvider> {
    state as Arc<dyn TodoStateProvider>
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oxicode_agent::tools::todo::TodoItem;

    #[test]
    fn empty_state() {
        let state = TodoState::new();
        assert!(state.get_phases().is_empty());
    }

    #[test]
    fn apply_ops_via_provider() {
        let state = TodoState::new();
        let provider: Arc<dyn TodoStateProvider> = provider_from_state(Arc::new(state));

        // tokio runtime 없이 apply_ops 동기 부분만 검증.
        let phases = provider.get_phases();
        assert!(phases.is_empty());

        // ops 적용 후 get_phases가 갱신됨을 검증.
        // (비동기 trait method 호출은 tokio test 필요 — 간단히 동기 .apply() 사용)
        let state_arc = Arc::new(TodoState::new());
        let result = state_arc.apply(vec![TodoOp::Init {
            list: None,
            items: Some(vec!["task1".into(), "task2".into()]),
        }]);
        assert!(result.errors.is_empty());
        let phases = state_arc.get_phases();
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].tasks.len(), 2);
    }

    #[test]
    fn provider_share_state() {
        // 같은 Arc를 공유하는 두 provider가 같은 상태를 본다.
        let state = Arc::new(TodoState::with_phases(vec![]));
        let p1: Arc<dyn TodoStateProvider> = state.clone();
        let p2: Arc<dyn TodoStateProvider> = state.clone();

        let _ = p1.get_phases(); // Arc::strong_count 증가 확인은 보이지 않지만 호출 가능
        let _ = p2.get_phases();
        assert!(state.get_phases().is_empty());
    }

    #[test]
    fn with_phases_initial() {
        let initial = vec![TodoPhase {
            name: "P".into(),
            tasks: vec![TodoItem {
                content: "t".into(),
                status: oxicode_agent::tools::todo::TodoStatus::Pending,
                notes: None,
            }],
        }];
        let state = TodoState::with_phases(initial);
        assert_eq!(state.get_phases().len(), 1);
        assert_eq!(state.get_phases()[0].name, "P");
    }
}
