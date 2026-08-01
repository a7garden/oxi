# Release Process

Release workflow for the oxi workspace. 모든 크레이트가 동시에 같은 버전으로 출시된다.

## Prerequisites

- `main` 브랜치, 모든 PR 병합 완료
- `cargo nextest run --workspace` 통과
- `cargo audit` / `cargo deny check` 이상 없음
- GitHub secrets: `CARGO_TOKEN` (crates.io publish token) 설정됨

## Step-by-Step

### 1. Version Bump (all crates)

현재 버전이 `v0.39.0`이라면 `v0.40.0`으로. 패치/메이저도 동일하게 일괄 적용.

**바꿀 파일 (6개 Cargo.toml + 내부 의존성 버전):**

- `oxi-ai/Cargo.toml` — `version = "current"`
- `oxi-agent/Cargo.toml` — `version = "current"` + `oxi-ai`, `oxi-hashline` deps
- `oxi-sdk/Cargo.toml` — `version = "current"` + `oxi-ai`, `oxi-agent` deps
- `oxi-tui/Cargo.toml` — `version = "current"` (독립적, oxi-* 의존성 없음)
- `oxi-cli/Cargo.toml` — `version = "current"` + `oxi-ai`, `oxi-agent`, `oxi-sdk`, `oxi-tui` deps
- `oxi-hashline/Cargo.toml` — `version = "current"`

각 `Cargo.toml`의 `version = "X.Y.Z"`와 `oxi-* = { version = "X.Y.Z", path = "..." }` 내부 의존성을 함께 올려야 한다. workspace `Cargo.toml`에는 버전 필드가 없으므로 건드리지 않는다.

### 2. CHANGELOG Update

`CHANGELOG.md` 상단 `## [Unreleased]`를 `## [X.Y.Z] - YYYY-MM-DD`로 변경.

Unreleased 섹션이 이미 모든 변경사항을 담고 있어야 한다. 비어 있으면 PR 제목/커밋 로그에서 **Added / Changed / Fixed / Deprecated / Removed / Security** 순으로 항목을 재구성해서 채운다.

### 3. Verification

```bash
cargo check                                   # 타입 검증, ~15초
cargo nextest run --workspace --profile ci    # 전체 테스트 (~5분)
```

`cargo clippy --workspace --all-targets -- -D warnings`도 통과해야 하지만, CI에서 이미 PR 단계에서 검증하므로 생략 가능. Native-browser feature도 항상 컴파일되어야 함:

```bash
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
cargo build -p oxi-agent --features native-browser
```

### 4. Commit & Tag

```bash
git add -A
git commit -m "Release vX.Y.Z"
git tag vX.Y.Z
```

커밋 메시지에는 **변경사항 대표 항목 5-10개**를 bullet으로 포함. CHANGELOG의 첫 번째 heading들을 축약.

### 5. Push

```bash
git push origin main
git push origin vX.Y.Z
```

### 6. CI/CD Pipeline (자동)

`v*` 태그 푸시 시 `publish.yml`이 4단계로 실행됨:

1. **tag-check** — 태그가 `origin/main` 위에 있는지 검증 (stale/force-pushed 방지)
2. **package-check** — `cargo package`로 모든 크레이트의 패키징 건전성 검증
3. **verify** — fmt + clippy + nextest (MSRV 1.96 전체 게이트)
4. **publish** — 의존성 순서대로 crates.io에 순차 게시 (max-parallel: 1):
   `oxi-hashline → oxi-ai → oxi-tui → oxi-agent → oxi-sdk → oxi-cli`

각 크레이트 게시 전 의존성 크레이트의 crates.io 인덱스 전파를 폴링(최대 60회×10초)한다. 게시는 멱등적 — `already exists`면 스킵.

### 7. Post-Release Validation

```bash
# 바이너리가 정상 설치되는지 확인
cargo install oxi-cli --version X.Y.Z
oxi --version
```


## Governance Conventions

### Breaking Change Policy

Any root-level `pub` symbol removal, signature change, or semantic change MUST
appear under `## Breaking` in the CHANGELOG with:

1. **Full symbol path** — e.g. `oxi_sdk::ProviderCircuitBreaker`
2. **Replacement API or migration path** — what consumers should use instead
3. **Minimum deprecation window** — how many releases before physical removal
4. **Known affected consumers** — from GitHub code search

The CI `cargo-public-api` diff gate (see `.github/workflows/ci.yml`) fails PRs
that remove public symbols without a matching `## Breaking` entry.

### Deprecation Window

A public symbol marked for removal gets **≥1 release** (ideally 2) of:

```rust
#[deprecated(since = "0.XX.0", note = "use X instead; will be removed in 0.YY.0")]
```

During the deprecation window:
- The API signature is frozen (no signature changes).
- The semantics are frozen (no behavioral changes).
- `cargo build` on consumer code produces a deprecation warning.

### Error Variant Stability

Public error enums (`SdkError`, `ProviderError`, `BreakerError`) are
`#[non_exhaustive]`:
- **New variants** may be added freely (consumers need a catch-all `_ =>` arm).
- **Existing named variants are frozen** — changing what a variant means is a
  silent break, even if the name stays the same.
- **Semantic changes** require a rename (new variant) + deprecation of the old.
- `ToolError` is a type alias for `String` — stable by construction, no variants.

### Heavy Dependency Policy

Adding a heavy build dependency (>50 transitive crates, or requires a vendored
binary) requires:

1. A CHANGELOG `## Changed` entry noting build impact (e.g.
   `+~120 crates, +~150s cold build time`).
2. Feature-gating behind an off-by-default cargo feature so consumers who don't
   need it pay no build cost.

## Quick Checklist

```bash
# 1-4: Prepare release
VERSION=vX.Y.Z  # set this
git pull --rebase origin main
#   edit Cargo.toml files (version + dep version)
#   edit CHANGELOG.md (Unreleased → dated)
cargo check
cargo nextest run --workspace --profile ci
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
git add -A && git commit -m "Release ${VERSION}"
git tag "${VERSION}"

# 5: Push
git push origin main
git push origin "${VERSION}"
```

## Rollback

태그를 푸시하기 전(`git push origin vX.Y.Z` 전)이라면:

```bash
git tag -d vX.Y.Z
git reset --soft HEAD~1     # 커밋 취소 (변경사항은 staged로 유지)
```

푸시 후라면:

- crates.io는 이미 게시된 버전을 덮어쓰거나 삭제할 수 없음
- `vX.Y.Z+1`로 패치 릴리스 진행
- CHANGELOG에 rollback 사유를 **Fixed** 섹션에 기록
