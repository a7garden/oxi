# Implementation Plan: MCP Disk-Path Customization

**Base proposal:** `docs/proposals/mcp-disk-path-customization.md` (RFC, 2026-06-13)
**Consumer spec:** oxios `docs/rfc-023-mcp-delegation-to-oxi.md`
**Status:** Ready for implementation — validated against both repos
**Target release:** oxicode 0.34.0
**Owner:** oxicode core

---

## 1. 목적

oxios가 자체 MCP 클라이언트(`oxios-mcp`, 1458 LOC, 비표준 JSONL 프레이밍)를
폐기하고 oxicode-sdk 0.33.0의 `McpManager`(표준 Content-Length + lifecycle + 캐시 +
consent)로 마이그레이션(RFC-023)하기 위해 필요한, oxicode 측 **additive API 3종**을
구현한다.

이 계획서는 두 RFC를 교차 검증하여 구현 범위를 확정한다. 모든 변경은 기존
공개 시그니처를 변경/제거하지 않는다.

## 2. 범위

### In scope (oxicode 측)
- **R1** — `oxicode-agent::mcp::McpManager::spawn_with_paths(config, cache, consent)`
- **R2** — `oxicode-sdk::OxicodeBuilder::with_mcp_paths(cache, consent)` + `build()` 분기
- **R3** — `oxicode_sdk::MetadataCache` 재내보내기 (선택적, 아래 §4.3)
- R1/R2에 대한 `TempDir` 기반 단위 테스트

### Out of scope (oxios 측, 별도 PR — RFC-023 Phase 2~4)
- config 스키마 확장(`cwd`/`debug`/`lifecycle`/`direct_tools` 등)
- `mcp_bridge.rs` 변환 브리지(`build_oxicode_mcp_config()`)
- `McpApi` 재구현(`McpManager` 래핑 + `AccessManager` 보안 계층)
- `oxios-mcp` 크레이트 폐기 + crates.io deprecate
- `enabled = false` 서버 제외 로직 — **oxicode `ServerEntry`에 `enabled` 필드가 없음을 확인**(§3.4). oxios 브리지가 변환 시 `mcp_servers`에서 제외하는 것으로 처리. oxicode 측 변경 불필요.

### Out of scope (oxicode 측, 명시적 결정)
- `McpManager::new_no_spawn()` / `impl Default` — doc이 "Intended for tests" 명시.
  paths가 필요한 테스트는 직접 구성체 조립(기존 `direct_tools_from_cache_*` 테스트가
  이미 이 패턴 사용).
- `oxicode-sdk::tool_factory::mcp_tools()` — 호출처 0건 + oxios가 이 경로를 쓰지 않음(§3.3).
  향후 실 소비자 생기면 별도 PR로 `mcp_tools_with_paths()` 추가.

## 3. oxios 실사용 검증 결과

RFC-023과 oxios 코드베이스를 직접 조사하여 API 요구사항을 확정했다.

### 3.1 ✅ oxios는 항상 config + paths 둘 다 주입

RFC-023 §5.3 + §5.7 + engine.rs 조사 결과, oxios의 엔진 조립 패턴:

```
~/.oxios/config.toml [mcp] ──→ build_oxicode_mcp_config() ──→ oxicode_sdk::McpConfig
                                                                │
                         ~/.oxios/mcp-cache.json ──────────────┤
                         ~/.oxios/mcp-consent.json ────────────┴─→ OxicodeBuilder
                                                                   .with_mcp_config(cfg)
                                                                   .with_mcp_paths(cache, consent)
                                                                   .build()
```

- `engine.rs`의 `OxicodeBuilder` 조립부(`OxiosEngineBuilder::build()`)에 MCP 코드는
  현재 전혀 없음 → 마이그레이션 시 R1+R2를 모두 사용하는 호출이 **추가**됨.
- oxios는 `~/.config/oxicode/mcp.json` 자동 발견을 **의도적으로 비활성화**해야 하므로
  config 주입은 필수(`with_mcp_config`). 따라서 R2의 `with_mcp_paths`는 항상
  `with_mcp_config`와 짝지어 호출됨.

### 3.2 ✅ 보안 계층 분리 — consent 경로 분리의 진짜 이유

RFC-023 §5.5의 `McpApi` 재설계:

```rust
pub struct McpApi {
    manager: Arc<oxicode_sdk::mcp::McpManager>,      // oxicode: consent(Allow/Deny 디스크)
    access: Arc<Mutex<AccessManager>>,            // oxios: RBAC + Merkle audit
}
```

- **oxicode `ConsentManager`**: 툴별 Allow/Deny 디스크 영속 정책 → `mcp-consent.json`
- **oxios `AccessManager`**: RBAC + Merkle audit 트리 → 별도 oxios 스토어

oxios가 oxicode consent 파일을 `~/.oxios/`로 옮겨야 하는 이유: oxios만의 consent
정책을 단일 진실 소스 아래 관리 + oxicode CLI 사용자의 consent와 분리. R1/R2가
이것을 가능하게 하는 직접적 기능적 요구.

### 3.3 ✅ 툴 등록은 `Oxicode::mcp()` 직접 패턴 — `mcp_tools()` factory 미사용

RFC-023 §5.6의 툴 등록 예시:

```rust
let mcp_manager = kernel.mcp.manager();   // Arc<oxicode_sdk::McpManager>  ← Oxicode::mcp()
for def in mcp_manager.direct_tools_from_cache() {
    registry.register(oxicode_sdk::mcp::McpDirectTool::new(mcp_manager.clone(), def));
}
if !mcp_manager.should_disable_proxy() {
    registry.register(oxicode_sdk::mcp::McpTool::new(mcp_manager.clone()));
}
```

검증: `McpDirectTool::new(manager, def)` / `McpTool::new(manager)` 시그니처가
정확히 일치(`direct_tool.rs:45`, `tool.rs:39`).

→ oxios는 `oxicode-sdk/src/tool_factory.rs::mcp_tools()` 함수를 **사용하지 않는다**.
이 함수는 현재 호출처 0건이며, R2를 통한 `OxicodeBuilder` → `Oxicode::mcp()` → 직접 등록
경로만 표준. 따라서 §2에서 `mcp_tools()` 제외 결정은 확정.

### 3.4 ✅ `enabled` 필드 — oxicode에 없음, oxios 브리지 책임

`oxicode-agent/src/mcp/types.rs::ServerEntry` 필드 전수 조사:
`command, args, env, cwd, url, headers, lifecycle, idle_timeout, debug,
directtools, exclude_tools` — **`enabled` 없음**.

RFC-023 §5.2가 제시한 해법: oxios config의 `enabled = false` 서버는
`build_oxicode_mcp_config()` 변환 시 `mcp_servers` 맵에서 **제외**. oxicode 측 변경 불필요.

### 3.5 ✅ 로컬 patch 경로 이미 준비됨

`oxios/Cargo.toml:170-176`에 주석 처리된 `[patch.crates-io]` 블록:
```toml
# oxicode-sdk = { path = "/Volumes/MERCURY/PROJECTS/oxicode/oxicode-sdk" }
```
RFC-023 Phase 1의 "임시 조치"가 주석 해제만으로 즉시 가능. oxios 측은
oxicode 0.34.0 릴리스 전까지 로컬 patch로 개발하고, 릴리스 후 patch 제거 + 버전 bump.

## 4. 사전 검토 보완점

RFC(oxicode 측 제안)는 코드 인용이 정확하지만 구현 시 반영해야 할 점.

### 4.1 🔴 `build()`의 doc/코드 모순 수정 (R2에 포함)

RFC가 제안한 `build()` 의사코드:
```rust
if self.mcp_cache_path.is_some() || self.mcp_consent_path.is_some() {
    let cfg = self.mcp_config.unwrap_or_default();  // ← 빈 config (서버 0개)
    ...
}
```
같은 RFC의 `with_mcp_paths` doc: *"otherwise oxicode auto-discovers from its
standard config file locations"*.

`unwrap_or_default()`는 **빈 McpConfig**를 만들어 doc과 모순. oxios는 항상
config를 주입(§3.1)하므로 당장 영향은 없지만, 일반 SDK 소비자에게 풋건.
`load_mcp_config()`가 `pub`이므로 이를 사용:

```rust
let cfg = match self.mcp_config {
    Some(cfg) => cfg,
    None => oxicode_agent::mcp::config::load_mcp_config(), // auto-discover
};
```

효과: "config = 무엇을 / paths = 어디에" 직교성 보존, doc이 사실이 됨.

### 4.2 🟡 R1 body의 "rest identical"는 ~40줄

RFC가 `spawn_with_paths` body를 `// ... rest identical ...`로 생략했지만,
옮겨야 할 로직이 있다(현재 `spawn_with_config` body):
1. `cached_servers` 사전 로드 (`cache.cached_servers()`)
2. `Arc::new_cyclic`로 lifecycle task에 `Weak<Self>` 주입
3. `inner.try_lock()`으로 `raw_tool_metadata` seed (prefix_mode 적용)
4. `tokio::spawn(start_eager_servers)` fire-and-forget

이 네 단계가 그대로 이동해야 "spawn / spawn_with_config 관측 동작 불변"
인수기준이 성립. 노동량 자체는 적지만 "zero-effort"는 아님.

### 4.3 🟡 R3(MetadataCache 재내보내기)는 선택적으로 강등

RFC-023 어디에도 `MetadataCache` 직접 사용 언급 없음 — oxios는
`McpManager::cache()` 접근자로 충분. R3는 향후 "reset MCP state" 같은
관리 기능을 위한 편의용이므로 **같은 PR에 포함하되 우선순위는 낮춤**.
비용이 1줄이므로 굳이 분리하지 않고 R2와 함께 처리.

## 5. 구현 단계

독립적으로 커밋 가능한 3단계. 의존성: A → B → C (B가 A의 새 생성자를 호출).
C는 독립적이지만 같은 PR에.

### Phase A — `oxicode-agent`: `spawn_with_paths` (R1)

**파일:** `oxicode-agent/src/mcp/mod.rs`

1. `spawn_with_paths(mcp_config, cache_path: Option<PathBuf>, consent_path: Option<PathBuf>) -> Arc<Self>` 추가.
   - body: §4.2의 4단계 로직 전체 포함.
   - `cache_path`/`consent_path`가 `None`이면 `MetadataCache::new()` / `ConsentManager::new()` 사용(기본 경로).
   - `use std::path::PathBuf;` 추가(mod.rs 상단에 확인 필요).
2. 기존 `spawn_with_config`를 thin wrapper로 리팩터:
   ```rust
   pub fn spawn_with_config(mcp_config: McpConfig) -> Arc<Self> {
       Self::spawn_with_paths(mcp_config, None, None)
   }
   ```
3. `spawn()`은 이미 `spawn_with_config(load_mcp_config())`이므로 그대로.
   doc 정리: "Primary constructor" 라벨을 `spawn_with_paths`로 이동,
   `spawn()`/`spawn_with_config()` doc에 "uses default disk paths" 명시.

**검증:**
- `cargo build -p oxicode-agent`
- `cargo nextest run -p oxicode-agent mcp`
- 기존 `mcp` 관련 테스트 전부 수정 없이 통과 (관측 동작 불변의 증거)

### Phase B — `oxicode-sdk`: `with_mcp_paths` + `build()` (R2)

**파일:** `oxicode-sdk/src/builder.rs`

1. `OxicodeBuilder`에 필드 2개 추가 + `new()` 초기화:
   ```rust
   mcp_cache_path: Option<std::path::PathBuf>,
   mcp_consent_path: Option<std::path::PathBuf>,
   // new(): 둘 다 None
   ```
2. builder 메서드:
   ```rust
   pub fn with_mcp_paths(mut self, cache_path: PathBuf, consent_path: PathBuf) -> Self {
       self.mcp_cache_path = Some(cache_path);
       self.mcp_consent_path = Some(consent_path);
       self
   }
   ```
3. `build()`의 MCP 분기를 §4.1 수정안으로 교체:
   ```rust
   let mcp_manager = if self.mcp_enabled {
       if self.mcp_cache_path.is_some() || self.mcp_consent_path.is_some() {
           let cfg = match self.mcp_config {
               Some(cfg) => cfg,
               None => oxicode_agent::mcp::config::load_mcp_config(),
           };
           Some(oxicode_agent::mcp::McpManager::spawn_with_paths(
               cfg, self.mcp_cache_path, self.mcp_consent_path,
           ))
       } else {
           Some(match self.mcp_config {
               Some(cfg) => oxicode_agent::mcp::McpManager::spawn_with_config(cfg),
               None => oxicode_agent::mcp::McpManager::spawn(),
           })
       }
   } else {
       None
   };
   ```
   else 분기는 0.33.0 코드와 byte-for-byte 동일 → 호환성.

**검증:**
- `cargo build -p oxicode-sdk`
- `cargo nextest run -p oxicode-sdk`
- `cargo build -p oxicode-cli` (호환성 게이트 — oxicode-cli는 `OxicodeBuilder` 경유만 사용)

### Phase C — `oxicode-sdk`: `MetadataCache` 재내보내기 (R3)

**파일:** `oxicode-sdk/src/lib.rs`

기존 블록(591-596행)에 `MetadataCache` 한 개 추가:
```rust
pub use oxicode_agent::mcp::{
    ConsentManager, ConsentState, DirectToolDef, DirectToolsConfig, LifecycleMode,
    McpCallResult, McpConfig, McpConnectionStatus, McpContent, McpDashboardData, McpDirectTool,
    McpManager, McpSamplingRequest, McpServerInfo, McpSettings, McpSettingsView, MetadataCache,
    McpTool, McpToolDef, McpToolInfo, ServerEntry, ToolMetadata, ToolPrefix,
};
```

**검증:** `cargo doc -p oxicode-sdk --no-deps`에서 `oxicode_sdk::MetadataCache` 페이지 생성 확인.

## 6. 테스트 계획

모두 `TempDir` 기반이고 **서버 연결/모킹 불필요**. 접근자(`cache().path()`,
`consent().path()`)가 `pub`이라 경로 비교만으로 충분.

### T1 — `oxicode-agent` 단위 테스트 (`mod.rs`의 `#[cfg(test)] mod tests`)

```rust
#[tokio::test]
async fn spawn_with_paths_uses_supplied_paths() {
    let dir = TempDir::new().unwrap();
    let cache_p = dir.path().join("c.json");
    let consent_p = dir.path().join("consent.json");
    let mgr = McpManager::spawn_with_paths(
        McpConfig::default(),
        Some(cache_p.clone()),
        Some(consent_p.clone()),
    );
    assert_eq!(mgr.cache().path(), cache_p);
    assert_eq!(mgr.consent().path(), consent_p);
}

#[tokio::test]
async fn spawn_with_paths_none_falls_back_to_defaults() {
    // None, None → 기본 경로 사용 (에러 없이 spawn)
    let mgr = McpManager::spawn_with_paths(McpConfig::default(), None, None);
    assert!(!mgr.cache().path().as_os_str().is_empty());
}
```

### T2 — `oxicode-sdk` 통합 테스트 (`oxicode-sdk/tests/mcp_paths.rs` 신규)

```rust
#[tokio::test]
async fn builder_with_mcp_paths_propagates_to_manager() {
    let dir = TempDir::new().unwrap();
    let cache_p = dir.path().join("c.json");
    let consent_p = dir.path().join("consent.json");

    let oxicode = OxicodeBuilder::new()
        .with_mcp_config(McpConfig::default())   // 빈 config → eager 서버 없음
        .with_mcp_paths(cache_p.clone(), consent_p.clone())
        .build();

    let mgr = oxicode.mcp().expect("mcp enabled");
    assert_eq!(mgr.cache().path(), cache_p);
    assert_eq!(mgr.consent().path(), consent_p);
}

#[tokio::test]
async fn builder_without_paths_uses_defaults() {
    // 호환성: with_mcp_paths 없이 build → mcp()는 Some, 경로는 기본
    let oxicode = OxicodeBuilder::new().with_mcp_config(McpConfig::default()).build();
    assert!(oxicode.mcp().is_some());
}
```

### T3 — `oxicode-sdk` 단위 테스트 (R3)

```rust
#[test]
fn metadata_cache_re_export_resolves() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("c.json");
    let c = oxicode_sdk::MetadataCache::with_path(p.clone());
    assert_eq!(c.path(), p);
}
```

### 회귀
- `cargo nextest run --workspace` 전체 통과 필수.
- 특히 기존 `oxicode-agent` mcp 테스트 4개 + `oxicode-sdk` builder 테스트가
  수정 없이 통과해야 함 (관측 동작 불변의 증거).

## 7. 체크리스트

구현 완료 시 아래를 모두 만족해야 PR 오픈.

- [ ] Phase A: `spawn_with_paths` 추가, `spawn_with_config` wrapper화
- [ ] Phase B: `with_mcp_paths` + `build()` 분기 (§4.1 수정안 포함)
- [ ] Phase C: `MetadataCache` 재내보내기
- [ ] T1/T2/T3 테스트 추가 및 통과
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo nextest run --workspace`
- [ ] `oxicode-cli` 빌드 성공 (호환성 게이트)
- [ ] CHANGELOG.md `## [Unreleased]` 섹션에 항목 추가
- [ ] Conventional Commit 타이틀 (예: `feat(sdk): MCP disk-path customization for SDK consumers`)

## 8. CHANGELOG 초안

```markdown
## [Unreleased]

### Added
- **`McpManager::spawn_with_paths(config, cache, consent)`**: SDK 컨슈머가
  MCP 메타데이터 캐시와 consent 저장소의 디스크 경로를 커스터마이즈할 수
  있도록 추가. 기존 `spawn()` / `spawn_with_config()`는 이 생성자의
  thin wrapper가 됨 (관측 동작 불변). oxios RFC-023이 요구.
- **`OxicodeBuilder::with_mcp_paths(cache, consent)`**: 프로그래밍 방식으로
  MCP 디스크 경로를 주입하는 빌더 메서드. `with_mcp_config`과 직교.
- **`oxicode_sdk::MetadataCache`** 재내보내기 (캐시 검사/초기화용 편의).

### Fixed
- `OxicodeBuilder::build()`에서 MCP paths-only 분기가 빈 `McpConfig`를 사용하던
  잠재적 풋건 수정 — 이제 `with_mcp_config` 없이 `with_mcp_paths`만 호출해도
  표준 경로에서 config를 자동 발견한다.
```

## 9. 리스크 평가

| 리스크 | 확률 | 영향 | 완화 |
|---|---|---|---|
| `spawn_with_config` wrapper화가 관측 동작 변경 | 낮 | 중 | T1 + 기존 mcp 테스트로 검증; body는 byte-for-byte 동일 |
| `build()` 분기 실수로 기본 경로 변경 | 낮 | 중 | else 분기는 0.33.0과 동일; T2로 검증 |
| `MetadataCache` 공개 API 향후 확장 시 재내보내기 깨짐 | 매우 낮 | 낮 | additive만 하므로 사실상 무해 |
| oxios가 `tool_factory::mcp_tools()` 경로 사용 | 없음 | — | §3.3 검증: oxios는 `Oxicode::mcp()` 직접 등록 패턴 사용 |

> **참고:** 모든 리스크가 "낮/매우 낮"이며 oxios 실사용 시나리오(§3)와
> 정확히 일치하므로, 구현 착수 전 추가 합의 불필요.

## 10. 예상 규모

| 단계 | 파일 | 라인(추정) |
|---|---|---|
| Phase A | `oxicode-agent/src/mcp/mod.rs` | +45 (body 이동), -30 (wrapper화) = net +15 + 테스트 +20 |
| Phase B | `oxicode-sdk/src/builder.rs` | +30 + 테스트 +15 |
| Phase C | `oxicode-sdk/src/lib.rs` | +1 + 테스트 +8 |
| **합계** | 3 파일 | ~90 라인 (코드+테스트) |

RFC의 "~45 라인" 추정과 일치. 단일 PR로 처리 적합.

## 11. oxios 측 후속 (참고용, 본 PR 범위 외)

본 oxicode PR 병합 후 oxios가 수행할 작업(RFC-023 Phase 1~4). oxicode 측 구현 완료를
전제로 하며, oxios 저장소에서 별도 진행.

1. **Phase 1** — `Cargo.toml`: `oxicode-sdk = "0.34.0"` bump + 로컬 patch 제거
2. **Phase 2** — `config.rs` 스키마 확장 + `mcp_bridge.rs` 신규(`build_oxicode_mcp_config()`)
3. **Phase 3** — `mcp_api.rs`/`tools/`/`kernel.rs` 교체
4. **Phase 4** — `oxios-mcp` 크레이트 폐기 + crates.io deprecate

oxicode 0.34.0 릴리스 전에는 oxios `Cargo.toml`의 주석 처리된 `[patch.crates-io]`를
해제(§3.5)하여 로컬 개발 가능.
