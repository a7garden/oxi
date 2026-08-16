# 세부 설계 ⑩ — Mnemopi 백엔드 (SQLite 메모리 스토리지 계층)

> 상태: **SUPERSEDED** (2026-08-17) — 이 설계는 더 이상 구현 대상이 아닙니다.
> 대체: [`docs/superpowers/specs/2026-08-17-oxi-foundation-contract.md`](../../superpowers/specs/2026-08-17-oxi-foundation-contract.md) § *Brain memory*.
> 구현 계획: [`docs/superpowers/plans/2026-08-17-oxi-foundation-integration.md`](../../superpowers/plans/2026-08-17-oxi-foundation-integration.md) § 5·6.
>
> **왜 superseded 인가.** oxicode는 Oxi Foundation v1 이후로 두 호스트 모두
> (oxicode, oxibrain)가 durable-memory 권한을 공유하지 않습니다. 이 설계가
> 제안한 SQLite-기반 로컬 메모리 스토리지는 oxicode 측의 두 번째 authority
> 가 되며, Foundation 컨트랙트는 그런 권한을 허용하지 않습니다. oxibrain
> daemon이 단일 durable-memory 권한이며, oxicode는 BrainMemoryBackend를
> 통해 typed 클라이언트로 읽기/쓰기/요약만 합니다. 별도의 SQLite/JSON/
> summary 파일은 archive-only 상태로 유지되며 활성 메모리 경로가 아닙니다.
>
> 본 문서의 역사적 설계 노트(하이브리드 리콜 공식, 2-tier working/episodic,
> 임베딩 폴백, 뱅크 스코핑)는 보존되지만 새 코드는 만들어지지 않습니다.
> SQLite / summary / JSON 백엔드 코드는 `oxicode-cli/src/store/memory_*.rs`
> 아래에 0.76 deprecation 표지 후 0.77에서 제거 예정입니다.
>
> 상태: 설계 **v2** (코드 검증 개정 — [`00-design-revisions.md`](./00-design-revisions.md) §4·§7·§9 참조)
> 작성: 2026-06-19 (v1), 개정 (v2)
> 선행: [`00-master-plan.md`](./00-master-plan.md), 1차 [`04-hindsight-memory.md`](../omp-adoption/04-hindsight-memory.md)
> omp 분석: `packages/mnemopi/` (~9,000줄), `packages/coding-agent/src/mnemopi/` (래퍼 4파일)
> 후속: N3 구현 → CHANGELOG.md
> 짝: [`12-hindsight-memory.md`](./12-hindsight-memory.md) (응용 계층)
