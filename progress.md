# Progress

## 2026-05-05 — Fix oxi-cli ignored tests

- Fixed `test_parse_version_invalid`: Changed `parse_version()` to reject non-exactly-3-component version strings (`parts.len() == 3` instead of `>= 3`)
- Fixed `test_calculate_versions_behind`: Rewrote to use per-component difference with weighting (major × 2, minor × 1, patch × 1) instead of raw `compare_versions` result
- Removed both `#[ignore]` attributes
- All 22 version_check tests pass (0 ignored)
