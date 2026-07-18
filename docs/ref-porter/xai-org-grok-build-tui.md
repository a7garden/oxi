# grok-build vs oxi-tui — UI/UX/DX 비교분석

**`Port partially`** — grok의 TUI 스택은 oxi-tui보다 라이브러리 기준 **3.7x**(73K vs 20K LOC). 466K vs 20K(23x)라는 숫자는 **오도용** — 393K는 grok 바이너리 자체의 제품 코드(`acp/tracker.rs` 271KB · `prompt_images.rs` 179KB · `voice/` · `credit_bar.rs` · `diagnostics.rs` 96KB 등)이지 **위젯 라이브러리 코드가 아니다**. oxi-tui는 AGENTS.md가 명시하는 **"순수 위젯 라이브러리 · oxi-* 의존성 없음 · 자체 도메인 타입(`ChatMessage`/`MessageRole`/`ContentBlock`)"** 정체성을 가지며, grok은 TUI를 제품 셸 안에 인라인으로 끼워 넣는다. 스케일의 격차는 "grok이 앞섰다"가 아니라 **"재사용성 vs 표현력"의 설계 선택 결과**. 이 차이를 인정한 위에서, **진정한 위젯 라이브러리 갭 5개**(OSC8 · 컬러 레벨 적응 · 스트리밍 마크다운 checkpoint · tmTheme 코드 하이라이트 · PTY e2e 테스트)를 additive 이식하는 것이 정답. 1차 보고서의 "TUI는 부수적" 평가를 TUI 관점에서 정정.

---

## 요약

`grok`(binary: `xai-grok-pager`)는 xAI 사내용 터미널 코딩 에이전트로, TUI를 **5개 전용 크레이트**로 분리한다:

| 크레이트 | LOC | 역할 |
|---|---:|---|
| `xai-grok-pager` (binary) | 393,203 | **제품 셸** — views/ + scrollback/ + slash/ + ACP(271KB) + voice + credit_bar + diagnostics. 위젯 라이브러리 코드가 아님 |
| `xai-grok-pager-render` | 35,610 | 프레임 렌더 · OSC8 링크 · 이미지/비디오 오버레이 · clipboard |
| `xai-grok-markdown` | 20,061 | 스트리밍 마크다운 + checkpoint + LaTeX + syntect |
| `xai-ratatui-textarea` | 12,610 | 408KB 텍스트 에디터 위젯 (atomic elements · 마우스 · 클립보드) |
| `xai-ratatui-inline` | 2,979 | 인라인 뷰포트 + native scrollback 보존 |
| `xai-grok-mermaid` | 1,990 | 다중 백엔드 머메이드 (pure Rust + mmdc + raster) |
| **합계** | **~466K (binary 포함) / 73K (lib만)** | oxi-tui와의 **공정 비교는 73K vs 20K = 3.7x**. 466K는 제품 코드까지 합한 것 |

oxi-tui는 **단일 크레이트 19,979 LOC**로 동일 도메인(채팅 위젯, 마크다운, 머메이드, 테마)을 컴팩트하게 담는다. 핵심 차이: **grok은 '터미널 통합형 UX'(OSC8 링크, 인라인 뷰포트, native scrollback, 마우스 선택)를 추구**하고, **oxi-tui는 'self-contained 위젯 라이브러리'(독자적 테마 시스템, 글리프 프리셋, DiffBackend)를 추구**한다. 둘은 다른 문제를 풀고 있으며, grok의 접근이 항상 우위인 것은 아님 — 테마 일관성과 심볼 일관성에서는 **oxi가 더 깔끔하다**.

---

## 비교 매트릭스 — TUI 기능별

| 영역 | oxi-tui | grok | 우위 |
|---|---|---|---|
| **전체 LOC** | 19,979 | 466,453 (binary 포함) / 73,250 (lib만) | grok 3.7–23x |
| **마크다운 렌더링** | `widgets/chat/markdown.rs` (17KB) + `widgets/chat/highlight.rs` (9.7KB) | `xai-grok-markdown` (20K LOC, 전용 크레이트) — streaming + checkpoint | **grok 우위** |
| **스트리밍 최적화** | `DiffBackend`(라인 체크섬 diff + CSI 2026 sync) — 전체 프레임은 매번 그리지만 변경 라인만 전송 | checkpoint 기반 "tail 재렌더만" — 변경 없는 과거 히스토리는 reflow 자체를 안 함 | **grok 우위** (알고리즘 레벨) |
| **머메이드 렌더링** | `render/mermaid.rs` (85KB, 2,608 LOC) — pure-Rust, 4종(flowchart/sequence/state/class), ASCII art only | `xai-grok-markdown/src/mermaid.rs` (164KB) + `xai-grok-mermaid` (1,990 LOC, raster/mmdc/pure 3백엔드) + `app/mermaid_worker.rs` (96KB) | **grok 우위** (이미지 백엔드) |
| **LaTeX** | `render/latex.rs` (12.9KB, internal `pub(crate)`) | `latex_delimiters.rs` (52KB) + pretty 모드 Unicode 변환(`E=mc²`) | **grok 우위** |
| **하이퍼링크** | ❌ (URL을 일반 텍스트로 렌더) | `render/osc8.rs` (67.8KB) + `hyperlinks.rs` (32.2KB) + `url_scan.rs` (16.7KB) — OSC8 clickable + 절대경로 파일 링크 + `linkify` 기반 URL 검출 + `is_safe_to_open` 스킴 필터 | **grok 독점** |
| **컬러 레벨 적응** | ❌ (truecolor 가정) | `xai-grok-markdown/src/colors.rs` — `ColorLevel::{None,Basic,Ansi256,TrueColor}`, `supports-color` crate 기반, `NO_COLOR`/`COLORTERM`/tmux-ssh-mosh 복구 | **grok 독점** |
| **테마 시스템** | `theme.rs` (75KB, 1,907 LOC) — 28개 semantic ColorScheme 슬롯 + 6 내장 scheme(dark/light/nord/catppuccin/github_dark/monokai) + ThemeManager hot-reload(TOML/JSON) | `.tmTheme` (TextMate) — `grok-day/night`, `tokyo-night` + 별도 `appearance/config.rs` (92KB) + `glyphs.rs` (29.4KB) | **oxi 우위**(일관성) / grok 우위(생태계) |
| **심볼/글리프 시스템** | `symbols.rs` (905 LOC) — `GlyphSet::{Unicode,Ascii,Nerd}` 프리셋, 모든 UI 심볼이 단일 소스에서 | `glyphs.rs` (820 LOC) — 함수-per-글리프, `is_legacy_windows_console()` 런타임 감지, CP437 폴백(`✓`→`√`) | **oxi 우위**(일관성) / grok 우위(부분 폰트 커버리지) |
| **텍스트 입력 위젯** | `widgets/input.rs` (466 LOC) — 단순 입력 | `xai-ratatui-textarea` (12,610 LOC, `textarea.rs` 9,716 LOC) — atomic `TextElement` (paste/파일참조 분할 불가), clipboard provider trait, 마우스 hover/click 이벤트, `EditCommand`/`EditPlan` undo/redo, `wrapping.rs` (605 LOC) | **grok 압도** |
| **인라인 뷰포트 + native scrollback** | ❌ (alternate screen) | `xai-ratatui-inline` (2,979 LOC) — UI 하단 고정, 히스토리는 터미널 native scrollback으로 flow, RIS 기반 resize, DCS 동기화 | **grok 독점** (패러다임 차이) |
| **마우스 지원** | 제한적 | `input/mouse.rs` (60.5KB) + `scrollback/text_selection.rs` (106.4KB) + `render/text_selection.rs` (3.7KB) — 전체 텍스트 선택/복사 | **grok 압도** |
| **이미지/미디어** | `render/image.rs` (10.6KB) | `prompt_images.rs` (179.2KB) + `wrap_clipboard_image.rs` (11KB) + `inline_media_ffmpeg.rs` (6.4KB) + `render/image_overlay/` + `render/video_overlay.rs` | **grok 압도** |
| **클립보드** | oxi-cli에서 처리 | `clipboard/` mod (1.8KB trust) + `tips/clipboard_focus.rs` (19KB) | **grok 우위** |
| **코드 신택스 하이라이트** | `widgets/chat/highlight.rs` (9.7KB) | `xai-grok-markdown/src/syntax.rs` (211 LOC) + `open_code_highlighter.rs` (439 LOC) — syntect + tmTheme, 스트리밍 tail만 incremental highlight | **grok 우위** |
| **Slash 명령어** | `widgets/chat/` + `oxi-cli/src/tui/slash/` | `slash/` (97KB mod + 48KB registry + 14KB mru + 23KB dropdown view) — MRU 추적, 퍼지 드롭다운 | **grok 우위** |
| **키바인딩** | `keybindings/` (registry 13.9KB + conflict 5.4KB + keys 16.1KB) | `actions/defaults.rs` (54.1KB) + `views/shortcuts_help.rs` (127.4KB) — 127KB 단순 도움말 페이지 | **grok 우위**(스케일) / oxi 우위(설계) |
| **알림/타이틀바** | ❌ (위젯 레이어에 없음 — **정답**) | `notifications/` (title 28.3KB + tmux 2.9KB + sleep 8.7KB + focus 9.7KB + hooks 8.7KB) | **제품 호스트 관심사** — oxi-cli/oxios 책임, oxi-tui에 올 이유 없음 |
| **팁 시스템** | ❌ | `tips/` (clipboard_focus 19KB + ephemeral 14.8KB + plan_nudge + small_screen + ssh_wrap + word_select) | **제품 호스트 관심사** — UX 계층, 위젯 라이브러리와 무관 |
| **진단(LSP)** | ❌ (위젯이 아니라 호스트 통합) | `diagnostics.rs` (96.6KB) | **제품 통합 관심사** — oxi-cli/oxios 별도 모듈이어야 함 |
| **ACP(IDE 통합)** | ❌ (호스트 프로토콜) | `acp/tracker.rs` (271.3KB) + leader_bridge + meta + spawn | **제품 프로토콜** — oxios 영역, oxi-tui는 갖지 않는 것이 정답 |
| **음성 입력** | ❌ (입력 디바이스 관심사) | `voice/` + `xai-grok-voice` 크레이트 | **제품 기능** — oxi-tui 범위 밖 |
| **PTY e2e 테스트** | `TestBackend` (ratatui mock) | `tests/pty_e2e_*.rs` (15+ 파일) + `xai-grok-pager-pty-harness` + `xai-tty-utils` — 실제 PTY로 전송된 bytes 검증 | **grok 압도** |
| **Diff 렌더링** | `render/diff.rs` (4.4KB) | `diff.rs` (49.5KB) + `wrap_filter.rs` (31.9KB) + `wrap_restore.rs` (18.8KB) | **grok 우위** |
| **퍼지 매칭** | `fuzzy.rs` (9KB) — 자체 구현 | `nucleo` (Helix fork) + `fuzzy-matcher` | grok 우위 (외부 의존) |

---

## 설계 철학 차이 — 5가지 발산점

### D0. "순수 위젯 라이브러리" vs "제품 셸 + 인라인 TUI" — 스케일이 아니라 정체성

**공통 기반 — 둘 다 ratatui + crossterm 위에 구축됨**. 이 점을 먼저 확정해야 아래 발산점들이 "다른 툴킷"이 아니라 **"같은 툴킷 위의 다른 선택"**으로 읽힌다:
- `ratatui = "0.29"` + `ratatui-core = "0.1"` + `crossterm = "0.28"` (grok workspace `Cargo.toml:200-201,124`)
- `xai-grok-pager/Cargo.toml:21`이 `ratatui`를 `features = ["crossterm", "unstable-widget-ref"]`로 직접 사용 — `unstable-widget-ref`는 `WidgetRef`/`StatefulWidgetRef` trait 접근용 (grok의 textarea가 씀, `textarea.rs:10-11`)
- `ansi-to-tui`, `tui-scrollbar` 등 ratatui 생태계 companion crate도 grok이 그대로 사용
- **하나의 포크**: `xai-ratatui-inline/NOTICE:1-5`가 ratatui derived code를 명시. inline viewport에 필요한 백버퍼/뷰포트 위치/resize 계산 내부 API가 upstream에 없어서 **`Terminal` struct 하나를 포크**. ratatui 전체를 대체한 게 아님.

oxi-tui와 grok는 **ratatui + crossterm이라는 동일한 기반** 위에서, (a) 백엔드 구현(DiffBackend vs pager-render), (b) 커스텀 위젯(chat vs textarea), (c) 마크다운 파이프라인(17KB vs 전용 크레이트)을 다르게 구성한 것. 이 문서의 비교는 모두 **"같은 기반 위의 어떤 계층을 더 얹았는가"**를 다룬다.

이 문서의 다른 모든 발산점(D1–D4)을 해석하기 위한 **메타 프레임**. 읽기 전에 이것부터 숙지할 것.

- **oxi-tui** (AGENTS.md `oxi-tui` 섹션 인용):
  > Built on `ratatui` + `crossterm`. **No oxi-* dependencies** — pure widget library. [...] The widget layer defines its own domain types (`ChatMessage`, `MessageRole`, `ContentBlock`) so it can be reused by any product that wants the chat UX. Products implement the conversion (one `From` impl per direction) in their own composition root.

  즉, oxi-tui는 **LLM 메시지 타입(`oxi-ai::Message`)에 의존하지 않는다**. `ChatMessage`라는 자체 타입을 쓰고, 제품(oxi-cli 또는 외부 oxios)이 `impl From<oxi_ai::Message> for ChatMessage`를 composition root에서 제공한다. 이 규칙이 강제하는 것: (a) 위젯은 host agent/runtime을 몰라야 하고, (b) ACP · IDE 통합 · voice · credit 시스템 · LSP 진단 같은 **제품 관심사는 oxi-tui에 올 수 없다** — oxi-cli 또는 oxios 호스트 책임이다. oxi-tui의 20K LOC는 이 정체성 하에서 최대치에 가깝다.

- **grok**: `xai-grok-pager`는 binary + library를 한 크레이트에 섞었고, 그 393K LOC 중 상당수가 제품 코드다:
  - `acp/tracker.rs` 271.3KB — Agent Client Protocol(IDE 통합)의 제품 구현
  - `prompt_images.rs` 179.2KB + `inline_media_ffmpeg.rs` 6.4KB — 멀티미디어 처리(제품 기능)
  - `voice/` + 별도 `xai-grok-voice` 크레이트 — 음성 입력(제품 기능)
  - `credit_bar.rs` 29.1KB — xAI 크레딧 표시(제품 비즈니스 로직)
  - `diagnostics.rs` 96.6KB — LSP 진단 표시(제품 통합 관심사)
  - `notifications/` 70KB+ — tmux 통합, 타이틀바, sleep/focus(제품 호스트 동작)
  - `app/mermaid_worker.rs` 96.1KB — 백그라운드 워커(제품 런타임)
  - `views/tasks_pane.rs` 110KB · `views/shortcuts_help.rs` 127KB — 제품 전용 패널
  - 이들을 빼고 **순수 라이브러리 코드**(pager-render + textarea + inline + markdown + mermaid)만 세면 73K LOC.

- **시사**: oxi-tui가 "부족해서" 20K인 것이 아니다. **정체성상 20K가 자연스럽다**. 매트릭스의 "grok 독점" 라벨 중 상당수는 (음성, ACP, 진단, 크레딧 표시, 타이틀바) oxi-tui가 **갖지 않는 것이 정답**인 제품 관심사다. 이 문서의 5개 이식 후보는 **oxi-tui의 정체성을 존중하는 진정한 위젯 갭**만을 선별했다 — 제품 기능(voice, ACP, prompt_images 등)은 후보에서 의도적으로 배제했다(후보 선정 근거는 아래 각 항목의 "대상" 참조).

### D1. "터미널은 캔버스" vs "터미널은 파트너"

- **oxi-tui**: ratatui의 alternate-screen 모델을 그대로 사용. 터미널은 그려질 캔버스이고, 모든 상태/스크롤백/선택 영역은 oxi 내부에 산다. 단순하고 예측 가능하며, terminal 외부 동작(tmux 스크롤, 마우스 선택)과 충돌하지 않음. **대신 사용자가 익숙한 터미널 기능을 잃음** — 마우스로 텍스트 선택하면 ratatui의 cell 정보가 아니라 raw ANSI를 가져옴.
- **grok**: 터미널과 **통합**. `xai-ratatui-inline`(README:138)이 명시하듯, "UI를 하단에 고정하고 히스토리는 터미널 native scrollback으로 흘러보내는" 패러다임. 사용자는 `tmux`/`less`/iterm2의 검색으로 과거를 탐색. OSC8로 URL을 클릭 가능하게, 마우스로 grok 내부 콘텐츠를 선택 후 클립보드로.
- **시사**: grok 방식은 UX 파괴력이 크지만, **터미널 의존도가 극심** — `xai-ratatui-inline/README.md:99-121`가 "resize 시 RIS(ESC c)로 전체 리셋 후 재출력"라는 넉백 해법을 쓰는 것에서 알 수 있듯, 터미널별 reflow 차이를 감당하는 비용이 큼. oxi는 안정성 vs grok은 표현력.

### D2. "글리프 프리셋" vs "터미널 능력 감지"

- **oxi-tui** (`symbols.rs`): `GlyphSet::{Unicode,Ascii,Nerd}` 세 개의 preset. 사용자가 설정에서 하나를 고르면 **모든 UI 심볼이 일괄 교체**. AGENTS.md의 Pitfalls에 명시된 "Never hardcode a symbol — read it from the symbol table" 규칙과 정합. 일관성 최우선.
- **grok** (`glyphs.rs`): 각 글리프 함수가 `is_legacy_windows_console()`을 호출해 **터미널별 폴백**. `✓`는 legacy ConHost에서 CP437 `√`(U+221A)로, `⧉`는 `c`로, `❯`는 `>`로. 부분 폰트 커버리지(어떤 폰트는 `✓`는 있지만 `⧉`는 없는)를 우아하게 처리. **대신 각 함수가 자체 결정하므로 전체 일관성은 없음**.
- **시사**: oxi는 "모두 ASCII" 또는 "모두 Unicode"만 선택 가능. 사용자 폰트가 일부만 커버하면 tofu가 섞임. grok은 그 문제를 회피하지만, 사용자가 Nerd Font를 강제할 수 없음. **정답은 하이브리드** — preset + capability detection 조합. (후보 5)

### D3. "단일 DiffBackend" vs "checkpoint 스트리밍"

- **oxi-tui** (`render/mod.rs:1-13`): 매 프레임 ratatui의 `draw()`를 호출하되, `DiffBackend`가 **라인 단위 u64 체크섬 diff**로 변경된 행만 crossterm으로 전송. CSI 2026 synchronized update로 tearing 방지. 깔끔하고 ratatui API를 그대로 유지. 단점: **ratatui가 매 프레임 전체 버퍼를 계산**해야 하므로 긴 응답(10K+ 토큰)에서 CPU가 올라감.
- **grok** (`xai-grok-markdown/lib.rs:1-12`): `StreamingMarkdownRenderer`가 checkpoint를 기준으로 **안정화된 앞부분은 재렌더하지 않음**. tail만 다시 파싱/렌더. `open_code_highlighter.rs`(439 LOC)가 닫힌 코드 블록은 캐시된 하이라이트를 재사용하고 열린 tail만 incremental highlight. CPU가 응답 길이에 비례하지 않음.
- **시사**: oxi의 DiffBackend는 **전송** 최적화이고, grok의 checkpoint는 **연산** 최적화. 1K 토큰 응답은 차이 무의미, 50K 토큰 응답(코드베이스 전체 요약 등)에서 grok 방식이 큰 차이. **이식 가치 높음**. (후보 1)
- **수렴 (convergence)**: 두 접근 모두 **CSI 2026 synchronized output** (a.k.a. DCS protocol)로 flicker를 잡는다 — oxi-tui `DiffBackend` (`render/mod.rs:13`)와 grok `xai-ratatui-inline` (`README.md:28` "Flicker-free rendering using DCS protocol")이 **독립적으로 같은 anti-flicker 기법에 도달**. 즉 DiffBackend가 flicker 면에서 뒤처진 게 아님 — 같은 프로토콜을 **다른 레이어**에 적용할 뿐: DiffBackend는 full-frame redraw의 변경 라인만 동기화하고, inline viewport는 incremental print 배치를 동기화. D3의 차이는 "flicker 있음 vs 없음"이 아니라 "전송 최적화 vs 연산 최적화" 한 축에서만 발생한다. → 이 수렴은 D0의 "같은 기반, 다른 계층" 프레임을 역으로 뒷받침한다.

### D4. "semantic 색상 슬롯" vs "tmTheme 생태계"

- **oxi-tui** (`theme.rs`): 28개의 이름 붙여진 semantic 슬롯(`response_bg`, `thinking_bg`, `surface_bg`, `panel_bg`, `diff_add_bg` 등). AGENTS.md가 명시하는 brightness 계층(`background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg`)을 강제. 모든 색이 의미론적으로 추상화되어 있어, 테마를 바꾸면 UI 전체가 일관되게 변함. **훌륭한 설계**.
- **grok**: `.tmTheme`(TextMate)를 코드 하이라이트용으로만 쓰고, UI 크롬 색은 `appearance/config.rs`(92KB)가 별도 관리. tmTheme는 syntect 생태계의 사실상 표준이라 **수백 개의 기존 테마**(tokyo-night, dracula, solarized, ...)를 그대로 가져올 수 있음. oxi는 자체 TOML/JSON 스키마로 매번 다시 그려야 함.
- **시사**: oxi의 접근이 **소프트웨어 엔지니어링 관점에서 더 나음**(강제된 일관성). grok의 접근이 **사용자 선택 폭 관점에서 더 나음**(생태계 재사용). oxi에 tmTheme를 **코드 블록 전용**으로 추가하면 양쪽 잇점을 모두 취함. (후보 4)

---

## 적용 후보 (oxi-tui 관점, 5개)

### 1. [high] OSC8 클릭커블 하이퍼링크 + URL/절대경로 자동 링크

- **대상**: `oxi-tui/src/render/osc8.rs` (신규, ~400 LOC) + `oxi-tui/src/widgets/chat/markdown.rs` (링크 렌더 분기) + `oxi-tui/src/widgets/chat/render.rs` (tool output의 파일 경로 링크)
- **현재 상태**: oxi-tui는 URL을 일반 텍스트로 렌더. 사용자가 링크를 클릭할 수 없고, 터미널이 자체 링크 감지를 안 하면 복사도 어려움.
- **근거**: grok `xai-grok-pager-render/src/render/osc8.rs:17-65`가 제시하는 **`LinkTarget::{Url, File}` 이원 모델**이 핵심. 단순 URL만이 아니라 **출력에 등장하는 절대 파일 경로**도 클릭커블하게 만들 수 있음 — tool 결과의 `/Volumes/MERCURY/PROJECTS/oxi/oxi-tui/src/lib.rs:42` 같은 라인 참조를 클릭하면 에디터로 점프. OSC8 escape(`\e]8;;URL\e\\TEXT\e]8;;\e\\`)는 iTerm2/WezTerm/Kitty/Windows Terminal/Alacritty가 지원 (ANSI WG 표준).
- **이식 표면**:
  - `osc8.rs:17-65`의 `LinkTarget`/`LinkPresentation`/`ResolvedLinkTarget` 트리오 그대로 — `osc8_url`(터미널 소유) vs `open_target`(앱 소유) 분리가 깔끔
  - `linkify` crate(`LinkFinder`)는 이미 Rust 표준 — workspace dep 추가만으로 URL 검출
  - `link_opener::is_safe_to_open` 패턴 채택 — `javascript:`, `file://` 등 위험 스킴 거부 허용 리스트
  - DiffBackend의 `build_row`에 OSC8 bytes를 셀 데이터에 포함하는 확장 필요
- **리스크**: OSC8 미지원 터미널(구 tmux, 구 screen)에서는 텍스트만 보임 — 자동 폴백이라 기능 손실 없음. **테스트 필수**: 지원 터미널 목록 하드코딩 금지, 시도 후 미지원 감지 시 자동 비활성화.

### 2. [high] 스트리밍 마크다운 checkpoint 렌더러 (재렌더 비용 선형 → 상수)

- **대상**: `oxi-tui/src/widgets/chat/markdown.rs` (현재 17KB) → 분해. 또는 별도 `oxi-tui/src/render/streaming_md.rs` (신규)
- **현재 상태**: 매 프레임 전체 응답을 재파싱/재렌더. DiffBackend가 전송은 최적화하지만 **파싱/하이라이트/레이아웃 계산은 매번 발생**. 10K+ 톡큰 응답에서 CPU 점유 육안 확인.
- **근거**: grok `xai-grok-markdown/lib.rs:30-46`의 구조 — `buffers`/`checkpoint`/`output`/`parse`/`render` 분리. `StreamingMarkdownRenderer::push_and_render(token)`가 들어올 때마다 **안정화 경계(checkpoint)까지만 재렌더**하고 tail은 증분. `open_code_highlighter.rs:1-50`가 닫힌 코드 블록은 캐시, 열린 tail만 syntect incremental highlight.
- **이식 표면**:
  - `Checkpoint` 추상 — 마크다운 AST의 "stable boundary"(예: 빈 줄 다음, 닫힌 코드 블록 끝)를 식별
  - `MarkdownBuffers` — 줄 단위 렌더 결과 버퍼, tail만 교체
  - oxi의 기존 `widgets/chat/markdown.rs`는 thin adapter로 축소, 실제 로직은 새 모듈로
- **리스크**: 기존 `tool_renderer.rs`와의 결합 재설계 필요. Markdown AST 경계 판단이 틀리면 stable 부분이 깜빡임. **최소 테스트**: 100K 토큰 더미 응답에서 CPU 프로파일 비교 before/after.

### 3. [medium] 컬러 레벨 적응 — truecolor 자동 다운그레이드

- **대상**: `oxi-tui/src/render/color_level.rs` (신규, ~250 LOC) + `theme.rs`의 `Color` 직렬화 경로
- **현재 상태**: oxi-tui는 truecolor(Rgb)를 가정. 16-color 터미널이나 `TERM=linux` 콘솔에서 색이 깨짐. 사용자가 수동으로 테마를 바꿔야 함.
- **근거**: grok `xai-grok-markdown/src/colors.rs:11-90`의 `ColorLevel::{None, Basic, Ansi256, TrueColor}` 4단계 모델. `supports-color` crate가 `COLORTERM`/`TERM`/`NO_COLOR`/`ITERM_SESSION_ID`를 종합 판단. **핵심 통찰**: tmux/SSH/mosh가 `COLORTERM=truecolor`를 떨어뜨리는 문제를 `terminal_supports_truecolor()`로 복구(grok colors.rs:78-86).
- **이식 표면**:
  - `ColorLevel` enum + `detect_color_level()` (OnceLock 캐시)
  - `Rgb(r,g,b) → Ansi256` 변환: xterm 216 cube 매핑 + grayscale 24단계 (grok colors.rs 후반부 참조)
  - `Ansi256 → Basic` 변환: 16색 fallback 휴리스틱
  - `oxi-tui::Color`에 `to_terminal_color(level: ColorLevel)` 메서드 추가
- **리스크**: 거의 없음. additive, 기존 truecolor 경로는 그대로. `NO_COLOR` 환경변수 존중 — `ColorLevel::None` 시 모노크롬.

### 4. [medium] tmTheme 로딩으로 syntect 생태계 테마 흡수 (코드 블록 전용)

- **대상**: `oxi-tui/src/widgets/chat/highlight.rs` (9.7KB → ~15KB) + `oxi-tui/src/theme.rs` (`Theme::code_theme: Option<CodeTheme>` 필드 추가)
- **현재 상태**: oxi-tui의 코드 하이라이트는 자체 구현. 사용자가 tokyo-night, dracula, solarized 등 인기 테마를 적용하려면 oxi 포맷으로 다시 정의해야 함.
- **근거**: grok `xai-grok-pager-render/assets/{grok-day,grok-night,tokyo-night}.tmTheme` (각 39.9KB) — TextMate 포맷의 사실상 표준. syntect의 `ThemeSet::load_tmbundle`/`load_from_reader`가 바로 읽음. 수백 개의 기존 tmTheme가 GitHub에 공개되어 있음(textmate themes 저장소 등).
- **이식 표면**:
  - `oxi-tui::CodeTheme` enum 추가 — `Preset(OxiBuiltIn)` | `TmTheme(PathBuf)`
  - syntect workspace dep 추가 (이미 oxi가 사용 중인지 확인 필요 — pulldown-cmark만 있음)
  - `ThemeFile`에 `code_theme` 필드 (옵션, 기본 oxi 내장)
- **리스크**: oxi의 semantic 색상 슬롯 철학과 충돌 — 코드 블록만 예외적으로 tmTheme를 쓴다는 명시적 분리 필요. 사용자가 tmTheme를 UI 전체에 적용하려 하면 안 됨(`code_fg`/`code_bg`는 여전히 oxi `Theme` 소유). 문서화 필수.

### 5. [medium] PTY 기반 e2e 테스트 하네스 — `TestBackend` 한계 돌파

- **대상**: `oxi-tui/tests/pty_e2e/` (신규 디렉토리) + `oxi-tui/tests/pty_harness.rs` (~300 LOC)
- **현재 상태**: oxi-tui 테스트는 ratatui의 `TestBackend`로 버퍼 셀 값을 검증. **실제 터미널에 출력되는 ANSI bytes는 검증 안 됨** — crossterm 버전업, 새 터미널 지원 추가, OSC8/이미지 도입 시 회귀를 못 잡음.
- **근거**: grok `xai-grok-pager/tests/pty_e2e_*.rs`(15+ 파일) + `xai-grok-pager-pty-harness`. real PTY를 열어 바이너리를 spawn하고 출력 bytes를 파싱해 단언. `pty_e2e_clipboard.rs`, `pty_e2e_minimal.rs`, `pty_e2e_config_ui.rs`, `pty_e2e_persistence.rs`, `pty_e2e_queue.rs`, `pty_e2e_scroll_selection.rs` 등 — 각 기능별 시나리오.
- **이식 표면**:
  - `portable-pty` crate (grok도 사용, workspace dep 가능)
  - `PtySession::spawn(args)` → `read_until(pattern, timeout)` → `assert_output_contains(...)`
  - oxi-cli 바이너리를 대상으로 하는 게 자연스러움 — `oxi-tui`만 단독으로 spawn하기 어려움. 따라서 **테스트 디렉토리는 `oxi-cli/tests/pty_e2e/`**가 더 적합
- **리스크**: CI 환경(`ubuntu-latest`)에서 PTY 할당 이슈 — `tty.IsEnabled()` 체크. macOS runner에서만 돌아가게 할지, Linux에서 `script`/`tmux`로 랩할지 결정 필요. grok은 pty_harness를 자체 크레이트로 분리해 재사용.

---

## 위험 / 검증

### 후보 1 (OSC8)
- **깨질 수 있는 것**: OSC8 미지원 터미널에서 escape bytes가 화면에 노이즈로 보일 수 있음 → 폴백 감지 실패 시.
- **최소 테스트**:
  - `test_osc8_emits_correct_escape` — URL이 `\e]8;;URL\e\\TEXT\e]8;;\e\\` 형태로 출력
  - `test_unsupported_terminal_falls_back_to_plain_text` — 환경변수로 미지원 흉내, 일반 텍스트만 출력 확인
  - `test_dangerous_scheme_rejected` — `javascript:` URL은 링크 처리 안 함
  - `test_absolute_path_becomes_file_link` — `/abs/path/file.rs:42` 패턴 감지
- **clippy 잡나?** 예 — `anstyle`/`linkify` 모두 clippy clean.

### 후보 2 (streaming checkpoint)
- **깨질 수 있는 것**: checkpoint 경계가 틀리면 stable 앞부분이 깜빡임. tool_call 결과와 assistant text가 interleaved될 때 token atomicity 깨짐.
- **최소 테스트**:
  - `test_checkpoint_stable_until_newline` — 빈 줄 전까지는 재렌더 안 함
  - `test_open_code_block_rehighlights_only_tail` — 닫힌 코드는 캐시, 열린 tail만 새로 하이라이트
  - `test_50k_token_response_cpu_profile` — 기존 대비 CPU 50%+ 절감
- **clippy 잡나?** 아니오 — 알고리즘 변경이라 clippy 무관. **수동 프로파일링 필수**.

### 후보 3 (color level)
- **깨질 수 있는 것**: 잘못된 다운그레이드 매핑이 색을 왜곡. 기존 truecolor 사용자가 오탐으로 256색 강등.
- **최소 테스트**:
  - `test_detect_truecolor_via_colorterm` — `COLORTERM=truecolor` → TrueColor
  - `test_tmux_stripped_colorterm_recovers_via_term` — `TERM=tmux-256color` + `ITERM_SESSION_ID` 존재 → TrueColor
  - `test_no_color_env_disables` — `NO_COLOR=1` → None
  - `test_rgb_to_ansi256_cube_mapping` — `Rgb(255,0,0)` → `Ansi256(196)` 등 검증
- **clippy 잡나?** 예.

### 후보 4 (tmTheme)
- **깨질 수 있는 것**: tmTheme scope selector가 oxi의 마크다운 AST와 매칭 안 됨. UI 색상까지 침범해 oxi 일관성 깨짐.
- **최소 테스트**:
  - `test_load_tokyo_night_tmtheme` — 표준 테마 로드 성공
  - `test_tmtheme_only_affects_code_blocks` — UI 색상(`primary`, `border` 등)은 영향 안 받음
  - `test_invalid_tmtheme_path_falls_back` — 잘못된 경로 시 기본 테마 유지
- **clippy 잡나?** 예.

### 후보 5 (PTY e2e)
- **깨질 수 있는 것**: CI 환경에서 PTY 할당 실패로 테스트가 전체 fail. macOS vs Linux PTY 동작 차이.
- **최소 테스트**:
  - `test_pty_minimal_boot` — `oxi` 실행 후 첫 프롬프트가 PTY에 보임
  - `test_pty_sends_message_and_receives_response` — 입력 → LLM mock → 응답 렌더
  - `test_pty_resize_triggers_clean_redraw` — SIGWINCH 후 잔상 없음
- **clippy 잡나?** 무관 — 테스트 코드.

---

## 마이그레이션 로드맵 (제안)

| 단계 | 후보 | 의존 | 위험 | 기간 추정 |
|---|---|---|---|---|
| **1** | 후보 3 (color level) | 없음 | 낮음 | ~2일, 1 PR |
| **2** | 후보 1 (OSC8) | 후보 3과 독립 | 중간 (폴백 감지) | ~5일, 1 PR |
| **3** | 후보 4 (tmTheme) | 후보 3 (색상 다운그레이드를 tmTheme에도 적용) | 낮음 | ~3일, 1 PR |
| **4** | 후보 2 (streaming checkpoint) | 독립 | 높음 (기존 렌더 재설계) | ~10일, 1 PR |
| **5** | 후보 5 (PTY e2e) | 독립 — 다른 후보 검증에도 쓰임 | 중간 (CI 환경) | ~5일, 1 PR |

**권장 순서**: 3 → 1 → 4 → 5 → 2. 후보 2는 가장 파괴적이므로 나중에. 후보 5는 후보 1/3 검증에 쓸 수 있어 일찍 도입하면 좋지만, 초기 도입 비용이 있어 후보 4 이후 추천.

**시기**: 1차 보고서의 권고를 유지 — v0.56은 안정화, v0.57+에서 후보 3/1부터 착수. 다만 **후보 3 (color level)**과 **후보 4 (tmTheme)**는 매우 독립적이라 v0.56 patch release에 끼워 넣어도 무방.

---

## 결론

**핵심부터**: grok과 oxi-tui는 같은 도메인(터미널 코딩 에이전트의 UI)을 풀면서도 **다른 정체성**을 선택했다. grok은 "TUI를 제품 셸에 인라인으로 흡수"하는 방향(393K LOC의 binary 코드가 증거), oxi-tui는 "재사용 가능한 순수 위젯 라이브러리(oxi-* 의존성 없음, 자체 도메인 타입)"를 고수하는 방향이다. **두 설계 모두 정당하며, 어느 쪽이 "앞섰다"가 아니다** — 트레이드오프가 다를 뿐. oxi-tui의 20K LOC는 결핍이 아니라 정체성의 결과다.

그 정체성을 인정한 위에서, grok의 TUI가 보여주는 **"터미널과의 통합"** 기법(OSC8 하이퍼링크, 인라인 뷰포트, native scrollback 보존, 마우스 텍스트 선택, PTY e2e 테스트) 중 **oxi-tui의 정체성과 충돌하지 않는 additive 이식**이 가능한 것들을 선별했다. 아래 세 가지는 **진정한 위젯 라이브러리 갭**이자 이식 가치가 확실하다:

1. **OSC8 하이퍼링크** — 단순 URL 링크를 넘어 tool 결과의 파일 경로까지 클릭커블하게. 사용자 워크플로우에 즉각적 영향.
2. **컬러 레벨 적응** — `TERM=linux`, `NO_COLOR`, tmux-mosh 환경 사용자를 위한 기본적 correctness.
3. **PTY e2e 테스트** — 향후 도입할 OSC8/이미지/인라인 뷰포트 같은 기능의 회귀를 잡을 수 있는 기반 시설.

반면, 다음은 **이식 보류**를 권한다:

- **`xai-ratatui-inline` (인라인 뷰포트)** — RIS 넉백 전략이 보여주듯 터미널 의존도가 극심. oxi의 "안정적이고 예측 가능한 위젯" 정체성과 충돌. oxios 별도 제품에서 실험할 것.
- **`xai-ratatui-textarea` (408KB 풀 에디터)** — atomic element / 클립보드 provider trait / 마우스 hover 이벤트 등 훌륭한 설계지만, 현재 oxi 입력 위젯(466 LOC)과 단절이 큼. 마이그레이션 비용이 이식 후보 2개 분량. 사용자 요구가 명확해질 때 보류.
- **`prompt_images.rs` (179KB 미디어 파이프라인)** — Kitty/iTerm2 graphics protocol은 매력적이지만 사용자 수요가 확인되기 전엔 과투자.
- **`acp/tracker.rs` (271KB IDE 통합)** — oxios 영역. oxi-tui는 위젯 라이브러리지 IDE가 아님.
- **`diagnostics.rs` (96KB LSP)** — 마찬가지로 oxi-cli의 별도 관심사. oxi-tui로 올 이유 없음.
- **`voice/`** — 범위 밖.

oxi-tui의 기존 설계(semantic theme slots, GlyphSet preset, DiffBackend)는 **버려야 할 것이 아니라 확장해야 할 기반이다**. grok이 제시하는 가치를 oxi의 철학 안에 흡수하는 것이 정답이다.

---

## 부록 — 읽은 파일 목록

**grok 소스**:
- `Cargo.toml` (workspace, 1-300)
- `crates/codegen/xai-grok-pager/src/` 디렉토리 트리 전수 스캔
- `crates/codegen/xai-grok-pager-render/src/lib.rs`, `glyphs.rs:1-123`, `render/` 디렉토리, `appearance/` 디렉토리
- `crates/codegen/xai-grok-pager-render/src/render/osc8.rs:1-63`
- `crates/codegen/xai-ratatui-inline/README.md` (전체)
- `crates/codegen/xai-ratatui-textarea/src/lib.rs`, `textarea.rs:1-103`
- `crates/codegen/xai-grok-markdown/src/lib.rs:1-179`, `render.rs:1-93`, `style.rs:1-114`, `colors.rs:1-90`
- `crates/codegen/xai-grok-mermaid/src/` 디렉토리
- LOC 총합: xai-grok-pager 393,203 / xai-grok-pager-render 35,610 / xai-ratatui-textarea 12,610 / xai-ratatui-inline 2,979 / xai-grok-markdown 20,061 / xai-grok-mermaid 1,990

**oxi-tui 검증**:
- `oxi-tui/src/lib.rs` (전체)
- `oxi-tui/src/theme.rs:1-93`
- `oxi-tui/src/render/mod.rs:1-123`
- `oxi-tui/src/render/mermaid.rs:1-83`
- `oxi-tui/src/widgets/chat/` 디렉토리 (8 파일: layout/state/types/render/markdown/mod/highlight/dashboard)
- LOC 총합: 19,979

**이전 보고서 참조**:
- `docs/ref-porter/xai-org-grok-build.md` (memory/compaction/permission 중심, TUI는 라인 38-40만 언급)
