# P0.5 — 완료

> **갱신**: 2026-07-28 — **모든 Phase 완료**
> **Git 커밋**: `cb8fb96` (Phase 4 — GitLab Duo Workflow full impl)
> **테스트**: 664/664 (oxicode-ai), clippy clean, consumer check clean

> **현재 상태 (2026-07-30):** P0.5 remote-AGENT transport는 아래 기록대로
> 완료되었고, P2 production tape cutover도 이후 완료되었습니다. P2
> rich-content는 부분 완료입니다. Codex Responses와 Gemini CLI도 explicit
> dispatch arm이 있어 backlog가 아닙니다 — Codex는 `OpenAiResponsesProvider`를
> 재사용하고, Gemini CLI는 의도된 `NotImplemented` stub입니다.

---

## Current State (ALL COMPLETE)

| Provider | File | Lines | Status | Protocol |
|----------|------|-------|--------|----------|
| **Cursor** | `oxicode-ai/src/providers/cursor.rs` | **1,117** | **✅ Full impl** | HTTP/2 + Connect + Protobuf (492 types via prost-build) |
| **Devin** | `oxicode-ai/src/providers/devin.rs` | **916** | **✅ Full impl** | HTTP/1.1 Connect + Protobuf |
| **GitLab Duo** | `oxicode-ai/src/providers/gitlab_duo.rs` | **480** | **✅ Working REST proxy** | REST + Auth Delegation |
| **GitLab Duo Agent** | `oxicode-ai/src/providers/gitlab_duo_agent.rs` | **1,238** | **✅ Full impl** | WebSocket + JSON protocol |

### Api Variant Mapping

| Api enum | Serde name | Dispatch | Status |
|----------|-----------|----------|--------|
| `Api::CursorAgent` | `cursor-agent` | `CursorProvider::new()` | **✅ Working** |
| `Api::DevinAgent` | `devin-agent` | `DevinProvider::new()` | **✅ Working** |
| `Api::GitLabDuo` | `gitlab-duo` | `GitLabDuoProvider::new()` | **✅ Working** |
| `Api::GitLabDuoAgent` | `gitlab-duo-agent` | `GitLabDuoAgentProvider::new()` | **✅ Working** |

### Test Suite
- oxicode-ai: **664/664** passing
- Workspace total: **3,653/3,653** passing
- clippy: clean (`-D warnings`)
- consumer check (`cargo check -p oxicode-cli`): clean

---

## Phase 4 — GitLab Duo Workflow (gitlab-duo-agent) ✅

**Source**: omp `packages/ai/src/providers/gitlab-duo-workflow.ts` (3136 lines)

### Architecture
- **Stateless-per-turn**: 각 `Provider::stream()` 호출이 fresh workflow 생성 → WebSocket 연결 → 이벤트 방출 → 종료
- **JSON-only wire format**: WebSocket 프로토콜 순수 JSON, protobuf/grpc 불필요
- **REST setup**: direct_access 토큰 캐싱 + workflow 생성 + 모델 조회
- **WebSocket**: `tokio-tungstenite` + `rustls-tls-webpki-roots` auth 헤더
- **Goal transcript**: ChatML `<|im_start|>role\nbody<|im_end|>` 형식
- **Checkpoint 핸들링**: 텍스트/thinking 델타 증분 방출, tool call action 추출

### Features
- Cached direct-access token (Provider 수명 동안 재사용, 401시 갱신)
- MCP tool schema 변환 (omp `duo_mcp_tools` → AgentTool 포맷)
- 모델 ref 선택 (catalog model ID → workflow model ref 매핑)
- namespace/env var 기반 설정 (`GITLAB_NAMESPACE_ID`, `GITLAB_ROOT_NAMESPACE_ID`)
- Completion/stop/error 상태에 따른 정리 (workflow 중단)

### Dependencies Added
- `tokio-tungstenite = "0.26"` (features: `connect`, `rustls-tls-webpki-roots`)

---

## 동시 완료된 기타 작업

P0.5 진행 중 발견되어 함께 처리한 작업들 (별도 문서 참조):

| 작업 | 설명 | 상태 |
|------|------|------|
| **P3.1** | `.md` 기반 시스템 프롬프트 | ✅ 이미 `include_str!()` 사용 중 |
| **P3.2** | CLI 명령 포팅 (completions/install/update/commit) | ✅ `misc.rs`에 모두 구현 |
| **P3.3** | `main.rs` 핸들러 분리 (F-5) | ✅ 62줄, `cli/commands/`에 위임 |
| **P4.1** | Issue 시스템 격리 | ✅ `store/issues/` |
| **P4.2** | Package manager 모듈화 | ✅ `storage/packages/` |
| **P1.6a** | Debug 도구 재등록 | ✅ `debug_tool.rs` 등록 완료 |
| **P4.3** | Language policy 제거 | ✅ `output_languages/KNOWN_CHANNELS/language_directive` 제거 |
| **GitLab Duo namespace** | "1" 하드코딩 → env var 설정 | ✅ `GITLAB_NAMESPACE_ID` / `GITLAB_ROOT_NAMESPACE_ID` |

---

## 이후 상태

- **P2 production tape cutover** — 완료
- **P2 rich-content** — 부분 완료
