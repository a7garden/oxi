# oxi-tui v2 — 남은 작업 정리

**작성일**: 2026-07-22
**브랜치**: `oxi-tui-v2-plan-a` (43 commits)
**현재 상태**: oxi-tui v2 라이브러리 완성 + 파이프라인 LIVE + ChatView 화면 렌더링

---

## 1. 점진적 렌더링 마이그레이션

현재 `OXI_V2_RENDER=1` 환경변수를 켜면 ChatView 위젯이 채팅 영역을 렌더링합니다. 하지만 footer, input, overlay는 여전히 legacy 렌더링을 사용합니다. 각 위젯을 독립적으로 마이그레이션할 수 있습니다.

### 1.1 Footer 마이그레이션 (~2시간)

**파일**: `oxi-cli/src/tui/v2_render.rs`

v2_render::draw_v2의 하단 4줄 영역에 새 `oxi_tui::widget::panel::Footer` 위젯을 렌더링합니다.

**필요 작업**:
- `AppState`에 `v2_footer: oxi_tui::widget::panel::Footer` 필드 추가
- `v2_render::draw_v2`에서 Footer::render 호출
- AppState의 model/tokens/cost 데이터를 Footer에 동기화
- `cargo run --bin oxi`로 시각 확인 (OXI_V2_RENDER=1)

**브로커 문제**: AppState의 `model`, `tokens_in/out`, `cost` 필드를 Footer에 전달해야 함. 간단한 setter 호출.

### 1.2 Input 마이그레이션 (~3시간)

**파일**: `oxi-cli/src/tui/v2_render.rs`

입력 영역을 새 `oxi_tui::input::InputArea` (stock ratatui-textarea wrapper)로 렌더링합니다.

**필요 작업**:
- `AppState`에 `v2_input: oxi_tui::input::InputArea` 필드 추가
- legacy `InputState`의 textarea와 동기화 (텍스트 + 커서 위치)
- `v2_render::draw_v2`에서 InputArea::render 호출
- 커서 위치를 `ctx.set_cursor()`로 브로커 (★ 중요 — 사용자에게 보이는 커서)

**주의**: InputArea::render가 `ctx.set_cursor(pos)`를 호출해야 사용자에게 커서가 보입니다. 이것이 `CursorState::reconcile`을 통해 터미널에 전달됩니다.

### 1.3 Overlay 마이그레이션 (~각 1시간, 18개)

각 overlay(settings, mcp_config, model_select 등)를 `LegacyOverlayAdapter`로 감싸서 RetainedTree의 최상위 레이어로 올립니다.

**필요 작업**:
- `v2_render::draw_v2`에서 활성 overlay를 `LegacyOverlayAdapter::new(overlay)`로 감싸서 렌더링
- overlay의 theme 타입 변환 (legacy Theme → v2 Theme)
- 각 overlay별로 렌더링이 정상 동작하는지 시각 확인

**주의**: advisory가 지적한 대로 overlay는 `draw_frame`의 `swap_buffers()` 전에 렌더링되어야 합니다. `LegacyOverlayAdapter`를 RetainedTree 최상위에 두면 자동으로 해결됩니다.

---

## 2. Plan D: oxi-tui-legacy 제거

**전제 조건**: 모든 렌더링 마이그레이션 완료 (§1.1-1.3)

### 2.1 workspace에서 oxi-tui-legacy 제거 (~1시간)

- `Cargo.toml` members에서 `"oxi-tui-legacy"` 제거
- `oxi-cli/Cargo.toml`에서 `oxi-tui-legacy` 의존성 제거
- `oxi-cli/src/`의 모든 `oxi_tui_legacy::` 참조를 `oxi_tui::`로 교체 (또는 제거)
- `oxi-tui-legacy/` 디렉토리 삭제

**리스크**: oxi-cli의 27개 파일이 여전히 legacy 타입을 사용 중. 각 파일을 새 v2 API로 마이그레이션해야 함.

### 2.2 widget inventory 정리 (~2시간)

legacy의 `symbols.rs`, `keybindings/`, `markdown_styles.rs`, `fuzzy.rs` 등을 oxi-cli로 이동 또는 폐기.

---

## 3. v2_render 기본 활성화

**전제 조건**: §1.1-1.3 마이그레이션 완료 + 시각 테스트 통과

- `OXI_V2_RENDER` 환경변수 체크 제거 (v2를 기본으로)
- legacy 렌더링 경로 제거
- `render::draw` (legacy) 폐기 또는 v2 전용으로 재작성

---

## 4. 테스트 강화

### 4.1 PTY 기반 e2e 테스트 (~1일)

`docs/ref-porter/xai-org-grok-build-tui.md` 후보 5번. `portable-pty` crate으로 실제 PTY에서 oxi 바이너리를 spawn하고 출력 bytes를 검증.

- `test_pty_minimal_boot` — 실행 후 첫 프롬프트 표시
- `test_pty_sends_message_and_receives_response` — 입력 → mock LLM → 응답
- `test_pty_cursor_blink_preserved` — 커서 깜빡임 타이머 보존 확인

### 4.2 벤치마크

- `bench_50k_token_streaming` — 50K 토큰 응답에서 CPU 프로파일 (RetainedChild skip 효과)
- `bench_cursor_dedup` — idle 화면에서 0 bytes emit 확인

---

## 5. 마이그레이션 체크리스트

- [ ] §1.1 Footer 마이그레이션
- [ ] §1.2 Input 마이그레이션 (★ 커서 브로커 포함)
- [ ] §1.3 Overlay 마이그레이션 (18개, 각각 LegacyOverlayAdapter)
- [ ] §2.1 oxi-tui-legacy workspace 제거
- [ ] §2.2 widget inventory 정리
- [ ] §3 v2_render 기본 활성화
- [ ] §4.1 PTY e2e 테스트
- [ ] §4.2 벤치마크

---

## 현재 달성된 것 (참고용)

| 항목 | 상태 |
|---|---|
| oxi-tui v2 라이브러리 | ✅ 42 파일, 9.7K LOC, 222 테스트 |
| draw_frame (14 LOC body) | ✅ autoresize → hash-skip → render → flush → cursor → swap |
| CursorSlot tri-state | ✅ NotSet/Show/Hide — 커서 깜빡임 방지 |
| RetainedChild<T> | ✅ per-subtree memoization |
| StreamingMarkdown checkpoint | ✅ stable freeze + tail reparse |
| OSC8 CSI 2026 inline emission | ✅ |
| Capability-aware theme | ✅ detect + adapt same module |
| Input textarea wrapper | ✅ stock ratatui-textarea 0.9 |
| oxi-cli 파이프라인 LIVE | ✅ draw_frame_closure |
| Dual-write ChatLog | ✅ agent events → v2_chat |
| ChatView 화면 렌더링 | ✅ OXI_V2_RENDER=1 |
| AGENTS.md + CHANGELOG | ✅ |

## 문서 위치

- **Spec**: `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md`
- **Plan A**: `docs/superpowers/plans/2026-07-21-tui-render-pipeline-plan-a-foundation.md`
- **Plan B**: `docs/superpowers/plans/2026-07-22-tui-render-pipeline-plan-b-content-widgets.md`
- **Plan C**: `docs/superpowers/plans/2026-07-22-tui-render-pipeline-plan-c-osc8-input-cutover.md`
- **진행 기록**: `.superpowers/sdd/progress.md` (git-ignored)
