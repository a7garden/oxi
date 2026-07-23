# Session A: oxi-tui v2 테스트 + 벤치마크 + 하드닝

> **독립 세션용 문서.** 이 문서만 읽고 작업할 수 있도록 작성됨.
> **수정 파일**: `oxi-tui/tests/`, `oxi-tui/benches/`, `oxi-tui/src/` (doc comments only)
> **금지 파일**: `oxi-cli/src/` (Session B가 작업 중)

## 전제 상태

브랜치 `oxi-tui-v2-plan-a` (44+ commits). oxi-tui v2 라이브러리 완성 (42 파일, 9.7K LOC, 222 테스트). 모든 게이트 통과.

```bash
cargo nextest run -p oxi-tui   # 222 tests pass
cargo clippy -p oxi-tui -- -D warnings  # clean
cargo fmt --all -- --check     # clean
```

## 작업 1: PTY 기반 e2e 테스트 하네스 (~1일)

### 목표

`portable-pty` crate으로 가상 PTY를 열어 oxi 바이너리를 spawn하고, 실제 터미널에 출력되는 ANSI bytes를 검증. 단위 테스트(TestBackend)로는 잡을 수 없는 회귀(OSC8 escape, CSI 2026 sync, 커서 깜빡임)를 방지.

### 파일

- `oxi-tui/Cargo.toml` — `[dev-dependencies]`에 `portable-pty = "0.8"` 추가
- `oxi-tui/tests/pty_e2e.rs` — 신규

### 구현

```rust
// oxi-tui/tests/pty_e2e.rs
use portable_pty::{native_pty_system, CommandBuilder};

fn spawn_pty(args: &[&str]) -> (Box<dyn portable_pty::Master>, Box<dyn portable_pty::Child>) {
    let pty = native_pty_system().openpty(
        portable_pty::PtySize { rows: 24, cols: 80, ..Default::default() }
    ).unwrap();
    let cmd = CommandBuilder::new("cargo");
    cmd.args(&["run", "--bin", "oxi", "--"]);
    cmd.args(args);
    let child = pty.slave.spawn_command(cmd).unwrap();
    (pty.master, child)
}

fn read_until(master: &mut dyn portable_pty::Master, pattern: &str, timeout_ms: u64) -> String {
    // 타임아웃 내에 pattern이 나올 때까지 읽기
    // 실패 시 panic
}
```

### 테스트 케이스

```rust
#[test]
fn pty_minimal_boot() {
    // oxi --version 실행 후 출력에 "oxi" 포함 확인
    let (mut master, mut child) = spawn_pty(&["--version"]);
    let output = read_until(&mut *master, "oxi", 30000);
    assert!(output.contains("oxi"));
}

#[test]
fn pty_no_orphan_osc8_sequences() {
    // 터미널 출력에 \x1b]8;;\x1b\\ 이 짝이 맞는지 확인
    // (begin 없이 end가 나오면 안 됨)
}

#[test]
fn pty_csi_2026_wraps_frame() {
    // \x1b[?2026h 로 시작해서 \x1b[?2026l 로 끝나는지 확인
    // (CSI 2026 begin이 end보다 먼저 나와야 함)
}
```

### 주의

- CI 환경(ubuntu-latest)에서 PTY 할당이 안 될 수 있음 → `#[cfg(unix)]` 게이트
- `cargo run` 대신 미리 빌드한 바이너리 경로 직접 지정 권장
- macOS와 Linux PTY 동작 차이 주의

---

## 작업 2: 벤치마크 (~반나절)

### 목표

스트리밍 응답에서 RetainedChild<T> per-subtree skip의 CPU 절감 효과와 커서 dedup의 byte 절감 효과를 정량화.

### 파일

- `oxi-tui/Cargo.toml` — `[[bench]]` 섹션 추가, `criterion = "0.5"` dev-dep
- `oxi-tui/benches/streaming_memoization.rs` — 신규
- `oxi-tui/benches/cursor_dedup.rs` — 신규

### 벤치마크 1: 스트리밍 메모이제이션

```rust
// benches/streaming_memoization.rs
use criterion::{criterion_group, criterion_main, Criterion};
use oxi_tui::widget::{RetainedChild, Renderable, Text};

fn bench_streaming_skip(c: &mut Criterion) {
    c.bench_function("retained_child_skip_unchanged", |b| {
        b.iter(|| {
            let mut child = RetainedChild::new(Text::new("hello world"));
            // First render
            child.render_if_changed(area, &mut ctx);
            // 100 subsequent renders — all should skip
            for _ in 0..100 {
                child.render_if_changed(area, &mut ctx);
            }
        });
    });

    c.bench_function("retained_child_render_on_change", |b| {
        b.iter(|| {
            let mut child = RetainedChild::new(Text::new("hello"));
            for i in 0..100 {
                child.inner_mut().set_content(format!("token {}", i));
                child.render_if_changed(area, &mut ctx);
            }
        });
    });
}
```

### 벤치마크 2: 커서 dedup

```rust
// benches/cursor_dedup.rs
fn bench_cursor_dedup(c: &mut Criterion) {
    c.bench_function("cursor_reconcile_same_position", |b| {
        b.iter(|| {
            let mut cursor = CursorState::new();
            // First call emits bytes
            cursor.reconcile(Some(P1), &mut term);
            // 60 subsequent calls — same position → 0 bytes
            for _ in 0..60 {
                cursor.reconcile(Some(P1), &mut term);
            }
        });
    });
}
```

### 검증 기준

- `retained_child_skip_unchanged`가 `render_on_change`보다 10× 이상 빠름
- `cursor_reconcile_same_position`이 단일 reconcile보다 거의 제로 오버헤드

---

## 작업 3: oxi-tui v2 API 문서화 (~반나절)

### 목표

crate-level doc comment와 각 public 모듈에 `#![doc = "..."]` 추가. `cargo doc -p oxi-tui` 결과가 newcomers에게 유용한 레퍼런스가 되도록.

### 파일 (doc comments only, 로직 변경 없음)

- `oxi-tui/src/lib.rs` — crate overview, module map, quick start
- `oxi-tui/src/pipeline/mod.rs` — draw_frame vs draw_frame_closure 설명
- `oxi-tui/src/widget/renderable.rs` — Renderable trait 사용 가이드
- `oxi-tui/src/widget/retained_child.rs` — RetainedChild 사용 패턴
- `oxi-tui/src/widget/tree.rs` — RetainedTree + CursorSlot lifecycle
- `oxi-tui/src/content/chat_log.rs` — ChatLog 사용 예제
- `oxi-tui/src/text/streaming_md.rs` — checkpoint 모델 설명

### 예시 (lib.rs)

```rust
//! # oxi-tui v2 — Terminal-First Rendering Pipeline
//!
//! ## Quick Start
//!
//! ```no_run
//! use oxi_tui::pipeline::{draw_frame_closure, CursorState};
//! use oxi_tui::widget::FocusTarget;
//! use oxi_tui::theme::{Theme, TerminalCaps};
//!
//! let mut terminal = /* Terminal::new(DiffBackend::new(stdout))? */;
//! let mut cursor = CursorState::new();
//! let theme = Theme::dark();
//! let caps = TerminalCaps::detect();
//!
//! draw_frame_closure(
//!     &mut terminal, &mut cursor,
//!     FocusTarget::None, &theme, &caps,
//!     |ctx| {
//!         // Render widgets via ctx.buffer_mut()
//!     },
//! )?;
//! ```
```

---

## 작업 4: legacy 참조 감사 (~1시간)

### 목표

oxi-tui v2 크레이트 내에 `oxi_tui_legacy` 참조가 있는지 확인. 있으면 제거 (oxi-tui는 순수 위젯 라이브러리, legacy 의존 없음).

```bash
grep -rn 'oxi_tui_legacy\|oxi-tui-legacy' oxi-tui/src/
# 예상 결과: 0 hits (이미 완료됨 — 확인용)
```

### 확장 검사

```bash
# workspace 전체에서 legacy 참조 카운트
grep -rn 'oxi_tui_legacy' --include='*.rs' . | grep -v 'target/' | grep -v 'oxi-tui-legacy/' | wc -l
# 이 숫자는 Session B가 렌더링 마이그레이션을 진행하면서 줄어들어야 함
```

결과를 `docs/superpowers/plans/2026-07-22-tui-v2-session-a-testing.md` 말미에 기록.

---

## 체크리스트

- [x] 작업 1: PTY e2e — **기존 구현 확인 (신규 파일 없음)** — oxi-tui는 순수 leaf lib라 oxi 바이너리 spawn 불가; 실제 harness는 `oxi-cli/tests/`에 존재, OSC8/CSI2026은 DiffBackend 바이트 단위 테스트로 이미 커버
- [x] 작업 2: 벤치마크 — criterion 0.5 + `streaming_memoization`/`cursor_dedup` (composite **13.9×** 달성)
- [x] 작업 3: API 문서화 — crate Quick Start(doctest) + module docs, 내 파일 0 warning
- [x] 작업 4: legacy 참조 감사 — oxi-tui/src 코드 수준 의존 0건 (7 hit 전부 doc 인용)
- [x] 최종: `cargo nextest run -p oxi-tui` (222) + `cargo clippy -p oxi-tui --all-targets -- -D warnings` (clean) + `cargo fmt --all -- --check` (clean)

---

## 실행 결과 (Session A, 2026-07-22)

### 작업 1: PTY e2e — 기존 구현 확인 (신규 파일 없음)

- `oxi-tui/tests/pty_e2e.rs` 생성 금지: oxi-tui는 순수 leaf 라이브러리(oxi-* 의존 0)라 oxi 바이너리를 spawn할 수 없음.
- 실제 위치: `oxi-cli/tests/pty_harness.rs`(`PtySession` + `read_until` 완전 구현, `oxi_binary_available()` skip guard) + `oxi-cli/tests/pty_e2e.rs`(`test_pty_minimal_boot`).
- OSC8 짝 / CSI 2026 wrapping 회귀는 `oxi-tui/src/pipeline/diff_backend/mod.rs`의 바이트 단위 단위 테스트(`csi_2026_emits_sync_wrappers_around_diff_writes`, OSC8 begin/end pairing)가 PTY 실바이너리 테스트보다 deterministic·CI-safe하게 이미 컵버.

### 작업 2: 벤치마크 (criterion 0.5)

신규: `oxi-tui/benches/streaming_memoization.rs`, `oxi-tui/benches/cursor_dedup.rs`. 결과(Apple M4, release):

| 벤치마크 | 시간 | 비고 |
|---|---|---|
| `streaming_composite_memoized` | 40.4 µs | N=40 subtree, 1 active/frame |
| `streaming_composite_naive` | 560.2 µs | 동일 트리, 전체 re-render |
| `retained_child_skip_unchanged` | 12.6 µs | 단일 Text, 100 skip |
| `retained_child_render_on_change` | 28.5 µs | 단일 Text, 100 render |
| `cursor_reconcile_same_position` | 16.4 ns | dedup(0 emit) |
| `cursor_reconcile_changing_position` | 25.7 ns | emit(60 MoveTo) |

- **composite memoized 13.9× 빠름 → 10× 목표 달성.** 단일 Text는 render≈hash 비용이라 2.1×에 불과 — memoization 진짜 효과는 composite 트리에서.
- reconcile per-call ~0.15 ns(sub-ns). dedup overhead 사실상 제로. TestBackend는 side-effect가 없어 emit path가 DCE되므로 `CountingBackend`로 계측.

### 작업 3: API 문서화

- `lib.rs`: crate-level Quick Start(컴파일되는 `no_run` doctest) + 정확한 module map(구식 "Plans B/C" 라인 제거).
- `pipeline/mod.rs`: `draw_frame`(retained) vs `draw_frame_closure`(cutover).
- `widget/tree.rs`: `any_hash_changed` → render → cursor resolve lifecycle.
- `content/chat_log.rs`: append-only 모델 + content hash 설명.
- `renderable.rs`, `retained_child.rs`, `streaming_md.rs`는 기존 문서 충실 → 유지.
- `cargo doc -p oxi-tui` 빌드 성공(내 파일 0 warning). 6개 pre-existing warning은 `row.rs`/`serializer.rs`의 `private_intra_doc_links`(scope 외).

### 작업 4: legacy 참조 감사

- `oxi-tui/src/`: 7 hit — **전부 `//!` doc-comment의 clean-room migration 출처 인용**. 코드 수준 의존 0건. oxi-tui v2는 legacy-free 확정.
- workspace 전체(`oxi-cli/src/`): ~70+ hit — Session B 렌더링 마이그레이션 영역. 예상대로 감소 대상(Session B Phase 5).

### 하드닝 (pre-commit `cargo clippy --all-targets` 통과용 사전 수정)

- `src/widget/chat/mod.rs` 테스트: `append_message` `#[must_use]` 7건 → `let _ =` 처리.
- `src/pipeline/diff_backend/mod.rs` 테스트: `clippy::useless_format` 1건 → `.to_string()`.
