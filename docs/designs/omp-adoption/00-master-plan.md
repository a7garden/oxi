# 마스터 설계: omp(oh-my-pi) 기능 도입 — 4개 핵심 기능

> 상태: 마스터 v2 (범위 확정 — ①②③④ 도입, ⑤⑥ 후순위 보류)
> 작성: 2026-06-19 (v1), 2026-06-19 재구성 (v2)
> 분석 대상: omp v16.1.1 (`/tmp/oh-my-pi`) · oxicode 현재 아키텍처
> 후속: 하위 설계 문서 4종 + CHANGELOG.md + AGENTS.md 갱신

이 문서는 **"무엇을 왜 도입하는가"**와 **"어떤 순서로, 어떤 의존성으로 도입하는가"**를 정의한다. **"어떻게 구현하는가"**는 각 하위 설계 문서가 담당한다.

---

## 0. 핵심 결정 (TL;DR)

omp의 기능 중 **oxicode 아키텍처(순수 Rust 단일 바이너리 + 포트 계약 + noop 폴백)와 정합하며 가치가 검증된 4개**를 도입한다. omp의 "완제품 CLI" 정체성(LSP/DAP/코드 실행 커널/ACP 등)은 oxicode 포지셔닝과 충돌하므로 영구 제외하고, AST 편집·Checkpoint는 가치는 있으나 **현재 우선순위/리소스에서 후순위**로 보류한다.

### 도입 4종 (가치 등급순)

| # | 기능 | 등급 | 상세 문서 | 리팩토링 |
|:-:|---|:-:|---|:-:|
| ① | **Hashline line-anchored edit** | 🟢 핵심 | [`01-hashline-edit.md`](./01-hashline-edit.md) | **대규모** |
| ④ | Hindsight 메모리 (MemoryStore impl) | 🟡 | [`04-hindsight-memory.md`](./04-hindsight-memory.md) | 소 |
| ② | Internal URL Router 포트 | 🟡 | [`02-internal-url-router.md`](./02-internal-url-router.md) | 중 |
| ③ | TTSR (스트림 룰 인터럽트) | 🟠 신중 | [`03-ttsr-rules.md`](./03-ttsr-rules.md) | 중 |

### 보류 (후순위, 본 로드맵 범위 외)

| # | 기능 | 보류 사유 |
|:-:|---|---|
| ⑤ | AST 편집 propose/resolve | tree-sitter 의존(무거움) + 대규모 codemod가 oxicode 주 사용 패턴인지 미검증. 4종 안정화 후 별도 검토 |
| ⑥ | Checkpoint/Rewind (CoW) | 물리적 CoW는 OS 깊은 결합 → oxios 제품으로 이관. oxicode-cli는 세션 트리 기반 논리적 rewind만 향후 추가 |

### 영구 제외 (oxicode 포지셔닝 충돌)

LSP 통합 · DAP 디버거 · 코드 실행 커널(Python+Bun) · ACP(Zed) · brush 셸 · omp commit · Job · IRC · 네이티브 브라우저 스텔스. → 이유: omp의 "완제품 CLI" 정체성에 속하며 oxicode의 "가벼운 임베더블 엔진" 원칙(AGENTS.md 설계 원칙 4·7·9)과 정면 충돌.

---

## 1. 배경: 왜 oxicode가 omp에서 배워야 하는가

### 1.1 두 pi 계보의 분기점

```
pi (badlogic/pi-mono)
├── omp (can1357/oh-my-pi) — "완제품": TS+Bun 하이브리드, 모든 기능 내장, v16.1.1
└── oxicode (project-oxi/oxicode)     — "엔진": 순수 Rust, 포트 계약, 임베더블, 초기 단계
```

oxicode는 pi의 **Rust 포트**로 출발했으나, omp는 pi의 **포크 + 대폭 확장**으로 이미 성숙한 제품이다. omp가 검증한 엔지니어링(경계 복구, 드리프트 복구, 스냅샷 스토어, 룰 인터럽트 등)은 oxicode가 독자적으로 재발견하기엔 비용이 큰 검증 지식이다.

### 1.2 oxicode가 이미 갖춘 기반 (도입을 싸게 만드는 자산)

| omp 기능 | oxicode의 기존 자산 | 도입 비용 절감 |
|---|---|---|
| Hashline | `edit_diff.rs`의 BOM/CRLF 정규화, `file_mutation_queue.rs`의 per-file 직렬화, `edit.rs`의 다중 edit + dry_run | 정규화/동시성/출력 인프라 재사용 |
| TTSR | `agent_loop/streaming.rs`의 토큰 처리 루프 (delta 이벤트 처리 인프라) | 스트리밍 루프에 TTSR 체크 인라인 주입 가능. 단, `cancel_signal`/`retry.rs`는 **재사용하지 않음** — 별도 `StreamOutcome` 제어 흐름 추가 (`03` §2.3) |
| Hindsight 메모리 | `MemoryStore` 포트(`ports/mod.rs:683`) + `NoopMemoryStore` | **포트만 충전** — 설계 변경 최소 |
| Internal URL | `ToolContext`(workspace/root/session_id) + `read`/`search` 도구 | 라우터 1개 주입 |

### 1.3 oxicode가 **부족한** 것 (Hashline이 해결하는 핵심)

현재 `edit.rs`는 str_replace 단일 방식이며 5가지 한계가 있다:

1. **불안정한 해시** — `DefaultHasher`(플랫폼/Rust 버전 의존). omp는 xxHash32(안정적 크로스플랫폼).
2. **드리프트 시 거부만** — omp는 3-way merge + session chain replay로 자동 복구.
3. **str_replace 고질병** — 부분 매칭 실패 → 재시도 루프 → 토큰 낭비. omp 데이터: Grok 4 Fast −61% 토큰, MiniMax pass 2.1×.
4. **"안 본 줄" 편집 허용** — omp는 `seenLines`로 차단.
5. **모델 실수 교정 없음** — omp는 `repairReplacementBoundaries`로 5패턴 자동 교정.

→ **Hashline이 최우선인 이유**: 위 5개를 모두 해결하며 기존 edit 인프라가 60% 갖춰져 있어 **가장 높은 ROI**.

---

## 2. 도입 원칙 (모든 하위 설계가 지켜야 할 불변량)

### 원칙 1 — 포트 또는 도구 형태, noop 폴백 필수

모든 기능은 다음 둘 중 하나:
- **포트**(oxicode-sdk): 트레잇 + `Noop*` 폴백. 미등록 시 `Err(PortNotConfigured)` 또는 빈 결과. 예: `MemoryStore`, `InternalUrlRouter`, `RuleRegistry`.
- **도구**(oxicode-agent): `AgentTool` 구현체. 비활성화 가능(`disabled_tools`).

> **불변량**: 기능 미등록/비활성화 시 기존 동작 100% 보존. regression 테스트로 보장.

### 원칙 2 — omp 소스를 있는 그대로 번역하지 않는다

omp는 TS+Bun, oxicode는 Rust. 번역 시 oxicode 관례를 따른다:
- `Promise.withResolvers()` → `tokio::sync::oneshot`.
- `lru-cache` → `lru` 크레이트.
- `Bun.hash.xxHash32` → `xxhash-rust` 크레이트.
- `Diff.applyPatch` (jsdiff) → `similar` 크레이트.
- `bun:sqlite` → `rusqlite` + `tokio::sync::Mutex`.
- 에러: omp는 `throw`, oxicode 라이브러리는 `thiserror` enum, 앱은 `anyhow`.

### 원칙 3 — 점진적 강화, 마이그레이션 안전

omp 기능을 도입할 때 **기존 경로를 제거하지 않는다**:
- Hashline 도입 후에도 `oldText/newText` str_replace 경로 유지 (설정으로 전환).
- Internal URL 도입 후에도 일반 파일 경로 100% 동작.
- TTSR 미등록 시 `cancel_signal`은 기존처럼 Ctrl+C 전용.

### 원칙 4 — 단일 소스 진실 (Single Source of Truth)

omp에서 가져오는 모든 상수/문법/메시지는 oxicode에 **하나의 모듈**에 정의 (prompt.md와 코드가 같은 상수 참조 → drift 방지).

### 원칙 5 — 테스트는 omp의 계약을 복사

omp의 핵심 알고리즘(boundary repair, recovery, snapshot fusion)은 속성 테스트와 회귀 케이스로 보호된다. oxicode 포팅 시 omp 테스트 케이스를 Rust 테스트로 **동일 입력/동일 출력**으로 이식 — omp 동작이 곧 oxicode 명세.

---

## 3. 의존성 그래프 & 도입 순서

```
                  ┌─────────────────────────────────────────┐
   M1 (핵심)      │  ① Hashline edit (oxicode-hashline 크레이트) │ ← 모든 것의 기반
                  │     - SnapshotStore (프로세스 내)         │
                  │     - read 도구 tag 발행 연동             │
                  └────────────────┬────────────────────────┘
                                   │ read.rs tag 발행 + edit.rs 모드 추가
              ┌────────────────────┴────────────────────┐
              ▼                                          ▼
   ┌──────────────────────┐                ┌──────────────────────┐
   │ ④ Hindsight memory   │ ← M1과 독립     │ ② Internal URL Router│ ← ① read.rs 후
   │  - MemoryStore SQLite│   (병렬 가능)   │  - read/search dispatch│
   │  - 4개 메모리 도구   │                │  - issue:// pr:// ...  │
   └──────────────────────┘                └──────────────────────┘
              │                                          │
              │  memory:// 핸들러 (②에)                   │ rule:// 핸들러 (②에)
              ▼                                          ▼
                  ┌─────────────────────────────────────────┐
   M3 (신중)      │  ③ TTSR rules                            │ ← StreamOutcome 신규 제어 흐름
                  │     - RuleRegistry 포트                  │   (agent_loop, ①과 독립)
                  │     - 정규식 매칭 (astCondition은 후순위) │
                  └─────────────────────────────────────────┘
```

### 순서와 병렬성

| 단계 | 작업 | 선행 | 병렬? |
|:-:|---|---|:-:|
| **M1** | ① Hashline (라인 op, tree-sitter 제외) | — | 기반 작업 |
| **M2** | ④ Hindsight 메모리 | M1과 독립 | **M1과 병렬** |
| **M2** | ② Internal URL Router | ①의 read.rs 변경 후 (동일 파일 충돌 회피) | M2-④와 병렬 |
| **M3** | ③ TTSR (정규식 매칭) | ①/④ 안정화 권장 (리스크 완화) | agent_loop만 건드려 독립 |

> **병렬 설계/구현 가능**: ④와 ③은 ①과 파일 충돌 없이 병렬 진행 가능. ②만 ①의 `read.rs` 변경(M1.12) 후가 안전.

### 가치 등급 vs 구현 순서 (분리)

- **가치 등급**(사용자 우선순위): ①(🟢) > ④·②(🟡) > ③(🟠)
- **구현 순서**(의존성/안정성): M1 ① → M2 ②·④(병렬) → M3 ③

③ TTSR은 가치는 높지만(🟠) false positive·과금·무한루프 위험이 있어 **①·④ 안정화 후** 진행을 권장한다(본인 판단).

---

## 4. 크레이트/포트/도구 구조 변화

### 4.1 신규 크레이트: `oxicode-hashline`

omp의 `@oh-my-pi/hashline` 독립 패키지와 대응. oxicode-agent 하위가 아닌 **독립 라이브러리 크레이트**로 분리하는 이유:
1. 코어(parser/patcher/snapshots)가 FS·에이전트 런타임에 의존하지 않게 함(순수 함수).
2. 임베더가 자체 edit 포맷을 원할 때 재사용 가능.
3. tree-sitter 의존을 `block-ops` feature 뒤에 격리 (**후순위 확장** — 본 로드맵은 `default`만).

```
oxicode-hashline/  (M1에서 라인 op만; block-ops SWAP.BLK 등은 후순위)
├── format.rs snapshots.rs parser.rs tokenizer.rs apply.rs recovery.rs
├── patcher.rs mismatch.rs messages.rs normalize.rs diff_preview.rs stream.rs
└── prompt.md   (모델용 문법 명세)
```

### 4.2 기존 크레이트에 미치는 영향

| 크레이트 | 변경 | 비고 |
|---|---|---|
| **oxicode-hashline** (신규) | 전체 | omp `packages/hashline/` 포팅 (라인 op) |
| oxicode-agent | `tools/edit.rs` hashline 모드, `tools/read.rs` tag 발행, `agent_loop/ttsr.rs` 신규, 메모리 도구 4종 | 기존 str_replace 보존 |
| oxicode-sdk | 포트 2개 추가: `InternalUrlRouter`, `RuleRegistry`. 기존 `MemoryStore`는 impl만 충전 | additive (기본 noop) |
| oxicode-cli | `store/memory_sqlite.rs`, `discovery/rules.rs`, settings 필드, internal URL 핸들러 | 제품 레이어 |

### 4.3 포트/도구 카운트 변화

```
oxicode-sdk ports:   11 → 13–14  (+InternalUrlRouter, +RuleRegistry; MemoryStore는 기존.
                                EmbeddingProvider(④용) 추가 시 14 — `04` §2.3에서 결정)
oxicode-agent tools: 15 → 19     (+memory_retain/recall/reflect/edit; hashline은 edit 내부 모드.
                                현재 with_builtins_cwd 등록 15개 + dynamic MCP/browse 도구 별도)
```

---

## 5. 리팩토링 허용 범위 (사용자 승인 전제)

사용자가 "리팩토링도 허용할 각오"를 밝혔다. 본 로드맵이 수반하는 리팩토링:

### 5.1 Hashline 도입에 필수 (M1)

- **`edit_diff.rs` → `oxicode-hashline/src/normalize.rs`**: BOM/CRLF 정규화 함수를 oxicode-hashline으로 이전. `edit.rs`는 oxicode-hashline을 통해 호출.
- **`edit.rs` 재구성**: `apply_edits`를 두 디스패치(str_replace / hashline)로 분리. 입력 `mode` 필드 추가.
- **`read.rs` 출력 형식**: snapshot tag 발행 추가 (`[path#TAG]` 헤더 + seenLines 기록). 모든 시스템 프롬프트 갱신.
- **해시 함수 교체**: `DefaultHasher` → `xxhash-rust`. `expected_hash` 호환성 깨짐 → 마이그레이션 (기존 hash 무시, 새 tag 재발행).

### 5.2 Internal URL Router (M2)

- **`path_security.rs` / `path_utils.rs`**: 경로 정규화 지점에 URL dispatch 추가. `<scheme>://` 감지 시 라우터로 위임.

### 5.3 TTSR (M3)

- **`agent_loop/streaming.rs`**: 토큰 delta마다 `ttsr.check_delta` 인라인 체크 추가. `StreamOutcome` 반환 타입 확장.
- **`agent_loop/mod.rs`**: `StreamOutcome::RuleInterrupt` 처리 — 룰 주입 + `continue` 재시도 (새 제어 흜름). **`cancel_signal` 건드리지 않음** (`03` §2.3 참조).
- **`compaction.rs`**: 인터럽트 이력 보존 훅.

### 5.4 하위 호환성 보장 전략

각 리팩토링은 **기능 플래그** 또는 **설정 토글** 뒤에 둔다:
- `Settings::edit_format: "str_replace" | "hashline"` (기본 `str_replace`).
- `Settings::ttsr_enabled: bool` (기본 false).
- `Settings::memory_enabled: bool` (기본 false).
- URL 라우터 미등록 시 자동 폴백 (설정 불필요).

> **CI 게이트**: 각 단계마다 `cargo nextest run --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` 통과 필수.

---

## 6. 각 하위 설계 문서 인덱스

| 문서 | 등급 | 핵심 질문 | omp 소스 맵 |
|---|:-:|---|---|
| [`01-hashline-edit.md`](./01-hashline-edit.md) | 🟢 | omp의 line-anchored edit을 어떻게 oxicode에 이식하고, str_replace와 공존시킬 것인가? | `packages/hashline/src/*` (4.7K LOC) |
| [`04-hindsight-memory.md`](./04-hindsight-memory.md) | 🟡 | 이미 있는 `MemoryStore` 포트를 mnemopi 스키마로 어떻게 충전할 것인가? | `packages/mnemopi/*` |
| [`02-internal-url-router.md`](./02-internal-url-router.md) | 🟡 | 프로토콜 스킴을 read/search에 투명하게 주입할 포트는? | `internal-urls/*` |
| [`03-ttsr-rules.md`](./03-ttsr-rules.md) | 🟠 | 스트리밍 중단 + 룰 주입 + 재시도를 위한 새 제어 흐름(`StreamOutcome`)을 어떻게 설계할 것인가? | `export/ttsr.ts`, `capability/rule.ts` |
---


## 7. 위험 & 의사결정 보드 (cross-cutting)

| 항목 | 상태 | 결정권자 | 비고 |
|---|:-:|---|---|
| Hashline 기본 포맷 (str_replace 유지 vs hashline 전환) | 🟡 미결정 | 리드 | `01` §5 — 기본 str_replace, 점진 전환 제안 |
| xxhash/lru/rusqlite 의존 추가 (M1) | 🟢 승인됨 | — | 모두 MIT/Apache, 가벼움. `similar`는 M1.5(feature 게이트)로 이동 (`01` §3.1) |
| 임베딩 제공자 (메모리용) | 🟡 미결정 | 리드 | `04` — `EmbeddingProvider` 포트 + 채팅 제공자 embeddings API 제안 |
| TTSR `interruptMode` 기본 | 🟡 미결정 | 리드 | `prose-only` 제안 (false positive 완화) |
| TTSR false positive (bash 출력 등) | 🟠 위험 | 리드 | `scope` 필터 + `prose-only` 기본값으로 완화 |
| 스트림 중단 토큰 비용 (제공자별 과금) | 🔴 확인 필요 | 조사 | ③ 도입 전 주요 제공자 정책 조사 필수 |
| TTSR 무한 루프 | 🟠 위험 | 리드 | `max_retries_per_turn`(기본 3) + 사용자 알림 |
| `edit_diff.rs` 이전 시 기존 테스트 보존 | 🟢 합의 | — | 동일 입력/출력, 위치만 이동 |
| tree-sitter 의존 도입 | 🔴 **보류** | — | ⑤·block-ops·astCondition 모두 후순위. 본 로드맵에 포함 안 함 |
| Hashline recovery 3-way merge 구현 | 🟢 **결정됨** | — | 2단계 분할: M1은 session chain replay만(외부 의존 없음), 3-way merge는 post-M1(M1.5)로 연기. `01` §3.7 참조 |
| TTSR `cancel_signal` 과부하 | 🟢 **해결됨** | — | `cancel_signal`/`retry.rs` 건드리지 않음. 별도 `StreamOutcome::RuleInterrupt` 제어 흐름으로 분리. `03` §2.3 참조 |

---

## 8. 마일스톤 (실행 계획)

각 마일스톤은 **noop 폴백 보존**을 전제로, 부분 도입이 안전하다.

### M1 — Hashline 핵심 (최우선, ①)
- [ ] `oxicode-hashline` 크레이트 스캐폴드 (의존: xxhash-rust, lru, thiserror)
- [ ] `format.rs` + `compute_file_hash` (xxHash32 하위 16-bit) — omp 벡터와 byte-identical 검증
- [ ] `snapshots.rs` (`SnapshotStore` trait + `InMemorySnapshotStore`, lru)
- [ ] `parser.rs` + `tokenizer.rs` (라인 op만: SWAP/DEL/INS.PRE/POST/HEAD/TAIL)
- [ ] `apply.rs` + `repair_replacement_boundaries` (omp 계약 이식)
- [ ] `recovery.rs` Phase 1 — session chain replay (외부 의존 없음, `01` §3.7)
- [ ] `patcher.rs` (prepare/commit/preflight, all-or-nothing)
- [ ] omp 테스트 케이스 Rust 이식 (속성 + 회귀)
- [ ] `edit.rs` hashline 모드 추가, `read.rs` tag 발행
- [ ] 시스템 프롬프트 갱신 + str_replace vs hashline 벤치마크
- 상세: [`01`](./01-hashline-edit.md) §8 (M1.1–M1.14)

### M2 — Internal URL Router + 메모리 (②·④, M1과 부분 병렬)
- [ ] ④ `MemoryStore` SQLite impl + 임베딩 제공자 결정 (`04` M2b.1)
- [ ] ④ 4개 메모리 도구 + 부트 시 recall 주입 + 종료 시 reflect
- [ ] ② `InternalUrlRouter` 포트 + `CompositeUrlRouter` + noop
- [ ] ② `issue://` / `pr://` 핸들러 (기존 GitHub 도구 재사용)
- [ ] ② `read`/`search` URL dispatch (라우터 None 시 폴백)
- [ ] ② 추가 핸들러: `agent://`, `skill://`, `local://`, `memory://`(④ 연동)
- 상세: [`02`](./02-internal-url-router.md) §4, [`04`](./04-hindsight-memory.md) §3

### M3 — TTSR (③, ①·④ 안정화 후 권장)
- [ ] `RuleRegistry` 포트 + noop + `.oxicode/rules/*.mdc` discovery
- [ ] `agent_loop/ttsr.rs` (`TtsrEngine`, 정규식 매칭만)
- [ ] `streaming.rs` `StreamOutcome` + TTSR 인라인 체크 + mod.rs interrupt/retry 제어 흐름
- [ ] compaction 생존 훅 + 기본 룰 번들 (Rust 7개)
- [ ] **사전**: 스트림 중단 토큰 과금 정책 조사 (위험 보드)
- 상세: [`03`](./03-ttsr-rules.md) §4 (M3a.1–M3a.7)

---

## 9. 후순위 (본 로드맵 종료 후 별도 검토)

| 기능 | 재검토 조건 |
|---|---|
| **⑤ AST 편집 propose/resolve** | ①②③④ 안정화 후. 대규모 codemod가 oxicode 실사용 패턴에서 빈도가 확인되면. tree-sitter 의존 도입 + `ResolveQueue` 2단계 커밋 메커니즘 필요. |
| **⑥ Checkpoint/Rewind** | oxicode-cli: 세션 트리 기반 **논리적 rewind**(append-only `parent_id` 활용)을 향후 추가 가능 — 디스크 CoW 없이 "이 지점에서 다른 접근 분기". 물리적 CoW는 oxios 제품으로 이관. |
| **block-ops** (① Hashline의 `SWAP.BLK` 등) | tree-sitter 의존이 필요하므로 ⑤와 함께 후순위. 라인 op만으로 대다수 edit 시나리오를 커버 — `SWAP.BLK`는 함수/클래스 전체 교체에 유용하지만, 다중 `SWAP` 라인 op로 동등한 결과 달성 가능 (토큰 약간 증가). omp에서 block-ops가 전체 edit의 비중은 추정 소수. |
| **`learn` 도구** (④ Hindsight) | omp의 능동 학습 UX(사용자가 에이전트에게 가르치기). 4개 메모리 도구(retain/recall/reflect/edit)로 코어 가치 충분. `learn`은 대화형 UX 설계가 별도로 필요하므로 ④ 안정화 후 별도 검토. |

> 이들은 "가치가 없어서"가 아니라 **"지금 우선순위가 아니다"**. 4개 핵심 기능이 안정화되면 별도 설계 문서로 재검토한다.

---

## 10. 부록: 의존 크레이트 결정 (본 로드맵)

| omp 원본 | oxicode 대응 크레이트 | 도입 시점 |
|---|---|:-:|
| `Bun.hash.xxHash32` | [`xxhash-rust`](https://crates.io/crates/xxhash-rust) | M1 |
| `Diff.applyPatch` (jsdiff) | [`similar`](https://crates.io/crates/similar) (feature 게이트) | M1.5 (3-way merge recovery, `01` §3.7) |
| `bun:sqlite` | [`rusqlite`](https://crates.io/crates/rusqlite) | M2(④) |
| `regex` (이미 사용) | `regex` | M3(③) |
| `arktype` (스키마) | `serde_json::Value` + 수동 검증 | 전 단계 |
| `Bun.file/write` | `tokio::fs` (이미 사용) | M1 |
| `tree-sitter` / `ast-grep` | — | **후순위** (⑤·block-ops·astCondition) |

---

> **시작점**: [`01-hashline-edit.md`](./01-hashline-edit.md)부터. 가장 큰 가치, 가장 큰 리팩토링, 그리고 ②·③·④의 기반(M1.12의 read.rs tag 발행은 ②가 의존).
