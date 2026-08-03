# oxicode 크레이트 재설계

> 버전: 2026-05-14
> 상태: ✅ 완료

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
oxicode-ai       28K                  oxicode-ai       28K
oxicode-agent    17K                  oxicode-agent    17K
oxicode-tui       4K                  oxicode-tui       4K
oxicode-cli      55K                  oxicode-store    12K  ← 신규 추출
                                  oxicode-cli      73K  ← 모듈 정리 완료
─────────────────────             ─────────────────────
총 104K / 4개                     총 134K / 5개
```

**참고**: oxicode-cli가 73K로 증가한 것은 모듈 디렉토리 구조화가 완료되어 파일 집합이
정확히 반영된 것임. 핵심 모듈(session, settings, auth 등 약 12K)이 oxicode-store로 분리됨.

## 3. 의존성 DAG

```
oxicode-ai
  ↑
oxicode-store   ← session, settings, auth, model_registry
  ↑
oxicode-agent   oxicode-tui
  ↑
  └──→ oxicode-cli (binary)
```

순환 없음. 각 화살표는 아래 크레이트가 위 크레이트에 의존함을 뜻한다.

## 4. 각 크레이트 정의

### 4.1 `oxicode-ai` — LLM API 추상화 (변경 없음)

- **역할**: 다중 프로바이더 LLM 스트리밍 API
- **의존**: 외부 크레이트만 (tokio, reqwest, serde, ...)
- **소비자**: oxicode-agent, oxicode-store, oxicode-cli
- **파일**: 40개

### 4.2 `oxicode-agent` — 에이전트 런프 + 도구 (변경 없음)

- **역할**: 도구 호출 루프, MCP 프로토콜, 내장 도구 16개
- **의존**: oxicode-ai
- **소비자**: oxicode-cli
- **파일**: 47개

### 4.3 `oxicode-tui` — 순수 TUI 위젯 (변경 없음)

- **역할**: 비즈니스 로직 없는 순수 UI 컴포넌트
- **의존**: ratatui, crossterm (oxicode-ai, oxicode-agent 의존 없음)
- **소비자**: oxicode-cli
- **파일**: 10개

### 4.4 `oxicode-store` — 공유 영속 상태 (신규)

```
oxicode-store/
├── src/
│   ├── lib.rs
│   ├── session.rs            # SessionEntry, SessionManager, 직렬화 (3K+)
│   ├── session_navigation.rs # 트리 탐색, 브랜치
│   ├── session_cwd.rs        # 세션 작업 디렉토리
│   ├── settings.rs           # Settings 스키마, 로드/저장/병합
│   ├── settings_validation.rs
│   ├── model_registry.rs     # 모델 발견/등록 (oxicode-ai model_db 래핑)
│   ├── model_resolver.rs     # model_id → (provider, model) 매핑
│   ├── auth_storage.rs       # API 키 영속 저장
│   └── auth_guidance.rs      # 인증 안내 텍스트
└── Cargo.toml
```

- **역할**: 디스크에서 읽고 디스크에 쓰는 공유 상태
- **의존**: oxicode-ai (model_db, Model 타입만)
- **소비자**: oxicode-cli
- **파일**: 10개

### 4.5 `oxicode-cli` — 진입점 + 애플리케이션 (내부 재정리)

```
oxicode-cli/src/
├── main.rs                  # 바이너리 진입점
├── lib.rs                   # App, InteractiveSession, CompactionContext
├── cli.rs                   # clap 정의
│
├── app/                     # 애플리케이션 코어
│   ├── mod.rs
│   ├── agent_session.rs     # 세션 라이프사이클
│   └── agent_session_runtime.rs
│
├── extensions/              # 확장 시스템
├── skills/                  # 스킬 매니저
├── tui/                     # TUI 화면
│   ├── app.rs
│   ├── handlers.rs
│   ├── render.rs
│   ├── slash.rs
│   ├── welcome.rs
│   └── overlay/
│
├── rpc_mode/                # RPC 서버 모드
│
├── storage/                 # 패키지/리소스/내보내기
│   ├── packages.rs
│   ├── resource_loader.rs
│   ├── resource_loader_compat.rs
│   └── export.rs
│
├── context/                 # 컨텍스트 관리
│   ├── compaction_utils.rs
│   ├── auto_compaction.rs
│   └── branch_summarization.rs
│
├── prompt/                  # 프롬프트 구성
│   ├── system_prompt.rs
│   ├── frontmatter.rs
│   └── templates.rs + templates/
│
├── media/                   # 이미지/파일 처리
│   ├── image_convert.rs
│   ├── image_resize.rs
│   ├── exif_orientation.rs
│   ├── file_processor.rs
│   ├── clipboard_image.rs
│   ├── clipboard_write.rs
│   └── mime_detect.rs
│
├── infra/                   # 인프라
│   ├── bash_executor.rs
│   ├── child_process.rs
│   ├── error_recovery.rs
│   ├── event_bus.rs
│   ├── output_guard.rs
│   ├── tools_manager.rs
│   ├── version_check.rs
│   ├── diagnostics.rs
│   ├── fs_watch.rs
│   └── shutdown.rs
│
├── ui/                      # UI 유틸
│   ├── keybindings.rs
│   ├── theme.rs
│   ├── footer_data.rs
│   ├── timings.rs
│   └── changelog.rs
│
└── util/                    # 잡유틸
    ├── git_utils.rs
    ├── paths.rs
    ├── source_info.rs
    ├── pi_user_agent.rs
    ├── provider_display_names.rs
    ├── messages.rs
    ├── tmux_detect.rs
    ├── telemetry.rs
    ├── defaults.rs
    ├── slash_commands.rs
    └── sleep.rs
```

- **의존**: oxicode-ai, oxicode-agent, oxicode-store, oxicode-tui
- **파일**: 131개 (모듈 디렉토리 13개)

## 5. 마이그레이션 기록

### Phase 1: 내부 모듈 정리 (완료)
- 12개 디렉토리 생성: app/, context/, extensions/, infra/, media/, prompt/, rpc_mode/, skills/, storage/, tui/, ui/, util/
- 파일 이동 + import 경로 업데이트
- templates/ → prompt/templates/ 이동

### Phase 2: oxicode-store 추출 (완료)
- oxicode-store 크레이트 생성
- 9개 모듈 이동: session, session_navigation, session_cwd, settings, settings_validation, auth_storage, auth_guidance, model_registry, model_resolver
- oxicode-cli의 모든 crate::session/settings/auth_storage/model_registry → oxicode_store:: 로 변경
- oxicode-cli Cargo.toml에 oxicode-store 의존성 추가

## 6. 하지 않은 것

| 항목 | 이유 |
|---|---|
| `oxicode-app` 분리 | 소비자가 oxicode-cli뿐 — 나중에 임베드/RPC 독립 배포 필요하면 그때 |
| `oxicode-util` 분리 | 모듈 30개 중 25개가 소비자 0~1개 — 크레이트 관리 비용이 이점보다 큼 |
| 기존 크레이트 리네임 | oxicode-ai, oxicode-agent, oxicode-tui 모두 명확하고 Rust 관례에 부합 |
| root-level 파일 정리 | Phase 1에서 일부만 처리됨. 다음에 정리 가능 |

## 7. 다음 작업 (선택)

1. **root-level 파일 정리**: agent_session.rs, agent_session_runtime.rs, cli.rs 등 root에 남은 파일들을 적절한 디렉토리로 이동
2. **workspace dependencies 정리**: 공통 의존성을 workspace 수준으로 끌어올리기
3. **cargo test --workspace**: 전체 테스트 실행 및 경고 해결
4. **cargo clippy --workspace**: 코드 품질 점검