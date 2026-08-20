# RFC-009: 미사용 Workflow DSL 제거; 새 loop hook 보류

| 메타 | 값 |
|------|-----|
| **상태** | Proposed |
| **작성일** | 2026-08-17 |
| **영역** | `oxicode-sdk`, `oxicode-agent` |
| **영향 크레이트** | `oxicode-sdk` |
| **관련** | RFC-003, RFC-006, SDK stabilization roadmap |

---

## 1. 결정

1. `WorkflowDefinition`과 `WorkflowEngine`을 제거한다. 현재 first-party consumer가 없고, YAML DSL의 동시성 계약도 구현되지 않았다.
2. `around_stream`, `around_steering`, `before_compact` hook은 추가하지 않는다. 현재 제품 consumer가 요구 사항을 제시하지 않았고, 메시지 소유권·실패·취소 계약이 정의되지 않았다.
3. oxios port 정합성 audit, TUI 입력 문제, 슬래시 명령 문제는 이 RFC 범위에서 제외한다. 독립 문제는 독립 근거와 owner를 가진 별도 RFC 또는 issue로 다룬다.

이 RFC는 agent loop를 그래프로 재설계하지 않는다. 고정된 loop 단계와 SDK의 기존 coordination API는 유지한다.

## 2. 근거

| 관찰 | 근거 | 결론 |
|------|------|------|
| DSL 표면 | `workflow_dsl.rs`는 YAML parser와 6개 step (`Run`, `Parallel`, `Chain`, `ForEach`, `Vote`, `SetState`)을 제공한다. | parser와 execution model 모두 제거 대상이다. |
| 실행기 완결성 | `workflow_engine.rs`는 6개 step을 실행하지만 `Parallel`의 `concurrency` 값을 받지 않고 무시한다. | 공개 DSL이 약속하는 동시성 의미가 불완전하다. |
| first-party 사용 | `oxicode-cli`와 sibling `oxios`에는 caller가 없다. SDK 내부 사용은 제거 대상인 DSL source, engine implementation, 단위 테스트, re-export에 한정된다. | 현재 제품 기능을 제거하지 않는다. |
| 실제 공개 범위 | root `WorkflowEngine` re-export만 `workflow-dsl` feature로 gated 되어 있다. 그러나 `lib.rs`의 `pub mod workflow_dsl`/`pub mod workflow_engine`과 `prelude.rs`의 DSL re-export는 gated 되어 있지 않다. | feature와 unstable 표지만으로 제거를 semver-safe라고 할 수 없다. 외부 consumer는 ungated module 또는 prelude path를 사용할 수 있다. |
| 현재 hook | `AgentLoopConfig`는 tool 전/후, approval, compaction 완료 후 hook을 가진다. SDK `HookRunner`와 CLI extension callback은 별도 계층이다. | 이들을 하나의 "`run_loop` hook 7개"로 세면 책임 경계가 흐려진다. |
| compaction 순서 | `maybe_compact`는 `should_compact` 뒤에 먼저 mechanical shake를 시도하고, LLM compaction 완료 뒤에만 `on_compaction`을 호출한다. | 사전 veto가 필요하다는 consumer가 생기면 shake와 LLM 경로 모두에 적용되는 별도 계약이 필요하다. |

first-party 검색은 downstream 사용 여부를 증명하지 않는다. 따라서 이 RFC는 API 제거를 의도된 호환성 변경으로 취급한다.

## 3. 제거 범위

구현 PR은 다음을 함께 제거한다.

- `oxicode-sdk/src/workflow_dsl.rs`
- `oxicode-sdk/src/workflow_engine.rs`
- `oxicode-sdk/src/lib.rs`의 두 public module 선언과 `workflow-dsl` root re-export
- `oxicode-sdk/src/prelude.rs`의 `WorkflowDefinition` / `WorkflowStepDef` re-export
- `oxicode-sdk/Cargo.toml`의 `workflow-dsl` feature, `unstable` feature 목록 항목, 그리고 이 DSL만 사용하는 `serde_yaml` dependency
- lockfile의 dependency 변화
- 현재 상태를 설명하는 문서의 `workflow-dsl` feature 목록 및 stabilization roadmap 항목

제거된 모듈의 단위 테스트도 모듈과 함께 삭제한다. 과거 설계 RFC와 과거 changelog 항목은 역사 기록이므로 수정하지 않는다. 새 `[Unreleased]` changelog 항목에는 제거된 public API와 migration을 명시한다.

## 4. 호환성 및 릴리스

현재 `oxicode-sdk`는 `0.76.0`이다. 따라서 기존의 "`v0.76 다음 minor`" target은 유효하지 않다.

`workflow-dsl` feature와 `#[oxicode_unstable]` annotation은 root re-export에만 적용된다. ungated public module과 prelude re-export가 존재하므로 이 제거는 external consumer에게 breaking change일 수 있다.

구현 전에 maintainer는 프로젝트의 0.x 호환성 정책을 확인한다.

- 다음 0.y release에서 breaking change를 허용하는 정책이면, 해당 release에서 이 RFC대로 제거한다.
- 그렇지 않으면 먼저 모든 public path를 deprecated로 표시하고, 명시한 deprecation window 뒤에 제거한다.

어느 경로든 changelog에는 다음 migration을 포함한다: declarative workflow가 필요한 consumer는 SDK의 `AgentGroup`, `CoordinatedGroup`, `SharedMemory`, `Consensus`를 직접 조합해야 한다. 제거된 YAML 형식에 대한 자동 변환이나 compatibility shim은 제공하지 않는다.

## 5. 보류한 hook 확장

현재 event stream은 관찰용이고, hook은 실행 의미를 바꾼다. 새 hook은 구체적인 product use case와 계약 없이 추가하지 않는다.

별도 RFC가 새 hook을 제안하려면 최소한 다음을 정의해야 한다.

1. 이름이 있는 consumer와 `AgentEvent` 또는 기존 tool/approval/compaction hook으로 해결되지 않는 관찰 가능한 요구 사항.
2. hook이 model input, pending steering, persisted message history 중 무엇을 읽거나 바꿀 수 있는지. append-only sync 전후의 소유권도 포함한다.
3. cancellation과 error가 turn을 중단하는지, 원래 동작으로 계속하는지, 그리고 어떤 event 또는 log를 남기는지.
4. 기존 TTSR, tool execution, append-only persistence와의 순서 및 회귀 테스트.

특히 `before_compact`는 새 RFC에서 다음을 결정해야 한다.

- `should_compact`가 true를 반환한 뒤, mechanical shake 전에 한 번만 호출할 것.
- skip이 mechanical shake와 LLM compaction 모두를 건너뛰는지.
- hook error가 fail-open인지 fail-closed인지.
- skip 이유가 public API, event payload, log 중 어디에 남는지.

`String` reason을 가진 public decision enum이나 `around_stream` 메시지 변환 API를 먼저 추가하지 않는다. 관측성과 소유권 계약이 없는 가변 표면은 제거하려는 DSL보다 더 큰 장기 호환성 부담을 만든다.

## 6. 명시적 비범위

- LangGraph식 동적 그래프, subgraph, checkpoint visualizer
- oxios의 `AccessGate`, `CapabilityResolver`, `HookRunner` 사용 여부 또는 통합
- TUI 한글/emoji cursor 및 슬래시 명령 동작
- `AgentLoop::run_loop`의 단계 재배치

이 항목들은 workflow DSL 제거의 선행 조건도, 수용 기준도 아니다.

## 7. 구현 및 검증

1. §4의 호환성 경로를 release owner가 확정한다.
2. §3의 source, manifest, lockfile, 현재 문서를 한 PR에서 제거한다.
3. 다음 검증을 실행한다.

```bash
cargo fmt --all -- --check
cargo clippy -p oxicode-sdk --all-targets --all-features -- -D warnings
cargo check --workspace --all-targets
cargo nextest run --workspace
cargo test -p oxicode-sdk --doc
```

4. active source와 current product documentation에서 `WorkflowDefinition`, `WorkflowStepDef`, `WorkflowEngine`, `WorkflowResult`, `StepOutput`, `workflow-dsl`, `serde_yaml`의 잔존 참조를 확인한다. 과거 RFC와 released changelog의 역사 참조는 허용한다.

## 8. 수용 기준

- [ ] §4의 호환성 경로와 target release가 결정되어 있다.
- [ ] DSL source, 모든 public path, Cargo feature, 전용 dependency가 함께 제거되었다.
- [ ] current feature/release/stabilization 문서와 `[Unreleased]` changelog가 제거 사실 및 migration을 설명한다.
- [ ] first-party workspace가 제거된 API를 참조하지 않는다.
- [ ] §7의 검증 명령이 통과한다.
- [ ] 새 loop hook, oxios integration change, TUI change가 이 PR에 포함되지 않는다.