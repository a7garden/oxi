# oxi로 oxi 코드 리뷰 — 실행 보고서

**날짜:** 2026-05-16  
**도구:** oxi v0.12.0 (self-hosted, zai/glm-5.1 모델)  
**대상:** oxi 프로젝트 자기 자신

---

## 1. 설치 과정

```bash
# 릴리즈 빌드
cd /Volumes/MERCURY/PROJECTS/oxi
cargo build --release     # 24.58초 소요

# 설치
cp target/release/oxi ~/bin/oxi
export PATH="$HOME/bin:$PATH"

# 확인
oxi --version  # oxi 0.12.0
```

**문제:** `/usr/local/bin` 권한 거부 → `~/bin`에 설치  
**모델:** zai/glm-5.1 (이미 설정됨)

---

## 2. 실행 방식

```bash
# --print: 단일 샷 비대화형 모드
# stdin에 "go"를 파이프로 전달 (TUI가 stdin을 잡는 문제 회피)
# stderr를 분리하여 stdout만 캡처
echo "go" | oxi --print -p zai -m glm-5.1 "질문" 2>/dev/null
```

### 발견한 실행 이슈

| 이슈 | 설명 | 해결 |
|------|------|------|
| TUI가 stdin을 독점 | 프롬프트만 전달해도 TUI 모드 진입 | `echo "go" \| oxi --print` |
| 긴 응답 시간 초과 | 파일 읽기 도구 사용 시 5분+ 소요 | 코드를 프롬프트에 직접 포함 |
| stderr에 진행 상태 | 툴 호출 로그가 출력에 섞임 | `2>/dev/null`로 분리 |
| `--no-session` 없음 | 세션이 자동 생성됨 | 불필요한 세션 정리 필요 |

---

## 3. 코드 리뷰 결과

### 리뷰 1: PathGuard 보안 (`oxi-agent/src/tools/path_security.rs`)

**oxi가 발견한 문제점:**

| # | 문제 | 심각도 | 요약 |
|---|------|--------|------|
| 1 | TOCTOU 경쟁 조건 | HIGH | `exists()` 후 `canonicalize()` 사이에 심볼릭 링크 교체 가능 |
| 2 | 심볼릭 링크 샌드박스 탈출 | HIGH | `..` 문자열 검사만으로 심볼릭 링크 공격 방어 불가 |
| 3 | 존재하지 않는 경로 검증 누락 | HIGH | 새 파일 생성 시 `exists()`가 false면 검증 스킵 |

**oxi의 제안:**
> "문자열 비교(`..`)가 아니라 `canonicalize()` 후 `starts_with()` 비교로 수행해야 합니다."

### 리뷰 2: ProviderError (`oxi-ai/src/error.rs`)

| # | 문제 | 심각도 | 요약 |
|---|------|--------|------|
| 1 | `HttpError(u16, String)` 타입 안전성 | HIGH | raw u16이 무효값을 통과시킴 → `StatusCode` 사용 제안 |
| 2 | `MissingApiKey`에 컨텍스트 없음 | MEDIUM | 어느 프로바이더의 키가 없는지 알 수 없음 |
| 3 | `InvalidResponse` 디버깅 정보 손실 | MEDIUM | 원본 응답 body가 String으로 평탄화 |

### 리뷰 3: CompactionConfig (`oxi-cli/src/context/auto_compaction.rs`)

| # | 문제 | 심각도 | 요약 |
|---|------|--------|------|
| 1 | `#[allow(dead_code)]` 전역 속성 | HIGH | 미사용 variant가 조용히 방치됨 |
| 2 | 명목형 타입 누락 | MEDIUM | `u32` 대신 newtype 래핑 제안 |

### 리뷰 4: MCP 타임아웃 (`oxi-agent/src/mcp/client.rs`)

| # | 문제 | 심각도 | 요약 |
|---|------|--------|------|
| 1 | 불완전 메시지 Partial Read | HIGH | 타임아웃 후 스트림 상태 오염 |
| 2 | 장시간 도구 호출 단절 | HIGH | 정상 응답을 오측으로 끊음 |
| 3 | 초기화 핸드셰이크 경합 | MEDIUM | 초기화 전 타임아웃 → 무한 재시작 루프 |

**oxi의 제안:**
> "전역 30초 대신 메서드별 차등 타임아웃: initialize=10s, tools/call=120s, ping=5s"

### 리뷰 5: Secret\<T\> Serialize (`oxi-ai/src/secret.rs`)

| # | 문제 | 심각도 | 요약 |
|---|------|--------|------|
| 1 | 직렬화 평문 → 로그/에러 응답 노출 | HIGH | Debug는 마스킹되지만 JSON 직렬화는 평문 |
| 2 | API 응답 본문에 평문 노출 | HIGH | 웹 프레임워크 자동 직렬화로 외부 노출 |

---

## 4. 종합 평가

### oxi가 잘한 점 ✅

1. **구체적인 코드 제안** — 단순히 "수정해"가 아니라 수정 전/후 코드를 비교 제시
2. **심각도 평가** — Critical/High/Medium/Low를 일관되게 적용
3. **실용적 관점** — 실제 공격 시나리오와 디버깅 상황을 예시로 들음
4. **Rust 관용구 제안** — newtype 패턴, StatusCode 타입 등 Rust다운 해결책 제시

### oxi의 한계점 ⚠️

1. **긴 파일 분석 불가** — 30줄 이상 코드는 시간 초과 (모델 응답 생성 시간)
2. **도구 호출 비효율** — 파일 읽기 도구 사용 시 응답까지 5분+ 소요
3. **컨텍스트 없이 리뷰** — 전체 프로젝트 맥락을 모른 채 코드 스니펫만 리뷰
4. **반복 발견** — 이미 수정된 문제도 "개선점"으로 제시 가능

### 모델 성능 (zai/glm-5.1)

| 지표 | 평가 |
|------|------|
| 응답 속도 | 짧은 질문: 5-15초, 코드 포함: 60-300초 |
| 코드 이해도 | 높음 — Rust 패턴 정확히 이해 |
| 제안 품질 | 높음 — 실제 적용 가능한 구체적 코드 |
| 보안 감지 | 우수 — TOCTOU, 심볼릭 링크, 직렬화 노출 등 포착 |
| 한국어 처리 | 자연스러움 — 기술 용어도 적절히 혼용 |

---

## 5. 개선된 점수카드

| 영역 | 분석 전 점수 | 수정 후 점수 | oxi 리뷰가 추가로 발견한 것 |
|------|------------|------------|--------------------------|
| **보안** | B | B+ | TOCTOU, Partial Read, Serialize 노출 |
| **아키텍처** | B+ | A- | 에러 타입 계층 구조화, 차등 타임아웃 |
| **코드 품질** | B | A | dead_code 전역 속성, newtype 패턴 |
| **성능** | B | B+ | (이번 리뷰에서 새로 발견된 건 없음) |

---

*이 보고서는 oxi v0.12.0 (zai/glm-5.1)을 사용하여 5회 코드 리뷰를 수행한 결과입니다.*