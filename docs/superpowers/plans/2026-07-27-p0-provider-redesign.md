# P0 — Provider/AI 재설계 실행 계획

> **상위 설계:** `docs/superpowers/specs/2026-07-27-omp-realignment-design.md` (Phase 0)
> **모드:** 자율 실행 (사용자 승인, 자러 감). 각 increment마다 build+clippy+nextest green 유지 후 커밋.

**Goal:** oxi-ai의 provider 정체성 붕괴를 수정하고 omp의 3-way 분리(transport / auth-login / metadata)로 재설계.

**Architecture:** omp처럼 `Api` dialect로 dispatch하는 streaming transport(identity 없음) + `ProviderDefinition` registry(auth/login) + `ProviderDescriptor`(catalog, metadata/discovery)로 분리. catalog는 별도 leaf 크레이트로 추출.

**Tech Stack:** Rust 2024 edition, cargo workspace. 검증: `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p oxi-sdk --features native-browser -- -D warnings`, `cargo nextest run --workspace`, `cargo fmt --all -- --check`.

## Global Constraints
- edition = "2024", rust-version = "1.96" (workspace). 새 크레이트 동일.
- 버전 0.60.0 (workspace 단일 소스). 새 크레이트도 0.60.0.
- `parking_lot::RwLock` (std 아님). async trait는 `async_trait`.
- 각 increment 끝에 회귀 게이트 전부 green. 깨면 그 increment 내에서 수정 후 커밋.

## Move Order (P0.1 — circular-dep 방지, advisor 검증)

`catalog/`가 oxi-ai core로 가는 의존성은 `crate::Api` 단 하나 (`models_dev.rs:49`, `materialize.rs:220` test). `InputModality`는 catalog/가 아니라 `model_db.rs`(bridge)에서만 사용 → model_db.rs는 oxi-ai 잔류.

1. `oxi-catalog` crate skeleton (`Cargo.toml`, `src/lib.rs`).
2. `Api` enum (`oxi-ai/src/types.rs:8-53`) → `oxi-catalog/src/api.rs`. oxi-ai는 `pub use oxi_catalog::Api;` 재-내보내기 (backward compat, callsite 대폭 감소).
3. `catalog/` 7파일 → `oxi-catalog/src/` (crate root). 내부 `crate::catalog::X` → `crate::X`. `crate::Api` → 그대로 (이제 local).
4. workspace `Cargo.toml` members에 `oxi-catalog` 추가. oxi-ai `Cargo.toml`에 `oxi-catalog` dep 추가.
5. workspace import 갱신: `oxi_ai::catalog::*` → `oxi_catalog::*`. `oxi_ai::Api` → re-export 유지로 그대로 동작.
6. `model_db.rs`/`model_registry.rs`: `crate::catalog::*` → `oxi_catalog::*`.
7. 회귀 게이트 green 확인 후 커밋.

## Task Sequence

각 task는 독립 커밋 단위. 회귀 게이트 전부 green이 gate.

- [x] P0.1 catalog extraction (위 move order)
- [ ] P0.2 complexity machinery 제거 (`multi_provider.rs`, `complexity_router.rs`, `circuit_breaker.rs`, `fallback_chain.rs`, `provider_pool.rs`, `router/` ~225KB). consumer를 direct dispatch로 rewire 후 삭제.
- [ ] P0.3 3-way Provider split — `ProviderDefinition` registry(auth/login) 추출, `Provider` trait에서 identity 제거, metadata → `ProviderDescriptor`(catalog). 정체성 붕괴 수정 검증: `provider_definition("deepseek").id == "deepseek"`.
- [ ] P0.4 AI 품질 — SSE 중앙화, per-provider 에러 계층, `ProviderEvent::ImageEnd`, `Api`를 `KnownApi` 14로 확장 + `Mistral` enum 제거.
- [ ] P0.5 provider 포팅 — Ollama(`ollama-chat`), Cursor(`cursor-agent`), Devin(`devin-agent`), GitLab Duo(`gitlab-duo-agent`). remote-AGENT 프로토콜은 고유 stream function.

## Verification per increment

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo nextest run --workspace
cargo fmt --all -- --check
```

하나라도 깨면 increment 내 수정. green이면 커밋.
