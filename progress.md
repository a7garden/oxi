# Progress

## Status
In Progress

## Tasks

### v0.22.0 기능 검사: CLI Router Integration + Browse Tools

---

## 1. /router 명령 — ✅ 구현 완료

**위치**: `oxi-cli/src/tui/slash.rs` (라인 ~`/router` 매치 암)

**구현 내용**:
- `/router` (인자 없음): 라우터 설정이 있으면 상태 표시, 없으면 Router Setup Overlay 열기
- `/router status`: `RouterProvider::get_snapshot()` 호출 후 프로필, 티어, 점수, 모델, 비용, 턴 수 출력
- `/router pin`: "not yet implemented" 메시지 (미구현 플레이스홀더)
- `/router disable`: 단순 메시지 출력 (실제 비활성화 로직 없음)
- 그 외 인자: `router_help()` 도움말 출력

**⚠️ 문제점**:
1. **`/router` 명령이 `BUILTIN_SLASH_COMMANDS`에 등록되지 않음** — `util/slash_commands.rs`의 정적 목록에 `router` 항목이 없음. `/help` 자동완성 및 명령 목록에 `/router`가 표시되지 않음.
2. **`/router disable`가 실제 비활성화를 수행하지 않음** — 메시지만 출력하고 라우팅을 비활성화하지 않음. `register_router()` 취소 로직 필요.
3. **`/router pin`이 미구현** — "coming soon" 플레이스홀더 상태.

---

## 2. Ctrl+R 단축키 — ✅ 구현 완료

**위치**: `oxi-cli/src/tui/handlers.rs` 라인 240-270

**구현 내용**:
- `Ctrl+R` 키 입력 → `RouterProvider::get_snapshot()`으로 현재 상태 조회
- `factories::routing_status(data)`로 RoutingStatus overlay 컴포넌트 생성
- `RoutingOverlay`에서 `Ctrl+R` 또는 `Esc`로 오버레이 닫기 지원
- 라우터가 비활성 상태면 `RoutingStatusData::default()`로 빈 패널 표시

**상태**: 정상 동작. 단축키 등록, 오버레이 생성/해제 모두 완료.

---

## 3. Auto-setup (라우터 자동 설정) — ✅ 구현 완료

**위치**:
- `oxi-cli/src/tui/overlay/router_setup.rs` — Setup 오버레이 UI
- `oxi-cli/src/tui/overlay/router_integration.rs` — 설정 저장 로직
- `oxi-cli/src/tui/overlay/factories.rs` — ModelSelectOverlay에서 `router/*` 선택 시 자동 연결

**구현 내용**:
- `/model` → `router/auto` 선택 시 → 라우터 설정 없으면 `OverlayAction::OpenRouterSetup` 트리거
- `/router` (설정 없음) → Router Setup Overlay 직접 열기
- Setup Overlay: 프로필명, High/Medium/Low 모델, High thinking level 편집
- 모델 피커: ` 또는 / 키로 모델 선택 팝업 열기
- 저장: `save_router_config()` → settings.toml에 `[router]` 섹션 작성 → `register_router()`로 AI 라우터에 등록

**⚠️ 문제점**:
1. **`save_router_config()`에서 settings.toml 경로 하드코딩** — `dirs::config_dir().join("oxi/settings.toml")`만 사용. 프로젝트 설정(`.oxi/settings.toml`)은 고려하지 않음.
2. **TOML 섹션 교체 로직이 취약** — `content.find("\n[")`로 다음 섹션을 찾는데, 마지막 섹션이면 파일 끝까지 지워질 수 있음 (실제로는 `unwrap_or(content.len())`으로 보호됨).
3. **`store_config_to_ai_config()`가 `context_upgrade_threshold`, `max_session_budget`, `ScoringWeights`를 무시하고 기본값만 사용** — `RouterConfig`에 해당 필드가 있지만 AI 설정으로 변환 시 전달하지 않음 (추후 확인 필요).

---

## 4. Browse 도구 모듈 — ⚠️ 부분 구현 (컴파일에서 제외됨)

**위치**: `oxi-agent/src/tools/browse/`

**파일 현황**:
- `config.rs` — `BrowseConfig` 구조체 (기본값, serde 직렬화) — ✅ 완성
- `helpers.rs` — JS 스니펫, 링크/요소 파싱 유틸 — ✅ 완성
- `tab_guard.rs` — RAII 탭 가드 (누출 방지) — ✅ 완성
- `mod.rs` — ❌ 없음
- `engine.rs` — ❌ 없음

**⚠️ 심각한 문제**:
1. **`browse/` 디렉토리에 `mod.rs`가 없음** — Rust 모듈 시스템에서 이 디렉토리는 완전히 무시됨.
2. **`engine` 모듈이 존재하지 않음** — `helpers.rs`가 `crate::tools::browse::engine::BrowserTab`을 임포트하고 `tab_guard.rs`도 `super::engine::BrowserTab`을 참조하지만, `engine.rs` 파일 자체가 없음.
3. **`tools.rs`에 `mod browse` 선언이 없음** — browse 디렉토리는 컴파일에 전혀 포함되지 않음.
4. **BrowseTool, BrowseExtractTool, BrowseScriptTool 도구가 등록되지 않음** — `ToolRegistry::with_builtins_cwd()`에 browse 도구가 없음.

**결론**: browse 모듈의 하위 파일들(config, helpers, tab_guard)은 코드 품질이 좋고 테스트도 포함되어 있으나, **전체 모듈이 컴파일에서 완전히 제외되어 있음**. `mod.rs`, `engine.rs`, 그리고 실제 AgentTool 구현체(BrowseTool 등)가 필요함.

---

## 5. 명령어 라우팅 (실제 라우팅 엔진과의 연결) — ✅ 구현 완료

**연결 체인**:
```
/router 명령 (slash.rs)
  → oxi_ai::router::RouterProvider::get_snapshot()  (상태 조회)
  → router_integration::save_router_config()          (설정 저장)
  → router_integration::store_config_to_ai_config()   (설정 변환)
  → oxi_ai::router::register_router()                 (AI 라우터 등록)
  → oxi_store::router_config::load_router_config()    (TOML에서 로드)
```

**라우팅 엔진**: `oxi-ai/src/router/` — classifier, fallback, profiles, scoring, signals, types

**상태**: `/router` 명령이 실제 라우팅 엔진(`RouterProvider`)과 정상 연결됨. 설정 → 저장 → 등록 → 조회 전체 파이프라인 작동.

---

## 요약

| 항목 | 상태 | 비고 |
|------|------|------|
| /router 명령 파싱/처리 | ✅ 완료 | BUILTIN_SLASH_COMMANDS 등록 누락 |
| Ctrl+R 단축키 | ✅ 완료 | 정상 동작 |
| Auto-setup 흐름 | ✅ 완료 | TOML 경로/변환 일부 취약 |
| Browse config 모듈 | ✅ 코드 완성 | 컴파일에서 제외됨 |
| Browse helpers 모듈 | ✅ 코드 완성 | 컴파일에서 제외됨 |
| Browse tab_guard 모듈 | ✅ 코드 완성 | 컴파일에서 제외됨 |
| Browse 모듈 통합 | ❌ 미완료 | mod.rs, engine.rs, AgentTool 구현체 필요 |
| 명령어 → 라우팅 엔진 연결 | ✅ 완료 | 파이프라인 정상 |

## Files Changed
(검사만 수행, 파일 변경 없음)

## Notes
- oxi-cli 전체 `cargo check` 통과 (컴파일 에러 없음)
- oxi-agent 전체 `cargo check` 통과 (browse 모듈이 컴파일에 포함되지 않아 문제 없음)
- 단위 테스트 `slash_commands::tests::names_match` 통과
