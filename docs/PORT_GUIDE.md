# Port System Guide

> oxi-sdk가 정의하는 **port trait**과 제품별 구현 가이드.

> ⚠️ **업데이트 (2026-06):** `oxi-fs` 크레이트는 `oxi-sdk/src/ports/fs/`로 흡수되었습니다.
> 아래 예제의 `oxi_fs::` 경로는 이제 `oxi_sdk::ports::fs::`입니다(타입명은 동일).
> port 수도 11개에서 15개로 늘었습니다(전체 목록은 `oxi-sdk/src/ports/mod.rs`).

## 왜 port인가?

oxi-sdk는 **에이전트 런타임**(oxi-agent) + **빌더/옵저버빌리티/멀티에이전트**(자체)를 제공합니다. 하지만 **영속·인증·이벤트버스·스킬·메모리** 같은 "인프라"는 제품마다 다릅니다:

- `oxi-cli`는 `~/.oxi/` 디렉토리에 JSONL/TOML/JSON
- `oxios-kernel`은 SQLite + 자체 StateStore
- 서버리스 신제품은 S3 + DynamoDB
- 테스트는 in-memory

이 모든 케이스를 **하나의 SDK가** 모두 구현하면 SDK가 무거워지고, 새 시스템의 진입장벽이 높아집니다. 해결책: **port trait**으로 계약을 정의하고, 각 제품이 자기 구현을 꽂습니다.

```text
   oxi-sdk  (defines 15 port traits)
      │
      │ implements
      ▼
   Product layer
   ├── oxi-cli        → oxi-sdk::ports::fs  (file-based)
   ├── oxios-kernel   → 자체 StateStore, EventBus, MemoryStore
   ├── oxios-mobile   → in-memory
   └── 새 시스템        → 자유 구현
```

## 15개 port 빠른 참조

| port | 책임 | oxi-cli가 사용? | oxios가 사용? |
|---|---|:-:|:-:|
| `StateStore` | 영속 key-value / append-only | ✅ (file-based) | ✅ (자체) |
| `ConfigStore` | 레이어드 설정 | ✅ (file-based) | ✅ (자체) |
| `AuthProvider` | API key + OAuth | ✅ (file-based) | ✅ (자체) |
| `EventBus` | pub/sub | ❌ (noop) | ✅ (자체) |
| `SkillLoader` | SKILL.md 발견·로드 | ✅ (file-based) | ✅ (자체) |
| `PersonaProvider` | 시스템 프롬프트 주입 | ❌ (noop) | ✅ (자체) |
| `AccessGate` | 도구 실행 전 정책 검사 | ❌ (noop) | ✅ (자체) |
| `CapabilityResolver` | subject → visible tools | ❌ (noop) | ✅ (자체) |
| `MemoryStore` | episodic/semantic memory | ❌ (noop) | ✅ (자체) |
| `CronScheduler` | 시간 기반 트리거 | ❌ (noop) | ✅ (자체) |
| `ResourceMonitor` | 사용량 모니터링 | ❌ (noop) | ✅ (자체) |

15개 모두 **optional**. 등록 안 하면 SDK가 noop fallback을 사용합니다.

## 기본 사용 패턴 (oxi-cli)

```rust
use std::sync::Arc;
use oxi_sdk::OxiBuilder;
use oxi_sdk::ports::fs::{FileStateStore, FileAuthProvider, FileConfigStore, FileSkillLoader};

let home = std::env::var("OXI_HOME")
    .unwrap_or_else(|_| format!("{}/.oxi", std::env::var("HOME").unwrap()));

let oxi = OxiBuilder::new()
    .with_builtins()
    .with_state(Arc::new(FileStateStore::new(format!("{home}/sessions"))))
    .with_auth(Arc::new(FileAuthProvider::new(format!("{home}/auth.json"))))
    .with_config(Arc::new(FileConfigStore::new(format!("{home}/settings.toml"))))
    .with_skills(Arc::new(FileSkillLoader::single(format!("{home}/skills"))))
    .build();

// 이후 어디서든:
let ports = oxi.ports();
let providers = ports.auth.list_providers().await?;
let keys = ports.config.list()?;
let skills = ports.skills.list().await?;
```

## 새 시스템이 port를 구현하는 패턴 (예: S3 백엔드)

```rust
use async_trait::async_trait;
use oxi_sdk::ports::{StateStore, PortId, PortValue};
use oxi_sdk::SdkError;

pub struct S3StateStore {
    client: aws_sdk_s3::Client,
    bucket: String,
}

#[async_trait]
impl StateStore for S3StateStore {
    async fn append(&self, entry: PortValue) -> Result<PortId, SdkError> {
        let id = uuid::Uuid::new_v4().to_string();
        let body = serde_json::to_vec(&entry).map_err(|e| SdkError::Internal(e.into()))?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(format!("entries/{id}.json"))
            .body(body.into())
            .send()
            .await
            .map_err(|e| SdkError::Internal(anyhow::anyhow!(e)))?;
        Ok(id)
    }

    async fn load(&self, id: &PortId) -> Result<Option<PortValue>, SdkError> {
        // S3 GET → serde_json::from_slice
        todo!()
    }

    async fn list(&self, prefix: &str) -> Result<Vec<PortId>, SdkError> {
        // S3 ListObjectsV2
        todo!()
    }

    async fn delete(&self, id: &PortId) -> Result<(), SdkError> {
        // S3 DeleteObject
        todo!()
    }
}

// 그리고 oxi-cli가 wire하듯:
let oxi = OxiBuilder::new()
    .with_builtins()
    .with_state(Arc::new(S3StateStore { client, bucket: "oxi-prod".into() }))
    .build();
```

**SDK는 한 줄도 바뀌지 않습니다.** 새 백엔드를 추가해도 모든 기존 사용자가 영향받지 않습니다.

## PortRegistry로 한 번에 등록

15개를 일일이 `with_*`로 호출하는 게 번거로우면 `PortRegistry`를 통째로 등록:

```rust
use oxi_sdk::{OxiBuilder, PortRegistry};

let mut ports = PortRegistry::noop();
ports.state = Arc::new(MyStateStore::new());
ports.auth = Arc::new(MyAuthProvider::new());

let oxi = OxiBuilder::new()
    .with_builtins()
    .with_ports(ports)
    .build();
```

## 어떤 port를 등록할지 결정하는 가이드

```
질문 1: 에이전트 출력을 나중에 보고 싶나?
  예 → StateStore 등록

질문 2: 사용자가 모델을 바꾸길 원하나 (config-driven)?
  예 → ConfigStore 등록

질문 3: API key를 안전하게 저장하고 관리해야 하나?
  예 → AuthProvider 등록

질문 4: 여러 agent/UI가 이벤트를 주고받아야 하나?
  예 → EventBus 등록

질문 5: 외부 skill 파일을 동적으로 로드하나?
  예 → SkillLoader 등록

질문 6: 시스템 프롬프트를 persona별로 다르게 주입하나?
  예 → PersonaProvider 등록

질문 7: 도구 실행 전 권한 검사가 필요한가?
  예 → AccessGate 등록

질문 8: agent마다 보이는 tool을 제한하나?
  예 → CapabilityResolver 등록

질문 9: episodic/semantic 메모리를 영속화하나?
  예 → MemoryStore 등록

질문 10: cron 같은 시간 트리거가 필요한가?
  예 → CronScheduler 등록

질문 11: 리소스 사용량에 따라 agent를 throttle하나?
  예 → ResourceMonitor 등록
```

## PortValue — 왜 JSON인가?

port trait의 시그니처가 `serde_json::Value`를 받는 이유:

```rust
async fn append(&self, entry: PortValue) -> Result<PortId, SdkError>;
```

이유:
1. **타입 독립성** — 각 제품이 자기 도메인 타입을 (de)serialize
2. **Schema evolution** — 새 필드 추가 시 기존 entry와 호환
3. **Cross-crate 호환** — `oxi_sdk::Message` 같은 구체 타입을 port가 의존하지 않음

단점:
- 컴파일 타임 타입 안전성 감소
- 잘못된 키 사용 시 런타임 에러

타협: port 자체는 `Value`로 다루고, **도메인 레이어**에서 typed 변환. oxi-cli의 services layer가 `serde_json::from_value::<SessionEntry>(value)?` 식으로 변환.

## Noop fallback 동작

등록 안 된 port는 SDK가 noop impl을 사용합니다. 이게 안전한 이유는 port별 설계:

| port | noop 동작 | 안전? |
|---|---|:-:|
| `StateStore` | `load` → None, `append` → 에러 | ✅ (읽기 OK) |
| `ConfigStore` | `get` → None, `set` → OK (메모리) | ✅ |
| `AuthProvider` | `get` → None, `set` → 에러 | ✅ (키 없으면 env var fallback) |
| `EventBus` | publish OK, subscribers receive nothing | ✅ |
| `SkillLoader` | empty | ✅ |
| `PersonaProvider` | empty | ✅ |
| `AccessGate` | `AllowAllAccessGate` (모두 허용) | ⚠️ 운영 환경에선 명시적 게이트 권장 |
| `CapabilityResolver` | empty list | ✅ (tool 안 보임) |
| `MemoryStore` | empty | ✅ |
| `CronScheduler` | empty | ✅ |
| `ResourceMonitor` | zero usage | ✅ |

`AccessGate`의 noop이 `AllowAll`이라는 점에 주의 — **운영 환경에선 반드시 명시적 게이트를 등록**해야 합니다.

## 마이그레이션 가이드 (oxi-cli 기준)

### 단계 1: 새 진입 경로 추가 (이미 완료)
```rust
// oxi-cli/src/services.rs
pub fn build_oxi(paths: &OxiPaths) -> Result<Oxi> { ... }
```

### 단계 2: main.rs에서 새 경로를 부르는 CLI 추가
- `--check` 플래그: `build_oxi()`로 wiring 검증
- 새 subcommand (예: `oxi run-official <prompt>`): 새 entry point
- 기존 `App::new()` 기반 TUI는 그대로

### 단계 3: App::new의 wiring을 services::build_oxi 결과로 대체
- App이 `Oxi`를 보관
- App이 `oxi.ports().auth`로 API key 조회
- 직접 `oxi_agent::Agent` 생성 대신 `oxi.agent(config).build()?`

### 단계 4: legacy `App` 제거
- 이 단계는 oxios/oxi-cli 둘 다 안정화 후 진행
- 한 번에 하지 말고 한 모드씩 (TUI → print → RPC 순)

## oxios-kernel에 port를 도입하는 가이드

oxios는 현재 자기 구현을 직접 사용 중 (state_store.rs 793줄, access_manager 5,111줄, memory 12,277줄). 이를 port trait impl로 감싸는 작업:

```rust
// oxios-kernel/src/state_store.rs → impl oxi_sdk::ports::StateStore for OxiosStateStore
// oxios-kernel/src/event_bus.rs → impl oxi_sdk::ports::EventBus for KernelEventBus
// ... 등등
```

**장점**:
- 새 SDK port가 추가될 때 자동 활용 가능
- 테스트에서 in-memory impl로 교체 쉬움
- 다른 Rust 프로젝트가 oxios의 인프라를 재사용 가능

**주의**:
- 50K+ 라인 중 일부만 감싸도 의미 있음 (전부 한 번에 X)
- 기존 API와 port API의 시그니처 차이 (`Value` vs typed)는 어댑터 함수로 해결
- 한 port 당 1 PR 권장 (큰 변경 방지)

## 흔한 실수

### ❌ port에 도메인 타입 강제
```rust
// 잘못된 예
#[async_trait]
pub trait StateStore {
    async fn append(&self, entry: oxi_sdk::Message) -> ...;  // SDK 타입 강제
}

// 올바른 예
#[async_trait]
pub trait StateStore {
    async fn append(&self, entry: PortValue) -> ...;  // Value로 추상화
}
```

### ❌ port에 sync/async 혼용
- 모든 port는 `async_trait` (async)
- 동기 IO가 필요하면 `tokio::task::spawn_blocking` 안에서
- port trait 자체에 sync variant 추가하지 말 것

### ❌ port impl이 SDK 구체 타입 import
```rust
// 잘못된 예 (impl이 oxi_sdk::Message를 import)
use oxi_sdk::Message;

// 올바른 예
use oxi_sdk::ports::PortValue;  // JSON Value
```

## 디렉토리 레이아웃 (oxi-sdk::ports::fs 기준)

```
~/.oxi/
├── auth.json         — FileAuthProvider (API keys + OAuth tokens)
├── settings.toml     — FileConfigStore (dotted-key nested)
├── sessions/         — FileStateStore (JSON append, one file per entry)
│   ├── <uuid>.json
│   └── ...
├── skills/           — FileSkillLoader
│   ├── git-commit/SKILL.md
│   ├── review/SKILL.md
│   └── ...
└── cache/            — (reserved for future ephemeral state)
```

환경변수 fallback (`FileAuthProvider::resolve_api_key`):
1. `auth.json`의 `providers[name].api_key`
2. `OXI_API_KEY_<UPPER>` (예: `OXI_API_KEY_ANTHROPIC`)
3. 표준 env var (7개 provider에 대해):
   - `ANTHROPIC_API_KEY`
   - `OPENAI_API_KEY`
   - `GOOGLE_API_KEY`
   - `DEEPSEEK_API_KEY`
   - (나머지 provider는 1-2번 경로만)

## 참고 파일

- `oxi-sdk/src/ports.rs` — port trait 정의
- `oxi-sdk/src/builder.rs` — `OxiBuilder::with_port_*` 메서드
- `oxi-sdk/src/ports/fs/` — file-based 구현
- `oxi-cli/src/services.rs` — composition root 예제
- `oxi-sdk/src/error.rs` — `PortNotConfigured` variant
