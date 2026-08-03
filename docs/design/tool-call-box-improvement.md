# 툴 콜 박스 개선 설계서

## 1. 개요

oxicode의 툴 콜 박스를 pi-mono 수준으로 개선한다. 현재는 모든 툴이 동일한 
generic 포맷(아이콘 + 이름 + JSON 키밸류)으로 렌더링된다. 
개선 후에는 edit의 diff 뷰, bash의 명령어+실행시간 등 툴별 특화 렌더링과 
자동 콘텐츠 감지를 제공한다.

## 2. 현재 아키텍처

### 데이터 흐름

```
[oxicode-agent/tools/*]
    ↓ AgentToolResult { output: String, metadata: Option<Value> }
[oxicode-agent/agent_loop/tool_exec.rs]
    ↓ AgentEvent::ToolExecutionEnd { result: ToolResult { content: String } }
[oxicode-cli/tui/app.rs]
    ↓ UiEvent::ToolExecutionEnd { result: ToolResult { content, is_error } }
[oxicode-cli/tui/handlers.rs]
    ↓ state.chat.stream_tool_result(id, name, result.content[..500], is_error)
[oxicode-tui/widgets/chat.rs]
    ↓ ContentBlock::ToolCall { name, arguments, result: Option<(String, bool)> }
    ↓ LayoutKind::ToolBox → EntryWidget::render
```

### 현재 렌더링 (chat.rs `LayoutKind::ToolBox`)

```
│ ○ edit
│   path: src/main.rs
│   edits: [{"oldText":"foo","newText":"bar"}]
```

- 모든 툴이 동일 포맷
- JSON 키-밸류 최대 3개
- 결과 최대 3줄 raw text
- 상태별 배경색만 다름

## 3. 문제점

1. **확장성 부족**: 툴 이름 기반 `match`를 하드코딩하면 커스텀/MCP 툴이 소외됨
2. **정보 손실**: edit의 diff, bash의 실행시간 등 의미 있는 데이터가 `output: String`에 섞여서 전달됨
3. **diff 미표시**: pi-mono는 줄번호 + 색상 diff를 보여주지만 oxicode는 raw text만
4. **truncation**: 핸들러에서 `result.content.chars().take(500)`으로 잘라버림

## 4. 설계 원칙

1. **pi-mono 패턴**: 각 툴이 자체 렌더러를 가질 수 있도록, but Rust에 맞는 방식으로
2. **점진적 개선**: 최소 변경으로 시작, 나중에 확장 가능
3. **커스텀 툴 지원**: 내장 툴은 정확한 힌트 제공, 커스텀 툴은 자동 감지 폴백
4. **레이어 분리**: TUI(oxicode-tui)는 agent(oxicode-agent)를 모름 — 힌트만으로 소통

## 5. 핵심 설계: ResultKind 힌트 + 자동 감지

### 5.1 Layer 1: Agent → TUI 데이터 전달

`AgentToolResult.metadata`에 이미 `serde_json::Value` 필드가 있다. 
이것을 활용해서 렌더링 힌트를 전달한다.

**AgentEvent 확장** — `ToolExecutionEnd`에 `render_hint` 필드 추가:

```rust
// oxicode-agent/src/events.rs
AgentEvent::ToolExecutionEnd {
    tool_call_id: String,
    tool_name: String,
    result: oxicode_ai::ToolResult,
    is_error: bool,
    // ↓ 신규 필드
    render_hint: Option<RenderHint>,
}
```

### 5.2 RenderHint 정의

```rust
// oxicode-tui/src/widgets/chat.rs (ContentBlock 근처에 배치)

/// 툴 결과 렌더링 힌트.
/// 에이전트가 제공하면 해당 렌더러를 사용하고,
/// 없으면 콘텐츠 자동 감지로 폴백한다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderHint {
    /// 파일 편집 diff 뷰
    Diff {
        /// 편집된 파일 경로 (단축 표시용)
        file_path: Option<String>,
        /// 총 추가된 라인 수
        added_lines: Option<u32>,
        /// 총 삭제된 라인 수
        removed_lines: Option<u32>,
    },
    /// 셸 명령어 실행
    Command {
        /// 실행된 명령어
        command: Option<String>,
        /// 실행 시간 (밀리초)
        duration_ms: Option<u64>,
        /// 종료 코드
        exit_code: Option<i32>,
    },
    /// 파일 읽기
    FileRead {
        /// 파일 경로
        file_path: Option<String>,
        /// 줄 범위 "1-50"
        line_range: Option<String>,
        /// 총 줄 수
        total_lines: Option<u32>,
    },
    /// 파일 쓰기
    FileWrite {
        /// 파일 경로
        file_path: Option<String>,
        /// 쓴 줄 수
        lines_written: Option<u32>,
    },
    /// 검색 (grep/find/ls)
    Search {
        /// 검색어/패턴
        pattern: Option<String>,
        /// 매치 수
        match_count: Option<u32>,
    },
    /// 일반 텍스트 (기본값)
    Generic,
}
```

### 5.3 Layer 2: ContentBlock 확장

```rust
// oxicode-tui/src/widgets/chat.rs — ContentBlock::ToolCall 수정

ContentBlock::ToolCall {
    id: String,
    name: String,
    arguments: String,
    result: Option<(String, bool)>,
    status: ToolCallStatus,
    render_hint: Option<RenderHint>,  // ← 추가
}
```

### 5.4 Layer 3: TUI 렌더러 모듈

```
oxicode-tui/src/
  widgets/
    tool_renderer.rs  (신규)
```

```rust
// oxicode-tui/src/widgets/tool_renderer.rs

use ratatui::text::{Line, Span};
use crate::theme::ThemeStyles;
use super::chat::RenderHint;

/// 툴 콜 내용을 포맷팅한다.
pub fn format_tool_call(
    name: &str,
    arguments: &str,
    hint: Option<&RenderHint>,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    // 힌트가 있으면 툴별 포맷
    match hint {
        Some(RenderHint::Command { command, .. }) => {
            format_command_call(name, arguments, command.as_deref(), max_width, styles)
        }
        Some(RenderHint::Diff { file_path, .. }) => {
            format_diff_call(name, arguments, file_path.as_deref(), max_width, styles)
        }
        Some(RenderHint::FileRead { file_path, line_range, .. }) => {
            format_read_call(name, arguments, file_path.as_deref(), line_range.as_deref(), max_width, styles)
        }
        Some(RenderHint::FileWrite { file_path, .. }) => {
            format_write_call(name, arguments, file_path.as_deref(), max_width, styles)
        }
        Some(RenderHint::Search { pattern, .. }) => {
            format_search_call(name, arguments, pattern.as_deref(), max_width, styles)
        }
        _ => {
            // 힌트 없으면 이름 기반 추론 + generic 폴백
            format_auto_call(name, arguments, max_width, styles)
        }
    }
}

/// 툴 결과를 포맷팅한다.
pub fn format_tool_result(
    name: &str,
    result: &str,
    is_error: bool,
    hint: Option<&RenderHint>,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    if is_error {
        return format_error_result(result, max_width, styles);
    }

    match hint {
        Some(RenderHint::Diff { .. }) => format_diff_result(result, max_width, styles),
        Some(RenderHint::Command { duration_ms, exit_code, .. }) => {
            format_command_result(result, duration_ms, exit_code, max_width, styles)
        }
        Some(RenderHint::FileRead { total_lines, .. }) => {
            format_read_result(result, total_lines, max_width, styles)
        }
        _ => {
            // 자동 감지: 콘텐츠가 diff 형식인지 확인
            if looks_like_diff(result) {
                format_diff_result(result, max_width, styles)
            } else {
                format_generic_result(result, max_width, styles)
            }
        }
    }
}
```

## 6. 툴별 렌더링 상세

### 6.1 edit 툴

**호출 표시:**
```
│ ✎ edit ~/src/main.rs  (1 replacement)
```

**결과 표시 (성공 시 — diff 뷰):**
```
│  -10  fn main() {
│  +10  fn main() -> Result<()> {
│   11      println!("hello");
```

**결과 표시 (에러 시):**
```
│  ✗ edit ~/src/main.rs
│    Text to replace not found in file
```

**구현:** `format_diff_result()` — 결과 텍스트에서 `@@ -` 헤더와 
`-`/`+` 프리픽스를 파싱해서 색상 렌더링. 최대 8줄 diff 표시.

### 6.2 bash 툴

**호출 표시:**
```
│ $ cargo build --release
```

**결과 표시:**
```
│   Compiling oxicode v0.10.0
│   Finished release [optimized]
│   Took 12.3s
```

**구현:** `format_command_result()` — output 최대 5줄 미리보기 + 
실행시간(duration_ms / 1000.0 포맷).

### 6.3 read 툴

**호출 표시:**
```
│ 📄 read ~/src/main.rs:1-50
```

**결과 표시:**
```
│  1 │ use std::io;
│  2 │ 
│  3 │ fn main() {
│  … (47 more lines, 1.2KB)
```

### 6.4 write 툴

**호출 표시:**
```
│ ✎ write ~/src/new_file.rs  (42 lines)
```

### 6.5 검색 툴 (grep/find/ls)

**호출 표시:**
```
│ ⌕ grep "pattern" ~/src/
```

**결과 표시:**
```
│   src/main.rs:10: let pattern = "hello";
│   src/lib.rs:5: pattern matching
│   (2 matches)
```

### 6.6 Generic 폴백 (커스텀/MCP 툴)

**힌트 없을 때 호출 표시:**
```
│ ○ my_custom_tool
│   param1: value1
│   param2: value2
```

**자동 감지 로직:**
- 결과가 `@@ -` 로 시작 → diff 렌더러
- 결과가 `Command exited with code` 포함 → command 렌더러
- 그 외 → generic (최대 5줄 + "...")

## 7. 변경 파일 목록

### Phase 1: 인프라 (최소 변경)

| 파일 | 변경 | 설명 |
|------|------|------|
| `oxicode-tui/src/widgets/chat.rs` | 수정 | `RenderHint` enum, `ContentBlock`에 `render_hint` 필드 추가 |
| `oxicode-tui/src/widgets/tool_renderer.rs` | **신규** | 툴별 포맷터, 자동 감지 로직 |
| `oxicode-tui/src/widgets/mod.rs` | 수정 | `mod tool_renderer` 추가 |
| `oxicode-tui/src/lib.rs` | 수정 | 모듈 노출 |

### Phase 2: Agent → TUI 힌트 전달

| 파일 | 변경 | 설명 |
|------|------|------|
| `oxicode-agent/src/events.rs` | 수정 | `ToolExecutionEnd`에 `render_hint: Option<RenderHint>` 추가 |
| `oxicode-agent/src/agent_loop/tool_exec.rs` | 수정 | 각 툴 결과에 힌트 추가 |
| `oxicode-agent/src/tools/edit.rs` | 수정 | `RenderHint::Diff` 힌트 포함 |
| `oxicode-agent/src/tools/bash.rs` | 수정 | `RenderHint::Command` 힌트 포함 |
| `oxicode-agent/src/tools/read.rs` | 수정 | `RenderHint::FileRead` 힌트 포함 |
| `oxicode-agent/src/tools/write.rs` | 수정 | `RenderHint::FileWrite` 힌트 포함 |

### Phase 3: CLI 브릿지

| 파일 | 변경 | 설명 |
|------|------|------|
| `oxicode-cli/src/tui/app.rs` | 수정 | `UiEvent::ToolExecutionEnd`에 `render_hint` 전달 |
| `oxicode-cli/src/tui/handlers.rs` | 수정 | `stream_tool_result`에 `render_hint` 전달 |

### Phase 4: 렌더링 개선 (chat.rs → tool_renderer.rs 위임)

| 파일 | 변경 | 설명 |
|------|------|------|
| `oxicode-tui/src/widgets/chat.rs` | 수정 | `measure_kind`, `EntryWidget::render`에서 `tool_renderer` 호출 |

## 8. 마이그레이션 전략

### 호환성 보장

1. **`render_hint`는 Option**: 기본값 `None` → 기존 동작 유지
2. **자동 감지 폴백**: 힌트 없어도 콘텐츠 분석으로 최선 렌더링
3. **점진적 적용**: Phase 1만 해도 자동 감지로 개선. Phase 2-3은 정확도 향상.

### 구현 순서

```
Phase 1 (인프라 + 자동 감지) → 즉시 개선 효과
  ├── tool_renderer.rs 신규 작성
  ├── chat.rs에서 호출
  └── 자동 감지만으로 edit diff, bash command 감지

Phase 2 (Agent 힌트) → 정확도 향상
  ├── events.rs에 render_hint 추가
  ├── 각 툴에 힌트 생성 로직 추가
  └── CLI 브릿지 연결

Phase 3 (고급 기능)
  ├── 실행 시간 표시 (bash)
  ├── 줄 번호 표시 (read)
  ├── 매치 카운트 (grep/find)
  └── 확장/축소 기능 (future)
```

## 9. RenderHint 직렬화

`serde`를 사용해서 `metadata` JSON에 힌트를 포함:

```json
{
  "kind": "diff",
  "file_path": "src/main.rs",
  "added_lines": 3,
  "removed_lines": 1
}
```

AgentToolResult의 `metadata` 필드를 통해 전달:

```rust
// oxicode-agent/src/tools/edit.rs
Ok(AgentToolResult {
    success: true,
    output: format!("{}\n\n{}", message, diff),
    metadata: Some(serde_json::to_value(RenderHint::Diff {
        file_path: Some(path.clone()),
        added_lines: Some(added),
        removed_lines: Some(removed),
    })?),
    ..Default::default()
})
```

Agent loop에서 metadata → render_hint 변환:

```rust
// oxicode-agent/src/agent_loop/tool_exec.rs
let render_hint = result.metadata
    .as_ref()
    .and_then(|m| serde_json::from_value(m.clone()).ok());

emit(AgentEvent::ToolExecutionEnd {
    tool_call_id: finalized.tool_call.id.clone(),
    tool_name: finalized.tool_call.name.clone(),
    result: oxicode_ai::ToolResult { ... },
    is_error: finalized.is_error,
    render_hint,  // ← 새 필드
});
```

## 10. diff 렌더러 상세 설계

pi-mono의 `renderDiff()`를 Rust로 포팅:

```rust
/// Unified diff 텍스트를 색상 라인으로 렌더링.
/// 입력: "@@ -10,3 +10,4 @@\n-old\n+new\n context"
fn format_diff_result(diff_text: &str, max_width: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut line_num = 0u32;
    
    for raw_line in diff_text.lines().take(10) {
        if raw_line.starts_with("@@") {
            // Hunk header — muted
            lines.push(Line::from(Span::styled(
                truncate_str(raw_line, max_width),
                styles.muted,
            )));
        } else if raw_line.starts_with('-') {
            // Removed — red
            lines.push(Line::from(Span::styled(
                format!(" {}", truncate_str(raw_line, max_width.saturating_sub(1))),
                styles.error,
            )));
        } else if raw_line.starts_with('+') {
            // Added — green
            lines.push(Line::from(Span::styled(
                format!(" {}", truncate_str(raw_line, max_width.saturating_sub(1))),
                styles.success,
            )));
        } else {
            // Context — normal
            lines.push(Line::from(Span::styled(
                format!(" {}", truncate_str(raw_line, max_width.saturating_sub(1))),
                styles.muted,
            )));
        }
    }
    
    let total_lines = diff_text.lines().count();
    if total_lines > 10 {
        lines.push(Line::from(Span::styled(
            format!(" … ({} more lines)", total_lines - 10),
            styles.muted,
        )));
    }
    
    lines
}
```

## 11. 자동 감지 로직

```rust
/// 결과 텍스트가 unified diff인지 감지.
fn looks_like_diff(text: &str) -> bool {
    // "@@ -" 로 시작하는 hunk header가 있으면 diff
    text.lines().take(5).any(|l| l.starts_with("@@ -"))
}

/// 결과 텍스트가 셸 명령어 출력인지 감지.
fn looks_like_command_output(text: &str) -> bool {
    text.contains("Command exited with code")
        || text.contains("Command timed out")
        || text.contains("Command aborted")
}
```

## 12. 테스트 계획

1. **단위 테스트** — `tool_renderer.rs`
   - `format_diff_result()` — 샘플 diff 입력
   - `format_command_result()` — 실행시간 포맷
   - `looks_like_diff()` — true/false 케이스
   - `RenderHint` 직렬화/역직렬화

2. **통합 테스트** — `chat.rs` 기존 테스트에 `render_hint` 포함
   - `tool_call_lifecycle` 힌트 포함 버전

3. **수동 테스트**
   - edit 실행 → diff 뷰 확인
   - bash 실행 → `$ command` + 실행시간 확인
   - MCP 툴 → generic 폴백 확인
