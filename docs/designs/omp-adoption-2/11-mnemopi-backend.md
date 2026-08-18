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

---

## Addendum (2026-08-18): 설계 폐기 — oxibrain 단일 권위로 대체

이 문서가 설계한 로컬 Mnemopi 백엔드(oxicode-mnemopi 크레이트, SQLite
저장, 임베딩 파이프라인)는 제거되었다. Oxi Foundation 계획(§5.h/§6.f)에
따라 **oxibrain 데몬이 유일한 durable-memory 권위**이며 로컬 폴백은
존재하지 않는다.

- `oxicode-mnemopi/` 크레이트 삭제. `oxicode-cli/src/store/`의
  `memory_sqlite`, `memory_mnemopi`, `memory_workers`, `memory_summary`,
  `mnemopi`, `extracting_backend` 삭제.
- 임베딩 파이프라인(`embedding_provider` 등 설정 포함) 제거.
  `oxicode-sdk`의 `EmbeddingProvider` 포트는 feature-gated 소비자
  (oxios)용으로만 유지.
- 대체 구현: `oxicode-cli/src/foundation/brain.rs` — 유닉스 소켓
  `~/.oxi/brain/oxibrain.sock`(`OXIBRAIN_SOCKET` 오버라이드) 위
  `oxibrain-client`로 `remember`/`search`/`retract` 매핑.
  게이트 조건은 `memory_enabled && 소켓 존재`.
- 레거시 `~/.oxicode/memory/items.jsonl`은 `oxicode migrate brain`으로
  이전(`foundation/migrate.rs`, `--archive-legacy` 지원).

본 문서는 역사 기록으로만 유효하다.
