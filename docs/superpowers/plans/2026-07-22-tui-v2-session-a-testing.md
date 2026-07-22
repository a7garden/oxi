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

- [ ] 작업 1: PTY e2e 테스트 하네스 (portable-pty, 3+ 테스트)
- [ ] 작업 2: 벤치마크 (streaming memoization, cursor dedup)
- [ ] 작업 3: API 문서화 (cargo doc 정리)
- [ ] 작업 4: legacy 참조 감사
- [ ] 최종: `cargo nextest run -p oxi-tui` + `cargo clippy -p oxi-tui -- -D warnings` + `cargo fmt --all -- --check`
