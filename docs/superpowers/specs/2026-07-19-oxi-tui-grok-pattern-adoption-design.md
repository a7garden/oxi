# oxi-tui: grok 패턴 additive 도입 설계

**날짜**: 2026-07-19
**상태**: 설계 (사용자 승인 대기)
**범위**: oxi-tui 단일 크레이트 내. 다중 크레이트 분리는 명시적으로 기각.
**버전 타겟**: v0.56 patch ~ v0.58

---

## 배경

`docs/ref-porter/xai-org-grok-build-tui.md` 비교분석에서 식별된 5개 패턴을 oxi-tui에 additive 도입한다. **패턴 도입이 1차 목적, 구조 변경은 부수 효과**. grok의 다중 크레이트 구조(`xai-grok-pager` / `-render` / `xai-ratatui-textarea` / `xai-ratatui-inline` / `xai-grok-markdown` / `xai-grok-mermaid`)는 oxi-tui에 그대로 적용하지 않는다 — AGENTS.md의 4-condition 분리 테스트(독립 재사용 · 독립 버전 · 빌드 격리 · 팀 경계)를 4/5가 실패하기 때문.

oxi-tui와 grok 모두 **동일한 기반**(ratatui 0.29 + crossterm 0.28 + 동일한 CSI 2026 synchronized output 프로토콜) 위에 구축됨을 확인했다. 이 설계는 그 기반을 유지하면서, grok이 보여준 **5가지 패턴**을 oxi-tui의 "순수 위젯 라이브러리" 정체성 안에 흡수한다.

---

## 설계 원칙

1. **단일 크레이트 유지** — `oxi-tui`를 분리하지 않는다. 내부 모듈 구조만 정비.
2. **기본 경험 unchanged** — 모든 신규 기능은 opt-in이거나 자동 폴백. 기존 사용자가 설정 변경 없이 동일한 UX를 봐야 한다.
3. **테스트 우선** — 파괴적 변경(후보 2)에 앞서 회귀 테스트 인프라(후보 5)를 먼저 갖춘다.
4. **기존 코드 존중** — `highlight.rs` 등 기존 모듈은 제거하지 않고 확장 지점으로 리팩터링.

---

## 모듈 레이아웃 (변경 후)

```
oxi-tui/src/
├── lib.rs                       [유지]
├── cell.rs                      [수정] +adapt_to(level) 메서드 (후보 3)
├── theme.rs                     [수정] +code_theme: Option<CodeTheme> 필드 (후보 4)
├── symbols.rs                   [유지]
├── render/
│   ├── mod.rs                   [수정] DiffBackend.build_row() OSC8 bytes 인식 (후보 1)
│   ├── color_level.rs           [신규] 후보 3
│   ├── osc8.rs                  [신규] 후보 1
│   └── (ansi/diff/image/latex/mermaid/terminal/deccara 유지)
├── markdown/                    [신규 최상위 모듈]
│   ├── mod.rs                   진입점 + StreamingMarkdownRenderer (후보 2)
│   ├── checkpoint.rs            안정화 경계 식별 (후보 2)
│   ├── buffers.rs               tail 재렌더용 라인 버퍼 (후보 2)
│   ├── style.rs                 MarkdownStyle per-element (후보 2)
│   ├── tmtheme.rs               syntect + tmTheme 로딩 (후보 4)
│   └── open_code.rs             열린 코드 블록 incremental highlight (후보 2)
├── widgets/
│   ├── chat/
│   │   ├── markdown.rs          [수정] thin adapter로 축소, 실제 로직 ../markdown/으로 (후보 2)
│   │   ├── highlight.rs         [수정] dispatcher로 변경, 기존 로직은 Default 백엔드로 유지 (후보 4)
│   │   └── render.rs            [수정] tool output 절대경로 → OSC8 File 링크 (후보 1)
│   └── (tool_renderer/input/list_selector/dashboard/... 유지)
├── keybindings/                 [유지]
└── fuzzy, table_renderer, ...   [유지]

oxi-cli/tests/
└── pty_e2e/                     [신규] 후보 5
    ├── mod.rs
    ├── pty_harness.rs           PtySession::spawn + read_until + assert_output_contains
    ├── minimal.rs               부팅 + 프롬프트 표시
    ├── osc8.rs                  OSC8 escape 배출 검증 (후보 1)
    ├── color_level.rs           256색/16색 강제 시 다운그레이드 (후보 3)
    ├── streaming.rs             50K 토큰 응답 안정성 (후보 2)
    └── resize.rs                SIGWINCH 후 잔상 없음
```

### 왜 `markdown/`을 최상위로 승격하는가

현재 `widgets/chat/markdown.rs`(17KB)는 chat 위젯 내부에 산다. 그러나 마크다운 렌더링은 chat 전용이 아니다:
- `widgets/tool_renderer.rs`(1725 LOC)도 마크다운을 렌더
- 장래 `oxi --print` 모드의 단순 렌더링 경로도 재사용 가능
- 후보 2(streaming checkpoint)가 가세하면 ~1500 LOC 추가 — 단일 파일 불가

최상위 `markdown/` 모듈로 승격하면, chat 위젯은 그냥 또 하나의 소비자가 된다. **이것은 내부 모듈 승격일 뿐 크레이트 분리가 아니다** — 외부 API에는 영향 없음.

### 왜 `highlight.rs`를 삭제하지 않는가

`widgets/chat/highlight.rs`(314 LOC)는 hand-rolled 토크나이저:
- 90줄짜리 `lang_keywords()` 테이블로 ~20개 언어 키워드 매칭
- `line_comment_prefix()`로 줄 주석 감지
- PascalCase / `.` 접두로 타입/메서드 감지
- `TokenType`을 oxi **`ThemeStyles` semantic 슬롯**에 매핑 (`token_style()`)

이것은 syntect 기반 tmTheme와 **완전히 다른 메커니즘**이다. tmTheme가 색을 결정하면 oxi의 semantic 슬롯 일관성이 깨진다. 그래서:
- **기본 백엔드는 `highlight.rs` 유지** — 추가 의존성 없이, 기존 사용자 경험 100% 보존
- **`CodeTheme::TmTheme(PathBuf)` opt-in 시에만** `markdown/tmtheme.rs`가 syntect로 하이라이팅
- `highlight_code()` 시그니처는 동일, 내부적으로 `CodeTheme`에 따라 분기

이 패턴은 "progressive enhancement" — oxi-tui의 기본 정체성(순수 위젯, 의존성 최소)을 지키면서 원하는 사용자만 syntect를 끌어올 수 있다.

---

## 패턴별 상세

### 후보 3 — 컬러 레벨 적응 (v0.56 patch, 첫 번째)

**근거**: oxi-tui는 현재 truecolor(Rgb)를 가정. `TERM=linux` 콘솔, 16색 터미널, `NO_COLOR=1` 환경에서 색이 깨짐.

**구현**:
- `render/color_level.rs` 신규 ~250 LOC
- `pub enum ColorLevel { None, Basic, Ansi256, TrueColor }` (default TrueColor)
- `pub fn detect_color_level() -> ColorLevel` — `OnceLock` 캐시
  - `NO_COLOR` env → None
  - `supports-color` crate로 `COLORTERM`/`TERM` 판단
  - tmux/SSH/mosh가 `COLORTERM=truecolor`를 떨어뜨리는 경우 `ITERM_SESSION_ID`/`TERM_PROGRAM` 등으로 truecolor 복구
- `Rgb → Ansi256` 변환: xterm 216 cube + grayscale 24단계 매핑
- `Ansi256 → Basic` 변환: 16색 fallback 휴리스틱
- `cell::Color::adapt_to(level) -> Color` 메서드 추가

**외부 의존**: `supports-color = "3.0"` (workspace dep)

**리스크**: 거의 없음. additive. 기존 truecolor 경로는 그대로. `NO_COLOR` 표준 존중.

**테스트**:
- `test_detect_truecolor_via_colorterm`
- `test_tmux_stripped_colorterm_recovers_via_term`
- `test_no_color_env_disables`
- `test_rgb_to_ansi256_cube_mapping` (`Rgb(255,0,0)` → `Ansi256(196)`)

---

### 후보 1 — OSC8 클릭커블 하이퍼링크 (v0.57, 병렬 가능)

**근거**: 현재 URL을 일반 텍스트로 렌더. 사용자가 링크 클릭 불가, 복사도 어려움. tool 결과의 파일 경로(`/abs/path/file.rs:42`)도 클릭커블하게 만들면 워크플로우 큰 개선.

**구현**:
- `render/osc8.rs` 신규 ~400 LOC
- `pub enum LinkTarget { Url(Arc<str>), File(Arc<Path>) }` — URL과 파일 경로 양쪽
- `pub enum LinkPresentation { Opaque, SelfResolvingPath }` — 터미널 소유 vs 앱 소유
- `pub struct ResolvedLinkTarget { osc8_url: Option<Arc<str>>, open_target: Option<LinkTarget> }`
- `pub fn is_safe_to_open(url: &str, filter: SchemeFilter) -> bool` — `javascript:`, `file://` 등 위험 스킴 거부
- `render/mod.rs::DiffBackend.build_row()` 수정: cell 데이터에 OSC8 escape bytes (`\e]8;;URL\e\\TEXT\e]8;;\e\\`) 포함
- `widgets/chat/render.rs`: tool output의 절대 경로 + 라인 번호 패턴 감지 → File 링크
- `widgets/chat/markdown.rs`: 마크다운 내 URL/이메일 자동 링크

**외부 의존**: `linkify = "0.10"` (workspace dep)

**지원 터미널**: iTerm2 · WezTerm · Kitty · Windows Terminal · Alacritty · foot · Contour (ANSI WG 표준 CSI 8). 미지원 터미널은 자동 폴백 — 하드코딩된 지원 목록 없이, 출력 후 미지원 감지 시 자동 비활성화.

**리스크**: 미지원 터미널에서 escape bytes가 노이즈로 보일 수 있음. **완화**: 후보 5(PTY e2e)의 `pty_e2e/osc8.rs` 시나리오로 실제 터미널 PTY 출력 검증.

**테스트**:
- `test_osc8_emits_correct_escape`
- `test_unsupported_terminal_falls_back_to_plain_text`
- `test_dangerous_scheme_rejected` (`javascript:` URL 거부)
- `test_absolute_path_becomes_file_link` (`/abs/path/file.rs:42` 감지)

---

### 후보 4 — tmTheme 코드 하이라이트 (v0.57, 후보 1과 병렬)

**근거**: 사용자가 tokyo-night, dracula, solarized 등 인기 테마를 적용하려면 현재 oxi 포맷으로 다시 정의해야 함. syntect + tmTheme는 사실상 표준이라 수백 개 기존 테마를 그대로 가져올 수 있음.

**구현**:
- `markdown/tmtheme.rs` 신규 ~400 LOC
- `pub enum CodeTheme { Preset(OxiBuiltIn), TmTheme(PathBuf) }` — `Preset`이 기본 (기존 highlight.rs 사용)
- `pub struct Syntect { ... }` — syntect ThemeSet 래퍼, `tmTheme` 파일 로드
- `theme.rs::Theme`에 `pub code_theme: Option<CodeTheme>` 필드 추가 (기본 `None` = Preset)
- `widgets/chat/highlight.rs::highlight_code()` 수정: `CodeTheme`에 따라 분기
  - `Preset` → 기존 hand-rolled 경로 (변경 없음)
  - `TmTheme(path)` → `markdown/tmtheme.rs`의 syntect 경로
- **제약**: 코드 블록 전용. UI 색상 슬롯(`primary`, `border`, `code_fg`, `code_bg`)에는 절대 영향 X. `code_fg`/`code_bg`는 oxi `Theme` 소유 유지.

**외부 의존**: `syntect = "5.3"` (workspace dep, **feature gate**)

**Feature gate**:
```toml
[features]
default = ["syntax"]
syntax = ["dep:syntect"]
```
`--no-default-features`로 syntect 제외 시 `CodeTheme::TmTheme`은 컴파일 에러 또는 런타임 무시. 임베디드 사용자나 바이너리 크기 민감 사용자를 위한 옵트아웃.

**리스크**: syntect 빌드 시간/바이너리 크기 증가 (~2-3MB). feature gate로 통제.

**테스트**:
- `test_load_tokyo_night_tmtheme`
- `test_tmtheme_only_affects_code_blocks` (UI 색상 영향 X)
- `test_invalid_tmtheme_path_falls_back_to_preset`
- `test_no_default_features_compiles_without_syntect`

---

### 후보 5 — PTY e2e 테스트 하네스 (v0.56 말~v0.57.0 초, W1 선행 조건)

**근거**: oxi-tui 테스트는 ratatui의 `TestBackend`로 버퍼 셀 값을 검증. 실제 터미널에 출력되는 ANSI bytes는 검증 안 됨 — crossterm 버전업, OSC8 도입, W1(가상 좌표계 + sticky 헤더) 도입 등 **시각적 변경**을 잡을 회귀 인프라가 필요. **R3 정정**: 원래 "v0.58 독립 인프라"로 분류했으나, UX 스펙의 W1 workstream이 렌더 OUTPUT을 바꾸므로(sticky 헤더 추가, viewport 이동) TestBackend snapshot만으로는 flicker/scroll jank를 못 잡음. W1 안전망으로 PTY가 필수. 단, 후보 2(streaming checkpoint)는 렌더 OUTPUT이 아니라 빈도만 바꾸므로 PTY 없이도 self-contained.

**구현**:
- `oxi-cli/tests/pty_e2e/` 신규 디렉토리
- `oxi-cli/tests/pty_harness.rs` ~300 LOC
  - `PtySession::spawn(args)` → real PTY + `oxi` 바이너리 실행
  - `read_until(pattern, timeout)` → 출력에서 패턴 매칭
  - `assert_output_contains(substr)` / `assert_osc8_link_present(url)`
  - `resize(cols, rows)` → SIGWINCH 시뮬레이션
- 시나리오 파일들:
  - `minimal.rs` — 부팅 + 첫 프롬프트 표시
  - `osc8.rs` — 후보 1 회귀 (OSC8 escape 배출, 미지원 터미널 폴백)
  - `color_level.rs` — 후보 3 회귀 (`NO_COLOR=1`, `TERM=linux` 강제)
  - `resize.rs` — SIGWINCH 후 잔상 없음

**대상**: oxi-tui 단독 spawn이 어려우므로 oxi-cli 바이너리로 테스트. oxi-tui 자체 테스트는 여전히 `TestBackend` 유지.

**외부 의존**: `portable-pty = "0.9"` (oxi-cli [dev-dependencies])

**CI 환경**: `ubuntu-latest`에서 PTY 할당 가능 (`tty.IsEnabled()` 체크). `#[cfg(unix)]` 게이트로 Windows 일시 제외. macOS runner matrix 확장은 별도 이슈.

**리스크**: CI 환경 PTY 타이밍 변동. **완화**: 모든 타이밍 의존을 `read_until(pattern, timeout)`으로 명시적 통제. 타임아웃 5초 기본.

---

### 후보 2 — 스트리밍 마크다운 checkpoint 렌더러 (v0.58, W1 이후로 이동)

**근거**: 현재 매 프레임 전체 응답을 재파싱/재렌더. DiffBackend가 전송은 최적화하지만 **파싱/하이라이트/레이아웃 계산은 매번 발생**. 10K+ 토큰 응답에서 CPU 점유 육안 확인.

**구현**:
- `markdown/` 신규 최상위 모듈 (~1500 LOC)
- `markdown/mod.rs` — `pub struct StreamingMarkdownRenderer`
  - `pub fn new(style, pretty) -> Self`
  - `pub fn push_and_render(&mut self, token: &str, syntect: Option<&Syntect>)`
  - `pub fn view(&self) -> MarkdownView` (lines + open_marker)
  - `pub fn finish(&mut self)`
- `markdown/checkpoint.rs` — 안정화 경계 식별
  - 빈 줄 다음, 닫힌 코드 블록 끝, 문단 경계를 stable boundary로 인식
  - 그 이전은 freeze, 이후는 tail 취급
- `markdown/buffers.rs` — 줄 단위 렌더 결과 버퍼 (tail만 교체)
- `markdown/style.rs` — `MarkdownStyle` per-element (`code_inner`, `code_outer`, `em_inner` 등; `_outer`는 pretty 모드에서 hidden)
- `markdown/open_code.rs` — 열린 코드 블록 incremental highlight
  - 닫힌 코드 블록은 캐시된 하이라이트 재사용
  - 열린 tail만 `syntect`增量 highlight

**기존 코드 마이그레이션**:
- `widgets/chat/markdown.rs` (17KB)는 thin adapter로 축소 (~3KB)
  - 존재 이유: chat widget 특화 옵션(라인 번호, 세로 스크롤 offset)을 `StreamingMarkdownRenderer`에 전달
- `widgets/chat/highlight.rs`는 **유지** (후보 4에서 이미 dispatcher로 리팩터됨)
- `widgets/tool_renderer.rs` (1725 LOC) — tool 결과에도 마크다운이 올 수 있으므로, `StreamingMarkdownRenderer`를 재사용하도록 수정

**외부 의존**: 후보 4가 이미 syntect를 끌어옴.

**안전 메커니즘 (자체 완결적, 후보 5에 의존하지 않음)**:
- **Feature flag 토글**: `RUSTFLAGS="--cfg oxi_legacy_render"`로 마이그레이션 중 언제든 기존 full-frame 렌더러로 롤백 가능. review용이며 최종 PR에서 제거.
- **CPU baseline 프로파일**: 후보 2 도입 **전**에 100K 토큰 더미 응답으로 baseline 측정해서 저장. 도입 후 동일 워크로드로 비교, 50%+ 절감을 객관적으로 검증.
- **Snapshot 테스트 (TestBackend)**: 동일 입력에 대해 신/구 렌더러가 byte-identical 출력을 내는지 단위 테스트로 검증. 출력이 다르면 correctness 버그로 간주.
- **Interleaving unit tests**: tool_call 결과와 assistant text가 섞인 픽스처를 직접构造해 atomicity 단위 테스트. checkpoint가 tool_call 블록 내부에서 발화하지 않는지 검증.

**리스크 (높음)**:
1. checkpoint 경계 판단 틀리면 stable 앞부분이 깜빡임 → **완화**: snapshot 테스트로 출력 안정성 검증 + CSI 2026 sync (이미 DiffBackend에 있음)로 flicker 자동 방어
2. tool_call 결과와 assistant text가 interleaved될 때 token atomicity 깨짐 → **완화**: tool_call 블록은 단일 atomic unit으로 취급, 내부에서는 checkpoint 발화 안 함 + interleaving unit test로 검증
3. 기존 `tool_renderer.rs`와 결합 → **완화**: adapter 패턴으로 래핑, tool_renderer 내부 로직은 그대로
4. CPU 절감 효과 불충분 → **완화**: baseline 대비 50% 절감 못 시키면 feature flag로 default off, 다음 마일스톤에서 재시도

**후보 5(PTY e2e)와의 관계**: 후보 5는 후보 2의 안전망이 **아니다** (후보 2는 자체 snapshot 테스트로 self-contained). 단, **UX 스펙의 W1(가상 좌표계 + sticky)은 PTY가 필수** — W1이 렌더 OUTPUT을 바꾸므로. 따라서 후보 5는 W1 직전(v0.56 말)에 도입되며, 후보 2는 W1 이후(v0.58)로 이동하여 렌더 파이프라인 충돌을 회피.

---

## 마이그레이션 순서 (decoupled safety + W1 의존성 반영)

```
v0.55.0 (현재)
   │
   ▼
[patch v0.56.0 / v0.56.1]
   후보 3 (color level)           ← 독립, 낮은 위험
   후보 5 (PTY e2e)               ← W1 직전 도입. W1이 렌더 OUTPUT 바꾸므로 PTY가 안전망 필수
   │
   ▼
[v0.57.0]
   후보 1 (OSC8)                  ← color level 약한 의존
   후보 4 (tmTheme)               ← color level 시너지 (병렬)
   (UX 스펙: W1 가상 좌표계 + FollowMode + sticky 헤더, B5, B7 병렬)
   │
   ▼
[v0.57.1]
   (UX 스펙: B1 scroll normalization, B2 slash dropdown, B6 shortcuts help)
   │
   ▼
[v0.58.0]
   후보 2 (streaming checkpoint)  ← W1 이후로 이동. W1이 LayoutEntry를 u32로 바꾸므로 checkpoint와 충돌 회피 위해 순서 조정
   (UX 스펙: B4 scrollback search — W1 의존)
```

**변경 이력**:
- **v1**: PTY e2e(후보 5)를 v0.58에 두고 후보 2의 안전망으로 삼음 → 안전망이 위험보다 늦게 도착하는 모순
- **v2**: 후보 5를 후보 2 직전으로 올려 test-first → 그러나 후보 5 자체의 CI 리스크(PTY 할당, 타이밍, 플랫폼 게이트)가 가장 가치 높은 패턴을 블록하는 역설 발생
- **v3**: 후보 2의 안전 메커니즘을 후보 5와 **완전 분리**. 후보 2는 feature flag + CPU baseline + snapshot + interleaving unit test로 self-contained. 후보 5는 v0.58에 독립 도입. 핵심 통찰: 후보 2가 바꾸는 것은 **렌더 빈도**지 **렌더 결과**가 아니므로 snapshot + CSI 2026 sync로 충분.
- **v4 (현재, 리뷰 R2/R3 반영)**: UX 스펙의 W1 workstream이 추가되면서 재조정. W1은 렌더 OUTPUT을 바꾸므로(sticky 헤더, viewport 이동) PTY가 필수 — 후보 5를 v0.56 말로 앞당김. 동시에 W1이 LayoutEntry를 u32로 마이그레이션하므로 후보 2(streaming checkpoint)와의 파이프라인 충돌을 피하기 위해 후보 2를 v0.58로 미룸. 핵심 통찰: "렌더 결과가 안 바뀌면 snapshot으로 충분"은 후보 2에만 해당, W1에는 해당 안 함.

---

## 명시적 비목표 (이번 설계에서 배제)

1. **`oxi-tui` 다중 크레이트 분리** — AGENTS.md 4-condition test 4/5 실패. grok의 5-crate는 제품 경계가 동력이라 oxi에 그대로 적용 안 됨.
2. **`xai-ratatui-inline` 포크 도입** — RIS 넉백 전략이 보여주듯 터미널 의존도 극심. oxi-tui 정체성(예측 가능한 위젯)과 충돌. oxios 별도 제품에서 실험 권장.
3. **풀 에디터 위젯 (grok `xai-ratatui-textarea` 408KB급)** — 현재 입력 위젯(466 LOC)과 단절 큼. 마이그레이션 비용이 본 설계 전체 분량. 사용자 요구 명확해질 때까지 보류.
4. **`prompt_images.rs` 미디어 파이프라인** — Kitty/iTerm2 graphics protocol은 매력적이나 사용자 수요 미확인.
5. **LSP 진단 / ACP / 음성** — 모두 제품 관심사. oxi-tui는 위젯 라이브러리지 IDE/호스트가 아님.
6. **`widgets/chat/highlight.rs` 제거** — hand-rolled 토크나이저는 기본 백엔드로 유지. tmTheme는 opt-in 대안일 뿐.

---

## 위험 요약

| 후보 | 주요 위험 | 완화 |
|---|---|---|
| 3 | 잘못된 다운그레이드 매핑 | 단위 테스트 + `NO_COLOR` 표준 준수 |
| 1 | 미지원 터미널 escape 노이즈 | 자동 폴백 + 단위 테스트 (PTY e2e는 후보 5에서 보강) |
| 4 | syntect 빌드 비용, UI 색상 침범 | feature gate + 코드 전용 제약 문서화 |
| 5 | CI PTY 타이밍 | 명시적 타임아웃 + `#[cfg(unix)]` |
| 2 | checkpoint 경계 틀림, atomicity 깨짐 | feature flag 토글 + CPU baseline + snapshot 테스트 + interleaving unit test (self-contained, 후보 5 비의존) |

**가장 치명적인 위험**: 후보 2의 tool_call + assistant text interleaving. **대비**: 마이그레이션 단계에서 tool_call 블록을 atomic unit으로 묶고 내부 checkpoint 발생 금지.

---

## 완료 기준 (acceptance criteria)

각 후보별로:

- **후보 3**: `NO_COLOR=1`로 실행 시 모노크롬. `TERM=linux`에서 16색 폴백. truecolor 터미널에서 기존과 동일.
- **후보 1**: 지원 터미널에서 URL/파일경로 클릭 시 브라우저/에디터 오픈. 미지원 터미널에서 일반 텍스트.
- **후보 4**: 기본값(`Preset`)은 기존과 동일. `TmTheme(path)` 로드 시 코드 블록 색상만 변경, UI unaffected.
- **후보 5**: `cargo nextest run --workspace` + PTY e2e 통과. CI `ubuntu-latest`에서 안정 통과. **W1의 안전망으로 필수** (W1이 렌더 OUTPUT을 바꾸므로 TestBackend만으로는 flicker/scroll jank 검증 불가). 후보 2(streaming checkpoint)의 블로커는 아님 (후보 2는 렌더 빈도만 바꾸므로 snapshot 테스트로 충분).
- **후보 2**: 100K 토큰 더미 응답에서 CPU 50%+ 절감 (baseline 대비). snapshot 테스트로 신/구 렌더러 출력 byte-identical 검증. interleaving unit test로 tool_call 블록 atomicity 검증. feature flag로 마이그레이션 중 언제든 legacy 경로 롤백 가능.

워크스페이스 차원:
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace` 모두 통과
- `cargo build -p oxi-sdk --features native-browser -- -D warnings` 통과 (oxi-tui 변경이 SDK에 영향 X)
- AGENTS.md의 해당 pitfall 업데이트 (새 제약/패턴 명시)

---

## 부록 — 외부 의존 추가 목록

| Crate | 버전 | 용도 | 도입 후보 | feature |
|---|---|---|---|---|
| `supports-color` | 3.0 | 터미널 컬러 능력 감지 | 3 | — |
| `linkify` | 0.10 | URL 검출 | 1 | — |
| `syntect` | 5.3 | 코드 신택스 하이라이팅 | 4 | `default = ["syntax"]` |
| `portable-pty` | 0.9 | PTY e2e 테스트 | 5 | dev-only |

총 4개. 모두 Rust 생태계 표준. `cargo audit`/`cargo deny` 통과 예상.

---

## 참고 문서

- `docs/ref-porter/xai-org-grok-build-tui.md` — 본 설계의 근거가 된 비교분석 보고서
- `docs/ref-porter/xai-org-grok-build.md` — 1차 보고서 (memory/compaction/permission 중심)
- `AGENTS.md` — oxi-tui 정체성, 4-condition test, 스타일 가이드
- grok source: `xai-grok-pager-render/src/render/osc8.rs`, `xai-grok-markdown/src/{lib.rs,colors.rs,checkpoint.rs}`, `xai-ratatui-inline/README.md`
