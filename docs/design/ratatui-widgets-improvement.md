# ratatui 0.30 위젯/API 개선 설계

대상: oxicode-tui (oxicode-tui crate 내부)

## 항목

| # | 항목 | 파일 |
|---|------|------|
| 3 | `Rect::layout()` / `Rect::centered()` — 선언적 레이아웃 | chat.rs, overlay_anchor.rs, footer.rs, routing.rs |
| 4 | `IntoCrossterm` → DiffBackend 간소화 | render/mod.rs |
| 5 | `BorderType` 다변형 → ToolBox/ErrorBox/Thinking | chat.rs |
| 6 | `Table` → 모델/툴/스킬 선택 오버레이 | stateful_list.rs (확장) |
| 7 | `Sparkline` → 토큰 사용량 추이 | footer.rs, chat.rs |

---

## 3. Rect::layout() / Rect::centered() — 선언적 레이아웃

### 3-1. ratatui 0.30 신규 API

```rust
// Rect에 직접 레이아웃 적용 → 배열 분해 (컴파일 타임 크기 체크)
impl Rect {
    pub fn layout<const N: usize>(self, layout: &Layout) -> [Self; N];
    pub fn layout_vec(self, layout: &Layout) -> Vec<Self>;
    pub fn try_layout<const N: usize>(self, layout: &Layout) -> Result<[Self; N]>;

    pub fn centered(self, width: Constraint, height: Constraint) -> Self;
    pub fn centered_vertically(self, constraint: Constraint) -> Self;
    pub fn centered_horizontally(self, constraint: Constraint) -> Self;

    pub const fn outer(self, margin: Margin) -> Self;
}
```

### 3-2. 변경: overlay_anchor.rs

**현재** (245줄, 수동 x/y 계산):
```rust
// resolve_overlay_layout() — 9방향 anchor를 if-else로 수동 계산
let (x, y) = match layout.anchor {
    OverlayAnchor::Center => (
        margin + (avail_w.saturating_sub(width)) / 2,
        margin + (avail_h.saturating_sub(height)) / 2,
    ),
    OverlayAnchor::TopLeft => (margin, margin),
    // ... 7 more cases
};
```

**개선**:
```rust
use ratatui::layout::{Constraint, Flex, Layout, Margin, Rect};

pub fn resolve_overlay_layout(layout: &OverlayLayout, term_w: u16, term_h: u16) -> Rect {
    let terminal = Rect::new(0, 0, term_w, term_h);
    let area = terminal.outer(Margin::new(layout.margin, layout.margin));

    // Resolve width/height
    let width = match layout.width {
        SizeValue::Fixed(w) => w.min(area.width),
        SizeValue::Percent(pct) => ((area.width as f32 * pct).ceil() as u16).min(area.width),
    };
    let width = layout.min_width.map_or(width, |min| width.max(min)).min(area.width);
    let height = layout.max_height.map_or(area.height / 2, |max| max.min(area.height));

    // 0.30: Rect::centered() + Rect::outer()
    let overlay = match layout.anchor {
        OverlayAnchor::Center => {
            area.centered(Constraint::Length(width), Constraint::Length(height))
        }
        OverlayAnchor::TopLeft => {
            Rect::new(area.x, area.y, width, height)
        }
        OverlayAnchor::TopCenter => {
            area.centered_horizontally(Constraint::Length(width))
                .intersection(Rect::new(0, area.y, term_w, height))
        }
        OverlayAnchor::TopRight => {
            Rect::new(area.right().saturating_sub(width), area.y, width, height)
        }
        OverlayAnchor::BottomCenter => {
            let base = area.centered_horizontally(Constraint::Length(width));
            Rect::new(base.x, area.bottom().saturating_sub(height), width, height)
        }
        OverlayAnchor::BottomLeft => {
            Rect::new(area.x, area.bottom().saturating_sub(height), width, height)
        }
        OverlayAnchor::BottomRight => {
            Rect::new(
                area.right().saturating_sub(width),
                area.bottom().saturating_sub(height),
                width, height,
            )
        }
        OverlayAnchor::LeftCenter => {
            let base = area.centered_vertically(Constraint::Length(height));
            Rect::new(area.x, base.y, width, height)
        }
        OverlayAnchor::RightCenter => {
            let base = area.centered_vertically(Constraint::Length(height));
            Rect::new(area.right().saturating_sub(width), base.y, width, height)
        }
    };

    // Apply offsets (clamped)
    let x = ((overlay.x as i16) + layout.offset_x)
        .max(0).min(term_w.saturating_sub(width) as i16) as u16;
    let y = ((overlay.y as i16) + layout.offset_y)
        .max(0).min(term_h.saturating_sub(height) as i16) as u16;

    Rect { x, y, width, height }
}
```

**기대 효과**: Center/LeftCenter/RightCenter 케이스에서 수동 산술 대신 `centered()`/`centered_vertically()`/`centered_horizontally()` 사용으로 의도 명확화. 전체 코드 길이 변화 없으나 가독성 향상.

### 3-3. 변경: footer.rs

**현재** (수동 Layout::default() 체인):
```rust
let rows = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
```

**개선**:
```rust
let [sep_row, row1, row2] = area.layout(&Layout::vertical([
    Constraint::Length(1),
    Constraint::Length(1),
    Constraint::Length(1),
]));
```

2행 분할도:
```rust
// 현재
let cols = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Min(1), Constraint::Min(1)])
    .split(rows[2]);

// 개선
let [left, right] = rows[2].layout(&Layout::horizontal([
    Constraint::Min(1),
    Constraint::Min(1),
]));
```

### 3-4. 변경: routing.rs

동일하게 `Layout::default()...split()` → `area.layout(&...)` 패턴 적용.

### 3-5. 추가 import

```rust
use ratatui::layout::{Constraint, Layout, Margin, Rect};
// centered(), outer()는 Rect의 메서드이므로 추가 import 불필요
```

---

## 4. IntoCrossterm → DiffBackend 간소화

### 4-1. 현재 문제

`render/mod.rs`에 두 개의 수동 변환 함수 (~80줄):

1. **`color_to_bytes()`** (27줄) — ratatui `Color` → `[u8; 4]` 직렬화 (diff 비교용)
2. **`ratatui_color_to_crossterm()`** (30줄) — ratatui `Color` → crossterm `Color` 변환

### 4-2. 분석: 교체 가능성

**`ratatui_color_to_crossterm()` 교체**:

ratatui 0.30은 `IntoCrossterm` 트레이트를 제공하지만, 시그니처가 다름:
```rust
// 0.30 API: self를 소비
trait IntoCrossterm<C> {
    fn into_crossterm(self) -> C;
}
```

DiffBackend에서는 `&cell.fg` (참조)를 사용하므로, 복제 필요:
```rust
// 현재
let fg = ratatui_color_to_crossterm(&cell.fg);

// 개선 (0.30)
use ratatui::backend::IntoCrossterm;
let fg = cell.fg.into_crossterm();  // Color는 Copy이므로 자동 역참조
```

`ratatui::style::Color`는 `Copy`이므로 `&Color` → `Color` 자동 역참조로 `into_crossterm()` 호출 가능.

**`color_to_bytes()` 유지**: 이 함수는 diff 비교용 내부 직렬화이며 ratatui가 제공하지 않는 기능. 유지.

### 4-3. 변경 설계

**render/mod.rs**:

```rust
// 제거: ratatui_color_to_crossterm() 함수 전체 (~30줄)

// 추가 import
use ratatui::backend::IntoCrossterm;

// DiffBackend::draw() 내부 변경:
// Before:
let fg = ratatui_color_to_crossterm(&cell.fg);
let bg = ratatui_color_to_crossterm(&cell.bg);

// After:
let fg = cell.fg.into_crossterm();
let bg = cell.bg.into_crossterm();
```

**주의**: `IntoCrossterm`은 `ratatui::backend::IntoCrossterm`에 있으며, `ratatui`의 `crossterm` 피처가 활성화되어야 함. 현재 `Cargo.toml`에 `ratatui = { version = "0.30", features = ["unstable-rendered-line-info"] }` — 기본적으로 crossterm 피처 포함됨.

### 4-4. 영향 범위

| 파일 | 변경 |
|------|------|
| `render/mod.rs` | `ratatui_color_to_crossterm()` 함수 삭제, `into_crossterm()` 호출로 교체 |

---

## 5. BorderType 다변형 — 콘텐츠 유형별 시각적 계층

### 5-1. ratatui 0.30 BorderType

```rust
pub enum BorderType {
    Plain,          // ┌─┐
    Rounded,        // ╭─╮
    Double,         // ╔═╗
    Thick,          // ┏━┓
    LightDoubleDashed,  // ╌╌ (0.30 신규)
    HeavyDoubleDashed,  // ╍╍ (0.30 신규)
    LightTripleDashed,  // ┄┄ (0.30 신규)
    HeavyTripleDashed,  // ┅┅ (0.30 신규)
    LightQuadrupleDashed, // ┈┈ (0.30 신규)
    HeavyQuadrupleDashed, // ┉┉ (0.30 신규)
}
```

### 5-2. 변경: chat.rs — EntryWidget

**현재**: 모든 블록이 `Block::default().borders(Borders::ALL)` 또는 `Block::bordered()` (Plain 기본값)

**개선**: LayoutKind 변형별 BorderType 매핑:

```rust
use ratatui::widgets::BorderType;

impl Widget for EntryWidget<'_> {
    fn render(self, rect: Rect, buf: &mut Buffer) {
        match &self.entry {
            LayoutKind::ToolBox { status, .. } => {
                let border_type = match status {
                    ToolCallStatus::Requested => BorderType::LightDoubleDashed, // ╌ 대기 중
                    ToolCallStatus::Executing => BorderType::Plain,              // ┌ 실행 중
                    ToolCallStatus::Done => BorderType::Plain,                   // ┌ 완료
                };
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(border_type)
                    .border_style(border_style)
                    .style(bg_style);
                // ...
            }

            LayoutKind::ErrorBox { .. } => {
                let block = Block::bordered()
                    .border_type(BorderType::Double)  // ╔═╗ 에러 — 강조
                    .border_style(self.styles.error)
                    // ...
            }

            LayoutKind::Thinking { collapsed, .. } => {
                let border_type = if *collapsed {
                    BorderType::LightTripleDashed // ┄ 접힌 생각
                } else {
                    BorderType::Plain // ┌ 펼쳐진 생각
                };
                let block = Block::default()
                    .borders(Borders::LEFT)
                    .border_type(border_type)
                    .border_style(self.styles.muted);
                // ...
            }

            // Dashboard
            LayoutKind::Dashboard { .. } => {
                let block = Block::bordered()
                    .border_type(BorderType::Rounded)  // ╭─╮ 환영 화면
                    .border_style(self.styles.border);
                // ...
            }

            _ => { /* 기존과 동일 */ }
        }
    }
}
```

### 5-3. 시각적 효과

```
현재 (모두 동일):
┌─edit──┐     ┌─bash──┐     ┌─error──┐
│  ...  │     │  ...  │     │  ...   │
└───────┘     └────────┘     └────────┘

개선:
╌╌edit──╌     ┌─bash──┐     ╔═error══╗
╌  ...  ╌     │  ...  │     ║  ...   ║
╌───────╌     └────────┘     ╚════════╝
 (대기)        (실행/완료)     (에러)

┄┄Thinking┄   ╭─Dashboard─╮
┄  ...    ┄   │  ...      │
┄─────────┄   ╰───────────╯
 (접힘)        (환영)
```

### 5-4. 영향 범위

| 파일 | 변경 |
|------|------|
| `widgets/chat.rs` | EntryWidget의 `Block::default()`/`Block::bordered()` 호출에 `.border_type()` 추가 |

---

## 6. Table → 모델/툴/스킬 선택 오버레이

### 6-1. 현재 아키텍처

`StatefulList<T: AsRef<str>>` — 단일 문자열 리스트만 표현 가능. 모델 선택 오버레이에서는 이름만 표시되고, 컨텍스트 크기, 가격, 상태 등의 부가 정보를 표시할 방법 없음.

### 6-2. 설계: StatefulTable<T>

`StatefulList`와 병렬로, 컬럼형 테이블을 제공하는 새 제네릭 구조체.

```rust
// 파일: oxicode-tui/src/widgets/table_list.rs (신규)

use ratatui::{
    layout::Constraint,
    style::Style,
    widgets::{Cell, HighlightSpacing, Row, Table, TableState},
};

/// 테이블 행으로 표시 가능한 아이템 트레이트.
pub trait TableItem {
    /// 각 컬럼의 셀 내용.
    fn cells(&self) -> Vec<Cell<'static>>;
    /// 컬럼 너비 제약.
    fn constraints() -> Vec<Constraint>;
}

/// 제네릭 테이블 리스트 — 필터링, 네비게이션, 선택 지원.
pub struct TableList<T> {
    items: Vec<T>,
    state: TableState,
    filter: String,
    filtered_indices: Vec<usize>,
}

impl<T: TableItem> TableList<T> {
    pub fn new(items: Vec<T>) -> Self { ... }
    pub fn select_next(&mut self) { ... }
    pub fn select_previous(&mut self) { ... }
    pub fn select_first(&mut self) { ... }
    pub fn select_last(&mut self) { ... }
    pub fn selected(&self) -> Option<&T> { ... }
    pub fn set_filter(&mut self, filter: &str) { ... }
    pub fn clear_filter(&mut self) { ... }

    /// 테이블 렌더링에 필요한 행/제약 반환.
    fn visible_rows(&self) -> (Vec<Row<'static>>, Vec<Constraint>) { ... }
}

/// 렌더링 설정.
pub struct TableListStyles {
    pub normal: Style,
    pub selected: Style,
    pub header: Style,
    pub highlight_symbol: &'static str,
}
```

### 6-3. TableItem 구현 예시 (oxicode-cli에서)

```rust
// oxicode-cli 측에서 정의 (oxicode-tui는 트레이트만 제공)
use oxicode_tui::widgets::table_list::TableItem;

struct ModelEntry {
    id: String,           // "anthropic/claude-sonnet-4"
    context: u32,         // 200_000
    input_price: f64,     // 3.0
    output_price: f64,    // 15.0
    is_available: bool,
}

impl TableItem for ModelEntry {
    fn cells(&self) -> Vec<Cell<'static>> {
        vec![
            Cell::from(self.id.clone()),
            Cell::from(format!("{}k", self.context / 1000)),
            Cell::from(format!("${:.1}/${:.1}", self.input_price, self.output_price)),
            Cell::from(if self.is_available { "●" } else { "○" }),
        ]
    }

    fn constraints() -> Vec<Constraint> {
        vec![
            Constraint::Min(25),    // model id
            Constraint::Length(8),  // context
            Constraint::Length(12), // pricing
            Constraint::Length(3),  // status
        ]
    }
}
```

### 6-4. 렌더링 (CompletionPopup과 유사한 오버레이)

```rust
// oxicode-tui/src/widgets/table_list.rs

impl TableListStyles {
    pub fn render<T: TableItem>(
        &self,
        table: &mut TableList<T>,
        frame: &mut ratatui::Frame,
        area: Rect,
        title: &str,
    ) -> Option<Rect> {
        let (rows, constraints) = table.visible_rows();

        let header = Row::new(
            ["Model", "Context", "Price", ""]
                .iter()
                .map(|h| Cell::from(*h).style(self.header))
                .collect::<Vec<_>>(),
        );

        let widget = Table::new(rows, constraints)
            .header(header)
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .title(format!(" {} ", title))
            )
            .highlight_style(self.selected)
            .highlight_symbol(self.highlight_symbol)
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(widget, area, &mut table.state);
        Some(area)
    }
}
```

### 6-5. 파일 구조 변경

```
oxicode-tui/src/widgets/
├── chat.rs            (기존)
├── completion.rs      (기존)
├── footer.rs          (기존)
├── input.rs           (기존)
├── mod.rs             ← table_list 모듈 추가
├── routing.rs         (기존)
├── stateful_list.rs   (기존)
├── table_list.rs      (신규)
└── tool_renderer.rs   (기존)
```

### 6-6. 모듈 등록

```rust
// widgets/mod.rs
pub mod table_list;
```

`lib.rs`에서 재export:
```rust
pub use widgets::table_list::{TableItem, TableList, TableListStyles};
```

---

## 7. Sparkline → 토큰 사용량 추이

### 7-1. 개요

Footer에 최근 N프레임의 토큰 소비율을 Sparkline으로 시각화.
스트리밍 중 응답 속도를 실시간으로 보여줌.

### 7-2. 데이터 모델

```rust
// footer.rs — FooterData에 스파크라인 히스토리 추가

pub struct FooterData {
    // ... 기존 필드 ...

    /// 최근 토큰 출력 속도 히스토리 (토큰/초, 최근 60샘플).
    /// 스트리밍 시작 시 초기화, 매 틱마다 push.
    pub token_rate_history: Vec<u64>,
}

impl FooterData {
    /// 토큰 속도 샘플 추가 (최대 60개 유지).
    pub fn push_token_rate(&mut self, tokens_per_sec: u64) {
        self.token_rate_history.push(tokens_per_sec);
        if self.token_rate_history.len() > 60 {
            self.token_rate_history.remove(0);
        }
    }
}
```

### 7-3. Footer 레이아웃 변경

현재 Footer는 3행:
```
────── separator ──────
↑1.2k ↓3.5k  45.2%/200k  Compacting...  2m
~/oxicode (main) *             (anth) claude-sonnet-4 • high
```

개선: 토큰 행을 확장하여 텍스트 + Sparkline 분할:

```
────── separator ──────
↑1.2k ↓3.5k  45.2%/200k  2m     ▁▂▃▅▇█▇▅▃▂▁▂▃▅
~/oxicode (main) *             (anth) claude-sonnet-4 • high
```

### 7-4. 구현

```rust
// footer.rs — Footer::render()의 Row 1 섹션

use ratatui::widgets::Sparkline;

impl StatefulWidget for Footer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // ... 기존 코드 ...

        let [sep_row, row1, row2] = area.layout(&Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ]));

        // Row 1: 토큰 정보 + 스파크라인
        {
            // 토큰 정보가 있고 히스토리가 충분하면 스파크라인 표시
            let show_sparkline = has_tokens && d.token_rate_history.len() >= 3;

            if show_sparkline {
                let [info_area, spark_area] = row1.layout(&Layout::horizontal([
                    Constraint::Min(30),   // 토큰 텍스트 (최소 30자)
                    Constraint::Min(10),   // 스파크라인 (나머지)
                ]));

                // 토큰 텍스트 렌더링 → info_area
                Paragraph::new(Line::from(left_spans))
                    .alignment(Alignment::Left)
                    .render(info_area, buf);

                // Sparkline 렌더링 → spark_area
                let sparkline_style = if pct > 0.8 {
                    styles.warning  // 80% 초과 시 경고색
                } else {
                    styles.primary
                };

                Sparkline::default()
                    .data(&d.token_rate_history)
                    .style(sparkline_style)
                    .max(100) // 최대 토큰/초 기준
                    .render(spark_area, buf);
            } else {
                // 기존과 동일: 텍스트만 전체 행에 렌더링
                Paragraph::new(Line::from(left_spans))
                    .alignment(Alignment::Left)
                    .render(row1, buf);
            }
        }

        // Row 2: 기존과 동일
        // ...
    }
}
```

### 7-5. 샘플링 전략 (oxicode-cli 측)

```rust
// oxicode-cli의 메인 이벤트 루프에서 (FooterData 업데이트 시)
// 이전 output_tokens과의 차이를 계산하여 push
let delta_tokens = current_output_tokens - prev_output_tokens;
let elapsed_secs = tick_duration.as_secs().max(1);
footer_data.push_token_rate(delta_tokens / elapsed_secs as u32);
```

### 7-6. 영향 범위

| 파일 | 변경 |
|------|------|
| `widgets/footer.rs` | `FooterData`에 `token_rate_history: Vec<u64>` 추가, Row 1 레이아웃 분할, `Sparkline` 렌더링 |
| `widgets/mod.rs` | 변경 없음 (Sparkline은 ratatui 내장) |
| `lib.rs` | 변경 없음 |

---

## 구현 순서 (권장)

```
Phase 1 (리스크 없는 정리):
  4 → IntoCrossterm 교체 (30줄 삭제, import 변경만)
  3 → Rect::layout() 적용 (footer.rs, routing.rs, overlay_anchor.rs)

Phase 2 (시각적 개선):
  5 → BorderType 다변형 (chat.rs EntryWidget)
  7 → Sparkline 추가 (footer.rs)

Phase 3 (새 기능):
  6 → TableList 위젯 (table_list.rs 신규)
```

## 호환성 노트

- 모든 변경은 ratatui 0.30 (현재 Cargo.toml에 지정된 버전) 내에서 동작
- 추가 크레이트 의존성 없음
- oxicode-tui의 공개 API는 `TableList` 추가를 제외하면 변경 없음 (내부 구현 개선)
- `IntoCrossterm`은 `ratatui`의 기본 `crossterm` 피처에 포함됨
