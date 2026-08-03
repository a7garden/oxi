# grok-build 비교분석 — `xai-org/grok-build`

**`Port partially`** — 5개 영역에서 가치 있으나, omp(mnemopi/advisor) 포팅 직후라 이식 우선순위는 낮음. **`oxicode-mnemopi dream`, `oxicode-ai compaction trait seams`, `oxicode-workspace checkpoint` 셋이 향후 분기에서 가장 큰 영향**.

---

## 요약

`grok` (binary: `xai-grok-pager`)은 xAI의 SpaceXAI 사내용 터미널 코딩 에이전트로, Rust 2024 edition의 **82-crate** 워크스페이스로 빌드된다. 핵심 구조는 `crates/codegen/*` (제품 코어, ~60 크레이트) + `crates/common/*` (공용 인프라, ~11 크레이트) + `third_party/*` (Mermaid 스택 등 vendored)이며, Apache-2.0 라이선스. 단일 바이너리 `grok`이 TUI/print/RPC/ACP 네 가지 진입점을 제공한다.

본질적으로 **oxicode의 6-crate 워크스페이스(`oxicode-ai`/`oxicode-agent`/`oxicode-sdk`/`oxicode-tui`/`oxicode-cli`/`oxicode-hashline` + `oxicode-mnemopi`)와 동일한 도메인**(LLM 에이전트, 툴, TUI, 메모리, 워크스페이스)이며, 같은 MentalModel을 채택하고 있다(스트리밍-우선, 트레이트-주도, 옵트인 어댑터). 다만 **유스케이스 스케일이 다르고**, **도메인 로직을 단일 fat-crate(`xai-grok-workspace`, 398KB `handle.rs` + 255KB `permission/manager.rs`)에 모으는 것 vs oxicode의 멀티크레이트 분리**라는 **분해 철학의 차이가 가장 두드러진다**.

핵심 가치: grok는 oxicode가 아직 갖지 못한 **5가지 분명한 차별점**을 가진다. (1) LLM이 세션 로그를 자율적으로 합치는 **`dream` 합치기**, (2) FS/git/hunk-tracker를 묶는 **turn-boundary `RewindCheckpoint`**, (3) **`{slug}-{hash8}` 워크스페이스 + arc_swap 락프리 watcher**, (4) **`intra`/`inter`/`code` 3-스타일 compaction 트레이트 seam**, (5) **`PermissionManager` 자동모드 (100KB)** + 256KB 해상도 엔진.

---

## oxicode 현재 상태

| 영역 | oxicode | grok | 비고 |
|---|---|---|---|
| 크레이트 수 | 7 | 82 | grok는 "제품 코어 + 공용 + vendored" 3-tier |
| 라이선스 | MIT | Apache-2.0 | oxicode 호환 |
| Provider 추상화 | `oxicode-ai::providers/trait_def.rs` (8개 provider, 1455 LOC 컴팩션) | 동등 | 거의 동급 |
| Tool 시스템 | `oxicode-agent::tools` (~21 도구, 33개 src 파일) | `xai-grok-tools` (38+ src) | oxicode가 압도적으로 컴팩트 |
| 메모리 엔진 | `oxicode-mnemopi` (13,857 LOC, 39 파일, **이미 omp 포팅 완료**) | `xai-grok-memory` (~3K+ LOC + `dream.rs` 53KB) | grok가 LLM-driven dream을 추가로 가짐 |
| MMR | `oxicode-mnemopi/src/mmr.rs` ✅ | `xai-grok-memory/src/mmr.rs` ✅ | 동급 (oxicode가 omp에서 포팅, grok는 자체) |
| Working→Episodic | `oxicode-mnemopi::consolidate.rs` (aaak 압축) | `xai-grok-memory::dream.rs` (LLM 합치기) | **다른 메커니즘** — grok는 LLM |
| Weibull decay | `oxicode-mnemopi::weibull.rs` ✅ | grok는 exponential half-life (`temporal_decay_multiplier`) | oxicode가 더 정교 |
| Polyphonic recall | `oxicode-mnemopi::polyphonic_recall.rs` (vec+graph+fact+temporal) | `xai-grok-memory::search.rs` (FTS5+KNN+temporal+source+MMR) | 동급 |
| Watcher / invalidation | ❌ 없음 | `xai-grok-memory/src/watcher.rs` (arc_swap + AtomicBool) | **grok만 가짐** |
| Scaffold-template 필터 | ❌ 없음 (`is_content_free` 없음) | `search.rs::is_content_free` + `dream::is_scaffold_template` | **grok만 가짐** |
| Compaction 스타일 | 단일 (`oxicode-ai/src/compaction.rs`, 1455 LOC) | 3스타일 (`code_compaction` + `intra_compaction` + `inter_compaction`) | grok가 분해가 정교, trait seams로 `CompactionItem`/`CompactionSampler` 노출 |
| Compaction trait seam | ❌ 없음 | ✅ (`CompactionItem`, `CompactionSampler`, `ItemTokenCounter`) | **grok만 가짐** |
| Turn-boundary FS checkpoint | ❌ 없음 (AgentSupervisor::suspend는 에이전트 상태만) | ✅ `xai-grok-workspace/src/session/checkpoint.rs` (1050 LOC) | **grok만 가짐**, FS+git+hunk 통합 |
| Worktree | ❌ 없음 | ✅ `xai-grok-workspace/src/session/worktree/` (112KB) | **grok만 가짐** |
| Permission manager | `oxicode-sdk::ports::AccessGate` (단순 룰기반, TOML allow/deny/approval) | `xai-grok-workspace/src/permission/` (13 파일, manager.rs 255KB, resolution.rs 161KB, auto_mode.rs 100KB) | grok가 압도적 — Claude Code 호환 `claude_settings.rs` 포함 |
| Hub protocol (multi-host) | ❌ 없음 | ✅ `hub.rs` 54KB + `hub_server.rs` 126KB + `hub_auth.rs` 17KB | **grok만 가짐** |
| TUI 라인 수 | `oxicode-tui` ~6K (37 src) | `xai-grok-pager` 38+ src | grok가 5x+ |
| TUI 컴팩션 디스플레이 | `oxicode-tui/src/widgets/chat/compaction/*` | `xai-grok-pager/src/compaction/` | 동급 |
| TUI 인터컴팩션 | ❌ | ✅ `inter_compaction.rs:10KB` + `intra_compaction.rs:57KB` | grok만 |
| 자동 워크스페이스 트러스트 | `oxicode-sdk/src/ports/fs/access.rs` (단순 룰) | `folder_trust.rs` 43KB + `trust.rs` 65KB | grok가 압도적 |
| 프롬프트 큐 wire 타입 | ❌ (단순 in-memory `steering_queue: RwLock<Vec<Message>>`) | `xai-prompt-queue/src/lib.rs` (wire 타입 `QueueChanged`/`QueueEntryMeta`/`QueueEntryWire`) | grok만 JSON-RPC 직렬화 정의 |
| 루트 Cargo.toml 자동생성 | ❌ (수동) | ✅ (`# Auto-generated workspace root. Prefer editing per-crate Cargo.toml files.`) | grok만 — discipline signal |
| Foreign 세션 임포트 | ❌ | ✅ `foreign_sessions/` (codex/claude 임포터 16KB) | grok만 |
| ACP (Agent Client Protocol) | ❌ | ✅ `xai-acp-lib` (`agent-client-protocol = 0.10.4`) | grok만 (IDE 통합) |

---

## 적용 후보

### 1. [high] LLM-driven `dream` 합치기를 `oxicode-mnemopi`에 추가

- **대상**: `oxicode-mnemopi/src/dream.rs` (신규 ~600 LOC) + `oxicode-mnemopi/src/lib.rs` (메서드 추가) + `oxicode-cli/src/services.rs` (`MnemopiDreamScheduler` 주기적 태스크)
- **근거**: 현재 oxicode-mnemopi의 `consolidate.rs`는 **결정론적** (aaak 압축 + group-by-source). grok의 `dream.rs:88-112`는 **"you are performing a dream — a reflective pass over memory files. Synthesize recent session logs into durable, well-organized memories"** 라는 시스템 프롬프트로 LLM이 여러 세션 로그를 **병합 + 모순 해결 + 상대일자→절대일자 + 이phemeral 디테일 폐기**한 단일 마크다운 문서를 만든다. oxicode의 사용자가 이 기능을 가지면 `/memory` 슬래시 없이도 능동적 합치기가 가능. omp에도 없는 진짜 신규 가치.
- **이식 표면**:
  - `crates/codegen/xai-grok-memory/src/dream.rs:1-123` — 3-게이트 (`DreamGate::Disabled`/`TooSoon`/`TooFewSessions`/`Open`/`Error`) + `DreamLock` (`dream_lock.rs`)
  - `DREAM_SYSTEM_PROMPT` (line 88-112) 그대로 이식 + Apache-2.0 attribution 헤더
  - `oxicode-mnemopi::Mnemopi`에 `pub fn dream(&self, model: Model) -> Result<DreamResult>` 추가, 내부적으로 LLM 호출 + 청크 인덱스 갱신
- **리스크**: LLM 호출 비용/지연. **게이트가 이를 제어** — `min_hours` + `min_sessions` 기본값 보수적 설정. 결정론적 `consolidate.rs`는 **유지**하고, `dream`은 별도 옵트인 (`Settings::mnemopi_dream_enabled` 기본 false)

### 2. [high] `oxicode-ai/src/compaction.rs`를 trait-seam 기반 3스타일 엔진으로 분해

- **대상**: `oxicode-ai/src/compaction.rs` (현재 1455 LOC 단일 파일) → `oxicode-ai/src/compaction/{item,sampler,token,prompt,select,code_compaction,intra_compaction,inter_compaction}.rs` (grok의 `xai-grok-compaction` 레이아웃 미러링, ~80% 스케일로 축소)
- **근거**: 현재 oxicode-ai 컴팩션은 `Compactor`/`LlmCompactor`/`CompactionManager`로 충분하지만, **모델 의존**(compaction model 교체)이 하드코딩이고, **다중 전략**(tail-keep vs full-replace vs streaming) 미지원. grok는 `CompactionItem` trait (line 56-97, `role`/`text`/`is_tool_result`/`has_tool_requests`/`is_compaction_summary`/`attachment_refs`) + `CompactionSampler` async-trait (line 110-136) + `ItemTokenCounter`로 host-agnostic 엔진 구현. oxicode에서 **OxI 메시지 타입을 `CompactionItem`에 그대로 impl**하면 grok의 알고리즘(`select.rs`, `assemble.rs`, `sample.rs`)을 거의 그대로 이식 가능.
- **이식 표면**:
  - `crates/common/xai-grok-compaction/src/item.rs:1-97` — `CompactionItem` trait (오버라이드 가능한 모든 메서드를 **required**로 — silent default는 회귀 위험)
  - `crates/common/xai-grok-compaction/src/sampler.rs:110-136` — `CompactionSampler` async trait
  - `oxicode-ai::Message`에 `impl CompactionItem for Message` 추가 (작업 ~50 LOC)
  - `CompactionItemFactory` (item.rs:99+, not object-safe) — full-replace용 carrier 생성
- **리스크**: 공개 API 변경. `oxicode-ai::compaction::LlmCompactor` 사용자가 있을 수 있음 (외부 consumer 없음 확인 필요). 마이그레이션 단계:
  1. 새 trait를 `oxicode-ai::compaction::seam` 모듈로 추가, 기존 `LlmCompactor`는 trait adapter로 유지
  2. `oxicode-agent::agent_loop`에서 새 seam 사용
  3. 기존 impl deprecated, 다음 메이저에서 제거

### 3. [high] `MemoryFileWatcher` — `arc_swap` 락프리 외부 편집 감지

- **대상**: `oxicode-mnemopi/src/watcher.rs` (신규 ~200 LOC) + `oxicode-mnemopi/src/lib.rs` (`Mnemopi`가 watcher 보유)
- **근거**: grok `crates/codegen/xai-grok-memory/src/watcher.rs:31-107` — `arc_swap::ArcSwap<HashSet<PathBuf>>` + `AtomicBool`로 **락 없이** dirty path 추적. 검색 경로에서 `is_dirty()` → `take_dirty()` → `MemoryIndex::reindex_file`/`delete_path`로 인덱스 동기화. **oxicode-mnemopi는 현재 외부 편집 감지 없음** — 사용자가 `$EDITOR ~/.oxicode/memory/MEMORY.md`로 편집해도 인덱스에 반영 안 됨. notify crate 이미 의존성에 있음 (oxicode Cargo.lock에 `notify v8`).
- **이식 표면**:
  - `crates/codegen/xai-grok-memory/src/watcher.rs` 패턴 그대로 (notify + arc_swap + AtomicBool)
  - `oxicode-mnemopi::Mnemopi::new`에 `notify::RecommendedWatcher` 등록
  - `Mnemopi::recall` 시작 시 `take_dirty()` 호출 → dirty 경로만 부분 재인덱스
  - `notify`는 `oxicode-mnemopi` Cargo.toml에 추가 (이미 다른 crate에서 사용 중이므로 workspace dep으로 추가만 하면 됨)
- **리스크**: 무한 루프 위험 (자기 자신이 쓴 파일에 다시 반응). 해결: dirty path set에 `&self.workspace_dir.join("sessions")`만 포함하도록 화이트리스트, 또는 마지막 쓰기 timestamp 기반 디바운스. 테스트: `notify_events_do_not_loopback` (자체 쓰기 → 재인덱스 0회)

### 4. [medium] `AccessGate`를 grok급 `PermissionManager`로 확장

- **대상**: `oxicode-sdk/src/ports/fs/access.rs` (현재 207 LOC, 6 함수) → `oxicode-workspace` (신규 크레이트 또는 `oxicode-sdk/src/ports/fs/permission/`)에 manager/resolution/rules/auto_mode 분리
- **근거**: oxicode `SimpleAccessGate`는 TOML allow/deny/approval만 지원. grok `xai-grok-workspace/src/permission/`은 **Claude Code `claude_settings.rs` 호환 (21KB)** + `auto_mode.rs` (100KB, 학습 기반 자동 승인) + `bash_command_splitting.rs` (45KB, 명령어 AST 파싱) + `hub_permission.rs` (20KB, 다중 호스트 허가). 가치: 사용자 trust 없이 자동 모드로 일하고 싶은 워크플로우 (sandboxed eval, CI-friendly)에서 결정적. **다만 oxicode의 "임베더블 엔진" 포지셔닝과 충돌 가능** — 기본값은 noop fallback 유지, opt-in.
- **이식 표면 (선별적)**:
  - `bash_command_splitting.rs` — oxicode의 `path_security.rs`에 통합 가능 (현재 5KB → 50KB 확장)
  - `claude_settings.rs` — `~/.claude/settings.json` 임포트 (oxicode 사용자 중 Claude Code 마이그레이션)
  - `auto_mode.rs` — 거절 (의사결정 위험 + 학습 데이터 부족으로 보류)
- **리스크**: grok의 `PermissionManager`는 xAI 내부 사용자에 최적화 (학습된 패턴). oxicode에 그대로 옮기면 결정성이 깨질 수 있음. **MVP는 `bash_command_splitting` + `claude_settings` 임포트만** 도입.

### 5. [medium] `{slug}-{hash8}` 워크스페이스 디렉토리 명명 + ephemeral CWD 감지

- **대상**: `oxicode-mnemopi/src/storage.rs` (현재 `bank`만 있음) — `path_layout.rs` 신규 (~100 LOC)
- **근거**: grok `crates/codegen/xai-grok-memory/src/storage.rs:55-74` — `MemoryStorage::new_inner`가 `{slug}-{hash8}` (예: `xai-a3f7b2c9`) 워크스페이스 디렉토리 사용. `is_ephemeral_cwd(cwd)`로 `/tmp/*` 등 임시 디렉토리 감지 → 자동 skip (영구화 방지). oxicode-mnemopi는 현재 `bank: String`만 가지고 디렉토리 명명 규칙이 모호.
- **이식 표면**:
  - `compute_workspace_hash(cwd: &Path) -> String` (blake3, 8 hex chars)
  - `is_ephemeral_cwd(cwd: &Path) -> bool` (`/tmp`, `*scratch*`, `cargo-target` 등 패턴 매칭)
  - `Mnemopi::bank_path()` 추가 — 기존 `bank`와 `cwd`로부터 경로 계산
- **리스크**: 기존 사용자 데이터 마이그레이션. 해결: 기존 `~/.oxicode/memory/<bank>.sqlite` 발견 시 그대로 사용 + 마이그레이션은 옵트인 (다음 메이저에서).

---

## 위험 / 검증

### 후보 1 (dream)
- **무엇이 깨질 수 있나**: LLM 호출이 메모리 API 쿼터를 소진. 거대한 dream 입력 (32K 토큰 초과 시 잘림). 동일 메모리에 대한 동시 dream → 중복 합치기.
- **최소 테스트**:
  - `test_dream_gate_disabled` — `config.enabled=false` → `DreamGate::Disabled`
  - `test_dream_gate_too_soon` — 마지막 합치기 후 <`min_hours` → `TooSoon`
  - `test_dream_lock_prevents_concurrent` — 두 스레드 동시 dream → 한쪽만 성공
  - `test_dream_consumes_only_eligible_sessions` — 현재 세션 ID 제외
- **`cargo clippy --workspace --all-targets -- -D warnings`** 가 잡나? ❌ 아니오 — LLM mock이 silent하게 통과시킬 수 있음. 통합 테스트 필수.
- **외부 의존**: LLM 모델. `oxicode-ai::Provider` 트레이트 시그니처 변경 없음 — `Provider::stream`을 그대로 호출.

### 후보 2 (compaction trait seam)
- **무엇이 깨질 수 있나**: `oxicode-ai::Message`에 `CompactionItem` impl 추가 시 `is_compaction_summary` 메서드의 기본값(`false`)이 silent하게 prior summary를 드롭 (grok item.rs:82-87가 명시한 함정). **required로 강제** 필요.
- **최소 테스트**:
  - `test_compaction_item_required_methods` — `Message`에 `impl CompactionItem` 작성 시 모든 메서드 빠지면 컴파일 에러
  - `test_sampler_retry_classification` — `CompactionSampleError::Build`/`Start`/`EmptyResponse`/`Other` 분류가 retry policy와 매치
  - `test_select_preserves_tool_pairing` — `Assistant(tool_request)` + `Tool` 결과가 분리되지 않음
- **`cargo clippy`** 가 잡나? 부분 — `dyn`-non-objective `CompactionItemFactory` 사용 시 warning. `#[allow]` 또는 generics만 사용.
- **외부 의존**: 없음. 순수 트레이트 추가.

### 후보 3 (watcher)
- **무엇이 깨질 수 있나**: 자기 자신이 쓴 파일에 watcher가 반응 → 무한 재인덱스. SIGBUS (네트워크 마운트에서 sqlite mmap) — grok는 `xai-sqlite-journal::JournalMode::for_db_path`로 해결. oxicode-mnemopi는 미해결.
- **최소 테스트**:
  - `test_watcher_no_loopback` — `Mnemopi::remember` 후 1초 내 `is_dirty() == false`
  - `test_watcher_external_edit_triggers_reindex` — 외부에서 `touch file.md` → 다음 `recall`이 새 내용 반환
  - `test_watcher_delete_removes_chunks` — 외부 `rm` → 인덱스에서도 제거
- **`cargo clippy`** 가 잡나? 부분 — `MutexGuard`가 `.await` 보유 시 컴파일 에러. `is_dirty`/`take_dirty`는 모두 sync.

### 후보 4 (permission)
- **무엇이 깨질 수 있나**: `claude_settings.rs`의 결정적 거절 규칙이 oxicode 사용자에게 너무 엄격. `auto_mode` 도입 시 false positive 위험.
- **최소 테스트**:
  - `test_claude_settings_import_allow_list` — Claude Code의 allow 패턴이 oxicode `AccessDecision::Allow`로 정확히 매핑
  - `test_bash_command_split_parsing` — `rm -rf /tmp/foo` → 명령어 토큰 분리 정확
- **`cargo clippy`** 가 잡나? 예.

### 후보 5 (path layout)
- **무엇이 깨질 수 있나**: 기존 사용자 데이터가 다른 디렉토리에 있어 인덱스에 안 보임. 마이그레이션 누락 시 silent data loss.
- **최소 테스트**:
  - `test_compute_workspace_hash_deterministic` — 같은 cwd → 같은 hash
  - `test_is_ephemeral_cwd_temp` — `/tmp/foo` → true
  - `test_legacy_migration_opt_in` — `~/.oxicode/memory/old.sqlite` 발견 시 env var `OXICODE_MIGRATE_MEMORY_LAYOUT=1`이면 마이그레이션
- **`cargo clippy`** 가 잡나? 예.

---

## 마이그레이션 로드맵 (제안)

| 단계 | 작업 | 의존 | 위험 | 기간 (추정) |
|---|---|---|---|---|
| **1** | 후보 5 (path layout) — `oxicode-mnemopi::path_layout` 모듈 추가, 기본 동작 유지 + opt-in 신 레이아웃 | 없음 | 낮음 | 1 PR, ~3일 |
| **2** | 후보 3 (watcher) — `oxicode-mnemopi::watcher` 모듈 + 기존 storage와 wire | 후보 5 (path layout — watcher가 감시할 디렉토리 명명이 먼저 확정되어야 함) | 중간 (loopback 함정) | 1 PR, ~5일 |
| **3** | 후보 2 (compaction seams) — `oxicode-ai::compaction::seam` + `Message: CompactionItem` | 없음 | 중간 (API 변경) | 1 PR, ~7일 |
| **4** | 후보 1 (dream) — `oxicode-mnemopi::dream` + `MnemopiDreamScheduler` | 후보 5 (path layout — 세션 로그를 읽을 디렉토리 명명; watcher는 보완적이나 필수 아님 — dream이 청크 인덱스를 직접 갱신함) | 높음 (LLM 비용) | 1 PR, ~10일 |
| **5** | 후보 4 (permission) — `bash_command_splitting` + `claude_settings` 임포트만 (auto_mode는 보류) | 없음 | 낮음 | 1 PR, ~5일 |

**권장 시작**: 후보 5 → 3 → 2 → 1 → 4 (의존성 순서). 단, **현재 Mnemopi/advisor omp 포팅이 v0.55/v0.54에 막 출시됐다는 사실** (`retain` 2026-07-03, `v0.54` 2026-07-05)을 고려하면, grok 이식은 **다음 분기 (v0.57+)** 우선순위로 미루는 것이 합리적. v0.56은 안정화 + ossification에 집중.

---

## 결론

**grok는 oxicode의 미래 방향에 대한 검증된 참조 구현**이다. 동일 도메인(Rust 코딩 에이전트)을 더 큰 규모로 풀고 있으며, 특히 **(a) `dream` LLM 합치기, (b) `CompactionItem`/`CompactionSampler` trait seams, (c) `RewindCheckpoint` turn-boundary fan-out** 세 곳은 oxicode가 향후 2-3개 분기에서 반드시 채택해야 할 패턴이다.

다만:
1. **oxicode는 omp 포팅 직후**라 grok 이식은 다음 분기(v0.57+)로 미루는 것이 안전
2. **`PermissionManager` 전체**는 oxicode의 "임베더블 엔진" 포지셔닝과 충돌 — `bash_command_splitting` + `claude_settings` 임포트만 선별 도입
3. **`hub` 프로토콜**, **`ACP` 통합**, **`worktree`**는 oxios 별도 제품으로 이관 (grok의 enterprise 워크플로우 영역)
4. **`prompt_queue` wire 타입**은 oxicode의 기존 `steering_queue`가 in-process로 충분하므로 **불필요** — JSON-RPC envelope가 필요한 시점에 추가 (RPC 모드 v2 작업 시 검토)

**행동 결정**: 이번 분기(v0.56)에는 **아무것도 이식하지 않음**. v0.57 플래닝 시 후보 5/3/2/1/4 순서로 진행 권장. 자동 Cargo.toml 생성(grok의 discipline signal)은 별도 PR로 즉시 도입 가능 (`oxicode/Cargo.toml` → `build.rs` + `members.toml.in`).

---

## 부록 — 읽은 파일 목록

grok 소스 (2000 LOC 예산 내):
- `crates/codegen/xai-grok-memory/src/lib.rs` (1–109)
- `crates/codegen/xai-grok-memory/src/dream.rs` (1–123)
- `crates/codegen/xai-grok-memory/src/storage.rs` (1–153)
- `crates/codegen/xai-grok-memory/src/search.rs` (1–153)
- `crates/codegen/xai-grok-memory/src/mmr.rs` (1–131, tests 132–348)
- `crates/codegen/xai-grok-memory/src/watcher.rs` (1–107)
- `crates/codegen/xai-grok-memory/src/schema.rs` (1–98)
- `crates/codegen/xai-grok-memory/Cargo.toml` (1–54)
- `crates/common/xai-grok-compaction/src/lib.rs` (1–84)
- `crates/common/xai-grok-compaction/src/sampler.rs` (1–136)
- `crates/common/xai-grok-compaction/src/item.rs` (1–103)
- `crates/common/xai-grok-compaction/src/code_compaction/mod.rs` (1–55)
- `crates/common/xai-grok-compaction/src/code_compaction/compact.rs` (1–103)
- `crates/common/xai-grok-compaction/src/code_compaction/config.rs` (1–42)
- `crates/common/xai-grok-compaction/Cargo.toml` (1–22)
- `crates/codegen/xai-grok-workspace/src/session/checkpoint.rs` (1–83)
- `crates/codegen/xai-grok-workspace/src/` (전체 dir tree)
- `crates/codegen/xai-prompt-queue/src/lib.rs` (1–6)
- `crates/codegen/xai-grok-tools/Cargo.toml` (1–120)
- `crates/codegen/xai-grok-workspace/Cargo.toml` (1–146)
- 루트 `Cargo.toml`, `README.md`

oxicode 검증 (~600 LOC):
- `oxicode-ai/src/compaction.rs` (1–103)
- `oxicode-mnemopi/src/lib.rs` (1–540, 핵심 API)
- `oxicode-mnemopi/src/mmr.rs`, `weibull.rs`, `temporal.rs`, `consolidate.rs`, `veracity_consolidation.rs`, `session.rs` 헤더
- `oxicode-sdk/src/ports/mod.rs` (AccessGate, EmbeddingProvider, PortRegistry)
- `oxicode-sdk/src/ports/fs/access.rs` (SimpleAccessGate)
- `oxicode-agent/src/agent_loop/{queues.rs,mod.rs}` 헤더
- `oxicode-agent/src/config.rs`, `events.rs`, `agent.rs` (steering/follow-up 큐)
- `oxicode-cli/src/services.rs`, `store/memory_mnemopi.rs`
- `AGENTS.md`, `CHANGELOG.md` (Pitfalls 섹션)
- `docs/designs/{omp-adoption/,omp-adoption-2/,2026-06-26-mnemopi-drift-supplement.md}` — 직전 분기 Mnemopi/Advisor 포팅 결정 문서

---

# 보강 (v2) — omp 교차검증 (2026-07-18)

1차 보고서는 grok 단독 분석이었다. omp(`can1357/oh-my-pi`)를 다시 클론해 교차검증한 결과 **3개 사실정정 + 4개 신규 후보**가 나왔다. 핵심 통찰: **omp는 "기능의 풍부함"에서 우위, grok는 "Rust 아키텍처의 정교함"에서 우위** — oxicode의 정답은 **omp 기능을 grok 아키텍처로 담기**다.

## A. 사실정정 (1차 보고서에서 틀린 부분)

### A1. [정정] 후보 1(dream)의 "omp에도 없는 신규 가치" 프레이밍 — 틀림

**1차 보고서 라인 54**: "omp에도 없는 진짜 신규 가치"

**검증 결과 (omp 코드 직접 확인)**: omp mnemopi는 **완전한 LLM 합치기 파이프라인**을 가지고 있다:

- `packages/mnemopi/src/core/extraction.ts` — `callHostLlm`/`callLocalLlm`/`callRemoteLlm` 임포트, `MNEMOPI_LLM_ENABLED` (기본값 `true`)
- `packages/mnemopi/src/core/local-llm.ts` — `DEFAULT_MODEL_REPO = "TheBloke/TinyLlama-1.1B-Chat-v1.0-GGUF"`, `completeSimple` 사용
- `packages/mnemopi/src/core/llm-backends.ts` — `LlmBackend` trait, `setHostLlmBackend`, `callHostLlm`
- `packages/mnemopi/src/core/shmr.ts:200-212` — `formatClusterForLlm`/`extractJsonFromLlmOutput` (LLM 기반 belief generation)
- `packages/mnemopi/src/config.ts:371` — **`sleepPrompt`** (sleep consolidation이 LLM을 쓴다는 직접 증거)
- `packages/mnemopi/src/core/beam/consolidate.ts:1020` — `metadata: { llm_used: ... }` (LLM 사용 추적)
- `packages/coding-agent/src/config/settings-schema.ts:4703` — `"Mnemopi LLM for fact extraction + consolidation"`

**oxicode-mnemopi 실제 상태**: omp에서 **알고리즘 부분(`consolidate.rs` aaak)만 포팅**, LLM 레이어를 통째로 스킵. `oxicode-mnemopi/src/extraction.rs`는 있지만 LLM 호출 없음. `LlmBackend` trait 없음.

**재구성된 프레이밍**: grok dream은 "신규 기능"이 아니라 **"omp가 TS로 구현한 LLM 합치기를 Rust로 포팅할 때 grok `dream.rs`를 참고 구현체로 쓰는 것"**이다.

**우선순위 변화**: 신규 기능 도입(중간) → 누락된 omp 기능 채우기(상향). 직전 분기 Mnemopi 포팅에서 선택적 스킵한 것을 다음 분기에 완성하는 작업.

**이식 표면 변화**: 1차 보고서의 ".md 세션 로그 파일 → LLM 호출" (grok 방식, `SessionLogWriter` 필요)에서 **"oxicode-mnemopi의 `working_memory` 테이블 → LLM"** (omp 방식, 기존 인프라 재사지)으로 변경.

**LOC 추정 변화**: ~800 (SessionLogWriter + dream) → **~400** (LLM backend trait + fact extraction + consolidation prompt만 — 저장소/인덱스는 이미 oxicode-mnemopi에 있음).

### A2. [정정] 후보 2(compaction)의 "3스타일 엔진 분해" — 범위 과대

(자체 리뷰 H2에서 이미 지적. v2에서 확정)

grok `intra_compaction`/`inter_compaction`은 **"Grok chat host"**(채팅 제품) 전용이므로 oxicode는 `code_compaction` + trait seams만 필요. 동시에 omp는 이미 **5개 전략**을 가짐:

```typescript
// omp packages/agent/src/compaction/compaction.ts:163
strategy?: "context-full" | "handoff" | "shake" | "snapcompact" | "off";
```

**결합 설계**: omp의 5 전략을 grok의 `CompactionItem`/`CompactionSampler` trait에 담기. oxicode는 1개 전략(`LlmCompactor`)에서 5개로 확장. snapcompact는 별도 서브 프로젝트(후보 E2).

### A3. [정정] 후보 5(path layout) → sqlite-journal 선행 필요

1차 보고서에서 watcher의 의존성을 path layout으로 잡았으나(advisory로 정정), 실제로는 **sqlite-journal이 먼저**여야 한다. 이유: path layout 변경이 기존 DB 마이그레이션을 동반하는데, NFS 마운트에서 WAL 모드 SIGBUS가 나면 마이그레이션 자체가 크래시.

## B. 교차검증 매트릭스 — 같은 문제, 다른 접근

| 문제 | omp (TS) | grok (Rust) | oxicode 현재 | 우위 |
|---|---|---|---|---|
| **LLM memory consolidation** | ✅ `mnemopi` 3-백엔드(local GGUF / host / remote) + `sleepPrompt` | ✅ `dream.rs` (LLM 합치기, 주기적) | consolidate.rs (omp 알고리즘만 포팅, LLM 스킵) | 동급 — **omp가 더 정교, grok는 Rust 참고구현** |
| **Session log 저장소** | ✅ `memories/index.ts` (47KB, 2-stage job queue) | ✅ `xai-grok-memory/storage.rs` (1.8K LOC, file-based) | ❌ | omp가 더 정교하지만 복잡 |
| **Compaction 전략 수** | ✅ 5개 (`context-full`/`handoff`/`shake`/`snapcompact`/`off`) | ✅ 3개 (`code_compaction` + `intra`/`inter`는 chat 전용) | 1개 (`LlmCompactor`) | **omp 기능 우위** |
| **Compaction 트리거** | ✅ 6개 (manual/overflow/incomplete-output/threshold/mid-turn/idle, `docs/compaction.md:54-65`) | ✅ trait seam으로 노출 | 1개 (threshold only) | **omp 압도** |
| **Split-turn 처리** | ✅ 지원 (`compaction.md:194-220`, history + turn-prefix 2개 요약) | ❌ | ❌ | **omp만** |
| **File-op 컨텍스트** | ✅ `<files>` 태그 (cumulative read/modified set, 트리 렌더) | 모름 | ❌ | **omp만** |
| **Useless-result elision** | ✅ `USELESS_NOTICE` placeholder (`compaction.md:166-173`) | 모름 | ❌ | **omp만** |
| **Native provider compaction** | ✅ OpenAI `/responses/compact` (`compaction.md:246`) | 모름 | ❌ | **omp만** |
| **Snapcompact (비트맵 압축)** | ✅ `packages/snapcompact` (1977 LOC TS + Rust `pi-natives` 렌더러, SQuAD evals 검증) | ❌ | 설계만 (`docs/designs/omp-adoption-2/07-...`) | **omp 독점** |
| **Doom-loop 검출** | ✅ `thinking-loop.ts` (3 shapes: verbatim tail, near-dup trigram, progress-lexicon stall) + `tool-call-loop-guard.ts` | ✅ `DoomLoopRecoverySettings` + `x-grok-doom-loop-check` 서버 헤더 | ❌ | **omp 우위** (검증됨), grok는 서버 연동 추가 |
| **Secret obfuscation** | ✅ `SecretObfuscator` (442 LOC, plain/regex, `#HASH#` placeholder, deobfuscation map) | `xai-grok-secrets` (미상세 조사) | internal `secret` mod only (env-key) | **omp 우위** |
| **Hunk-tracker (actor)** | ❌ | ✅ `xai-hunk-tracker` (actor, fs_notify, attribution) | ❌ | **grok 독점** |
| **Sqlite journal awareness** | ❌ | ✅ `xai-sqlite-journal` (780 LOC, NFS-aware, SIGBUS 방어, `GROK_SQLITE_JOURNAL_MODE` kill-switch) | ❌ | **grok 독점** |
| **Sandboxing** | ❌ (사용자 책임) | ✅ `xai-grok-sandbox` (Landlock/Seatbelt via nono, 자식 프로세스 네트워크 차단) | ❌ | **grok 독점** |
| **Fast worktree (CoW)** | ❌ | ✅ `xai-fast-worktree` (btrfs/overlay/copy + SQLite metadata + pool) | ❌ | **grok 독점** |
| **Foreign 세션 임포트** | ❌ | ✅ `foreign_sessions/` (codex/claude 16KB) | ❌ | grok 독점 |

## C. 발산점 분석 — 같은 문제를 다르게 푼 곳

### C1. Memory LLM: TS vs Rust
- **omp**: TS(`packages/mnemopi/src/core/{extraction,llm-backends,local-llm,shmr}.ts`)로 풍부하게 구현. 세 백엔드(local GGUF 기본 TinyLlama / host LLM / remote OpenAI). 사실 추출 + sleep 합치기 + SHMR harmonization.
- **grok**: Rust(`crates/codegen/xai-grok-memory/src/dream.rs`)로 단일 LLM 합치기. 주기적 게이트(enabled/time/sessions).
- **시사**: oxicode는 **omp의 다중 백엔드 아키텍처 + grok의 게이트 패턴** 결합. oxicode-mnemopi는 이미 SQLite 저장소가 있으므로 dream.rs의 `.md` 파일 파이프라인은 불필요 — `working_memory` 테이블에서 직접 LLM 호출.

### C2. Compaction 전략: 풍부함(omp) vs 아키텍처(grok)
- **omp**: 5개 전략 + 6개 트리거 + split-turn + file-op + useless-result elision + OpenAI native endpoint. **기능 압도**.
- **grok**: `CompactionItem`/`CompactionSampler`/`ItemTokenCounter` trait seams로 host-agnostic. `code_compaction` (full-replace) + `intra/inter` (chat 전용). **아키텍처 우위**.
- **시사**: oxicode는 **omp의 전략/트리거 기능을 grok의 trait에 담기**. 1차 보고서 후보 2는 `code_compaction` trait만 좁혀서 시작, omp의 5 전략을 차례로 trait에 impl.

### C3. Doom-loop 검출: stream-side(omp) vs turn-side(grok)
- **omp**: stream-side. `thinking-loop.ts`가 3 shapes(verbatim tail / near-dup trigram / progress-lexicon stall)를 실시간 스트림에서 검출 → 스트림 종료 + 재샘플링. `tool-call-loop-guard.ts`가 cross-turn tool-call 반복 임계(기본 5) 검출.
- **grok**: turn-side + 서버 연동. `DoomLoopRecoverySettings`가 `tail_repetition` 임계(기본 8) + `max_retries`(기본 2). `x-grok-doom-loop-check` 헤더로 서버 트리거 수신.
- **시사**: oxicode는 **omp의 stream-side 검출을 먼저 포팅** (검증된 3 shapes). grok의 서버 연동은 oxios(자체 허브)에서 검토.

### C4. Session log: 2-stage job queue(omp) vs 단순 .md(grok)
- **omp**: `memories/index.ts` (47KB)가 2-stage job queue로 동작. `stage1Concurrency: 8`, `stage1LeaseSeconds: 120`, `phase2LeaseSeconds: 180`. 롤아웃/하트비트/클레임 분산 처리. 멀티프로세스 safe.
- **grok**: `storage.rs`가 단순 파일 I/O. `~/.grok/memory/{hash}/sessions/YYYY-MM-DD-{slug}-{sid8}.md`. 단일 프로세스 가정.
- **시사**: oxicode는 단일 프로세스이므로 **grok의 단순 패턴이 충분**. omp의 job queue는 oxios(다중 에이전트)에서만 필요.

## D. 새로운 high-value 후보 (1차 5개에 추가)

### E1. [high] omp thinking-loop + tool-call-loop-guard → oxicode agent_loop 포팅

- **대상**: `oxicode-agent/src/agent_loop/streaming.rs` (stream 토큰 루프) + `oxicode-agent/src/agent_loop/mod.rs` (cross-turn 루프)
- **근거**: omp `packages/ai/src/utils/thinking-loop.ts`는 3 shapes 검출(verbatim tail, near-dup trigram, progress-lexicon stall)로 Gemini reasoning-summarizer 무한 루프 잡음. `tool-call-loop-guard.ts`는 cross-turn tool-call 반복 임계 검출. oxicode는 둘 다 없음 (grep 결과 no matches). oxicode agent_loop의 `CircuitBreaker`는 에러 기반이지 반복 기반이 아님.
- **이식 표면**:
  - `packages/ai/src/utils/thinking-loop.ts` (TS, ~250 LOC 추정) → `oxicode-ai/src/utils/thinking_loop.rs`
  - `packages/agent/src/utils/tool-call-loop-guard.ts` → `oxicode-agent/src/agent_loop/tool_call_loop_guard.rs`
  - `agent_loop/streaming.rs`의 delta 처리 루프에 250-char rolling tail + Jaccard word-trigram 윈도우 삽입
  - `agent_loop/mod.rs`의 cross-turn 루프에 tool-call 시그니처 해시 카운터 삽입
- **리스크**: false positive (정당한 반복 — 예: 테스트 실행 N회). omp의 `exemptTools` 패턴 도입. 테스트: `test_thinking_loop_verbatim_tail_break`, `test_tool_call_loop_guard_exempt`.

### E2. [high] omp snapcompact → 독립 크레이트 `oxicode-snapcompact` (마스터플랜 ⑦ 구체화)

- **대상**: 신규 크레이트 `oxicode-snapcompact` (omp 패키지 전체 이식)
- **근거**: omp `packages/snapcompact`는 1977 LOC TS + Rust `pi-natives` 네이티브 렌더러. SQuAD 200k-token evals로 provider별 shape(`11on16-bw` Anthropic, `8on22-bw@2048` Google, `8on22-bw` OpenAI) 검증. 마스터플랜(`docs/designs/omp-adoption-2/07-...`)에 설계만 있고 구현은 안 됨.
- **이식 표면**:
  - omp `packages/snapcompact/src/snapcompact.ts` (1977 LOC TS) → Rust 포팅
  - omp `crates/pi-natives/src/snapcompact.rs` (Rust 렌더러) → oxicode-snapcompact에 직접 흡수 (이미 Rust)
  - `pi-natives`의 폰트 번들(5x8, 8x8, 6x12, 8x13, Silver TTF) — Apache-2.0 / public domain
  - provider shape 테이블 — 그대로 이식 (evals 검증값)
- **리스크**: 폰트 라이선스 정리 (Silver TTF는 embedded, X.org 폰트는 public domain). vision-capable model 감지 로직 (oxicode-ai catalog의 `model.input` 필드 필요 — 현재 없음, 추가 작업).

### E3. [high] grok sqlite-journal → oxicode-mnemopi + oxicode-cli StateStore 즉시 적용

- **대상**: `oxicode-mnemopi/src/db.rs` (`MnemopiDb::open`) + `oxicode-sdk/src/ports/fs/state.rs` (`FileStateStore`) + `oxicode-cli/src/store/session.rs`
- **근거**: grok `crates/codegen/xai-sqlite-journal/src/lib.rs:1-103` — WAL 모드가 NFS에서 SIGBUS를 일으키는 문제(NFS가 mmap'd `-shm`을 안전하게 공유 못함)를 감지하고 TRUNCATE 모드 + per-host DB 파일로 회피. oxicode-mnemopi는 `Connection::open`으로 raw WAL 사용 — NFS 홈(NFS 마운트된 회사/학교 맥)에서 크래시 위험. 주석(`lib.rs:7-13`)이 이 문제를 정확히 설명.
- **이식 표면**:
  - `JournalMode::for_db_path(db_path)` → `Wal`/`Truncate` 자동 선택 (statfs 기반 네트워크 FS 감지)
  - `effective_db_path(db_path)` → 네트워크 FS일 때 `<name>.h-<hostname>.db`로 per-host 분리
  - `JournalMode::open`/`open_readonly` 헬퍼 (journal-aware PRAGMA 설정)
  - `OXICODE_SQLITE_JOURNAL_MODE` env kill-switch (`wal`/`truncate`)
  - macOS: `statfs::MNT_LOCAL` 플래그 확인
- **리스크**: macOS/Windows/Linux 각각 statfs API가 다름. grok는 Linux/Windows만 지원. macOS 지원 추가 작업 (~50 LOC). per-host DB가 기존 사용자에게 예상치 못한 DB 파일 증식으로 인지 — 마이그레이션 안내 필수.

### E4. [medium] omp SecretObfuscator → oxicode-cli settings/API키 보호

- **대상**: `oxicode-cli/src/store/auth_storage.rs` (현재 평문 JSON 저장 추정) + `oxicode-cli/src/services.rs` (시스템 프롬프트 빌드)
- **근거**: omp `packages/coding-agent/src/secrets/obfuscator.ts` (442 LOC)는 API키/시크릿을 placeholder(`#HASH#`)로 치환해 LLM 컨텍스트/세션 로그에 노출 방지. plain/regex 두 모드 + 결정론적 replacement + deobfuscation map. oxicode는 현재 API키를 평문으로 저장/취급 — 세션 로그나 시스템 프롬프트에 실수로 노출될 위험.
- **이식 표면**:
  - `obfuscator.ts` (442 LOC TS) → `oxicode-cli/src/secrets/obfuscator.rs` Rust 포팅 (~300 LOC, Bun.hash → blake3)
  - `AgentSession::build_system_prompt`에 obfuscation 적용
  - `session.rs` (JSONL 세션 영속화)에 적용 — 세션 파일이 디스크에서 placeholder 형태
- **리스크**: 낮음. 순수 로컬 변환. 단, compaction summary에 placeholder가 들어가면 재구성 시 정보 손실 가능 → placeholder는 컨텍스트 빌드 직전에만 적용, 영속화는 원문.

## E. 원본 5개 후보 최종 재평가 (v2)

| 후보 | 1차 평가 | v2 평가 | 비고 |
|---|---|---|---|
| 1. dream | [high] omp에 없는 신규 | [high] **omp 누락 기능 채우기** — 우선순위 상향, LOC 800→400 | A1 참조 |
| 2. compaction trait | [high] 3스타일 분해 | [high] **code_compaction + omp 5전략 결합** — 범위 재조정 | A2 참조 |
| 3. watcher | [high] arc_swap 락프리 | [high] 그대로 — 단 sqlite-journal 선행(E3) | 독립 |
| 4. permission | [medium] bash_command_splitting + claude_settings | [medium] 그대로 — 단 auto_mode.rs는 읽어보기 전까지 거절 보류(H3) | 미검증 축소 |
| 5. path layout | [medium] {slug}-{hash8} + ephemeral | [medium] 그대로 — 단 sqlite-journal 먼저(A3) | 의존성 정정 |

## F. v2 통합 마이그레이션 로드맵

모든 후보가 이제 독립적이므로 **위험/가치 기준** 정렬. v0.57~v0.59 3분기에 걸친 분산.

| 순서 | 후보 | 위험 | 가치 | 비고 |
|---|---|---|---|---|
| 1 | **E3 sqlite-journal** | 낮음 | high (신뢰성) | 독립, v0.57 조기 착수 가능 — oxicode-mnemopi NFS 크래시 방어 |
| 2 | **1 dream (재구성)** | 중간 | high (omp 누락 채우기) | v0.57 — 직전 분기 Mnemopi의 LLM 레이어 완성 |
| 3 | **E1 thinking-loop + tool-call-guard** | 중간 | high (안정성) | v0.57 — agent_loop 강화, omp 검증 알고리즘 |
| 4 | **3 watcher** | 중간 | high | v0.58 — sqlite-journal 이후 |
| 5 | **5 path layout** | 낮음 | medium | v0.58 — sqlite-journal 이후 |
| 6 | **2 compaction trait + 전략** | 높음 | high | v0.58 — 공개 API 변경 동반 |
| 7 | **E4 SecretObfuscator** | 낮음 | medium | v0.58 — 독립 |
| 8 | **E2 snapcompact** | 높음 | high (마스터플랜 ⑦) | v0.59 — vision-capable model 감지 인프라 선행 |
| 9 | **4 permission** | 중간 | medium | v0.59 — auto_mode 사전 조사 후 재결정 |

## G. 전략적 시사사항

1. **omp가 "기능의 풍부함"에서 압도** — snapcompact, 5 compaction 전략, 6 트리거, thinking-loop 3 shapes, SecretObfuscator, 2-stage job queue. oxicode가 omp에서 포팅한 것은(mnemopi/ advisor) 일부에 불과.
2. **grok는 "Rust 아키텍처의 정교함"에서 우위** — trait seams(`CompactionItem`/`CompactionSampler`), actor 패턴(hunk-tracker), lock-free 패턴(watcher의 arc_swap), 파일시스템 인식(sqlite-journal).
3. **oxicode의 정답은 "omp 기능 + grok 아키텍처 결합"** — 예: omp의 5 compaction 전략을 grok의 trait에 담기. omp의 LLM 합치기를 Rust로 옮길 때 grok dream.rs를 참고 구현체로 사용.
4. **oxicode-mnemopi의 LLM 누락은 직전 분기의 선택적 스킵** — 다음 분기 최우선. 단, grok dream.rs의 `.md` 파일 파이프라인은 oxicode에 불필요(이미 SQLite 저장소가 있으므로).
5. **omp 자체가 TS→Rust 마이그레이션 중** (`crates/pi-{ast,shell,natives,uu-diff,uu-grep,walker,iso}` + 50+ uutils vendor) — oxicode의 태생적 Rust 접근은 **사후 검증**된 것이다. snapcompact의 Rust 렌더러(`crates/pi-natives/src/snapcompact.rs`)는 omp가 이미 Rust로 검증한 부분이므로 포팅 부담이 적다.
6. **마스터플랜 ⑦(snapcompact)은 이제 구체화 가능** — omp의 전체 코드 + Rust 렌더러가 공개됨. v0.59에 독립 크레이트로 착수 권장.

## H. v2 결론

1차 보고서의 판정(`Port partially`)은 유효하나, **진술의 정확성**이 3곳에서 정정됨(A1, A2, A3). 새로운 후보 4개(E1~E4)가 추가되어 총 9개 후보로 확장.

**oxicode의 전략적 방향**:
- **v0.57 (다음 분기)**: E3 sqlite-journal + 1 dream(재구성) + E1 thinking-loop — 직전 분기 Mnemopi 포팅의 누락 보강 + 안정성 강화
- **v0.58**: 3 watcher + 5 path layout + 2 compaction trait + E4 SecretObfuscator — 인프라 확장
- **v0.59**: E2 snapcompact (마스터플랜 ⑦) + 4 permission — 대규모 기능 추가

**행동 결정 (v2)**: v0.56은 여전히 안정화. **v0.57 착수 순서를 E3 → 1 → E1로 확정** — 세 작업 모두 직전 분기 Mnemopi 포팅과 직접 연관되어 가장 빠른 가치 환원. E2 snapcompact는 v0.59로 미룸 (vision-capable model 인프라 선행 필요).

## I. v2 추가 조사 파일 목록

omp (클론 `/tmp/omp`, 145MB):
- `packages/mnemopi/src/core/{extraction,llm-backends,local-llm,shmr,beam/consolidate,store}.ts` — LLM 합치기 파이프라인 전체
- `packages/mnemopi/src/config.ts:25-374` — LLM 설정 + `sleepPrompt` (line 371)
- `packages/mnemopi/src/core/runtime-options.ts` — `MnemopiLlmRuntimeOptions`
- `packages/coding-agent/src/config/settings-schema.ts:4694-4707` — `providers.memoryModel` 설정
- `packages/coding-agent/src/tiny/models.ts:170-220` — `TINY_MEMORY_MODEL_VALUES`
- `packages/coding-agent/src/memories/index.ts:1-83` — 2-stage job queue 저장소
- `packages/coding-agent/src/hindsight/index.ts` — hindsight 모듈 (dream 없음 확인)
- `packages/coding-agent/src/secrets/obfuscator.ts:1-83` — `SecretObfuscator`
- `packages/ai/src/utils/thinking-loop.ts` — 3 shapes 무한 루프 검출 (line 17-21 주석)
- `packages/agent/src/utils/tool-call-loop-guard.ts` — cross-turn tool-call 반복 임세
- `packages/agent/src/compaction/compaction.ts:162-197` — 5개 전략 + 트리거 게이트
- `packages/snapcompact/src/snapcompact.ts:1-123` — 비트맵 압축 엔진
- `packages/snapcompact/README.md` — 전체 API 서피스
- `docs/compaction.md:54-300` — 6 트리거 + split-turn + snapcompact 전략
- `crates/pi-natives/src/` — Rust 네이티브 렌더러/텍스트 처리
- `crates/pi-ast/src/` — Rust AST(tree-sitter)
- `crates/pi-shell/src/` — Rust 셸 (152KB shell.rs)
- `AGENTS.md:1-83` — TS+Rust 하이브리드 전략

grok (클론 `/tmp/ref-porter/xai-org-grok-build`, 73MB, 2차 조사):
- `crates/codegen/xai-sqlite-journal/src/lib.rs:1-103` — NFS-aware journal mode
- `crates/codegen/xai-hunk-tracker/src/lib.rs` — actor 패턴
- `crates/codegen/xai-fast-worktree/src/lib.rs:1-67` — CoW worktree
- `crates/codegen/xai-grok-sandbox/src/lib.rs:1-83` — Landlock/Seatbelt 샌드박스
- `crates/codegen/xai-grok-hooks/src/` — 외부 훅 시스템(config.rs 52KB, dispatcher.rs 33KB)
- `crates/codegen/xai-grok-config-types/src/lib.rs:1-83` — `DoomLoopRecoverySettings`