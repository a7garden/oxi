# OXI — pi 동등화/능가 로드맵

**버전**: 0.23.0 → 1.0.0  
**전체 완성도**: ~75% (코드베이스 기반 정확 측정)  
**목표**: 6개 RFC를 통해 각 영역 95%+ 동등성 달성, 일부 영역 능가  

---

## RFC 인덱스

| RFC | 영역 | 우선순위 | 현재 | 목표 | 예상 기간 |
|-----|------|---------|------|------|-----------|
| [RFC-001](RFC-001-TUI-PARITY.md) | TUI (차동 렌더링, 키바인딩, 자동완성, 오버레이, 이미지) | **P0** | 28% | 95% | 9주 |
| [RFC-002](RFC-002-AI-PROVIDER-COVERAGE.md) | AI Provider (모델·프로바이더·이미지 생성·Claude Code) | **P1** | 85% | 95% | 7주 |
| [RFC-003](RFC-003-AGENT-TOOL-SUPERIORITY.md) | Agent/Tool (우위 확보, MCP 고도화, 스트리밍) | **P1** | 115% | 150% | 5.5주 |
| [RFC-004](RFC-004-EXTENSION-SKILLS.md) | Extension & Skills (UI 컨텍스트, Skills 강화, 패키지) | **P2** | 80% | 100% | 4주 |
| [RFC-005](RFC-005-CI-CD-INFRA.md) | CI/CD (크로스 컴파일, 자동 업데이트, 배포) | **P2** | 50% | 95% | 3주 |
| [RFC-006](RFC-006-SDK-MULTI-AGENT.md) | SDK (에이전트 정의, 워크플로우 DSL) | **P2** | 92% | 100% | 1.5주 |
| [RFC-007](RFC-007-BROWSE-PROGRESS-ENRICHMENT.md) | Browse 진행 정보 (구조화된 컨텍스트, 스크린샷) | **P1** | 90% | 95% | 1주 |
| [RFC-008](RFC-008-GRACEFUL-LOOP-TERMINATION.md) | 에이전트 루프 정상 종료 보장 (max_iterations 후 텍스트 응답) | **P1** | 60% | 95% | 1.5주 |

---

## 우선순위 의존성 그래프

```
RFC-001 (TUI) ──────────────┐
    │                        │
    ├── keybindings ─────────┤──→ handlers.rs 리팩토링
    ├── diff backend ────────┤──→ ratatui Backend 트레이트 구현
    ├── overlay anchors ─────┤──→ 기존 7개 오버레이 컴포넌트 확장
    ├── completion ──────────┤──→ 기존 slash.rs 위에 파일경로/퍼지 추가
    └── terminal image ──────┘──→ Kitty/iTerm2 프로토콜
                             │
RFC-002 (AI Provider) ───────┤
    │                        │
    ├── 이미지 생성 API ─────┤──→ RFC-001 (터미널 이미지 표시)
    ├── Claude Code 스텔스 ──┤
    ├── 모델 DB 확대 ────────┤──→ 585 → 850+
    └── WebSocket 스트리밍 ──┘

RFC-003 (Agent/Tool) ──────── 기존 20개 툴 + MCP 확장
    │
    ├── AgentTool render ────┤──→ tool_renderer.rs + render_utils.rs 확장
    ├── MCP 고급 ────────────┤──→ 기존 resources 위에 prompts/sampling/logging
    ├── 툴 스트리밍 ─────────┤──→ on_progress(String) → ToolProgress enum
    └── 에디트 강화 ─────────┘──→ file_mutation_queue + expected_hash

RFC-004 (Ext/Skills) ──────── 기존 33개 훅 + WASM 샌드박스 위에 확장
    │
    ├── Extension UI ────────┤──→ TUI/RPC 듀얼 다이얼로그
    ├── Skills YAML ─────────┤──→ 기존 SkillManager에 frontmatter 추가
    └── 패키지 병렬화 ───────┘──→ 기존 PackageManager 병렬 업데이트

RFC-005 (CI/CD) ───────────── 기존 8잡 CI + 5타겟 릴리즈 위에 확장
    │
    ├── musl/aarch64-win ────┤──→ release.yml 타겟 추가
    ├── 자동 업데이트 ───────┤──→ self_update 크레이트
    └── 배포 채널 ───────────┘──→ Homebrew + crates.io

RFC-006 (SDK) ────────────── 기존 34모듈/6서브시스템 위에 확장
    │
    ├── 에이전트 정의 ───────┤──→ YAML frontmatter 파싱
    └── 워크플로우 DSL ─────┘──→ 기존 coordination API 매핑
```

---

## 권장 실행 순서

### Sprint 1-3 (P0 + P1 병렬)

```
Week 1-2:  RFC-001 Phase 1 (키바인딩: oxi-tui에 keybindings/ 모듈)
           RFC-002 Phase 1 (이미지 생성 API)

Week 3-4:  RFC-001 Phase 2 (차동 렌더링: DiffBackend)
           RFC-002 Phase 2 (Claude Code 스텔스 모드)
           RFC-003 Phase 1 (AgentTool render_call/render_result)

Week 5-6:  RFC-001 Phase 3 (오버레이 앵커: 기존 7개 컴포넌트 확장)
           RFC-002 Phase 3 (모델 DB 확대 585→850+)
           RFC-003 Phase 2 (MCP: prompts, sampling, logging)
```

### Sprint 4-6 (P1 완료 + P2 시작)

```
Week 7-8:  RFC-001 Phase 4 (자동완성: 파일경로 + 퍼지)
           RFC-003 Phase 3-4 (Subagent 조건부, 툴 스트리밍)
           RFC-005 Phase 1 (릴리즈 타겟 확장)

Week 9:    RFC-001 Phase 5 (터미널 이미지)
           RFC-001 Phase 6 (에디터 평가)
           RFC-005 Phase 2 (자동 업데이트)

Week 10-12: RFC-003 Phase 5 (에디트 강화)
            RFC-004 Phase 1-2 (Extension UI, Stale 감지)
            RFC-005 Phase 3 (PR 게이트, 배포 채널)
            RFC-006 전체 (에이전트 정의 + 백프레셔)
```

---

## 핵심 메트릭 목표

| 메트릭 | 현재 | 목표 (v1.0) |
|--------|------|-------------|
| 모델 수 | 546 | 850+ |
| 프로바이더 수 | 47 (빌트인) | 50+ |
| API 프로토콜 | 8 | 8 (충분) |
| 내장 툴 | 20 | 20 (render 추가) |
| MCP 기능 | tools + resources | tools + resources + prompts + sampling + logging |
| 에디터 기능 | ratatui-textarea (undo/redo/word/CJK) | 평가 후 결정 |
| 키바인딩 | 하드코딩 (handlers.rs) | 31개 + 동적 리바인딩 + 충돌 감지 |
| 크로스 컴파일 | 5 타겟 (release.yml) | 8 타겟 (musl + aarch64-win 추가) |
| Extension 훅 | 34개 메서드 + 14개 이벤트 | 35개 (단축키 등록 추가) |
| SDK 모듈 | 34개 (6 서브시스템) | 36개 (에이전트 정의 + 워크플로우 DSL) |
| 코드 라인 | 125.6K | ~140K (TUI 확장) |
| 테스트 | 16,411 `#[test]` | 20K+ |
| CI 잡 | 13개 (8 CI + 5 릴리즈) | 15+ (PR 게이트 + 배포) |

---

## oxi가 pi를 능가하는 영역 (v1.0 이후)

1. **성능**: 네이티브 바이너리 vs Node.js 런타임
2. **Compaction**: LLM 기반 컨텍스트 압축 (pi에 없음)
3. **Circuit Breaker + Fallback Chain**: 프로덕션급 안정성 (pi에 없음)
4. **샌드박스**: WASM 확장 + 권한 시스템 (pi는 Node.js 전체 권한)
5. **SDK**: 독립적인 멀티 에이전트 라이브러리 9,597LOC / 34모듈 / 6서브시스템 (pi는 모놀리식)
6. **내장 툴**: 20개 툴 (pi는 7개) — browse, github, web_search, context7, subagent 등
7. **관측성**: Tracer + AuditLog + CostTracker + EventStore (SDK에 내장)
8. **보안**: Authorizer (RBAC) + Capability + SecurityMiddleware (SDK에 내장)
9. **조정**: WorkQueue + SharedMemory + Consensus + CoordinatedGroup (SDK에 내장)
10. **CI 품질**: 8개 CI 잡 + audit + deny (pi와 동등)
11. **Atomic I/O**: temp+rename 전면 도입
12. **TOML 설정**: JSON보다 인간 친화적 설정 포맷

---

## 위험 요소

| 위험 | 가능성 | 영향 | 완화 |
|------|--------|------|------|
| TUI 복잡도 과증가 | 중 | 일정 지연 | 단계적 구현, ratatui 생태계 활용 |
| Kitty 프로토콜 호환성 | 낮음 | 이미지 미작동 | 능력 감지 + graceful fallback |
| 크로스 컴파일 의존성 충돌 | 중 | 특정 타겟 빌드 실패 | Cross.toml + CI 매트릭스 점진 확대 |
| API 변경으로 인한 모델 DB 갱신 | 높음 | 모델 메타데이터 오류 | 자동 생성 스크립트 + 수동 검증 |
| WASM 확장 ABI 불안정 | 낮음 | 확장 호환성 깨짐 | Extism 버전 고정 + 어댑터 레이어 |
