# Legacy Cleanup & Incomplete Features — Implementation Log

**Date**: 2026-06-01
**Status**: 완료 (oxicode), 완료 (oxios)

---

## Completed Items

### oxicode Project

#### #10: oxicode-cli → oxicode-sdk 의존성 제거 ✅
- **Commit**: `oxicode-cli/Cargo.toml`에서 `oxicode-sdk` 의존성 제거
- **Files**: `Cargo.toml`, `lib.rs`
- **Before**: `OxicodeBuilder::new().with_builtins().build()` → `engine.create_provider()` + `engine.resolve_model()`
- **After**: `oxicode_ai::get_provider_arc()` + `oxicode_ai::lookup_model()`
- **Impact**: 빌드 의존성 트리에서 oxicode-sdk + 하위 모듈 제거, 컴파일 시간 감소

#### #1: TUI 세션 브랜치 전환 ✅
- **Files**: `oxicode-cli/src/tui/app.rs`, `oxicode-cli/src/tui/handlers.rs`, `oxicode-store/src/session.rs`
- **Before**: `NavigateToEntry` → notification만 표시 (아무 일 안 일어남)
- **After**: `TuiNextAction::GotoEntry(entry_id)` → `SessionManager::set_leaf_from_entry()` → 채팅 메시지 리로드
- **Key additions**:
  - `TuiNextAction::GotoEntry(String)` variant
  - `SessionManager::set_leaf_from_entry(entry_id)` method (oxicode-store)
  - Handler가 overlay에서 entry 선택 시 `GotoEntry` action 트리거
  - Main loop에서 session 열고 leaf 변경 후 메시지 리로드

#### #5: Setup OAuth 숨김 ✅
- **Files**: `oxicode-cli/src/tui/handlers.rs`, `oxicode-cli/src/tui/render.rs`
- **Before**: OAuth 선택지가 표시되지만 선택하면 아무 일 없이 provider 선택으로 넘어감 (사용자 오해)
- **After**: OAuth 선택지를 주석 처리. API Key만 표시. 구현 완료 시 주석 해제 가능

### oxios Project

#### #11: oxios-kernel lib.rs dead re-export 74개 제거 ✅
- **Files**: `crates/oxios-kernel/src/lib.rs`, `crates/oxios-kernel/src/coordination.rs`
- Block 1 (top-level 21개): 제거
- Block 2 (`sdk_exports` 모듈 33개): 제거
- Block 3 (`coordination.rs` re-export 15개): 제거
- Block 4 (`CircuitBreaker` alias): 제거
- Comment 추가: "Consumers should depend on oxicode-sdk directly"

#### #7: WasmSandbox feature-gate ✅
- **File**: `crates/oxios-kernel/src/wasm_sandbox.rs`
- 전체 모듈을 `#[cfg(feature = "wasm-sandbox")]`로 감쌈
- `#[cfg(not(feature = "wasm-sandbox"))]` stub 블록 제거
- feature off 시 컴파일에서 완전 제외

#### #9: Orchestrator 세션 복원 구현 ✅
- **File**: `crates/oxios-kernel/src/orchestrator.rs`
- **Before**: `Ok(())` 빈 stub
- **After**: `state_store.list_sessions()` → active_seed_id 필터링 → session 복원 → in-memory map에 삽입

#### #6: WorkerManager trait-based 구현 ✅
- **File**: `crates/oxios-kernel/src/workers.rs`
- **Before**: 12개 하드코딩 가짜 문자열 반환
- **After**: `Worker` trait 정의 → `register_implementation()` → trait 호출
- 기본 구현 없음 — 외부에서 명시적으로 등록해야 작동
- 미등록 워커는 에러 반환: "No implementation registered"

#### #8: Memory 5단계 Compaction 구현 ✅
- **File**: `crates/oxios-kernel/src/memory/compaction.rs`
- `#[allow(dead_code)]` 전부 제거
- `CompactionLevel`에 `as_u8()`, `from_u8()`, `compression_ratio()`, `target_summary_lines()` 추가
- `CompactionTree`에 `should_promote()`, `compact_to_level()`, `promote()`, `compact_single_level()` 추가
- 구조적 압축: 헤더 보존 + head/tail 보존 + middle 샘플링
- 5단계 승격: Raw(30%) → Daily(40%) → Weekly(50%) → Monthly(60%) → Root

#### #12: degraded_seed() 연결 ✅
- **Files**: `crates/oxios-ouroboros/src/degraded.rs`, `ouroboros_engine.rs`
- `ouroboros_engine.rs`의 `generate_seed()`에서 inline fallback을 `degraded::degraded_seed()`로 교체
- `#[allow(dead_code)]` + TODO 제거

#### #13: record_access() 연결 ✅
- **Files**: `crates/oxios-kernel/src/memory/auto_protect.rs`, `store.rs`
- `store.rs`의 `get()`, `search()`, semantic search에서 `AutoProtector::record_access()` 호출
- `#[allow(dead_code)]` 제거
- 인라인 접근 추적 코드를 `record_access()` 호출로 교체

#### #14: hyperbolic c() 정리 ✅
- **File**: `crates/oxios-kernel/src/memory/hyperbolic.rs`
- `#[allow(dead_code)]` 제거
- `c()` 메서드에 문서 주석 추가 (설계 의유 설명)

---

## Blocked Items (kernel API 필요)

이 항목들은 oxios-kernel API 변경이 선행되어야 함:

#### #2: Web chat tool_calls
- **Blocker**: kernel이 trajectory_steps를 API로 노출해야 함

#### #3: Web A2A 로깅
- **Blocker**: kernel A2AProtocol에 메시지 로깅 추가 필요

#### #4: CLI 모델/페르소나 전환
- **Blocker**: kernel model switching / persona switching API 필요
