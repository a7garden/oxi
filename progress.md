# oxi 버전 업그레이드 진행 상황

## 작업 상태: ✅ 완료

### 수행된 작업
- [x] 루트 Cargo.toml 확인 (workspace only, 버전 없음)
- [x] oxi-ai/Cargo.toml: 0.3.1 → 0.4.0
- [x] oxi-agent/Cargo.toml: 0.3.1 → 0.4.0 + oxi-ai 의존성 0.3.1 → 0.4.0
- [x] oxi-tui/Cargo.toml: 0.3.1 → 0.4.0
- [x] oxi-cli/Cargo.toml: 0.3.1 → 0.4.0 + 모든 oxi-* 의존성 0.3.1 → 0.4.0
- [x] oxi-cli/src/main.rs: clap version 0.1.0 → 0.4.0 (기존 불일치 수정)
- [x] 결과 보고서: /tmp/oxi-version-bump.md
