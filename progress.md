# Progress

## Status
In Progress

## Tasks

### 문제 2: 박스 폭이 터미널 절반만 차지하는 원인 분석 ✅
- **원인**: `push_blocks()`에서 `box_width`가 `50`으로 하드코딩. `area.width`가 함수에 전달되지 않음
- **wrap 영향**: wrap 자체는 50 < area.width일 때 문제 없으나, 긴 body 텍스트 시 오른쪽 `│` 생략으로 박스 깨짐 가능
- **해결 방안**: `push_blocks`에 `area.width` 전달, 하드코딩 `50` → `area.width`로 교체
- **상세 분석**: `/tmp/analysis_width.md`

## Files Changed

## Notes

- `chat.rs`의 `push_blocks` (L514)에 `area_width: u16` 파라미터 추가 필요
- 모든 `50` 리터럴 → `area_width` 또는 `area_width - 인덴이션`으로 교체 필요
- 직접 Buffer 조작은 불필요 (Paragraph + wrap 구조 유지 가능)
