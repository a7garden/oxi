# 설계: oxicode-tui 테마 시스템 전면 재설계 (확정版)

> 상태: **확정** (구현 착수 가능)
> 작성: 2026-06-24 (v1 초안 → v2 자체 리뷰 → 확정)
> 감사: oxicode-tui/src 전수 audit + ratatui 0.30.2 소스 교차검증
> 선행: 기존 Theme / ColorScheme / ThemeStyles / ThemeManager / ThemeRegistry 인프라
> 후속: CHANGELOG.md + AGENTS.md pitfalls + 6개 built-in 테마 컬러 재조정

---

## 0. 핵심 (TL;DR)

**진단:** 인프라는 완성 — 데이터 모델(21 슬롯), TOML/JSON 로더, hot-reload, 레지스트리, 6개 built-in, 3개 GlyphSet 전부 동작. 문제는 **렌더 코드가 background 슬롯을 소비하지 않는 것.**

- 21개 슬롯 중 **3개 dead** (`user_bg`, `code_bg`, `selection_bg` — 정의·packed 되었으나 단 한 곳도 안 읽음)
- 전체 영역에 `buf.set_style(area, bg)`로 배경을 칠하는 곳은 **input 영역 1곳뿐**
- 나머지 전부 terminal default 투명 → "테마를 바꿔도 뭐가 변하는 게 없는" UX의 근본 원인

**결정 (5개):**

1. 데이터 모델 유지 + **7개 신규 슬롯 추가** (총 28개). scrollbar·link_bg는 YAGNI로 제외.
2. dead 슬롯 3개 **wire-up** + 신규 7개 소비 → 모든 주요 영역에 background fill.
3. **밝기 계층 원칙 확정** — `background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg`. 6개 테마 전부 구체적 RGB 도출 완료.
4. `Style::patch()` 패턴 전면 통일. `.fg.unwrap_or_default()` 금지.
5. **Phase 1 = 단일 PR** (compile-clean, 모든 슬롯 정의 + wire-up + 테마 값). Phase 2 = inline code + 문서.

---

## 1. 진단: 현재 무엇이 동작하고 무엇이 동작하지 않는가

### 1.1 동작하는 인프라 (변경 없음)

| 컴포넌트 | 파일 | 상태 |
|---|---|---|
| `Theme` (name + colors + spacing + symbols) | `theme.rs:21` | ✅ 완성 |
| `ColorScheme` (21 슬롯) | `theme.rs:38` | ✅ 확장만 |
| `ThemeManager` (hot-reload, mtime polling, `check_external`) | `theme.rs:724` | ✅ 완성 |
| `ThemeRegistry` (built-in + custom layering, `~/.oxicode/themes/*.toml`) | `theme.rs:914` | ✅ 완성 |
| `ThemeFile` (TOML/JSON 로더, `into_theme()`) | `theme.rs:498` | ✅ 확장만 |
| 6개 built-in 테마 (dark/light/nord/catppuccin/github_dark/monokai) | `theme.rs:91-246` | ✅ 값 재조정 |
| 3개 GlyphSet (Unicode/Ascii/Nerd) | `symbols.rs` | ✅ 변경 없음 |

### 1.2 동작하지 않는 것 (wire-up 부재)

| 증상 | 원인 | 렌더 코드 위치 |
|---|---|---|
| user 행이 1-cell border stripe만 있고 행 전체 bg 없음 | `user_bg` 정의만 됨, 소비 0건 | `chat/render.rs:57-77` |
| code block 배경이 항상 다크 amber | `OxicodeStyleSheet::code()` hardcoded `#231e14`; fenced code block은 bg 전혀 없음 | `markdown_styles.rs:38`, `highlight.rs:27-38` |
| selection이 terminal default swap | `Modifier::REVERSED` 사용, `selection_bg` 미소비 | `dashboard.rs:323` |
| chat viewport / footer / completion / routing / thinking / tool-result 모두 terminal default 투명 | `buf.set_style(area, bg)` 호출 0건 (input 제외) | 각 위젯 render() |

---

## 2. 밝기 계층 원칙 (확정)

### 2.1 원칙

터미널 테마의 보편적 관례: **"두드러짐(prominence) = 배경 극단에서 멀어지는 방향."**

- **dark 테마:** 두드러짐 ↑ = 밝아짐 (검정에서 멀어짐)
- **light 테마:** 두드러짐 ↑ = 어두워짐/회색화 (흰색에서 멀어짐)

### 2.2 5단계 계층

```
background (viewport 기본, 가장 평탄)
  = response_bg (assistant = default, user와 구분하기 위해 user보다 평탄)
    < thinking_bg (thinking block, 미세한 accent tint)
      < surface_bg (footer/status bar, 한 단계)
        < user_bg (user message, 기존 값 유지)
          < panel_bg (overlay popup, 가장 두드러짐)
```

### 2.3 도출 규칙 (모든 테마에 동일 적용)

| 슬롯 | 도출 공식 | 의미 |
|---|---|---|
| `response_bg` | `= background` | assistant는 default |
| `thinking_bg` | `blend(background, accent, 0.06)` | 미세한 accent tint, surface보다 아래 |
| `surface_bg` | `blend(background, user_bg, 0.5)` | footer/status, background와 user_bg의 중간 |
| `panel_bg` | `blend(user_bg, border, 0.5)` | overlay, user_bg와 border의 중간 (가장 두드러짐) |
| `diff_add_bg` | `= tool_success_bg` (재사용) | 동일한 의미론 (녹색 tint) |
| `diff_remove_bg` | `= tool_error_bg` (재사용) | 동일한 의미론 (적색 tint) |
| `diff_hunk_bg` | `blend(background, muted, 0.12)` | hunk header, 미세한 muted tint |

`blend(c1, c2, t) = c1 × (1−t) + c2 × t` (채널별 선형 보간).

### 2.4 dark 테마 밝기 검증

```
background   #000000  sum=    0  (가장 어두움)
thinking_bg  #0b090f  sum=   35
surface_bg   #090b13  sum=   39
user_bg      #121626  sum=   78
panel_bg     #35384b  sum=  184  (가장 밝음)
```

계층 역전 없음. ✅

---

## 3. 6개 테마 확정 RGB 값 (신규 7 슬롯)

### dark

```rust
response_bg:   Color::Rgb(0, 0, 0),         // = background
thinking_bg:   Color::Rgb(11, 9, 15),       // #0b090f
surface_bg:    Color::Rgb(9, 11, 19),       // #090b13
panel_bg:      Color::Rgb(53, 56, 75),      // #35384b
diff_add_bg:   Color::Rgb(16, 26, 14),      // = tool_success_bg
diff_remove_bg: Color::Rgb(32, 16, 18),     // = tool_error_bg
diff_hunk_bg:  Color::Rgb(15, 16, 19),      // #0f1013
```

### light

```rust
response_bg:   Color::Rgb(239, 241, 245),   // = background #eff1f5
thinking_bg:   Color::Rgb(233, 230, 245),   // #e9e6f5
surface_bg:    Color::Rgb(232, 238, 250),   // #e8eefa
panel_bg:      Color::Rgb(190, 198, 216),   // #bec6d8
diff_add_bg:   Color::Rgb(230, 248, 230),   // = tool_success_bg #e6f8e6
diff_remove_bg: Color::Rgb(255, 230, 235),  // = tool_error_bg #ffe6eb
diff_hunk_bg:  Color::Rgb(221, 223, 230),   // #dddfe6
```

### nord

```rust
response_bg:   Color::Rgb(46, 52, 64),      // = background nord0
thinking_bg:   Color::Rgb(54, 57, 71),      // #363947
surface_bg:    Color::Rgb(52, 59, 73),      // #343b49
panel_bg:      Color::Rgb(68, 76, 94),      // #444c5e (≈ nord2)
diff_add_bg:   Color::Rgb(40, 56, 44),      // = tool_success_bg
diff_remove_bg: Color::Rgb(56, 42, 44),     // = tool_error_bg
diff_hunk_bg:  Color::Rgb(52, 59, 73),      // #343b49
```

### catppuccin

```rust
response_bg:   Color::Rgb(30, 30, 46),      // = base #1e1e2e
thinking_bg:   Color::Rgb(40, 38, 58),      // #28263a
surface_bg:    Color::Rgb(40, 40, 57),      // #282839
panel_bg:      Color::Rgb(68, 70, 90),      // #44465a (≈ surface1)
diff_add_bg:   Color::Rgb(32, 46, 36),      // = tool_success_bg
diff_remove_bg: Color::Rgb(48, 34, 40),     // = tool_error_bg
diff_hunk_bg:  Color::Rgb(42, 42, 59),      // #2a2a3b
```

### github_dark

```rust
response_bg:   Color::Rgb(13, 17, 23),      // = canvas.default #0d1117
thinking_bg:   Color::Rgb(22, 23, 36),      // #161724
surface_bg:    Color::Rgb(18, 22, 28),      // #12161c
panel_bg:      Color::Rgb(35, 40, 48),      // #232830
diff_add_bg:   Color::Rgb(18, 30, 20),      // = tool_success_bg
diff_remove_bg: Color::Rgb(34, 18, 20),     // = tool_error_bg
diff_hunk_bg:  Color::Rgb(28, 33, 39),      // #1c2127
```

### monokai

```rust
response_bg:   Color::Rgb(39, 40, 34),      // = background #272822
thinking_bg:   Color::Rgb(47, 45, 47),      // #2f2d2f
surface_bg:    Color::Rgb(50, 50, 42),      // #32322a
panel_bg:      Color::Rgb(68, 66, 56),      // #444238
diff_add_bg:   Color::Rgb(34, 44, 26),      // = tool_success_bg
diff_remove_bg: Color::Rgb(50, 30, 38),     // = tool_error_bg
diff_hunk_bg:  Color::Rgb(48, 49, 41),      // #303129
```

---

## 4. ColorScheme 최종 슬롯 목록 (28개)

기존 21개 + 신규 7개. 제외: ~~scrollbar_bg~~, ~~scrollbar_thumb_bg~~ (소비 코드 없음, YAGNI), ~~link_bg~~ (hover 인터랙션 없음, 의미 불명).

```rust
pub struct ColorScheme {
    // ── 기존 21개 (변경 없음) ──
    pub foreground: Color,
    pub background: Color,
    pub primary: Color,
    pub secondary: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub muted: Color,
    pub accent: Color,
    pub border: Color,
    pub user_border: Color,
    pub user_bg: Color,                 // ← wire-up 대상
    pub cursor_fg: Color,
    pub cursor_bg: Color,
    pub selection_bg: Color,            // ← wire-up 대상
    pub code_fg: Color,
    pub code_bg: Color,                 // ← wire-up 대상
    pub tool_pending_bg: Color,
    pub tool_executing_bg: Color,
    pub tool_success_bg: Color,
    pub tool_error_bg: Color,

    // ── 신규 7개 ──
    pub response_bg: Color,             // assistant text 배경 (= background)
    pub thinking_bg: Color,             // thinking block 배경
    pub surface_bg: Color,              // footer / status bar 배경
    pub panel_bg: Color,                // overlay popup 배경
    pub diff_add_bg: Color,             // diff 추가 줄 배경 (= tool_success_bg)
    pub diff_remove_bg: Color,          // diff 삭제 줄 배경 (= tool_error_bg)
    pub diff_hunk_bg: Color,            // diff hunk header 배경
}
```

`ThemeStyles` / `ThemeFileColors`에도 동일 7개 대응 필드 추가.

---

## 5. 렌더 사이트별 wire-up 계획 (수정 확정)

### 핵심 기술 사실 (ratatui 0.30.2 검증)

- `Buffer::set_style(area, style)` → 각 cell에 `Cell::set_style(style)` → **patch semantics**: `style.bg`가 `Some`이면 덮어쓰고 `None`이면 기존 유지.
- `Buffer::set_line()` / `set_stringn()` → **그래펨이 있는 셀만** 스타일 적용, trailing 공백 셀은 건드리지 않음.
- 프레임 시작 시 cell은 `Cell::EMPTY` (bg=Reset).
- `input.rs:296`이 동일 패턴(`buf.set_style(area, Style::default().bg(bg))`)으로 검증됨.

**순서 규칙:** viewport fill을 **가장 먼저** → 이후 entry가 bg 명시 없이 text를 그리면 fill bg가 유지됨.

### 5.1 ChatView viewport 전체 (`chat/mod.rs:52`)

```rust
fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
    // ── 가장 먼저: viewport 전체 background fill ──
    buf.set_style(area, Style::default().bg(self.theme.colors.background));
    // 이후 모든 entry는 bg를 명시하지 않으면 background를 상속.
    // ...
}
```

이 한 줄로 `ToolResultBox`, `ErrorBox`, `ResponseDivider`, `Rule`, `Dashboard`, `Spinner` 등 **명시적 bg가 없는 모든 LayoutKind가 자동으로 background를 가짐.**

### 5.2 User message bg — `user_bg` wire-up (`chat/render.rs:57`)

```rust
if *is_user {
    // 1) rect 전체를 user_bg로 fill
    buf.set_style(rect, self.styles.user_bg);  // user_bg는 bg-only Style
    // 2) left-border block을 그 위에 덮음
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(self.styles.user_border);
    let inner = block.inner(rect);
    block.render(rect, buf);
    // 3) text rows (기존과 동일)
    for (i, line) in lines.iter().enumerate() { /* ... */ }
}
```

### 5.3 Assistant response bg (`chat/render.rs:78`)

```rust
} else {
    // response_bg로 rect fill
    buf.set_style(rect, self.styles.response_bg);
    // 기존 set_line 루프 (변경 없음)
}
```

### 5.4 Code block bg — `code_bg` wire-up (두 경로)

**경로 A: fenced code block** (`highlight.rs:294`)

`highlight_code()`가 이미 `&ThemeStyles`를 받지만 `token_style()`(`highlight.rs:27`)이 fg-only만 반환. 각 Line에 `code_bg`를 Line-level style로 설정:

```rust
pub(crate) fn highlight_code(content: &str, lang: &str, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let code_bg = styles.code_bg;  // bg-only Style
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(/* header */, styles.muted)));
    for line in content.lines() {
        let mut highlighted = highlight_line(line, lang, styles);
        highlighted.style = code_bg;  // ← Line-level bg: 모든 span에 propagate
        lines.push(highlighted);
    }
    lines
}
```

> **검증 필요:** ratatui의 `Line::style`이 span 스타일과 어떻게 compose되는지 (line.style.patch(span.style)인지 span.style만인지). `render_markdown:402-407`에서 `line_style.patch(s.style)` 패턴을 이미 사용 중이므로, Line-level style이 span에 patch됨이 확인됨. ✅

**경로 B: inline `` `code` ``** (`markdown_styles.rs:31`)

`OxicodeStyleSheet::code()`의 hardcoded `#231e14`를 `Color::Reset`로 변경 (fenced와 달리 inline은 theme code_bg를 받기 어려움 — `tui_markdown`이 `ThemeStyles`를 전달하지 않음). **Phase 2**에서 `OxicodeStyleSheet`를 theme-aware 구조체로 교체.

```rust
// Phase 1 (최소 수정): hardcoded 값 제거
fn code(&self) -> Style {
    Style::new()
        .fg(Color::Rgb(255, 200, 100))
        .bg(Color::Reset)  // ← was Color::Rgb(35, 30, 20). 테마 전환 시 다크 amber 고정 버그 제거.
        .add_modifier(Modifier::BOLD)
}
```

### 5.5 Selection bg — `selection_bg` wire-up (`dashboard.rs:320`)

```rust
// before: .add_modifier(Modifier::BOLD | Modifier::REVERSED)
// after:
Style::default()
    .fg(theme.colors.primary)
    .bg(theme.colors.selection_bg)
    .add_modifier(Modifier::BOLD)
```

### 5.6 Diff lines bg (`tool_renderer.rs:1079`)

`Style::patch()` 패턴:

```rust
// Added line
Span::styled(text, styles.success.patch(styles.diff_add_bg))
// Removed line
Span::styled(text, styles.error.patch(styles.diff_remove_bg))
// Hunk header
Span::styled(text, styles.muted.patch(styles.diff_hunk_bg))
// Context line (변경 없음)
Span::styled(text, styles.muted)
```

diff 외 포맷터(`format_bash_result`, `format_read_result` 등)는 ToolBox 내부에 렌더되므로 `tool_success_bg`/`tool_error_bg`를 상속 — **변경 없음.**

### 5.7 Footer bg (`footer.rs:138`)

```rust
fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
    buf.set_style(area, Style::default().bg(self.theme.colors.surface_bg));
    // 기존 렌더링 (변경 없음)
}
```

### 5.8 Thinking block bg (`chat/render.rs:349`)

```rust
LayoutKind::Thinking { .. } => {
    buf.set_style(rect, self.styles.thinking_bg);
    // 기존 라인 빌딩 + Paragraph::render (변경 없음)
}
```

### 5.9 Overlay popup bg — `panel_bg` (completion / settings / routing)

공통 패턴: `Clear` → `set_style(panel_bg)` → border block render.

```rust
// completion.rs:182 / settings.rs:525 / routing.rs:185
frame.render_widget(Clear, popup_area);
buf.set_style(popup_area, Style::default().bg(theme.colors.panel_bg));  // ← 신규
// 기존 border block render
```

---

## 6. 사전 조건: `dashboard.rs` Theme 파라미터화

**현재:** `dashboard.rs:262` — `let theme = Theme::dark();` (하드코딩)

**변경:**

```rust
pub struct DashboardWidget<'a> {
    data: DashboardData,
    theme: &'a Theme,
}

impl<'a> DashboardWidget<'a> {
    pub fn new(data: DashboardData, theme: &'a Theme) -> Self {
        Self { data, theme }
    }
}
```

**호출 사이트:** LSP references로 식별 후 `DashboardWidget::new(data, &theme)`로 일괄 수정. oxicode-cli `tui/overlay/*`에만 존재 예상.

---

## 7. 구현 페이즈

### Phase 1 — 단일 PR (compile-clean, 모든 핵심 변경)

> 하나의 PR에 모두 포함. 중간 상태가 compile-broken이 되지 않도록 슬롯 정의 → 테마 값 → wire-up을 한 커밋 시퀀스로 구성.

| 순서 | 변경 | 파일 |
|:----:|------|------|
| 1 | `ColorScheme` + `ThemeStyles` + `ThemeFileColors`에 7개 슬롯 추가 | `theme.rs` |
| 2 | `to_styles()`에 7개 스타일 packing | `theme.rs` |
| 3 | 6개 built-in 테마 `ColorScheme::*()`에 §3 값 추가 | `theme.rs` |
| 4 | `into_theme()`에 7개 resolve 추가 | `theme.rs` |
| 5 | `dashboard.rs` Theme 파라미터화 + `selection_bg` wire-up | `dashboard.rs` + 호출 사이트 |
| 6 | viewport background fill | `chat/mod.rs` |
| 7 | `user_bg` / `response_bg` wire-up | `chat/render.rs` |
| 8 | `thinking_bg` wire-up | `chat/render.rs` |
| 9 | `code_bg` wire-up (fenced) | `highlight.rs` |
| 10 | `code_bg` inline hardcoded 제거 | `markdown_styles.rs` |
| 11 | `diff_*_bg` wire-up (`patch()` 패턴) | `tool_renderer.rs` |
| 12 | `surface_bg` footer fill | `footer.rs` |
| 13 | `panel_bg` overlay fill | `completion.rs`, `routing.rs`, `settings.rs` (oxicode-cli) |
| 14 | 단위 테스트: 각 fill 사이트의 cell bg 검증 | `*_test` |
| 15 | `cargo fmt` + `cargo clippy --workspace -- -D warnings` + `cargo nextest run` | — |

### Phase 2 — 별도 PR

| 순서 | 변경 |
|:----:|------|
| 1 | `OxicodeStyleSheet` → theme-aware 구조체 (`&ThemeStyles` 보유). inline code에 `code_fg`/`code_bg` 반영 |
| 2 | `docs/THEME_GUIDE.md` 작성 (확장 TOML 스키마 + 예시) |
| 3 | `examples/theme_demo.rs` 갱신 (28 슬롯 전체 출력) |
| 4 | CHANGELOG.md + AGENTS.md pitfalls 갱신 |

---

## 8. DECCARA 상호작용 (성능 검증)

전면 bg fill 후, viewport의 모든 row trailing space가 `Color::Reset`가 아닌 concrete RGB가 됨.

- `deccara::analyze_trailing` (`deccara.rs:126`): `bg_sgr(bg)?`에서 `Color::Reset`을 거르므로, **concrete RGB여야 DECCARA 발동.** 모든 built-in 테마의 `background`는 concrete RGB이므로 ✅.
- 효과: DECCARA가 **더 자주 발동** → trailing space를 rectangle escape로 대체 → **terminal I/O 감소.** 부정적 영향 없음.

---

## 9. 마이그레이션 & 호환성

### 기존 커스텀 테마 (`~/.oxicode/themes/*.toml`)

`ThemeFileColors`의 모든 필드가 `Option<String>`. 신규 7개 필드가 없는 파일 → `None` → `into_theme()`에서 **dark 테마 기본값**으로 fallback. **기존 테마 100% 호환.**

### 스키마 문서화 (Phase 2)

```toml
[colors]
# 기존 21개 ...
# ── 신규 background 슬롯 (Phase 1) ──
response_bg = "#000000"
thinking_bg = "#0b090f"
surface_bg = "#090b13"
panel_bg = "#35384b"
diff_add_bg = "#101a0e"
diff_remove_bg = "#201012"
diff_hunk_bg = "#0f1013"
```

---

## 10. 구현 전 검증 체크리스트

| # | 항목 | 방법 |
|:-:|---|---|
| 1 | `Line::style`이 span에 patch되는지 | ratatui `render_markdown:402`에서 `line_style.patch(s.style)` 확인됨 ✅ |
| 2 | `Block::style(bg)`가 inner area 전체에 적용되는지 | `chat/render.rs:142` ToolBox가 동일 패턴 사용 중 ✅ |
| 3 | `buf.set_style` 후 `Paragraph::render` 상호작용 | Paragraph가 자체 bg 설정 안 하면 set_style bg 유지. 단위 테스트로 검증 |
| 4 | `dashboard.rs` 호출 사이트 식별 | LSP `references` on `DashboardWidget::new` |
| 5 | light 테마 밝기 계층 | thinking_bg < surface_bg 순서 단위 테스트 |
| 6 | DECCARA 발동 여부 | `deccara_emits_rectangle_for_full_bg_rows` 테스트 통과 확인 |
