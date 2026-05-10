# Progress

## Status
In Progress

## Tasks

### 문제 2: 박스 폭이 터미널 절반만 차지하는 원인 분석 ✅
- **원인**: `push_blocks()`에서 `box_width`가 `50`으로 하드코딩. `area.width`가 함수에 전달되지 않음
- **wrap 영향**: wrap 자체는 50 < area.width일 때 문제 없으나, 긴 body 텍스트 시 오른쪽 `│` 생략으로 박스 깨짐 가능
- **해결 방안**: `push_blocks`에 `area.width` 전달, 하드코딩 `50` → `area.width`로 교체
- **상세 분석**: `/tmp/analysis_width.md`

### 문제 1: ToolCall + ToolResult 박스 머지가 안 되는 원인 분석 ✅
- **근본 원인**: `AgentEvent::ToolCall`이 events.rs에 정의만 되어 있고 **어디서도 emit되지 않음**
- `AgentLoop`는 `ToolExecutionStart/End`을 emit하지만, `app.rs`의 match에서 `_ => continue`로 무시됨
- `stream_tool_call()`이 절대 호출되지 않아 ToolCall 블록이 생성되지 않음
- `stream_tool_result()`가 last block에서 ToolCall을 못 찾아 fallback으로 standalone ToolResult block 생성
- **경쟁상태**: 없음 (finish_streaming은 모든 tool 실행 완료 후에만 발생)
- **해결 방안**: `app.rs`의 event forwarder에 `ToolExecutionStart → UiEvent::ToolCall`, `ToolExecutionEnd → UiEvent::ToolResult` 매핑 추가
- **상세 분석**: `/tmp/analysis_merge.md`

### 문제 3: tracing WARN 로그가 TUI에 섞여서 렌더링 깨지는 원인 분석 ✅
- **원인**: `tui-markdown` 0.3.7이 코드 블록 언어 미인식/미지원 markdown 기능마다 `warn!()` emit → stderr로 출력 → TUI 렌더링과 섞임
- **핵심 버그**: main.rs의 `"info,tui_markdown=warn"`은 WARN을 **유지**하는 설정. `tui_markdown=error`로 변경 필요
- **WARN 17개**: 코드 블록 언어 1개 + 미지원 기능(Html, Table, Math, Footnote, Definition list 등) 16개
- **추가 소스**: oxi-tui(1개), oxi-cli(약 25개)에서도 WARN 발생 가능
- **추천 해결**: 필터 `tui_markdown=error` 변경 + TUI 모드에서 로그 파일 리다이렉트
- **상세 분석**: `/tmp/analysis_tracing.md`

## Files Changed

## Notes

- `chat.rs`의 `push_blocks` (L514)에 `area_width: u16` 파라미터 추가 필요
- 모든 `50` 리터럴 → `area_width` 또는 `area_width - 인덴이션`으로 교체 필요
- 직접 Buffer 조작은 불필요 (Paragraph + wrap 구조 유지 가능)
