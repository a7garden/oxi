# omp(oh-my-pi) 기능 도입 설계 문서

> oxicode에 omp가 검증한 기능 **4개**를 도입하는 종합 설계. 2026-06-19 기준 omp v16.1.1 심층 분석 기반. 범위 확정 v2.

## 문서 구조

| 문서 | 등급 | 내용 |
|---|:-:|---|
| **[`00-master-plan.md`](./00-master-plan.md)** | — | 🎯 **시작점.** 전체 로드맵, 의존성 그래프, 도입 원칙(5개 불변량), 리팩토링 허용 범위, 마일스톤 |
| [`01-hashline-edit.md`](./01-hashline-edit.md) | 🟢 핵심 | **line-anchored edit + content tag + 드리프트 복구 + boundary repair.** `oxicode-hashline` 크레이트 설계 |
| [`04-hindsight-memory.md`](./04-hindsight-memory.md) | 🟡 | 세션 간 메모리. 기존 `MemoryStore` 포트 충전 + SQLite impl |
| [`02-internal-url-router.md`](./02-internal-url-router.md) | 🟡 | 프로토콜 스킴을 read/search에 투명 주입. `InternalUrlRouter` 포트 12 |
| [`03-ttsr-rules.md`](./03-ttsr-rules.md) | 🟠 신중 | 스트림 중단 + 룰 주입 + 재시도. `StreamOutcome` 신규 제어 흐름. `RuleRegistry` 포트 13 |

## 읽는 순서

1. **`00-master-plan.md`** — 왜 이 4개인지, 어떤 순서로, 어떤 원칙으로.
2. **`01-hashline-edit.md`** — 가장 큰 가치·가장 큰 리팩토링·②③④의 기반. **핵심 문서.**
3. 그 외는 관심 기능별로.

## 핵심 결정 요약 (v2)

- **도입 4종**: ① Hashline(🟢) · ④ Memory(🟡) · ② Internal URL(🟡) · ③ TTSR(🟠).
- **후순위 보류**: ⑤ AST 편집, ⑥ Checkpoint — 가치는 있으나 현재 우선순위/리소스에서 후순위. 4종 안정화 후 별도 검토.
- **영구 제외**: LSP/DAP/코드실행커널/ACP/brush 셸 등 — omp "완제품" 정체성과 oxicode "가벼운 임베더블 엔진" 정체성 충돌.
- 모든 기능은 **포트 또는 도구** 형태, **noop 폴백** 필수 → 부분 도입 안전, regression 제로.
- **① Hashline이 최우선** — omp 5가지 str_replace 한계를 모두 해결하며 기존 edit 인프라가 60% 갖춰져 있어 최고 ROI.
- oxicode 기존 자산(`MemoryStore` 포트, `file_mutation_queue`, `streaming.rs` 토큰 루프) 재사용으로 도입 비용 절감. TTSR는 `cancel_signal`이 아닌 별도 `StreamOutcome` 제어 흐름 사용.
- tree-sitter 의존은 **후순위**로 미룸 — block-ops(①), astCondition(③), ⑤ AST 편집 모두 라인/정규식 기반 MVP로 omp 가치의 대부분 달성.

## 가치 등급 vs 구현 순서

- **가치 등급**(사용자 우선순위): ①(🟢) > ④·②(🟡) > ③(🟠)
- **구현 순서**(의존성/안정성): M1 ① → M2 ②·④(병렬) → M3 ③

```
M1  ① Hashline (oxicode-hashline 크레이트)        ← 최우선, ②의 기반
M2  ④ Hindsight memory     ┐
    ② Internal URL Router  ┘ ← ① read.rs 변경 후, 서로 병렬
M3  ③ TTSR rules           ← ①·④ 안정화 후 (리스크 완화), agent_loop 독립
```

> 각 문서의 "의존성 & 마일스톤" 섹션에서 서브태스크 단위로 분해.
