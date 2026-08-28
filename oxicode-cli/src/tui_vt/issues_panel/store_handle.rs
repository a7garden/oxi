//! Lazily opens the project's `FileIssueStore`, cached on `RenderState` so
//! every panel action reuses the same in-memory cache instead of re-reading
//! the issues directory from scratch.

use std::sync::Arc;

use crate::store::issues::FileIssueStore;
use crate::tui_vt::main_loop::RenderState;

pub(crate) fn get_or_open_store(state: &mut RenderState) -> anyhow::Result<Arc<FileIssueStore>> {
    if let Some(store) = &state.issue_store {
        return Ok(store.clone());
    }
    let dir = crate::store::issues::issues_dir(&state.cwd);
    let store = Arc::new(FileIssueStore::open(dir)?);
    state.issue_store = Some(store.clone());
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_call_reuses_the_cached_arc() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = RenderState {
            cwd: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let first = get_or_open_store(&mut state).unwrap();
        let second = get_or_open_store(&mut state).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }
}
