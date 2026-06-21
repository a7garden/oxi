# omp `ask` 표절 설계 — oxi questionnaire → ask 재설계

> **작성:** 2026-06-21
> **표절 대상:** [`can1357/oh-my-pi`](https://github.com/can1357/oh-my-pi) 의 `ask` 도구 + `HookSelectorComponent` + `framedBlock` 렌더 체계
> **분석 소스:** `/tmp/omp` 클론(`packages/coding-agent/src/tools/ask.ts`, `modes/components/hook-selector.ts`, `tui/output-block.ts`, `tui/status-line.ts`, `modes/theme/theme.ts`)
> **관련 기존 트랙:** [`omp-adoption`](./omp-adoption/) · [`omp-adoption-2`](./omp-adoption-2/) · [`2026-06-20-omp-vs-oxi-tui-analysis.md`](./2026-06-20-omp-vs-oxi-tui-analysis.md)
> **범위:** 본 설계는 **`ask`/questionnaire 도구와 그 셀렉터 위젯·트랜스크립트 렌더링**에 한정. 렌더링 엔진(스크롤백 네이티브·DECCARA·SGR delta)은 위 분석 트랙에서 이미 다루므로 여기서 다루지 않는다.

---

## 0. 한 줄 결론

oxi의 `questionnaire`는 **화면을 덮는 모달 탭 오버레이**다. omp의 `ask`는 **하나의 범용 셀렉터(`HookSelectorComponent`)를 재사용**해 질문을 **한 번에 하나씩** 보여주고, ←/→ 로 이동하며, 답변 후엔 **채워진 메뉴(filled menu)** 모양으로 트랜스크립트에 남는다.

표절의 핵심은 두 가지다:
1. **모달 탭 → 범용 셀렉터 1회 호출의 순차 반복**(질문 N개 = 셀렉터 N번 호출 + ←/→ 네비게이션).
2. **트랜스크립트 결과를 "id → 답" 한 줄 요약이 아니라 "선택 마커가 채워진 전체 옵션 목록 + 커스텀 입력"으로 렌더**.

나머지(라디오/체크박스 행 마커·"Other" 자동 추가·"(Recommended)" 접미사·카운트다운·컴팩트+퍼지 검색)는 이 두 축 위에 얹히는 디테일이다.

---

## 1. 현황 비교 — 코드 수준

### omp `ask` (`ask.ts` 978줄)

| 측면 | omp 구현 | 소스 |
|------|----------|------|
| 흐름 | **순차 1문항씩**, ←/→ 로 이전/다음 질문 이동, 진행률 `(2/3)` | `askSingleQuestion` + `NavigationControls` (`ask.ts:190-391`, `610-650`) |
| UI 원시 | `ui.select()` / `ui.editor()` 두 가지(`ExtensionUiController`) | `ask.ts:164-188`, `extension-ui-controller.ts` |
| 셀렉터 | `HookSelectorComponent` — **모든 오버레이(모델/세션/설정 선택)가 공유** | `hook-selector.ts:149-659` |
| 행 마커 | `selectionMarker: "radio"|"checkbox"` + `checkedIndices` + `markableCount` → 행 앞에 `◉`/`○` 또는 `☑`/`☐` | `hook-selector.ts:327-343` |
| "Other" | **자동 추가** `"Other (type your own)"` → 선택 시 `ui.editor()` 인라인 프롬프트 | `ask.ts:101, 275, 304-315, 368-376` |
| "Done" | 멀티선택 + forward-nav 없을 때 `"✔ Done selecting"` 자동 추가 | `ask.ts:108-110, 272-274` |
| Recommended | 옵션 라벨에 `" (Recommended)"` 접미사 + 초기 커서 | `ask.ts:102, 112-124, 335` |
| 컴팩트 모드 | 옵션이 `maxVisible`(12) 초과 시 라벨만 + 하이라이트 항목 설명 + **퍼지 검색** | `hook-selector.ts:478-532, 569-614` |
| 타임아웃 | 타이틀에 `(30s)` 카운트다운, 만료 시 recommended 자동 선택 | `hook-selector.ts:222-238`, `ask.ts:515-520` |
| 동시성 | `concurrency = "exclusive"` (셀렉터가 단일 공유 표면이라 병렬 ask 금지) | `ask.ts:466-471` |
| 트랜스크립트 | `mergeCallAndResult: true` — 호출/결과가 **하나의 framed block**으로 병합 | `ask.ts:818-977` |
| 결과 렌더 | **모든 옵션을 재출력하되 선택된 것만 마커 채움** + 커스텀 입력 + "auto-selected after timeout" 각주 | `renderAnswerOptionLines` (`ask.ts:786-816`) |
| 프롬프트 규율 | "Other 넣지 마라(UI가 넣는다)", "2-5 옵션", "default to action" | `prompts/tools/ask.md` |

### oxi `questionnaire` (현재)

| 측면 | oxi 구현 | 소스 |
|------|----------|------|
| 흐름 | **모달 탭 오버레이**, 상단 탭바(Q1…Qn, Submit), 화면 중앙 | `questionnaire.rs`(overlay) `48-311` |
| 브리지 | `QuestionnaireBridge`(Arc-Mutex) — agent 스레드↔TUI 메인 스레드, oneshot 채널 | `oxi-agent/src/tools/questionnaire.rs:26-93` |
| 행 마커 | **오버레이 행에 마커 없음**. 트랜스크립트 프리뷰에만 `radio_off`/`nav_selected(★)` 사용 | `tool_renderer.rs:700-715` |
| "Other" | `allow_other` 플래그 → 별도 인라인 편집 모드(`input_mode`) | overlay `31-46` |
| 탐색 | 탭 전환(숫자키/Tab), 엔터 선택/토글, Submit 탭 | overlay `343-505` |
| 타임아웃 | `started_at` 기반, 만료 시 `auto_select_defaults()` | overlay `228-340` |
| 결과 렌더 | 헤더 한 줄 + `"id → 답"` 한 줄씩, 타임아웃 항목에 `⏱` | `format_questionnaire_result` `725-766` |
| 심볼 | radio_on/off·checkbox_on/off·cursor·tool_ask(`?`)·sep_dot **이미 보유** | `symbols.rs:231-291, 346-349, 373` |

### 격차 요약

```
omp ask                            oxi questionnaire
─────────────────────────          ─────────────────────────────
순차 1문항 (←/→ 네비)      ◀︎▶︎    탭 모달 (화면 강탈)
범용 셀렉터 재사용          ◀︎▶︎    전용 QuestionnaireOverlay (재사용성 無)
행 마커 ◉/○ · ☑/☐          ◀︎▶︎    행에 마커 無 (심볼은 있는데 안 씀)
"Other" 자동 + 인라인 ed    ◀︎▶︎    allow_other 플래그 (별도 모드)
"(Recommended)" 접미사      ◀︎▶︎    recommended 인덱스만 (시각 표시 無)
컴팩트 + 퍼지 검색          ◀︎▶︎    없음 (옵션 많으면 짤림)
filled-menu 결과            ◀︎▶︎    "id → 답" 한 줄 요약
framed block 병합 렌더      ◀︎▶︎    call/result 분리 렌더
```

> **주의:** omp는 **스크롤백 네이티브**라 셀렉터가 "트랜스크립트 흐름 속"에 등장한다. oxi는 **ratatui 대체 화면**이라 진짜 인라인은 불가능(이것은 렌더링 엔진 트랙의 범위). 따라서 oxi에서는 **"오버레이 기구는 유지하되 omp의 셀렉터 미학 + 순차 흐름으로 재설계"** 하고, **완료 후 트랜스크립트 블록만 omp의 filled-menu 미학**으로 바꾼다. 이 차이는 아래 설계 전체의 전제다.

---

## 2. 설계 원칙 (omp에서 가져올 것·가져오지 않을 것)

### 가져올 것
1. **범용 셀렉터 단일화** — omp처럼 모든 리스트형 오버레이(ask·model·session·resume·logout·provider)가 **하나의 `ListSelector` 프리미티브**를 공유. oxi는 현재 각 오버레이가 `List`+`Paragraph`를 직접 조립(`model_select.rs`, `resume_select.rs`, …).
2. **순차 1문항 흐름** — N개 질문 = 셀렉터 N회 호출, ←/→ 로 이동, `progress_text "(k/N)"`. 탭 모달 폐기.
3. **행 마커 칼럼** — 라디오(단일)/체크박스(멀티) 마커를 **셀렉터 행 자체에** 그린다. oxi는 심볼(`radio_on/off`, `checkbox_on/off`)을 이미 가지고 있으나 호출처가 없다.
4. **"Other" 자동 추가 + 인라인 편집기** — `allow_other=true` 가 의미가 있게(현재는 별도 input_mode 분기).
5. **"(Recommended)" 접미사 + 초기 커서** — recommended 인덱스 라벨에 접미사, 커서 시작 위치.
6. **filled-menu 결과 렌더** — 결과를 모든 옵션 재출력 + 선택 마커 채움 + 커스텀 입력 + 타임아웃 각주.
7. **카운트다운 타임아웃** — 타이틀에 `(Ns)`, 만료 시 recommended 자동 선택(oxi는 `auto_select_defaults`가 이미 있으나 카운트다운 표시 無).
8. **컴팩트 + 퍼지** — 옵션 ≥ 임계치면 라벨만 + 하이라이트 설명 + 타이핑 검색. (oxi는 model_select에 filter 문자열이 있으나 ask에는 없다.)

### 가져오지 않을 것 (oxi 스택 한계 / 이미 우위)
- **스크롤백 인라인 등장** — ratatui 대체 화면 한계. 범위 밖.
- **omp의 chalk 함수형 테마** — oxi의 타입화된 `ThemeStyles` + `Symbols` 구조체가 **유지보수 면에서 우위**(기존 분석 TL;DR 확인). 그대로 유지.
- **omp의 arkType 스키마** — oxi는 `serde` + 트레이트. Rust 관습 유지.

---

## 3. 파일 매핑 (omp 소스 → oxi 대상)

| omp 소스 (`/tmp/omp`) | 역할 | oxi 대상 (신규/수정) |
|---|---|---|
| `modes/components/hook-selector.ts` (660줄) | 범용 셀렉터 컴포넌트 | **신규** `oxi-cli/src/tui/overlay/list_selector.rs` (범용 `ListSelectorOverlay`) |
| `tools/ask.ts:190-391` (`askSingleQuestion`) | 순차 질문 상태기계 | **수정** `oxi-cli/src/tui/overlay/questionnaire.rs` → 순차 흐름으로 재작성 (또는 `ask.rs`로 개명) |
| `tools/ask.ts:418-670` (`AskTool`) | 도구 본체 | **수정** `oxi-agent/src/tools/questionnaire.rs` → `name="ask"`, `label="Ask"`, 순차 호출 |
| `tools/ask.ts:786-816` (`renderAnswerOptionLines`) | filled-menu 결과 렌더 | **수정** `oxi-tui/src/widgets/tool_renderer.rs::format_questionnaire_result` |
| `tools/ask.ts:818-883` (`renderCall`) | 호출 프리뷰 | **수정** `format_questionnaire_call` (마커 + 접미사) |
| `tui/output-block.ts` (`framedBlock`) + `status-line.ts` | 프레임/상태헤더 | oxi는 이미 `chat/render.rs:96-136`에 상태별 프레임 구현 → **재활용**, 섹션 분리자만 추가 |
| `modes/theme/theme.ts:234-431` (UNICODE_SYMBOLS) | 심볼 테이블 | oxi `symbols.rs` 이미 상응 필드 보유 → **확장 불필요**(아래 §5) |
| `prompts/tools/ask.md` | 도구 프롬프트 규율 | **신규** `oxi-agent` 시스템 프롬프트의 questionnaire 설명을 ask.md 규율로 교체 |

---

## 4. 단계별 구현 계획

### Phase A — 범용 `ListSelectorOverlay` 프리미티브 (토대)

**목표:** omp `HookSelectorComponent`의 ratatui 판. 모든 리스트 오버레이의 공통 조상.

**대상:** 신규 `oxi-cli/src/tui/overlay/list_selector.rs`

**API (계약):**
```rust
pub struct ListSelectorOverlay {
    // 표시
    title: String,                    // omp: accent 마크다운 타이틀
    options: Vec<SelectorOption>,     // { label, description: Option<String> }
    // 상태
    cursor: usize,
    search_query: String,             // 컴팩트 모드에서만 활성
    // 마커 (omp selectionMarker)
    marker: Option<SelectionMarker>,  // Radio | Checkbox
    checked: HashSet<usize>,          // 체크박스용
    markable_count: usize,            // 제어 행(Other/Done)은 마커 제외
    // 제어 행 (omp가 ask에서 주입)
    auto_other: bool,                 // "Other (type your own)" 자동 추가
    done_label: Option<String>,       // 멀티용 "Done selecting"
    // 탐색 콜백
    on_left: Option<Box<dyn FnMut()>>,
    on_right: Option<Box<dyn FnMut()>>,
    on_other: Option<Box<dyn FnMut()>>, // Other → 인라인 편집기
    on_select: Box<dyn FnMut(&SelectionResult)>,
    // 타임아웃
    timeout: Option<Duration>,
    started_at: Instant,
}

pub enum SelectionMarker { Radio, Checkbox }
pub struct SelectionResult { /* 선택된 인덱스/라벨, Other 여부, cancelled, timed_out, nav: Option<Back|Forward> */ }
```

**렌더 구조 (omp `hook-selector.ts:209-251` 대응):**
```
╭─ 동적 보더(상단)                                          ─╮   ← omp DynamicBorder
                                                             
  <accent> 타이틔 (30s)</accent>                              ← 마크다운 + 카운트다운
                                                             
  ◉ 옵션 A                                                     ← 행 마커 컬럼 (radio)
  ○ 옵션 B (Recommended)                                       ← 접미사
  ○ 옵션 C
    ↳ 설명 (muted, 인라인 마크다운)                            ← omp: indent + ↳
  ☐ Other (type your own)                                      ← 자동 추가
                                                             
  (2/4)  Type to search                                        ← omp status line (컴팩트일 때만)
                                                             
  <dim>up/down navigate  enter select  ←/→ question  esc cancel</dim>
                                                             
╰─ 동적 보더(하단)                                          ─╯
```

**입력 매핑 (omp `handleInput` `hook-selector.ts:616-645` 대응):**
- ↑/↓ (또는 j/k, 컴팩트 아닐 때): `move_selection` (비활성 행 스킵)
- Enter: 선택(라디오) 또는 토글+잔류(체크박스) → omp 멀티 루프(`ask.ts:269-332`)
- ←/→: `on_left`/`on_right` (질문 간 이동)
- Esc: `cancelled`
- 인쇄 가능 키: 컴팩트 모드일 때만 `search_query` 누적 + 퍼지 필터
- Other 행에서 Enter: `on_other` → 인라인 편집기(omp `ui.editor`)

**컴팩트 모드 게이트 (omp `#isSearchEnabled` 대응):** `total_option_rows > max_visible(12)` → 라벨만 + 하이라이트 설명 + 검색.

**수용 기준:**
- 단일 오버레이로 라디오·체크박스·검색·타임아웃·←/→ 모두 동작.
- `OverlayComponent` 구현(`render`/`handle_key`/`timeout_tick`).
- 단위 테스트: 마커 렌더 스냅샷, 컴팩트 전환 임계, 퍼지 필터, 비활성 스킵.

### Phase B — `ask` 도구 본체 재설계

**대상:** `oxi-agent/src/tools/questionnaire.rs` (개명 검토: `questionnaire` → `ask`)

**변경:**
1. `AgentTool::name()` → `"ask"` (하위호환: `questionnaire` 별칭 레지스트리는 제거 — clean cutover 원칙). `label` → `"Ask"`, 요약 → `"Ask the user a clarifying question"`.
2. **순차 흐름**: `execute()`가 질문 벡터를 받아 **한 번에 하나씩** 브리지에 푸시. omp `askSingleQuestion` + while 루프(`ask.ts:610-650`)를 Rust로 이식.
   - 각 질문 완료 시 `SelectionResult` 수신 → 다음 질문 푸시.
   - `nav = Back` → 이전 질문 재표시(이전 답을 `initialSelection`로 사전 채움, omp `initialSelection` 대응).
   - `nav = Forward` 또는 Enter → 다음.
3. **"Other" 자동**: 도구가 "Other" 행을 옵션 끝에 주입하지 않음 → **셀렉터가 자동 추가**(`auto_other`). 모델이 "Other"를 옵션에 넣으면 중복이므로 프롬프트로 금지(ask.md 규율).
4. **타임아웃**: `Settings::questionnaire_timeout_secs` → 유지. plan-mode일 때 omp처럼 비활성(`ask.ts:516-520` 참고 — oxi에 plan-mode가 있으면 동일 적용, 없으면 스킵).
5. **동시성**: omp `concurrency="exclusive"` 대응 — `QuestionnaireBridge`는 이미 단일 pending만 허용(`set()`이 `false` 반환)하므로 동등. 다만 `Agent::is_running` 검사는 별도(AGENTS.md pitfall).
6. **결과 포맷**: omp `formatQuestionResult`(`ask.ts:393-404`) 이식 — `id: "답"` / `id: [a, b]` / `id: "커스텀"` / `(auto-selected after timeout)` 접미사. 이 문자열이 트랜스크립트 렌더러 입력이 된다.

**브리지 변경:** 현재 `PendingQuestionnaire { questions: Vec<Question>, responder }` (한 번에 전체) → **`PendingAsk { question: Question, responder }` (한 번에 하나)** 로 변경. 순차 루프는 도구 쪽에 있으므로 브리지는 단일 질문만 운반.

**수용 기준:**
- 단일 질문·다중 질문(←/→)·멀티선택·Other·타임아웃 자동선택 모두 동작.
- 기존 회귀 테스트 `session_id_wiring_tests` 등 브리지 사용처 전수 수정.

### Phase C — 트랜스크립트 렌더 재설계 (filled menu)

**대상:** `oxi-tui/src/widgets/tool_renderer.rs::format_questionnaire_call` / `_result`

**결과 렌더(omp `renderAnswerOptionLines` `ask.ts:786-816` 이식):**
```
✔ Ask                                          ← status_success + tool_ask 아이콘
  Which auth method?                            ← 질문 프롬프트(accent)
  ◉ JWT                                         ← 선택됨: radio_on(success색)
  ○ OAuth2                                      ← 미선택: radio_off(dim색)
  ○ Session cookies
  ✔ 커스텀 입력: "둘 다"                         ← custom: status_success + 본문
  auto-selected after timeout — not a user choice  ← 타임아웃 각주(dim)
```
- 선택 마커 색: omp처럼 `success`(선택)/`dim`(미선택) 대비. oxi는 `radio_on`/`checkbox_on` + `success`, `radio_off`/`checkbox_off` + `dim`.
- 취소 시: `⚠ Cancelled` (이미 oxi에 있음).
- 멀티: `☑`/`☐` + 다중 행.

**호출 렌더(omp `renderCall` `ask.ts:820-883` 이식):**
- 다중 질문: `? Ask  3 questions` 헤더 + 각 질문 섹션(`[id] · multi · options:N` 메타) + 빈 마커 옵션 목록.
- 단일: `? Ask · options:N` + 질문 + 빈 마커 옵션(`radio_off` dim).
- "(Recommended)" 접미사는 호출 프리뷰에는 붙이지 않음(omp 동작) — 결과에서만 실제 선택이 드러남.

**마커 일관성:** 호출 프리뷰(미선택 `radio_off`/`checkbox_off` dim) ↔ 결과(선택 `radio_on`/`checkbox_on` success). oxi는 현재 호출에 `nav_selected(★)`/`radio_off` 혼용(`tool_renderer.rs:702-706`) → omp처럼 recommended는 **접미사**로, 마커는 **상태**로 분리.

**수용 기준:** 렌더 스냅샷 테스트(단일/다중/멀티/Other/취소/타임아웃 6케이스).

### Phase D — 프롬프트 규율 + 설정 정리

1. **시스템 프롬프트**: omp `ask.md` 규율 이식 — "default to action", "Other 넣지 마라", "2-5 옵션", "recommended로 기본 표시", "questions 배열로 한 번에". oxi의 `Question.allow_other` 기본값은 `true` 유지(이제 실제로 의미 있게 됨).
2. **설정명**: `questionnaire_timeout_secs` → `ask_timeout_secs` (별칭 마이그레이션 또는 clean cutover). `Settings` 필드명 정리.
3. **`/settings` 오버레이** 라벨 정리.

---

## 5. 심볼/테마 격차 — 추가 불필요 (이미 보유)

omp의 ask가 쓰는 모든 글리프가 oxi `Symbols`에 이미 존재한다:

| omp 심볼 | omp 값 | oxi 필드 | oxi 값(unicode) | 상태 |
|---|---|---|---|---|
| `radio.selected` | `◉` | `radio_on` | `◉` | ✅ 동일 |
| `radio.unselected` | `○` | `radio_off` | `○` | ✅ 동일 |
| `checkbox.checked` | `☑` | `checkbox_on` | `☑` | ✅ 동일 |
| `checkbox.unchecked` | `☐` | `checkbox_off` | `☐` | ✅ 동일 |
| `status.success` | `✔` | `status_success` | `✔` | ✅ 동일 |
| `status.warning` | `⚠` | `status_warning` | `⚠` | ✅ 동일 |
| `nav.cursor` | `❯` | `cursor` | `❯ ` | ✅ 동일 |
| `tool.ask` | `?` | `tool_ask` | `?` | ✅ 동일 |
| `sep.dot` | ` · ` | `sep_dot` | ` · ` | ✅ 동일 |
| `format.bracketLeft/Right` | `⟦`/`⟧` | (oxi에 **없음**) | — | ⚠️ 추가 후보(메타용) |

> **유일한 갭:** omp의 `[id]` 메타 표기용 괄호 글리프(`⟦`/`⟧`, `format.bracketLeft/Right`). oxi는 일반 괄호로 대체 가능하므로 **심볼 추가는 선택**. 추가 시 `Symbols`에 `bracket_left`/`bracket_right` 필드 + 3 프리셋 생성자 채우기(AGENTS.md "Adding a glyph" 절차).

---

## 6. 아키텍처 메모 — ratatui 제약 하의 적응

### omp "인라인 등장"을 oxi에서 흉내내는 법
omp는 스크롤백 네이티브라 셀렉터가 트랜스크립트 흐름에 직접 나타난다. oxi(ratatui 대체 화면)는 불가능. 대신:
- **진행 중**: `ListSelectorOverlay`가 기존 모달처럼 화면 위 합성(OverlayComponent). omp 미학(동적 보더·마커·카운트다운)으로 스타일.
- **완료 후**: 트랜스크립트 블록이 Phase C의 filled-menu로 렌더. 사용자 경험은 "오버레이가 닫히며 아래에 채워진 메뉴가 남는" 형태 — omp와 시각적으로 근접.

### 브리지 스레딩 (변경 없음)
`QuestionnaireBridge`(Arc-Mutex + oneshot) 패턴은 그대로. 다만 페이로드가 `Vec<Question>` → 단일 `Question`로 좁아진다(Phase B). `ui_attached` AtomicBool + 타임아웃 그대로.

### 회귀 주의 (AGENTS.md pitfall 준수)
- `session_id` 소유권: `QuestionnaireTool`→`ToolContext.session_id` 체인 유지. 브리지 재작성 시 `session_id: None` 재발현 금지(#13 회귀).
- `parking_lot::MutexGuard`는 `.await` 전 드롭. 브리지 잠금 후 oneshot `rx.await` 금지.

---

## 7. 의존성·순서·리스크

```
Phase A (ListSelectorOverlay)  ←─ 토대, 독립
   │
   ├─→ Phase B (ask 도구 순차화)  ←─ A의 프리미티브 사용
   │       │
   │       └─→ Phase D (프롬프트/설정)  ←─ B 완료 후
   │
   └─→ Phase C (트랜스크립트 렌더)  ←─ A와 독립, B와 병렬 가능
```

| 위험 | 완화 |
|------|------|
| 모델이 "Other"를 옵션에 넣어 중복 | 프롬프트로 금지 + 셀렉터가 "Other" 중복 시 하나 무시(omp `OTHER_OPTION` 상수 비교) |
| 탭 모달 사용처(슬래시 `/question`? 테스트?) | `lsp references`로 `QuestionnaireOverlay`/`Question` 사용처 전수 조사 후 마이그레이션 |
| 기존 회귀 테스트 깨짐 | `questionnaire` 이름을 유지하면서 내부를 ask로 바꾸는 것(=별칭)은 clean cutover 위반 → **이름까지 통일**하고 테스트 전면 수정 |
| ratatui 오버레이 렌더 비용 | Phase C의 filled-menu는 완료 후 1회 렌더(캐시 대상). `ToolFormatCache`가 이미 있음 |

---

## 8. 수행 전 조사 체크리스트 (구현 착수 전)

- [ ] `lsp references` `QuestionnaireOverlay` → 모든 사용처(overlay 팩토리·핸들러·테스트)
- [ ] `lsp references` `Question` / `QuestionnaireBridge` / `QuestionnaireResponse` → 도구·설정·세션 직렬화
- [ ] `Settings::questionnaire_timeout_secs` 사용처 전수
- [ ] `format_questionnaire_call/_result` 호출처(`format_tool_call/_result` 디스패치 + 테스트)
- [ ] oxi에 plan-mode 존재 여부(omp 타임아웃 비활화 조건)
- [ ] 기존 `2026-05-14-questionnaire-tool-design.md` 원 설계 의도 보존 포인트 확인

---

## 부록 A — omp ask 상태기계 (참고용 요약)

```
ask(questions: Vec<Q>)
  │
  ├─ len==1 ──► askSingleQuestion ──► {selected|custom|cancelled|timeout}
  │
  └─ len>1  ──► i=0
                 loop:
                   askSingleQuestion(q[i], nav={back: i>0, fwd, progress "(i+1)/N"})
                     ├─ nav=Back ─► i--
                     ├─ nav=Fwd/Enter ─► save result, i++
                     ├─ cancelled ─► abort turn
                     └─ timeout ─► auto-select recommended, i++
                   i==N ─► done
```

멀티선택(단일 질문 내): `while(true)` 루프에서 토글 후 잔류, "Done"/"Other" 행으로 탈출(`ask.ts:269-332`).

## 부록 B — omp 심볼 프리셋 교차 참조

oxi 3 프리셋(unicode/ascii/nerd)이 omp 3 프리셋(unicode/nerd/ascii)과 거의 동일. oxi는 이미 `radio_on: "(*)"`/`radio_off: "( )"`(ascii) 등 보유. ask 재설계에 **새 심볼 필드 불필요** (bracket_left/right만 선택적).
