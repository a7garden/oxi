# 2026-07-30 — Rust Ratatui TUI 프로젝트 6종 정밀조사 결과

## 0. tl;dr

| # | 프로젝트 | 라이선스 | 무엇을 주는가 | oxicode 재사용 전략 |
|---|----------|----------|---------------|-----------------|
| 1 | **Codex CLI** (OpenAI) | Apache 2.0 | `Renderable` trait, 셀 단위 diff, terminal color probing, ChatWidget + BottomPane, theme hardcoded | **레퍼런스 코드 베껴오기** — Renderable + ChatWidget + BottomPane 구조는 금방 따라할 수 있고 peer pressure 있는 추적 대상. 라이브러리 재사용 X |
| 2 | **Grok Build** (xAI) | Apache 2.0 | `xai-grok-pager-render` (테마+렌더), `xai-ratatui-inline` (커스텀 ratatui fork, blink-preserving), `xai-grok-markdown` (스트리밍 markdown + LaTeX + Mermaid), `xai-ratatui-textarea`, 5 built-in themes | **핵심 채택 후보** — 특히 `xai-ratatui-inline`의 blink-preserving flush, `xai-grok-markdown`의 streaming markdown가 oxicode의 폭주 방지/스트리밍 UX의 정답에 가까움 |
| 3 | **VT Code** (vinhnx) | Apache 2.0 | `vtcode-ui` 자체가 분리 가능한 TUI 라이브러리. `HostAdapter` trait + `spawn_core_session()` API, 40+ 테마 registry, markdown 렌더러, 디자인 시스템 | **임베드 가능 라이브러리 후보 #1** — `crates/codegen/vtcode-ui`가 단독 dep 가능. 다만 agent loop과 강결합이라서 surface 한정적으로만 |
| 4 | **rust-code** (fortunto2) | MIT | `sgr-agent-tui` (TUI scaffold: ChatState, FocusLayer, CommandPalette, AppEvent), 채널 기반 event loop | **부분 차용** — `sgr-agent-tui`의 module 분리(FocusLayer, CommandPalette 등)가 깔끔. 테마는 hardcoded라 그대로 못 씀 |
| 5 | **CodeWhale** (Hmbown) | MIT | 18-crate workspace, `crates/tui` 거대형, `crates/agent` BYOM registry, `crates/state` SQLite persistence, `crates/lsp` post-edit diagnostics, `crates/sandbox` (Seatbelt, bwrap, Landlock) | **Architecture 참고용** — `crates/tui`가 너무 거대(자체 ARCHITECTURE 문서도 "still the live runtime"이라 표기). 직접 dep 비추, BYOM 모델·sandbox 구조는 따라할 것 |
| 6 | **capo-tui** (motosan-dev) | MIT (crates.io v0.12.0-beta.13) | Elm-style MVU runtime, markdown 렌더러(1220 lines), `drive_headless()` 테스트, 6 BottomPane overlays, `TerminalGuard` RAII | **개별 파일 차용** — MVU 패턴은 우리와 안 맞지만 markdown/highlight 모듈은 코드 베끼기 가능 |

**결론**: 6개 다 "한 통으로 dep 걸자" 후보는 아님. **명시적인 두 채택**: `xai-ratatui-inline` (스트리밍 출력 fix), `xai-grok-markdown` (스트리밍 markdown). **구조 차용**: Codex의 `Renderable` + `ChatWidget` + `BottomPane` 3단 구조, VTCode의 `HostAdapter` 임베드 API. **버릴 패턴**: capo-tui의 MVU, rust-code의 hardcoded theme, CodeWhale의 거대 단일 `crates/tui`.

---

## 1. 메서드 + 결과 데이터

6개 stream에 scout subagent를 동시에 dispatch. 각각:
- 정확한 파일 경로 + 라인 번호
- TUI 모듈 구조 분리도
- Reusable 라이브러리 후보 (crates.io or workspace 내부)
- 라이선스
- 이벤트 루프, 위젯 트리, 테마, markdown 렌더링

모든 scout의 raw 출력은 `agent://Scout{Codex,GrokBuild,VTCode,RustCode,CodeWhale,CapoTui}` 에 보존. 이 문서는 합본.

---

## 2. 프로젝트별 정밀 조사

### 2.1 Codex CLI (OpenAI) — github.com/openai/codex

- **Stars**: 102K+, **License**: Apache 2.0 ✅, **Stack**: Rust + Ratatui (TS에서 재작성 중)
- **Workspace**: `codex-rs/` 약 100크레이트
- **TUI 위치**: `codex-rs/tui/` (단일 크레이트 ~83K LOC)
- **핵심 트레이트 — `Renderable`** (`tui/src/render/renderable.rs`):
  ```rust
  pub trait Renderable {
      fn render(&self, area: Rect, buf: &mut Buffer) -> anyhow::Result<()>;
      fn desired_height(&self, area: Rect) -> Option<u16>;
      fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)>;
  }
  ```
- **이벤트 루프** (`tui/src/app.rs` `App::run()`):
  - `tokio::select!` 3-arm — `AppEvent` channel, `thread_event` channel, `TuiEventStream`
  - 펌프 → draw → drain → repeat
- **위젯 트리**:
  - `ChatWidget` (8.3K, `chatwidget.rs`) — HistoryCell[] + BottomPane + overlay UI
  - `BottomPane` (`bottom_pane/`) — status, approval, ChatComposer
  - `PagerOverlay` (`pager_overlay.rs`) — Ctrl+T transcript scrollback
- **Markdown**: `markdown_render.rs` 107K — pulldown-cmark + syntect/two-face
- **Syntax**: `render/highlight.rs` 60K — Syntect
- **Terminal color probing** (`terminal_palette.rs`): OSC 10/11로 fg/bg 추출, TrueColor→RGB, 256→CIE76 nearest-neighbor, 16→default
- **Theme**: hardcoded per-widget (`style.rs`) — **pluggable/hot-reload 없음**
- **Public reusable**: `public_widgets/composer_input.rs` (단 4KB) — ComposerInput wrapper만
- **재사용 판정**: ❌ crates.io 공개 라이브러리 없음.  Renderable + ChatWidget + BottomPane 구조 차용 대상 (소스 복붙 OK)

### 2.2 Grok Build (xAI) — github.com/xai-org/grok-build

- **Open source**: 2026-07, **License**: Apache 2.0 ✅
- **Workspace**: 77 crates
- **TUI 위치**: `crates/codegen/xai-grok-pager/` (lib v0.2.114)
- **이벤트 루프** (`xai-grok-pager/src/app/event_loop.rs` 5,126 lines):
  - Biased `tokio::select!`
  - `AppView::handle_input()` + `AppView::draw()` 둘 다 호출
- **모놀리식 구조**: `app_view.rs` 단일이 10,965 lines (root 소유)
- **렌더 레이어 분리 잘 됨** — 외부 의존성 후보:
  - `xai-grok-pager-render` (v0.1.0) — appearance, render, theme, syntax, terminal
  - `xai-ratatui-inline` — **Custom Ratatui `Terminal` fork** with **blink-preserving diff flush returning bool**
  - `xai-grok-markdown` — pulldown-cmark + syntect + LaTeX + Mermaid streaming
  - `xai-grok-markdown-core` — headless markdown analyzer
  - `xai-ratatui-textarea` — text input widget
- **Markdown**: streaming, 코드 하이라이팅, **LaTeX, Mermaid** 지원
- **Theme**: 5 built-in (groknight, tokyonight, grokday, rosepine, oscura) + 70+ semantic colors + 자동 dark/light follow
- **Terminal detection**: brand, multiplexer, Kitty keyboard, hyperlinks (1.1K lines)
- **Syntax**: per-theme `.tmTheme` + polarity-safe mapping
- **crates.io 확인 필요**: 이 크레이트들이 publish 되어 있는지 git log 확인 필요. xai-org는 거의 다 workspace-only일 가능성 높음
- **재사용 판정** ⭐⭐⭐: **의존성 채택 최우선**. 특히 `xai-ratatui-inline` (blink-preserving flush는 oxicode의 streaming tail 종료 시 깜빡임 문제 해결 가능), `xai-grok-markdown` (스트리밍 중간 코드 블록 처리)

### 2.3 VT Code (vinhnx) — github.com/vinhnx/vtcode

- **Stars**: ~780, **License**: MIT, **Stack**: Rust + Ratatui
- **Workspace**: 30 crates
- **TUI 위치**: `crates/codegen/vtcode-ui/` — **그 자체가 standalone TUI 라이브러리**
- **임베딩 API**: (`tui/src/tui/host.rs`)
  ```rust
  pub trait HostAdapter: WorkspaceInfoProvider + NotificationProvider + ThemeProvider
  ```
  - `spawn_core_session()` (`tui/src/tui/core.rs`) — drop-in entry point
  - `spawn_session_with_prompts_and_options()` (`tui/src/tui/core_tui/session.rs`)
- **3-layer 구조**:
  - `design/` — color 변환, layout, diff, panel primitives
  - `theme/` — 40+ theme registry, runtime state, syntax theme binding
  - `tui/` — core_tui (session lifecycle, runner loop, widget tree) + ui (markdown, interactive list, search, syntax)
- **Theming**: 40+ theme (Catppuccin, Gruvbox, Solarized, …), runtime active swap, hot-reload 가능성
- **Markdown**: custom renderer (pulldown-cmark + syntect) — `tui/ui/markdown/mod.rs`
- **재사용 판정** ⭐⭐: **embed 가능한 라이브러리 후보 #1**. `HostAdapter` 기반이라 agent loop은 oxicode 것을 쓰고 TUI만 빌릴 수 있음. 다만 vtcode-core 의미 모델(`InlineCommand`, `InlineEvent`, `InlineHandle`)과 결속되어 있어 surface 일부만 활용 가능

### 2.4 rust-code (fortunto2) — github.com/fortunto2/rust-code

- **License**: MIT ✅, **Stack**: Rust + Ratatui + sgr-agent + BAML + tmux + MCP
- **Workspace**: 8 crates
- **핵심 라이브러리**: `sgr-agent-tui` v0.4.7 — **TUI 라이브러리 dep 후보**
  - `ChatState`, `FocusLayer`, `CommandPalette`, `AppEvent`
  - Terminal init, channel-driven event loop
- **App glue**: `rc-cli/src/app.rs` 5K lines
- **이벤트 루프**: mpsc channel + background agent task
- **Modes**: 10 search/browse modes (Cmd+1..0)
- **Markdown rendering**: ❌ 없음
- **Theme**: ❌ hardcoded
- **MCP**: `rmcp` crate
- **Agent loop**: sgr-agent의 schemars JSON schema (BAML 아님 — scout 결과에 명시)
- **tmux**: 백그라운드 task 백엔드
- **재사용 판정** ⭐: `sgr-agent-tui` module 분리는 참고 (FocusLayer / CommandPalette / AppEvent). 테마/마크다운은 차용 불가

### 2.5 CodeWhale (Hmbown) — github.com/Hmbown/CodeWhale

- **License**: MIT ✅, **Stars**: 40.2K, **Rust**: 1.88
- **Workspace**: 18 crates
- **TUI 위치**: `crates/tui` — 자체 ARCHITECTURE 문서 ("v0.9.1: still the live end-user runtime") 가 명시하는 **거대 단일 크레이트** (Live runtime API, LSP, MCP, hooks, llm_client, execpolicy, palette, plugins, prompts, remittance, repl, RLM, runtime_api, runtime_threads, runtime_web, sandbox, … 다 들어가 있음)
- **Layer diagram** (ARCHITECTURE.md):
  ```
  TUI (ratatui)  +  One-shot Mode  +  Config/CLI
      ↓
  Core Engine (engine.rs, turn_loop.rs, session.rs, ops.rs)
      ↓
  Tools + Skills + Hooks + MCP Servers
      ↓
  Runtime API (HTTP/SSE) + Task Manager (durable)
      ↓
  LLM Client (DeepSeek native, OpenAI-compatible)
  ```
- **BYOM**: `crates/agent`의 `ModelRegistry` (DeepSeek, OpenRouter, HuggingFace, vLLM, SGLang, Ollama)
- **Persistence**: `crates/state` SQLite (sessions, threads, tasks 모두)
- **LSP**: `crates/tui/src/lsp/` — post-edit 진단을 `core/engine/lsp_hooks.rs`에 연결, edit 후 자동 diagnostics flush
- **Sandbox**: `crates/sandbox/` — macOS Seatbelt, Linux bwrap, Landlock, seccomp, Windows contract
- **체크포인트**: `~/.codewhale/sessions/checkpoints/latest.json` + `offline_queue.json`
- **재사용 판정** ⭐: **dep 비추**, **아키텍처만 차용**: BYOM registry, SQLite persistence, LSP post-edit hooks, sandbox 분리

### 2.6 capo-tui (motosan-dev) — github.com/motosan-dev/capo

- **License**: MIT ✅, **crates.io v0.12.0-beta.13**, **SLoC**: 40K
- **모노레포**: 3 crates — `capo-tui` (lib), `capo-agent` (SDK), `capo` (binary)
- **패턴**: **Elm-style MVU**
  - `state` (pure model) → `update(state, event) → Vec<Command>` → `render(state) → frame`
  - `run_tui(state, update, render)` merges crossterm input + tick + UiEvent stream
- **RAII**: `TerminalGuard` raw mode + alt screen
- **Testability**: `drive_headless()` for headless testing
- **Widget hierarchy**: transcript (`MessageBlock` collapse/expand), editor, footer, spinner + 6 BottomPane overlays (approval, palette, file picker, model picker, resume picker, fork picker, branch tree, settings)
- **Markdown**: pulldown-cmark, 1220 lines — pub
- **Highlight**: syntect, pub
- **Theme**: 9 static `Style` 함수 — ❌ pluggable 아님 ("full theming arrives in M3")
- **Critical**: `render() is pub(crate)` — **외부에서 layout 커스터마이즈 불가**
- **capo-agent coupling**: 미공개 SDK에 강결합
- **재사용 판정** ⭐: **MVU는 oxicode의 state+v2 있었고 비추**. 다만 markdown/highlight 모듈 소스만 빌려오면 됨

---

## 3. 차원별 비교표

| 차원 | Codex | Grok Build | VT Code | rust-code | CodeWhale | capo-tui |
|---|---|---|---|---|---|---|
| **이벤트 루프** | tokio::select 3-arm | biased tokio::select | Tokio task + crossterm | mpsc + background | (TUI monolithic) | Elm MVU |
| **렌더 단위** | Renderable trait | Renderable-style | imperative | imperative | imperative | Model→frame |
| **테마** | hardcoded | 5 built-in + hot reload | 40+ registry | hardcoded | (TUI 내) | 9 static |
| **Markdown** | pulldown+syntect (107K) | pulldown+syntect+LaTeX+Mermaid | pulldown+syntect | ❌ | (있을 듯) | pulldown (1.2K) |
| **스트리밍** | token 단위 | token + structured | token | token | token | token |
| **Composer** | ComposerInput (public) | tui-textarea | 자체 | 자체 | 자체 | 자체 |
| **Alt screen** | yes | yes + minimal mode | yes | yes | yes | yes (RAII Guard) |
| **Crash recovery** | checkpoint | ? | ? | ? | SQLite + offline queue | ❌ |
| **Terminal detection** | OSC 10/11 + CIE76 | brand + multiplexer + Kitty | ? | ? | Codex와 비슷 | TerminalGuard |
| **crates.io dep 가능** | ❌ | ⚠️ (확인 필요) | ✅ vtcode-ui | ✅ sgr-agent-tui | ❌ (TUI monolithic) | ✅ capo-tui (render() not pub) |
| **License** | Apache 2.0 | Apache 2.0 | MIT | MIT | MIT | MIT |
| **OXI 적합도** | 구조 차용 | 핵심 채택 | 임베드 후보 | 부분 차용 | 아키텍처만 | 파일 차용 |

---

## 4. oxicode 현재 상태 (v0.62.0)와의 매핑

```
oxicode-tui/src/
├── lib.rs                    54
├── theme.rs              1,906   ← 6 colorschemes + 28 color slots + GlyphSet
├── symbols.rs              905   ← Unicode / Ascii / Nerd presets
├── cell.rs                    7
├── text.rs                   54
├── markdown_styles.rs        93
├── fuzzy.rs                 294
├── tape/                  2,328   ← TapeEngine (component, container, transcript, streaming)
├── widgets/
│   ├── chat/                          (state 1,227, mod 793, render 480, layout 480, mouse 601, markdown 459, …)
│   ├── tool_renderer.rs            1,725
│   ├── todo_panel.rs                 419
│   ├── input.rs                      475
│   ├── dashboard.rs                  467
│   ├── list_selector.rs              920
│   └── …                            7,839
├── keybindings/, input/, render/    ~ small
─────────────────────────────────────
oxicode-tui total                  ~10K LOC
```

```
oxicode-cli/src/tui/
├── app.rs               2,069    ← root state, dispatch, event loop
├── handlers.rs          1,752    ← input → AppEvent
├── render.rs                23
├── welcome.rs              144
├── overlay/                      (ask 662, extensions 814, factories 643, fork_select 189,
│                                  mcp_config 1,978, mcp_dashboard 358, mcp_presets 167,
│                                  model_select 225, provider_select 1,016, roles_config 325,
│                                  router_integration 154, router_setup 701, settings 676,
│                                  text_viewer 200, tree_navigator 690, anchor 151, mod 178)
│                           9,307
├── slash/                  builtin/~10 commands
├── completion/             path, fuzzy_file
─────────────────────────────────────
oxicode-cli tui total          ~13K LOC
```

**현재 oxicode의 강점**:
- 5종 colorscheme + 28 color slot 이미 wired (재구축 시 또 같은 짓 반복 X)
- GlyphSet (Unicode/Ascii/Nerd) 분기
- TapeEngine이 RetainedTree + memoization

**현재 oxicode의 약점** (최초 메시지에서 냉정히 진단):
- **렌더링이 안 뜨다가 Ctrl+C에 flush** ← 사용자 보고. 스트리밍 출력 끝에서 blink/flush 누락이 의심
- `oxicode-cli/src/tui/app.rs` 2,069 + `handlers.rs` 1,752은 monolith
- widget 분리는 잘 됐지만 event loop이 단순 (단일 채널, biased select도 아님)
- Markdown streaming 처리가 widgets/chat/markdown.rs 459 LOC 한 곳에 응집
- BottomPane/overlay가 9K+ LOC로 비대 (overlay는 사실 작은 view들이고, 공유 footer/status widget 없어서 중복)

---

## 5. oxicode TUI 재구축을 위한 구체 추천

### 5.1 즉시 차용 (코드 베끼기 OK, Apache 2.0 / MIT 둘 다 OK)

| 자원 | 출처 | 어디에 적용 |
|---|---|---|
| `Renderable` trait | Codex `tui/src/render/renderable.rs` | oxicode의 theme 모듈 옆에 `render/mod.rs` 신설 |
| ChatWidget + BottomPane 구조 | Codex `tui/src/chatwidget.rs` + `bottom_pane/` | oxicode의 `widgets/chat/` + `tui/overlay/` 합리적 분리 |
| **blink-preserving flush Terminal** | Grok Build `xai-ratatui-inline` | oxicode의 main 화면이 alt screen 진입/이탈 + 스트리밍 tail 시 깜빡임 해결 |
| **Streaming markdown** | Grok Build `xai-grok-markdown` | `widgets/chat/markdown.rs` 교체 (459 LOC → 더 견고) |
| **Terminal color probing** | Codex `tui/src/terminal_palette.rs` | oxicode의 theme.rs에 자동 follow 추가 |
| `HostAdapter` trait | VT Code `tui/src/tui/host.rs` | oxicode-cli ↔ oxicode-tui 경계 정리 (현재 App::from_oxicode() 보완) |
| `drive_headless()` 테스트 헬퍼 | capo-tui | nextest 파이프라인에 headless 회귀 |
| `TerminalGuard` RAII | capo-tui | ratatui raw mode leak 방지 |

### 5.2 crates.io 의존성 후보 (실사용 전 publish 여부 확인 필수)

| 크레이트 | 확인 포인트 | oxicode 채택 가부 |
|---|---|---|
| `xai-grok-pager-render` | xai-org crates.io publish 여부. workspace-only 라면 xai-ratatui-inline, xai-grok-markdown만 따로 추출 가능한지 | 가능 |
| `xai-ratatui-inline` | ratatui fork — 버전 호환성 확인 필요 | 가능 |
| `xai-grok-markdown` | apache 2.0, publish 여부 확인 | 가능 |
| `vtcode-ui` | crates.io publish 확인 | 가능 (단 surface 한정) |
| `sgr-agent-tui` | crates.io v0.4.7 — markdown이 없음 | 부분 |
| `capo-tui` | v0.12.0-beta.13, render()가 pub(crate) | 비추 (전체 dep) |

### 5.3 절대 따라하지 않을 패턴

| 패턴 | 출처 | 이유 |
|---|---|---|
| Elm-style MVU | capo-tui | 우리 oxicode v2에서 이미 실패한 적 있음 |
| Hardcoded theme | rust-code, Codex, capo-tui | 우리 theme 시스템은 28 color slot + 5 scheme 이미 있음 |
| 거대 단일 `crates/tui` | CodeWhale | ARCHITECTURE 문서도 "still the live runtime"이라 명시 (자체 한계 인정) |
| TOML/JSON 선언형 theme 5종만 | Grok Build | 우리 28-slot + 6 scheme이면 더 강력 |

### 5.4 oxicode TUI 재구축 권장 아키텍처

```
crates/
├── oxicode-tui/                          (현재 이름 유지, chat_widget / tape는 폐기)
│   ├── lib.rs
│   ├── render/                       ← NEW: Renderable trait + diff flush + blink-preserving
│   │   ├── mod.rs
│   │   ├── renderable.rs             (from Codex)
│   │   ├── terminal.rs               (RAII + ratatui::Terminal fork from xai-ratatui-inline)
│   │   ├── color.rs                  (from Codex terminal_palette)
│   │   └── headless.rs               (from capo-tui drive_headless)
│   ├── theme/                        (refactor from theme.rs)
│   │   ├── mod.rs
│   │   ├── scheme.rs                 (6 schemes × 28 slots)
│   │   ├── glyph_set.rs              (Unicode / Ascii / Nerd)
│   │   ├── styles.rs                 (ThemeStyles packing)
│   │   ├── hot_reload.rs             (NEW)
│   │   └── auto_follow.rs            (NEW: OSC 10/11 follow)
│   ├── markdown/                     ← NEW: from widgets/chat/markdown.rs
│   │   ├── mod.rs
│   │   ├── stream.rs                 (incremental pulldown-cmark from xai-grok-markdown)
│   │   ├── highlight.rs              (syntect)
│   │   └── code_block.rs             (streaming partial block)
│   ├── widgets/
│   │   ├── chat/                     (chat widget, Renderable impl)
│   │   ├── composer/                 (from xai-ratatui-textarea or 자체)
│   │   ├── footer/                   (BottomPane의 status 분리)
│   │   ├── overlay/                  (이전 widgets/overlay/ → overlay widget base)
│   │   ├── status/                   (model, ctx, branch)
│   │   └── todo/                     (현재 todo_panel)
│   ├── event_loop.rs                 (biased tokio::select 3-arm from Codex)
│   ├── host_adapter.rs               (HostAdapter trait from VT Code)
│   └── input/                        (keybindings, mouse)
├── oxicode-cli/src/tui/                  (composition root)
│   ├── app.rs                        (refactor: app state only, widget tree assembly)
│   ├── handlers.rs                   (smaller; delegate to widgets)
│   ├── overlay/                      (thin slices delegating to oxicode-tui widgets/overlay)
│   └── slash/                        (slash commands)
```

핵심 변화:
1. **Event loop**: biased tokio::select 3-arm (AppEvent, TuiEvent, yield)
2. **Renderable trait**: 모든 widget이 자체 크기/커서 위치를 선언
3. **Streaming markdown**: incremental pulldown-cmark + 코드 블록 partial buffer
4. **Blink-preserving flush**: 매 프레임 `Terminal::flush()`가 bool 반환, 안 바뀌었으면 깜빡임 안 함
5. **HostAdapter**: oxicode-cli ↔ oxicode-tui 경계 명확화 (현재 `App::from_oxicode()` 보완)
6. **Headless drive**: nextest에서 render → snapshot 픽스
7. **Auto-follow theme**: OSC 10/11로 terminal 색 따라가기

---

## 6. Risk + Caveat

- **xai-org crates.io publish 상태**: workspace-only일 가능성 높음.  publish 안 됐다면 `xai-ratatui-inline`/`xai-grok-markdown` 만 들고 와서 vendoring. 둘 다 Apache 2.0이라 NOTICE 파일만 잘 달면 OK
- **ratatui version drift**: Grok Build = 0.29, oxicode = 0.30(2026-07).  0.29→0.30 patch 적용 필요
- **Codex는 매일 변하는 코드**: GitHub `codex-rs/tui`를 fork해서 둘 필요 없음. `Renderable`/terminal probing은 안정 API라 그냥 복붙
- **CodeWhale의 거대 TUI**: "incremental split" 이 ARCHITECTURE 문서 자체가 명시. 참고만
- **vtcode-ui의 의미 모델**: vtcode-core 강결합 채택 시 의미 모델 두 개(co에 oxicode-ai + vtcode-ui) 공존.  `HostAdapter` surface만 빌리는 게 안전

---

## 7. 한 줄 결론

- **Grok Build (xai-org)** 의 `xai-ratatui-inline` + `xai-grok-markdown` 모듈을 핵심 채택,
- **Codex** 의 `Renderable` + `ChatWidget` + `BottomPane` 구조 차용,
- **VT Code** 의 `HostAdapter` 임베드 API 차용,
- **capo-tui** 에서 `TerminalGuard` + `drive_headless` 차용,
- **CodeWhale / rust-code** 는 아키텍처 참고용,
- **결국 "한 통으로 dep" 라이브러리는 없음** — oxicode의 테마 시스템은 우리 것이 더 강력하므로 그냥 유지.

재구축은 1) chat streaming UX (ClI C 에서 발견된 blink/flush 문제)부터 고치고, 2) Renderable 추상화 깔고, 3) markdown 스트리밍을 pulldown-cmark incremental로 옮기는 순서로.
