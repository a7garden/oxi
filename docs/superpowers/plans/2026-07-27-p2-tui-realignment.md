# P2 — TUI omp tape 모델 재정렬 구현 계획 (다-월간)

> **상위 설계:** `docs/superpowers/specs/2026-07-27-omp-realignment-design.md` (Phase 2, 결정 T1)
> **omp 소스:** `/tmp/omp/packages/tui/src/` (tui.ts 173KB — Component/Container/TUI + 3-전략 차등 렌더링 + native scrollback; components/ — editor 117KB, markdown 98KB, input, select-list, settings-list, image, box, scroll-view, tab-bar, …; terminal.ts 62KB; terminal-capabilities.ts 43KB)
> **대상 크레이트:** `oxi-tui-legacy/` (74K LOC, 주축으로 진화) → `oxi-tui`로 rename; 현 `oxi-tui` v2(9.7K LOC, grok-inspired)는 폐기.
> **의존:** P1 느슨하게 (위젯 도메인 타입이 agent 이벤트에 의존).

**Goal:** legacy를 omp의 3-전략 차등 렌더링(component memo → native scrollback commit → ED3 replay)과 append-only "tape" 계약을 Rust로 진화시키고, 현 v2를 폐기해 이중 크레이트 교착 해소.

**Architecture:** omp TUI의 핵심 혁신 = native scrollback + append-only tape (commit된 row 불변, mutable 접미사만 in-place repaint). oxi-tui v2는 이게 없는 grok-inspired 재해석. legacy가 omp에 더 가까움(전체 위젯·glyph·mermaid 보유).

## Global Constraints
- 매 단계 green 게이트. v2 폐기는 단계적 (callsite를 legacy로 하나씩 옮긴 뒤 v2 crate 삭제).
- glyph 시스템은 legacy `symbols.rs`(GlyphSet Unicode/Ascii/Nerd, 50+ 필드)를 단일 소스로.
-AGENTS.md의 v2 약속(glyph in v2)은 doc/code 모순 — legacy 것으로 단일화로 해소.

## 작업 분해 (점진적, 각 단계 독립 배포)

### P2.1 — legacy → oxi-tui rename + v2 callsite 전환 (선행, 정리)
- legacy를 `oxi-tui`로 rename, 현 v2 crate를 단계적 폐기.
- `LegacyOverlayAdapter`(always-dirty, hash-skip 불가)에 의존하는 callsite를 legacy 직접 사용으로 전환.
- v2 의존 callsite를 legacy로 옮긴 뒤 v2 crate + `oxi-tui-v2-plan-a` 잔재 삭제.
- **수락 기준**: `oxi-tui` 단일 크레이트, `oxi-tui-legacy` 제거, 빌드 green.

### P2.2 — Native scrollback + append-only tape (핵심 혁신, omp 정렬)
- omp `NativeScrollbackLiveRegion`: finalized row를 터미널 scrollback에 commit(불변), mutable 접미사만 repaint.
- Rust 구현: crossterm의 alternate screen 대신 주 scrollback에 write하는 경로. `NativeScrollbackLiveRegion` 동등물.
- ED3 replay(CSI 3 J): finalized block 교체 시 erase-and-replay.
- **수락 기준**: 스트리밍 중 완료된 메시지가 scrollback에 commit, 재렌더 없이 유지.

### P2.3 — Component memoization 모델 (omp render(width) => string[])
- omp Component: `render(width) => readonly string[]`, 참조 identity = memoization 증명. Container는 unchanged child의 같은 배열 참조 skip.
- Rust: 위젯 trait에 content_hash 기반 memoization(legacy/v2에 부분적). omp의 참조-identity 모델과 정렬.
- **수락 기준**: unchanged 위젯은 재렌더 skip (hash 비교).

### P2.4 — 전체 입력 시스템 (가장 큰 UX gap)
omp 대응 (oxi에 전부 없음):
- **Kitty keyboard protocol**: `keys.ts`(16KB).
- **Bracketed paste**(paste markers): omp stdin-buffer(27KB).
- **Keybinding system**(conflict resolution): `keybindings.ts`.
- **Mouse**(SGR 1006): `mouse.ts`.
- **Kill ring / undo**: editor(117KB)의 기능.
- 현재 legacy는 stock ratatui 입력만.
- **수락 기준**: Kitty/bracketed-paste/keybinding/mouse 지원.

### P2.5 — 렌더링 풍부화
- **LaTeX**: omp latex-block(42KB) + latex-to-unicode(51KB) — inline Unicode + block ANSI.
- **Mermaid**: legacy `render/mermaid.rs`(85KB) 이미 보유 → v2로 이관/단일화.
- **Image rendering**: Kitty/iTerm2/Sixel 프로토콜 — omp kitty-graphics(8KB) + image.ts(16KB).
- **Autocomplete**: slash 명령 + 파일 경로 — omp autocomplete(37KB) + fuzzy(11KB). legacy fuzzy 보유.
- **Markdown**: omp markdown(98KB, marked + LaTeX + OSC 66 headings). legacy tui-markdown → omp 수준으로 보강.

### P2.6 — Theme/glyph 단일화
- v2 ColorScheme(28 슬롯, legacy와 스키마 다름) ↔ legacy(26 슬롯) 통합. theme 파일 이식성 확보.
- glyph 시스템: legacy `symbols.rs`를 단일 소스(v2에는 아예 없음 → doc/code 모순 해소).

## 위험
- P2 전체는 **다-월간** 작업(omp tui.ts 173KB + components 합 수백 KB의 Rust 번역).
- native scrollback은 Rust/crossterm에서 nontrivial (alternate screen 패러다임 전환).
- 점진적 접근 필수: P2.1(정리) → P2.2(tape) → P2.4(입력) → P2.5(풍부화) 순, 각 단계 독립 배포.

## 수락 기준 (P2 전체)
- `oxi-tui` 단일 크레이트(v2/legacy 이중 제거).
- native scrollback + append-only tape 동작.
- 전체 입력 시스템(Kitty/paste/keybinding/mouse/kill ring).
- LaTeX/mermaid/image/autocomplete 지원.
- glyph 시스템 단일화.
- `cargo nextest run --workspace` green.

## 참고: 현 상태 (ScoutTui 2026-07-27)
- v2: 9.7K LOC, 222 테스트, ratatui 0.30 + crossterm. 3 기둥(draw_frame pipeline, RetainedTree, capability). 위젯 ChatView/Footer/Sticky/Overlay/Border/List/Scrollbar/Text.
- legacy: 74K LOC. theme(75KB), symbols(34KB), widgets/chat(~167KB), tool_renderer(61KB), render/mermaid(85KB), render/color_level(DEAD), keybindings/.
- revert `c37b6a3f`(grok-build) 직후 v2를 clean-room 재작성 → v2는 omp 포팅이 아니라 grok-inspired. legacy가 omp에 더 가까움.
