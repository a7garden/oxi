# omp vs oxi — TUI 정밀 비교 분석 및 개선 방향

> **작성:** 2026-06-20
> **대상:** `can1357/oh-my-pi`(omp, TypeScript + Bun + Rust)의 TUI 계층 vs
> `oxi-tui` / `oxi-cli::tui`(Rust, ratatui 0.30 기반)
> **목적:** "omp가 더 수려하다"는 체감의 기술적 원인을 코드 수준에서 규명하고,
> ratatui 스택을 유지한 채로 oxi가 따라잡을 수 있는 **현실적 개선 로드맵**을 제시한다.

---

## 0. TL;DR

| 영역 | oxi 현재 상태 | omp 상태 | 격차 |
|------|---------------|----------|------|
| 렌더링 모델 | ratatui **대체 화면(full-frame)** + `DiffBackend` 행 단위 diff | **스크롤백 네이티브**(live region만 소유) | **근본적 차이** |
| 동기화 출력(CSI 2026) | ✅ `DiffBackend`가 매 diff 프레임 감쌈 | ✅ | 동등 |
| 행 단위 diff | ✅ u64 체크섬 | ✅ (3-전략) | 동등 |
| **DECCARA 배경 채우기 최적화** | ❌ 모든 배경 셀을 일일이 기록 | ✅ 사각형 1개 escape로 대체 | **큼** |
| **터미널 캡 능력 탐지** | 환경변수 스니핑(4KB) | live probe + Sixel + XTGETTCAP(37KB) | **큼** |
| 서브트리/컴포넌트 메모이제이션 | Layout cache만(전체 프레임은 매번 빌드) | ✅ render() 참조 동일 → 스킵 | 중간 |
| Bracketed paste | ✅ 활성화(원시 모드) | ✅ + ">10줄 paste 마커" | 작음 |
| Kitty 키보드 프로토콜 | ✅ `PushKeyboardEnhancementFlags` | ✅ | 동등 |
| 인라인 이미지 | Kitty / iTerm2 | Kitty / iTerm2 / **Sixel** | 작음~중간 |
| LaTeX 수식 | ❌ | ✅ latex-to-unicode(51KB) + latex-block | 중간 |
| 툴 출력 블록 프레이밍 | ✅ 이미 상태별 테두리색/배경/보더타입+아이콘(`render.rs:96-136`) | `renderOutputBlock` + 섹션 분리자 + `CachedOutputBlock` bigint 해시 | 작음 |
| SGR 송출 효율 | modifier 변경 시 **전체 Reset 후 fg/bg 재적용** | 최소 delta SGR | 중간 |
| 렌더 루프 와치독 | ❌ | ✅ `loop-watchdog` | 작음 |
| **GlyphSet(Unicode/ASCII/Nerd)** | ✅ `symbols.rs` | 부분적 | **oxi 우위** |
| **CJK 안전 줄바꿈** | ✅ 직접 구현(`buf.set_line` 와이드 문자 처리) | Bun.wrapAnsi | 동등~oxi 우위 |
| **타입화된 ThemeStyles** | ✅ 구조체 + TOML/JSON hot-reload | 함수형 `(s)=>chalk.c(s)` | **oxi 우위(유지보수)** |

**결론:** omp가 "더 수려해 보이는" 감각의 **80%는 두 가지**에서 온다.
1. **스크롤백 네이티브 렌더링** — 끝난 메시지가 터미널 자체 스크롤백으로 넘어가, 히스토리가 "무료"고 터미널 네이티브 검색/복사가 그대로 동작한다. ratatui 대체 화면은 이것이 불가능하다.
2. **배경이 있는 카드 UI에서 DECCARA** — omp는 단색 배경 패널을 사각형 escape 1개로 치환해 송출 바이트를 극적으로 줄인다. oxi는 상태별 테두리색/배경/아이콘은 이미 구현(`render.rs:96-136`)했으나, 배경 셀을 매 프레임 일일이 기록한다. "박스 UI가 빠르고 반응적인" 느낌의 남은 격차는 여기에 있다.

oxi는 **타입화된 테마, GlyphSet, CJK 처리**에서는 오히려 앞서므로, 이를 버릴 필요는 없다. 아래 개선안은 전부 **ratatui 스택 안에서** 구현 가능하다.

---

## 1. 아키텍처 뿌리: full-frame vs scrollback-native

### 1.1 omp — 스크롤백 네이티브 (3-전략 diff)

omp의 TUI(`packages/tui`)는 터미널을 "아래쪽 live region + 위쪽은 터미널 자체 스크롤백"으로 모델링한다.
`packages/tui/README.md`에 명시된 3-전략:

1. **First Render** — 스크롤백을 지우지 않고 모든 줄 출력
2. **Width Changed / Change Above Viewport** — 화면 클리어 후 풀 리렌더
3. **Normal Update** — 첫 변경 줄로 커서 이동 → clear-to-end → 변경 줄만 출력

컴포넌트는 `render(width): readonly string[]`을 반환하며, **내용이 안 바뀌었으면 같은 배열 참조를 반환**하여 렌더러 수준 메모이제이션을 건다(`requestComponentRender`는 해당 서브트리만 다시 그린다).

**파생 이점(전부 "공짜"):**
- 히스토리가 터미널 스크롤백에 그대로 남음 → 세션 로그를 tmux/터미널 스크롤백에서 그대로 검색/복사
- 화면 밖으로 나간 과거 내용은 프레임당 비용 0
- 터미널 네이티브 selection이 의미 있게 동작

### 1.2 oxi — ratatui 대체 화면 + DiffBackend

`oxi-cli/src/tui/app.rs:46,73`에서 `Terminal<DiffBackend<io::Stdout>>`를 생성하고,
`DiffBackend`(`oxi-tui/src/render/mod.rs:131`)는 ratatui `draw()` 이터레이터를 행 단위로 묶어 이전 프레임과 u64 체크섬으로 비교한 뒤 **변경된 행만** crossterm으로 기록한다. CSI 2026(`\x1b[?2026h … l`)로 전체를 감싼다(`mod.rs:215,327`).

```rust
// oxi-tui/src/render/mod.rs:200-210  (요지)
if self.force_full_redraw || self.prev_rows.is_empty() {
    let all_cells = row_cells.into_iter().flatten().collect();
    self.inner.draw(all_cells.into_iter())?;   // 풀 리드로우
    ...
}
// 이후: 행별 체크섬 비교 → 변경 행만 MoveTo + 셀 기록
```

**한계(근본적):**
- 전체 프레임은 매 렌더마다 빌드된다(Layout cache는 레이아웃만 캐시). diff가 I/O를 줄일 뿐, CPU 측 "전체 버퍼 생성" 비용은 그대로.
- 대체 화면이므로 터미널 스크롤백을 활용 못 함. 히스토리는 뷰포트 안에서 가상화.
- 서브트리 단위 스킵이 없다. (omp의 `requestComponentRender`에 해당하는 것이 없음)

> **이 격차는 ratatui를 버리지 않는 한 완전히 닫을 수 없다.** 하지만 1.x절의 "가짜 비용" — 즉 매 프레임 전체 버퍼 빌드 + 모든 배경 셀 기록 — 은 ratatui 안에서도 크게 줄일 수 있다(§3 참조).

---

## 2. "수려함"의 실체를 이루는 기술적 디테일 (oxi가 놓친 것들)

### 2.1 DECCARA 사각형 배경 채우기 — **가장 큰 가성비 격차**

omp `packages/tui/src/deccara.ts`는 Kitty가 확장한 VT510 DECCARA("Change Attributes in Rectangular Area")를 이용해, **단색 배경으로 채워진 행의 trailing-space 패딩 전체를 사각형 escape 1개로 치환**한다.

```
<ESC>[2*x                      DECSACE: 사각형 extent 선택
<ESC>[Pt;Pl;Pb;Pr;<sgr>$r      DECCARA: rows Pt..Pb × cols Pl..Pr 에 <sgr> 적용
<ESC>[*x                       DECSACE: 기본 extent 복원
```

`analyzeBgFillLine()`은 행이 "단일·일정·비기본 배경 위의 trailing 공백"임을 **증명 기반으로** 검증한다(모호하면 `null` 반환 → 원본 유지). `planDeccaraFills()`는 인접 행의 동일 span을 하나의 사각형으로 병합하고, "사각형 바이트 비용 < 제거된 공백 바이트"일 때만 적용해 **결코 원본보다 커지지 않는다.**

**oxi의 현주소:** `DiffBackend::draw()`는 배경 셀을 행마다 전폭 스페이스로 일일이 기록한다(`mod.rs:248-319`). 즉, 좌측 보더가 있는 사용자 메시지 박스나 단색 배경 카드가 화면에 여러 줄 있으면, **매 diff 프레임마다 그 배경 스페이스 런 전체가 바이트로 나간다.** omp는 사각형 1개로 끝난다.

이것이 "omp가 더 깔끔하고 빠르게 느껴지는" 지각적 차이의 상당 부분이다. 그리고 **ratatui와 완전히 호환된다** — 프레임 빌드 후, DiffBackend가 변경 행을 기록할 때 적용하면 된다(§3.1).

### 2.2 터미널 캡 능력 탐지의 깊이 차이

| | oxi `render/terminal.rs` (≈4KB, 92행) | omp `terminal-capabilities.ts` (37KB) |
|---|---|---|
| 방식 | 환경변수 스니핑(`TERM`, `TERM_PROGRAM`, `COLORTERM`, `KITTY_WINDOW_ID`) | live DA1/DA2/DA3, XTGETTCAP, XTVERSION, DECRQSS 질의 |
| Sixel | ❌ | ✅ (DA1 attribute 4) |
| 동기화 출력 지원 여부 | 가정(무조건 CSI 2026 송출) | ✅ 실제 지원 여부 probe 후 게이트 |
| DECCARA 지원 여부 | ❌ | ✅ |
| 셀 크기(px) | ✅ (window_size 유도) | ✅ |

oxi는 CSI 2026을 **지원하지 않는 터미널에도 무조건 송출**한다(`mod.rs:215`에서 에러 무시). 대부분의 최신 터미널은 무해하게 무시하지만, probe 기반이면 DECCARA/CSI2026/Sixel을 **실제 지원 터미널에서만** 켜는 깔끔한 게이트가 된다.

### 2.3 SGR 송출: Reset-then-reapply vs 최소 delta

oxi `DiffBackend`는 셀의 `Modifier`가 바뀌면 **`SetAttribute(Reset)`로 전 속성을 날린 뒤** bold/italic/...를 다시 켜고 fg/bg를 다시 적용한다(`mod.rs:267-315`). 정확하지만 **SGR 바이트가 비대하다.** omp는 원시 ANSI 문자열을 직접 제어하므로 필요한 SGR delta만 송출한다.

ratatui 비트마스크 `Modifier` 모델에서도 "이전 비트 → 새 비트"의 **set/unset delta만 송출**하는 SGR emitter를 쓸 수 있다(§3.3).

### 2.4 툴 출력 블록 프레이밍 — **oxi는 이미 대부분 갖춤**

omp `coding-agent/src/tui/output-block.ts`의 `renderOutputBlock` 특징:
- **상태 → 테두리색 매핑:** `running/pending = accent`, `success = dim`, `error = error`, `warning = warning`.
- **섹션 분리자:** `teeRight`/`teeLeft` 글리프로 라벨 있는 섹션 구분.
- **배경 안정화:** 중첩 콘텐츠의 `\x1b[0m`가 패널 배경을 지우지 않도록 reset 뒤에 배경 SGR 재주입.
- **`CachedOutputBlock`:** 옵션을 bigint 해시로 요약해 동일하면 캐시된 줄 반환("~99% render 호출에서 재계산 스킵").

**oxi의 현주소(`chat/render.rs:96-136`) — 오히려 이 축에서는 동등 이상:**
- ✅ **상태 → 테두리색:** `Requested=muted`, `Executing=warning`, `Done=success/error` (`render.rs:96-107`).
- ✅ **상태 → 배경색:** `tool_pending_bg / tool_executing_bg / tool_success_bg / tool_error_bg` (`render.rs:112-123`, `theme.rs:73-80`에 색 정의됨).
- ✅ **상태 → 보더 타입:** `Requested=LightDoubleDashed`, `Executing/Done=Plain` (`render.rs:127-130`).
- ✅ **상태 → 아이콘:** `dot_off / dot_on / status_success / status_error` (`render.rs:96-107`).

**남은 진짜 격차(작음):** (a) 라벨 있는 **다중 섹션 분리자**(tee 글리프), (b) `CachedOutputBlock` 식 **결과 해시 캐시**(완료된 툴 블록은 거의 안 바뀌므로 매 프레임 재포맷을 스킵). 이 둘만 §3 P1에 남긴다.

### 2.5 그 외(작지만 체감되는 것들)
- **LaTeX 수식:** omp는 `latex-to-unicode.ts`(51KB)로 `$...$`/`$$...$$`를 유니코드 수식으로 렌더. oxi는 원시 텍스트.
- **Sixel:** omp는 Kitty/iTerm2 외에 Sixel 터미널(mlxterm 등)도 지원. oxi `ImageProtocol`은 Kitty/iTerm2 두 가지(`terminal.rs:7-13`).
- **대용량 paste 마커:** omp는 bracketed paste에서 ">10줄"이면 `[paste #1 +50 lines]` 마커. oxi는 bracketed paste를 **활성화는** 하지만(`app.rs:63`) 스마트 마커 로직이 없다.
- **렌더 루프 와치독:** omp `loop-watchdog.ts`(3.7KB)가 프레임 시간을 측정해 렌더 폭주를 감지·백오프. oxi는 무방비.

---

## 3. 개선 로드맵 (ratatui 호환, 우선순위 순)

> 모든 항목은 **ratatui 0.30 + DiffBackend 스택을 유지**한다. 스크롤백 네이티브로의 전환은 별도의 대형 아키텍처 결정이므로 이 보고서의 범위 밖(§4에서만 언급).

### P0 — DECCARA 배경 채우기 최적화 (`DiffBackend`에 통합)
**왜:** 단색 배경 카드/보더 UI에서 송출 바이트와 체감 지연을 크게 줄인다. omp와 oxi의 "수려함 격차"의 가장 큰 조각.
**어떻게:** omp `deccara.ts`의 `analyzeBgFillLine` / `planDeccaraFills`을 Rust로 포팅. `DiffBackend::draw()`가 변경 행을 기록하기 직전에:
1. 캡 능력(DECCARA 지원)이 참일 때만,
2. 변경 행의 셀 시퀀스를 스캔해 "trailing이 단일·일정·비기본 bg의 공백"임을 증명하면,
3. trailing 스페이스 런을 버리고, 인접 행과 병합해 `\x1b[Pt;Pl;Pb;Pr;<sgr>$r` 사각형 1개로 치환.
**위험:** 좌표 1-based inclusive, DECSACE 래퍼 per-frame 비용 회계(omp와 동일). 증명 기반 보수 설계 유지.
**검증:** 배경 채운 카드 N줄에서 송출 바이트 비교 벤치; DECCARA 미지원 터미널에서는 no-op임을 확인.

### P0 — 터미널 캡 능력 탐지 강화 + 기능 게이트
**왜:** 환경변수만으로는 Sixel/DECCARA/CSI2026 지원을 확신할 수 없다. P0의 전제 조건.
**어떻게:** `render/terminal.rs`를 확장 — DA1/DA2/DA3 live 질의, XTGETTCAP(`Sync`, `rectangular...`), XTVERSION. 결과로 `capabilities.deccara / synchronized_output / sixel` 플래그 추가. DiffBackend는 이 플래그가 참일 때만 CSI 2026 / DECCARA 송출.
**검증:** Kitty/Ghostty/WezTerm/iTerm2/Tmux/일반 xterm에서 캡 매트릭스 단위 테스트.

### P1 — 툴 블록 다중 섹션 분리자 + 결과 해시 캐시
**왜:** 상태별 테두리색/배경/보더타입은 이미 구현됨(`render.rs:96-136`, §2.4). 남은 것은 (a) 라벨 있는 다중 섹션 시각적 분리, (b) 완료된 툴 블록의 매-프레임 재포맷 비용.
**어떻게:** (a) `Symbols` 테이블에 `tee_right/tee_left` 글리프 추가 후, 콜/결과 사이의 `rule`(`render.rs:187-191`)을 상태색 tee 분리자로 교체. (b) `(name, args, result_hash, status, width)`를 키로 `format_tool_call`/`format_tool_result` 결과를 캐시 — `Layout cache`(`chat/state.rs`) 툴 엔트리에 캐시 레이어 추가.
**검증:** 멀티 섹션 툴 출력 스크린샷; 동일 결과 재렌더 시 포맷 호출 0회(적중률 로깅); ASCII GlyphSet 폴백 확인.

### P1 — SGR delta emitter (`DiffBackend`)
**왜:** modifier 변경 시 전체 Reset+재적용보다 바이트가 적고 깔끔하다.
**어떻게:** 이전 `Modifier` 비트와 현재 비트의 차집합만 송출 — 추가된 비트는 해당 SGR on, 제거된 비트는 off. fg/bg는 이미 delta 처리 중(`mod.rs:256-265`)이므로 modifier만 개선.
**검증:** 동일 프레임 송출 바이트 비교; 시각적 회귀 없음 확인(특히 bold→italic 전환).

### P1 — `CachedOutputBlock` 스타일 툴 블록 해시 캐시
**어뜻게:** 툴 블록은 완료 후 내용이 거의 안 바뀌므로, (name, args, result, status, width)를 키로 렌더 결과 캐시. omp `CachedOutputBlock`과 동일. oxi는 이미 `Layout cache`(`chat/state.rs`)가 있으니, 그 안의 툴 엔트리에 캐시 레이어 추가.
**검증:** 캐시 적중률 로깅; 동일 결과 재렌더 시 CPU 0에 가까움.

### P2 — 서브트리 단위 dirty 스킵
**왜:** omp `requestComponentRender`가 "변경된 루트 서브트리만" 다시 그리는 것이 ratatui에서는 직접 대응이 안 된다. 하지만 Layout cache를 더 적극적으로 써서 **변경되지 않은 엔트리는 행 체크섬이 동일**하게 유지되면, DiffBackend가 자연스럽게 스킵한다. 즉 "재계산을 피해 체크섬이 안정적으로 유지되도록" 캐싱을 짜는 것으로 충분한 효과.
**검증:** 긴 세션에서 매 프레임 "변경 행 수" 프로파일링.

### P2 — LaTeX-to-unicode (마크다운 수식)
**왜:** omp의 수식 렌더링은 마크다운 출력의 품격을 한 단위 올린다.
**어떻게:** omp `latex-to-unicode.ts`의 매핑 테이블(그리스/위첨자/아래첨자/연산자) 서브셋을 Rust `render/markdown` 통합 지점에 포팅. `pulldown-cmark`는 수식 노드를 직접 주지 않으므로 `$...$` 텍스트 노드 후처리.

### P2 — Sixel 이미지 + 렌더 루프 와치독 + 대용량 paste 마커
- Sixel: `render/image.rs` + `ImageProtocol::Sixel` 추가(캡 탐지 P0 결과에 연동).
- 와치독: 프레임 시간 측정, 임계치 초과 시 `request_render` 쓰로틀/백오프.
- paste 마커: bracketed paste 시퀀스에서 줄 수 세어 >10이면 `[paste #N +M lines]` 마커(omp `bracketed-paste.ts` 참조).

---

## 4. (참고) 스크롤백 네이티브로의 전환은 할 것인가?

omp의 "가장 큰 체감 우위"인 스크롤백 네이티브 모델은 **ratatui를 버려야** 얻을 수 있다. 이는:
- 이점: 무한 히스토리, 터미널 네이티브 검색/복사, 과거 내용 0비용.
- 대가: ratatui의 방대한 위젯 생태계·커뮤니티·안정성을 포기; oxi-tui를 사실상 자체 TUI 프레임워크로 재건(omp `packages/tui/src/tui.ts` 147KB 규모).

**권고:** **당분간 비권장.** §3의 P0/P1만으로 "수려함 격차"의 체감적 대부분(DECCARA + 효율적 SGR + 섹션 분리)을 닫을 수 있다. 스크롤백 네이티브는 "터미널 순수주의자" 타겟의 별도 모드(옵션)로, 장기적으로만 평가할 가치가 있다.

---

## 5. oxi가 이미 앞서거나 동등한 부분 (유지·홍보할 것)

- **`GlyphSet`(Unicode/ASCII/Nerd) 시스템** (`symbols.rs`, 647행) — 7비트 직렬 콘솔부터 Nerd Font까지 한 설정으로 전환. omp에 이런 깔끔한 단일 추상화가 없다. **마케팅 포인트.**
- **CJK 안전 줄바꿈 + 와이드 문자 처리** — ratatui `WordWrapper`가 CJK를 못 써서 직접 처리(`render.rs:52-81` 주석). omp는 Bun.wrapAnsi에 의존.
- **타입화된 `ThemeStyles` + TOML/JSON hot-reload** — omp의 `(s)=>chalk.c(s)` 함수형 테마보다 유지보수·정적 분석에 유리. "유연함"은 omp가, "안전함"은 oxi가 이긴다.
- **포팅 정합성:** DiffBackend의 행 체크섬 diff와 CSI 2026은 omp와 기능적으로 동등.

---

## 6. 추천 실행 순서

1. **P0-A 캡 능력 탐지 강화** → 모든 기능 게이트의 기반.
2. **P0-B DECCARA 최적화** → 가장 큰 가성비.
3. **P1 툴 블록 섹션 분리자 + 결과 해시 캐시** → 렌더 효율(상태별 테두리는 이미 구현됨).
4. **P1 SGR delta emitter + CachedOutputBlock** → 렌더 효율.
5. 이후 P2 (LaTeX / Sixel / 와치독 / paste 마커)는 선택적.

P0+P1까지 마치면 omp 대비 **기능적 격차는 사실상 소멸**하고, 남는 차이는 "스크롤백 네이티브냐 아니냐"라는 철학적 선택 하나뿐이다.
