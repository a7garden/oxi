# oxi TUI 개선 설계

## 개요

ratatui 프레임워크에 맞는 3가지 실용적 개선.

---

## 1. 채팅 스크롤바 (시각적)

### 현재 상태
- `tui-scrollview` 사용 중, 세로 1칸 예약해둠
- 하지만 시각적 스크롤바가 없어서 "내가 어디쯤 보고 있나" 감이 안 옴
- 긴 대화에서 스크롤 위치 파악 어려움

### 설계
- `tui-scrollview`의 `vertical_scrollbar_visibility`를 `Always`로 변경 (현재 `Never`)
- 또는 `ratatui::widgets::Scrollbar` 위젯을 채팅 영역 우측에 별도 렌더링
- `ScrollViewState`의 `offset()`과 전체 `content_height`로 스크롤바 상태 계산
- 테마 색상(`muted`, `accent`) 사용

### 변경 파일
- `oxi-tui/src/widgets/chat.rs` — ScrollView 설정 + Scrollbar 위젯 추가

### 난이도: ★☆☆ (낮음)

---

## 2. 세션 목록 Table 위젯

### 현재 상태
- `render_selectable_list()`로 텍스트 한 줄 표시
- `"name — cwd (N messages)"` 형태
- `SessionInfo`에 `created`, `modified`, `message_count`, `first_message` 등 풍부한 데이터가 있음

### 설계
- `ratatui::widgets::Table` + `TableState` 사용
- 열 구성:

| 열 | 필드 | 너비 | 정렬 |
|---|---|---|---|
| Name | `name` 또는 `id[:8]` | 20% | 좌 |
| Messages | `message_count` | 10% | 우 |
| Preview | `first_message` (잘림) | 40% | 좌 |
| Modified | `modified` (상대시간) | 15% | 우 |
| CWD | `cwd` (잘림) | 15% | 좌 |

- `/resume` 오버레이에서 `render_selectable_list()` → `Table` 교체
- `TableState`로 행 선택 관리 (Up/Down, Enter)
- 헤더 스타일: 볼드 + 테마 primary 색상
- 선택 행: 테마 강조색 배경
- `Constraint::Percentage()`로 반응형 너비

### 변경 파일
- `oxi-cli/src/tui/render.rs` — `render_resume_select()` 재작성
- `oxi-cli/src/tui/app.rs` — `ResumeSelect` 오버레이에 `TableState` 추가
- `oxi-cli/src/tui/handlers.rs` — 테이블 네비게이션 키 처리 (거의 동일)

### 난이도: ★★☆ (중간)

---

## 3. 설정/확장 탭 패널

### 현재 상태
- `/settings` → 텍스트 메시지로 출력 (인터랙티브 아님)
- `/extensions` → 텍스트 메시지로 출력 (인터랙티브 아님)
- `/tools` → 텍스트 메시지로 출력 (인터랙티브 아님)
- `/model` → 별도 오버레이 (ModelSelect)

### 설계
새 오버레이 `AppOverlay::SettingsPanel` 추가:

```
┌─ Settings ─────────────────────────────────────────┐
│ [Model] [Tools] [Extensions] [Auth]                 │
│─────────────────────────────────────────────────────│
│                                                     │
│  (탭에 따른 내용)                                    │
│                                                     │
│─────────────────────────────────────────────────────│
│ ←/→ 탭 전환  |  ↑/↓ 선택  |  Enter 확인  |  Esc 닫기│
└─────────────────────────────────────────────────────┘
```

#### 탭 구성

**Model 탭** (기존 ModelSelect 통합):
- 모델 목록 + 필터
- 현재 모델 표시
- Enter로 전환

**Tools 탭** (기존 /tools 대체):
- 툴 이름 | 상태(ON/OFF) | 설명 테이블
- Enter로 토글
- essential 툴은 비활성화 표시

**Extensions 탭** (기존 /extensions 대체):
- 확장 이름 | 타입(wasm/builtin) | 상태
- WASM 경로 안내

**Auth 탭** (기존 /provider, /logout 통합):
- 프로바이더 | 키 상태(설정됨/없음)
- Enter로 키 설정/제거

### 데이터 구조

```rust
pub(crate) enum SettingsTab {
    Model,
    Tools,
    Extensions,
    Auth,
}

pub(crate) struct SettingsPanelState {
    pub tab: SettingsTab,
    pub model_filter: String,
    pub model_selected: usize,
    pub tool_selected: usize,
    pub auth_selected: usize,
    // 기존 데이터는 AppState에서 가져옴
}

// AppOverlay에 추가
pub(crate) enum AppOverlay {
    // ... 기존 ...
    SettingsPanel(SettingsPanelState),
}
```

### 명령어 매핑
- `/settings` → `AppOverlay::SettingsPanel` 열기 (Model 탭)
- `/model` → `AppOverlay::SettingsPanel` 열기 (Model 탭)
- `/tools` → `AppOverlay::SettingsPanel` 열기 (Tools 탭)
- `/extensions` → `AppOverlay::SettingsPanel` 열기 (Extensions 탭)
- `/provider` → `AppOverlay::SettingsPanel` 열기 (Auth 탭)
- `/logout` → `AppOverlay::SettingsPanel` 열기 (Auth 탭)

→ 기존 명령어는 유지하되, 인터랙티브 패널로 라우팅

### 변경 파일
- `oxi-cli/src/tui/app.rs` — `SettingsPanelState`, AppOverlay 확장
- `oxi-cli/src/tui/render.rs` — `render_settings_panel()` 새 함수 + 탭별 렌더러
- `oxi-cli/src/tui/handlers.rs` — 탭 전환/선택 키 핸들러
- `oxi-cli/src/tui/slash.rs` — 명령어를 패널로 라우팅

### 난이도: ★★★ (높음)

---

## 구현 순서

1. **스크롤바** (간단, 즉각적 UX 개선)
2. **세션 Table** (중간, 독립적)
3. **설정 탭 패널** (복잡, 가장 많은 코드 변경)

---

## 공통 원칙

- 테마 시스템 준수: `theme.colors.*`, `theme.to_styles()` 사용
- 기존 오버레이 패턴 유지: `centered_popup` + `render_popup_frame`
- 키바인딩 일관성: `↑/↓` 선택, `Enter` 확인, `Esc` 취소, `←/→` 탭 전환
- 푸터 힌트: 모든 패널 하단에 사용 가능한 키 표시
