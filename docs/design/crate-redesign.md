# oxi 크레이트 재설계

> 버전: 2026-05-14
> 상태: 승인 대기

## 1. 설계 원칙

| 원칙 | 설명 |
|---|---|
| **소비자 기준** | 크레이트는 "누가 쓰는가"로 나눈다. 공유되지 않는 것은 분리하지 않는다 |
| **의존성 DAG** | 순환 없이 아래에서 위로만 흐른다 |
| **최소 분리** | 관리 비용(버전 동기화, pub API 설계)을 정당화할 때만 크레이트를 만든다 |
| **모듈로 정리** | 크레이트 분리가 필요 없으면 모듈 디렉토리로 탐색성을 개선한다 |

## 2. 전후 비교

```
전 (4개)                          후 (5개)
─────────────────────             ─────────────────────
oxi-ai       28K                  oxi-ai       28K  (변경 없음)
oxi-agent    17K                  oxi-agent    17K  (변경 없음)
oxi-tui       4K                  oxi-tui       4K  (변경 없음)
oxi-cli      55K                  oxi-store     8K  (신규 추출)
                                  oxi-cli      37K  (내부 재정리)
─────────────────────             ─────────────────────
총 104K / 4개                     총 94K / 5개
```

## 3. 의존성 DAG

```
oxi-ai
  ↑
oxi-store
  ↑       ↘
oxi-agent   oxi-tui
  ↑       ↗     ↑
  oxi-cli ──────┘
```

순환 없음. 각 화살표는 아래 크레이트가 위 크레이트에 의존함을 뜻한다.

## 4. 각 크레이트 정의

### 4.1 `oxi-ai` — LLM API 추상화 (변경 없음)

```
oxi-ai/
├── src/
│   ├── lib.rs
│   ├── types.rs              # Message, ContentBlock, ToolCall, ...
│   ├── messages.rs
│   ├── context.rs
│   ├── compaction.rs
│   ├── secret.rs
│   ├── oauth.rs
│   ├── env_api_keys.rs
│   ├── error.rs
│   ├── high_level.rs
│   ├── model_db.rs           # 정적 모델 카탈로그
│   ├── model_registry.rs     # 런타임 모델 등록
│   ├── provider_registry.rs  # 프로바이더 등록
│   ├── tools.rs
│   ├── transform.rs
│   ├── utils/
│   └── providers/
│       ├── anthropic.rs
│       ├── openai.rs
│       ├── google.rs
│       └── ... (14개 프로바이더)
└── Cargo.toml
```

- **역할**: 다중 프로바이더 LLM 스트리밍 API
- **의존**: 외부 크레이트만 (tokio, reqwest, serde, ...)
- **소비자**: oxi-agent, oxi-store, oxi-cli
- **라인 수**: ~28K

### 4.2 `oxi-agent` — 에이전트 런프 + 도구 (변경 없음)

```
oxi-agent/
├── src/
│   ├── lib.rs
│   ├── agent.rs              # Agent 구조체
│   ├── agent_loop/           # 메인 루프
│   ├── config.rs             # AgentConfig
│   ├── state.rs              # AgentState
│   ├── events.rs             # AgentEvent
│   ├── error.rs
│   ├── recovery.rs
│   ├── proxy.rs
│   ├── stream_retry.rs
│   ├── compaction.rs
│   ├── model_id.rs
│   ├── types.rs
│   ├── mcp/                  # MCP 클라이언트
│   └── tools/                # 내장 도구 16개
│       ├── bash.rs
│       ├── read.rs
│       ├── edit.rs
│       ├── write.rs
│       └── ...
└── Cargo.toml
```

- **역할**: 도구 호출 루프, MCP 프로토콜, 내장 도구
- **의존**: oxi-ai
- **소비자**: oxi-cli
- **라인 수**: ~17K

### 4.3 `oxi-tui` — 순수 TUI 위젯 (변경 없음)

```
oxi-tui/
├── src/
│   ├── lib.rs
│   ├── theme.rs              # Theme, ThemeStyles, ColorScheme
│   ├── cell.rs
│   ├── fuzzy.rs              # fuzzy 매칭
│   ├── table_renderer.rs
│   └── widgets/
│       ├── chat.rs           # 채팅 메시지 위젯
│       ├── input.rs          # 입력 위젯
│       ├── footer.rs         # 푸터 위젯
│       ├── tool_renderer.rs  # 도구 결과 렌더링
│       └── mod.rs
└── Cargo.toml
```

- **역할**: 비즈니스 로직 없는 순수 UI 컴포넌트
- **의존**: ratatui, crossterm (oxi-ai, oxi-agent 의존 없음)
- **소비자**: oxi-cli
- **라인 수**: ~4K

### 4.4 `oxi-store` — 공유 영속 상태 (신규)

```
oxi-store/
├── src/
│   ├── lib.rs
│   ├── session.rs            # SessionEntry, SessionManager, 직렬화
│   ├── session_navigation.rs # 트리 탐색, 브랜치
│   ├── session_cwd.rs        # 세션 작업 디렉토리
│   ├── settings.rs           # Settings 스키마, 로드/저장/병합
│   ├── settings_validation.rs
│   ├── model_registry.rs     # 모델 발견/등록 (oxi-ai model_db 래핑)
│   ├── model_resolver.rs     # model_id → (provider, model) 매핑
│   ├── auth_storage.rs       # API 키 영속 저장
│   └── auth_guidance.rs      # 인증 안내 텍스트
└── Cargo.toml
```

- **역할**: 디스크에서 읽고 디스크에 쓰는 공유 상태
- **의존**: oxi-ai (model_db, Model 타입만)
- **소비자**: oxi-cli (agent_session, tui, main, setup_wizard에서 참조)
- **라인 수**: ~8K

#### 추출 근거

이 모듈들은 현재 oxi-cli 내부에서 **5개 이상의 소비자**가 참조합니다:

```
session.rs       → agent_session, compaction_utils, branch_summarization, export, tui/app
settings.rs      → agent_session, extensions, setup_wizard, tui, main, model_resolver
auth_storage.rs  → model_registry, agent_session_runtime, tui/app, main, setup_wizard
model_registry   → agent_session_runtime, main
model_resolver   → main, setup_wizard
```

oxi-cli에 있으면 `crate::`로만 접근 가능하여, agent_session과 tui가
같은 크레이트에 있어야 하는 강제 결합이 발생합니다.
oxi-store로 분리하면 이 결합이 풀립니다.

#### Cargo.toml

```toml
[package]
name = "oxi-store"
version = "0.11.0"
edition = "2021"
description = "Shared persistent state for oxi — sessions, settings, auth, model registry"

[dependencies]
oxi-ai = { version = "0.11.0", path = "../oxi-ai" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
thiserror = { workspace = true }
dirs = "5"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
parking_lot = "0.12"
```

### 4.5 `oxi-cli` — 진입점 + 애플리케이션 (내부 재정리)

```
oxi-cli/
├── src/
│   ├── main.rs                          # 바이너리 진입점
│   ├── lib.rs                           # App, InteractiveSession, CompactionContext
│   ├── cli.rs                           # clap 정의
│   │
│   ├── app/                             # 애플리케이션 코어
│   │   ├── mod.rs
│   │   ├── agent_session.rs             # 세션 라이프사이클
│   │   └── agent_session_runtime.rs     # 스트리밍, 재시도, 확장 훅
│   │
│   ├── extensions/                      # 확장 시스템 (현행 유지)
│   │   ├── mod.rs
│   │   ├── context.rs
│   │   ├── loading.rs
│   │   ├── registry.rs
│   │   ├── types.rs
│   │   ├── wasm.rs
│   │   ├── wasm_hooks.rs
│   │   ├── wasm_tool.rs
│   │   └── ext_cli.rs
│   │
│   ├── tui/                             # TUI 화면 (현행 유지)
│   │   ├── mod.rs
│   │   ├── app.rs
│   │   ├── handlers.rs
│   │   ├── render.rs
│   │   ├── slash.rs
│   │   └── welcome.rs
│   │
│   ├── rpc_mode/                        # RPC 서버 모드 (현행 유지)
│   │   ├── mod.rs
│   │   ├── protocol.rs
│   │   ├── state.rs
│   │   ├── handlers.rs
│   │   ├── utils.rs
│   │   └── tests.rs
│   │
│   ├── storage/                         # 패키지/리소스 관리
│   │   ├── mod.rs
│   │   ├── packages.rs
│   │   ├── resource_loader.rs
│   │   ├── resource_loader_compat.rs
│   │   └── export.rs
│   │
│   ├── media/                           # 이미지/파일 처리
│   │   ├── mod.rs
│   │   ├── image_convert.rs
│   │   ├── image_resize.rs
│   │   ├── exif_orientation.rs
│   │   ├── file_processor.rs
│   │   ├── clipboard_image.rs
│   │   ├── clipboard_write.rs
│   │   └── mime_detect.rs
│   │
│   ├── infra/                           # 인프라
│   │   ├── mod.rs
│   │   ├── bash_executor.rs
│   │   ├── child_process.rs
│   │   ├── error_recovery.rs
│   │   ├── event_bus.rs
│   │   ├── output_guard.rs
│   │   ├── tools_manager.rs
│   │   ├── version_check.rs
│   │   ├── diagnostics.rs
│   │   ├── fs_watch.rs
│   │   ├── shutdown.rs
│   │   └── sleep.rs
│   │
│   ├── ui/                              # UI 유틸
│   │   ├── mod.rs
│   │   ├── keybindings.rs
│   │   ├── theme.rs
│   │   ├── footer_data.rs
│   │   ├── timings.rs
│   │   ├── changelog.rs
│   │   └── setup_wizard.rs
│   │
│   ├── prompt/                          # 프롬프트 구성
│   │   ├── mod.rs
│   │   ├── system_prompt.rs
│   │   ├── frontmatter.rs
│   │   └── templates.rs
│   │
│   ├── context/                         # 컨텍스트 관리
│   │   ├── mod.rs
│   │   ├── auto_compaction.rs
│   │   ├── compaction_utils.rs
│   │   └── branch_summarization.rs
│   │
│   └── util/                            # 잡유틸
│       ├── mod.rs
│       ├── git_utils.rs
│       ├── paths.rs
│       ├── source_info.rs
│       ├── pi_user_agent.rs
│       ├── provider_display_names.rs
│       ├── messages.rs
│       ├── tmux_detect.rs
│       ├── telemetry.rs
│       ├── defaults.rs
│       ├── slash_commands.rs
│       ├── session_cwd.rs → (oxi-store 참조)
│       ├── skills/
│       └── print_mode.rs
│
└── Cargo.toml
```

- **역할**: 모든 것의 조립 + CLI/TUI/RPC 진입점
- **의존**: oxi-ai, oxi-agent, oxi-store, oxi-tui
- **라인 수**: ~37K

## 5. 버전 관리

모든 크레이트가 같은 버전을 유지한다:

```toml
# workspace Cargo.toml
[workspace]
resolver = "2"
members = ["oxi-ai", "oxi-agent", "oxi-store", "oxi-tui", "oxi-cli"]

[workspace.dependencies]
thiserror = "2"
# 공통 의존성을 점진적으로 이곳으로 끌어올린다
```

## 6. 마이그레이션 계획

단계별로 **항상 컴파일 가능한 상태**를 유지한다.

### Phase 1: 내부 모듈 정리 (oxi-cli만, 영향 최소)

```
1. oxi-cli/src/ 아래에 디렉토리 생성 (app/, storage/, media/, ...)
2. 파일들을 디렉토리로 이동
3. mod.rs 작성
4. cargo check 통과 확인
```

예상 시간: 1~2시간
위험도: 낮음 (단순 파일 이동 + mod 선언)

### Phase 2: oxi-store 추출

```
1. oxi-store 크레이트 생성 (빈 껍데기)
2. session.rs → oxi-store로 이동
3. oxi-cli에서 crate::session → oxi_store::session 으로 변경
4. cargo check
5. settings.rs → oxi-store로 이동
6. 동일하게 import 변경
7. cargo check
8. auth_storage.rs, model_registry.rs, model_resolver.rs 순차 이동
9. 각 단계마다 cargo check
10. 전체 cargo test
```

예상 시간: 2~3시간
위험도: 중간 (import 경로 전면 변경, 하지만 기계적 작업)

### Phase 3: 검증

```
1. cargo test --workspace
2. cargo clippy --workspace
3. cargo doc --workspace (공개 API 확인)
4. 수동 smoke test (oxi 실행 → TUI, 단일 프롬프트, RPC 모드)
```

### Phase 4 (선택): workspace dependencies 정리

```toml
# 공통 의존성을 workspace 수준으로 끌어올리기
[workspace.dependencies]
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
tokio = { version = "1", features = ["full"] }
parking_lot = "0.12"
dirs = "5"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
tracing = "0.1"
```

## 7. 하지 않는 것

| 항목 | 이유 |
|---|---|
| `oxi-app` 분리 | 소비자가 oxi-cli뿐 — 나중에 임베드/RPC 독립 배포 필요하면 그때 |
| `oxi-util` 분리 | 모듈 30개 중 25개가 소비자 0~1개 — 크레이트 관리 비용이 이점보다 큼 |
| 기존 크레이트 리네임 | oxi-ai, oxi-agent, oxi-tui 모두 명확하고 Rust 관례에 부합 |
| oxi-store에 packages 포함 | 소비자가 main.rs뿐 — 불필요한 의존만 추가됨 |
