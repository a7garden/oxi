# Progress

## Status
In Progress

## Tasks
- [x] Fix 1: navigate_tree block_on panic (session_navigation.rs)
- [x] Fix 2: get_branch() lock thrashing (session.rs)
- [x] Fix 3: _append_entry lock ordering comment (session.rs)
- [x] Fix 4: SessionCwd escape fix (session_cwd.rs)
- [x] Fix 5: Double markdown parsing (table_renderer.rs)
- [x] Fix 6: Theme parse_color warning (theme.rs)
- [x] Fix 7: input_history pop() bug (handlers.rs, app.rs)
- [x] Fix 8: Footer HOME env var → dirs::home_dir() (footer.rs)
- [x] Fix 9: measure_call_height bash double-count (tool_renderer.rs)

## Files Changed
- oxi-store/src/session_navigation.rs
- oxi-store/src/session.rs
- oxi-store/src/session_cwd.rs
- oxi-tui/src/table_renderer.rs
- oxi-tui/src/theme.rs
- oxi-cli/src/tui/handlers.rs
- oxi-cli/src/tui/app.rs
- oxi-tui/src/widgets/footer.rs
- oxi-tui/src/widgets/tool_renderer.rs

## Notes
- Session navigation fixes: See fix_session_navigation.md for details.
- TUI fixes: See fix_tui_issues.md for details. All changes compile (oxi-tui clean; oxi-cli has pre-existing oxi-store errors).