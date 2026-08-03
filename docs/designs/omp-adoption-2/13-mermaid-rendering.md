# 세부 설계 ⑫ — Mermaid 터미널 렌더링

> 상태: **설계 v1 (구현 전 합의용)** — 그러나 아래 "(A) mmdc 기본" 설계는 단일-바이너리 원칙 위반으로 폐기됨. **2026-06-21 순수 Rust 서브셋 렌더러로 재구현됨** (CHANGELOG `[Unreleased]` 참조).
> 작성: 2026-06-19
> 선행: [`00-master-plan.md`](./00-master-plan.md)
> omp 분석: `modes/theme/mermaid-cache.ts`, `@oh-my-pi/pi-utils`의 `renderMermaidAsciiSafe`
> 후속: N1 구현 → CHANGELOG.md

---

## 0. 핵심 (TL;DR)

omp는 에이전트가 ` ```mermaid ` 코드 블록을 출력하면 **터미널에 ASCII 다이어그램으로 렌더**한다. 시퀀스·플로우차트·상태·ER 다이어그램 등을 텍스트 아트로 변환하여 구조를 시각적으로 전달한다.

oxicode는 현재 코드 블록 구문 강조만 지원하고 Mermaid 렌더는 없다. 본 설계는 oxicode-tui 마크다운 렌더에 Mermaid 블록 감지 + ASCII 변환을 추가한다. **가장 낮은 비용, 가장 빠른 가치 실현** 기능.

### omp가 검증한 가치
- **구조 전달** — 아키텍처·흐름·상태 머신을 텍스트보다 효율적으로 전달.
- **캐시** — 동일 소스는 한 번만 렌더 (옵션+방향 변형별 캐시).
- **뷰포트 적응** — 터미널 너비 초과 시 TD/LR 방향 전환으로 좁은 다이어그램 선택.

---

## 1. omp 메커니즘

### 1.1 렌더 파이프라인 (`modes/theme/mermaid-cache.ts`)

```
Mermaid 소스 (```mermaid 블록)
  → resolveMermaidAscii(source, {maxWidth, colorMode, ...})
    → renderMermaidAsciiSafe(source, options)  [pi-utils]
       → @mermaid-js/mermaid-cli 또는 내부 렌더러
    → 캐시 (Map<string, string|null>, 옵션+방향+소스 키)
    → maxWidth 초과 시 TD/LR 강제 전환 → 가장 좁은 것 선택
  → ASCII 문자열 (또는 null = 렌더 실패)
```

### 1.2 캐시 전략

```typescript
// 옵션 + 방향 + 소스로 키 구성
const key = `${baseKey}\x00${direction ?? ""}\x00${source}`;
const cache = new Map<string, string | null>();  // null = 실패도 캐싱
```

- **실패 캐싱**: 렌더 실패(null)도 캐싱 → 반복 시도 방지.
- **방향 변형**: 원본 + TD 강제 + LR 강제 3종을 캐싱 → 너비 변경 시 재렌더 없이 선택.

### 1.3 뷰포트 적응 (`resolveMermaidAscii`)

```typescript
function resolveMermaidAscii(source: string, options?: MermaidResolveOptions): string | null {
    const base = renderVariant(normalizedSource, baseOptions, baseKey, null);
    if (base === null) return null;
    if (maxWidth === undefined) return base;

    let best = base;
    let bestWidth = asciiDisplayWidth(base);
    if (bestWidth <= maxWidth) return base;

    // 너비 초과: 두 방향 모두 렌더 → 가장 좁은 것
    for (const direction of ["TD", "LR"] as const) {
        const variant = renderVariant(normalizedSource, baseOptions, baseKey, direction);
        if (variant === null) continue;
        const variantWidth = asciiDisplayWidth(variant);
        if (variantWidth < bestWidth) {
            best = variant;
            bestWidth = variantWidth;
        }
    }
    return best;
}
```

### 1.4 디스플레이 너비 계산

```typescript
function asciiDisplayWidth(ascii: string): number {
    let max = 0;
    for (const line of ascii.split("\n")) {
        const width = Bun.stringWidth(line);  // ANSI + CJK 인식
        if (width > max) max = width;
    }
    return max;
}
```

---

## 2. oxicode-tui 설계

### 2.1 렌더러 선택 — 핵심 결정

omp는 Node.js 생태계의 `@mermaid-js/mermaid-cli`에 의존한다. oxicode(Rust)는 세 가지 옵션이 있다:

| 옵션 | 설명 | 장점 | 단점 |
|---|---|---|---|
| **(A) mermaid-cli 외부 프로세스** | `mmdc` CLI 호출 (Node 필요) | omp와 동일 품질 | Node 의존, 프로세스 오버헤드 |
| **(B) charabia/rascii 자체 렌더** | Rust 순수 ASCII 아트 렌더러 | 의존 없음 | Mermaid 문법 파싱을 직접 구현해야 함 (큰 작업) |
| **(C) mermaid.js WASM 번들** | mermaid.js를 WASM으로 컴파일하여 `include_bytes!` | 단일 바이너리, 품질 보장 | WASM 런타임 의존 (wasmtime), 바이너리 크기 증가 |

**제안**: **(A) 기본 + (B) 폴백**.
- `mmdc`가 `$PATH`에 있으면 사용 (가장 높은 품질).
- 없으면 간단한 자체 렌더러(시퀀스/플로우차트 한정)로 폴백.
- 둘 다 불가능하면 원본 Mermaid 소스를 코드 블록으로 표시 (현재 동작).

> **결정 필요**: (A) vs (C). N1.1에서 합의. 우선 (A)로 시작, 사용자 피드백 후 (C) 검토.

### 2.2 렌더 인터페이스

`oxicode-tui/src/render/mermaid.rs` (신규):

```rust
use std::sync::OnceLock;
use parking_lot::RwLock;
use std::collections::HashMap;

/// Mermaid 렌더 옵션.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MermaidRenderOptions {
    pub color_mode: MermaidColorMode,
    pub use_ascii: bool,
    pub max_width: Option<u16>,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum MermaidColorMode {
    None,       // 색상 없음 (기본)
    Themed,     // 테마 색상
}

/// Mermaid 소스를 ASCII로 렌더. 실패 시 None.
pub fn render_mermaid_ascii(source: &str, options: &MermaidRenderOptions) -> Option<String> {
    let normalized = source.replace("\r\n", "\n").trim();
    if normalized.is_empty() { return None; }

    let cache = mermaid_cache();
    let key = cache_key(normalized, options);
    if let Some(cached) = cache.read().get(&key) {
        return cached.clone();
    }

    let rendered = render_uncached(normalized, options);
    cache.write().insert(key, rendered.clone());
    rendered
}

/// 캐시 무효화.
pub fn clear_mermaid_cache() {
    mermaid_cache().write().clear();
}

fn mermaid_cache() -> &'static RwLock<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<RwLock<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn cache_key(source: &str, options: &MermaidRenderOptions) -> String {
    format!("{}\x00{:?}\x00{}", source, options.color_mode, options.use_ascii)
}
```

### 2.3 외부 프로세스 렌더 (옵션 A)

```rust
fn render_uncached(source: &str, options: &MermaidRenderOptions) -> Option<String> {
    // 옵션 A: mmdc (mermaid-cli) 호출
    if let Some(ascii) = render_with_mmdc(source, options) {
        return Some(ascii);
    }

    // 옵션 B: 자체 간이 렌더러 (시퀀스/플로우차트)
    if let Some(ascii) = render_with_builtin(source, options) {
        return Some(ascii);
    }

    None
}

fn render_with_mmdc(source: &str, options: &MermaidRenderOptions) -> Option<String> {
    let mmdc = which::which("mmdc").ok()?;
    
    // 임시 파일에 소스 작성
    let temp_dir = tempfile::tempdir().ok()?;
    let input = temp_dir.path().join("diagram.mmd");
    let output = temp_dir.path().join("diagram.txt");
    std::fs::write(&input, source).ok()?;

    // mmdc -i input -o output -t default --outputFormat ascii
    let output = std::process::Command::new(mmdc)
        .arg("-i").arg(&input)
        .arg("-o").arg(&output)
        .arg("-t").arg("default")
        .arg("--outputFormat").arg("ascii")
        .output()
        .ok()?;

    if !output.status.success() { return None; }
    let ascii = std::fs::read_to_string(&output).ok()?;
    
    // 뷰포트 적응: maxWidth 초과 시 방향 전환
    if let Some(max_width) = options.max_width {
        adapt_to_viewport(&ascii, source, options, max_width)
    } else {
        Some(ascii)
    }
}
```

### 2.4 뷰포트 적응

```rust
fn adapt_to_viewport(
    base: &str,
    source: &str,
    options: &MermaidRenderOptions,
    max_width: u16,
) -> Option<String> {
    let base_width = ascii_display_width(base);
    if base_width <= max_width as usize {
        return Some(base.to_string());
    }

    // 원본에 direction이 있으면 보존, 없으면 두 방향 시도
    let has_direction = source.lines().take(5).any(|l| {
        l.trim_start().starts_with("flowchart")
            || l.trim_start().starts_with("graph")
            || l.contains("direction ")
    });

    if has_direction {
        return Some(base.to_string());  // 사용자가 명시한 방향 존중
    }

    // TD와 LR 강제 → 가장 좁은 것
    let mut best = base.to_string();
    let mut best_width = base_width;

    for direction in &["TD", "LR"] {
        let forced = format!("direction {}\n{}", direction, source);
        if let Some(variant) = render_with_mmdc(&forced, options) {
            let variant_width = ascii_display_width(&variant);
            if variant_width < best_width {
                best = variant;
                best_width = variant_width;
            }
        }
    }

    Some(best)
}

fn ascii_display_width(ascii: &str) -> usize {
    ascii.lines()
        .map(|line| unicode_width::UnicodeWidthStr::width(line))
        .max()
        .unwrap_or(0)
}
```

### 2.5 마크다운 렌더 통합

`oxicode-tui/src/widgets/chat/markdown.rs`의 코드 블록 처리에 Mermaid 분기 추가:

```rust
/// 펜스 코드 블록을 처리. ```mermaid 블록은 ASCII 다이어그램으로 렌더.
fn render_code_block(
    lang: &str,
    code: &str,
    theme: &Theme,
    max_width: u16,
) -> Vec<Line<'_>> {
    if lang == "mermaid" {
        let options = MermaidRenderOptions {
            color_mode: MermaidColorMode::None,
            use_ascii: true,
            max_width: Some(max_width),
        };
        if let Some(ascii) = render_mermaid_ascii(code, &options) {
            return render_ascii_diagram(&ascii, theme);
        }
        // 렌더 실패: 원본을 코드 블록으로 표시 (현재 동작)
    }

    // 기존 코드 블록 렌더 (구문 강조)
    render_highlighted_code(lang, code, theme)
}

/// ASCII 다이어그램을 테두리 박스로 렌더.
fn render_ascii_diagram(ascii: &str, theme: &Theme) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let border_style = theme.dim();

    // 상단 테두리
    lines.push(Line::from(vec![
        Span::styled("┌─", border_style),
        Span::styled(" diagram ", theme.accent()),
    ]));

    for line in ascii.lines() {
        lines.push(Line::from(vec![
            Span::styled("│ ", border_style),
            Span::raw(line.to_string()),
        ]));
    }

    // 하단 테두리
    lines.push(Line::from(vec![
        Span::styled("└─", border_style),
    ]));

    lines
}
```

---

## 3. 설정

```rust
pub struct Settings {
    pub mermaid_render_enabled: bool,      // 기본 true
    pub mermaid_renderer: MermaidRenderer,  // 기본 Auto
}

pub enum MermaidRenderer {
    Auto,       // mmdc 우선, 폴백 내장
    Mmdc,       // mmdc만 (없으면 원본)
    Builtin,    // 내장 렌더러만
    Disabled,   // 렌더 안 함 (코드 블록으로 표시)
}
```

---

## 4. 의존성 & 마일스톤

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| N1.26 | `render/mermaid.rs` — 캐시 + 인터페이스 | — |
| N1.27 | `render_with_mmdc` (외부 프로세스) | N1.26 |
| N1.28 | `adapt_to_viewport` (뷰포트 적응) | N1.27 |
| N1.29 | `render_ascii_diagram` (박스 렌더) | N1.26 |
| N1.30 | `markdown.rs` Mermaid 블록 분기 | N1.29 |
| N1.31 | 설정 (`mermaid_render_enabled`, `mermaid_renderer`) | N1.30 |
| N1.32 | (선택) 내장 간이 렌더러 (시퀀스/플로우차트) | N1.26 |

> **독립성**: ⑤ todo와 완전히 독립. N1에서 병렬 진행 가능.

---

## 5. 내장 간이 렌더러 (옵션 B, 후순위)

`mmdc`가 없을 때의 폴백. **시퀀스 다이어그램**과 **간단한 플로우차트**만 지원:

```rust
/// 시퀀스 다이어그램 간이 렌더.
/// sequenceDiagram
///   A->>B: message
///   B-->>A: reply
fn render_sequence_diagram(source: &str) -> Option<String> {
    let mut actors: Vec<&str> = Vec::new();
    let mut messages: Vec<(&str, &str, &str, bool)> = Vec::new();  // (from, to, msg, is_reply)

    for line in source.lines() {
        let line = line.trim();
        // participant A as Name
        if let Some(rest) = line.strip_prefix("participant ") {
            let name = rest.split_whitespace().next()?;
            actors.push(name);
        }
        // A->>B: message  또는  B-->>A: reply
        else if let Some(idx) = line.find("->>") .or_else(|| line.find("-->>")) {
            let is_reply = line[idx..].starts_with("-->>");
            let from = line[..idx].trim();
            let rest = &line[idx + (if is_reply { 4 } else { 3 })..];
            let (to, msg) = rest.split_once(':')?;
            actors_push_unique(&mut actors, from);
            actors_push_unique(&mut actors, to.trim());
            messages.push((from, to.trim(), msg.trim(), is_reply));
        }
    }

    if actors.is_empty() { return None; }

    // ASCII 렌더
    let mut out = String::new();
    // 헤더: actor 이름들
    // 메시지별: 화살표 라인
    // ...
    Some(out)
}
```

> **후순위**: N1.32. 기본은 (A) mmdc. 사용자 피드백 후 내장 렌더러 확장 여부 결정.

---

## 6. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| 렌더러 선택 (mmdc vs WASM vs 자체) | 🟡 미결정 | (A) mmdc 기본 제안. (C) WASM은 바이너리 크기 검토 후 |
| `mmdc` 의존 (Node 필요) | 🟠 위험 | Rust 순수 바이너리 철학과 충돌. 설정으로 비활성화 가능 |
| 렌더 지연 (프로세스 스폰) | 🟡 최적화 | 캐시로 1회만. 첫 렌더 ~500ms |
| 복잡한 다이어그램 (gantt, pie) | 🔴 후순위 | 시퀀스/플로우차트/상태 머신 우선 |
| 터미널 이미지 프로토콜 (Kitty/Sixel) | 🟢 별도 | `oxicode-tui/src/render/image.rs`가 이미 있음. PNG 렌더 옵션 추가 가능 |
| CJK 너비 계산 | 🟢 해결됨 | `unicode_width` 크레이트 사용 |

---

## 7. 테스트 계획

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_returns_same_result() {
        let source = "sequenceDiagram\n  A->>B: hello";
        let opts = MermaidRenderOptions {
            color_mode: MermaidColorMode::None,
            use_ascii: true,
            max_width: None,
        };
        // mmdc가 없으면 None → 캐시에 None 저장
        let result1 = render_mermaid_ascii(source, &opts);
        let result2 = render_mermaid_ascii(source, &opts);
        assert_eq!(result1, result2);  // 캐시 히트
    }

    #[test]
    fn empty_source_returns_none() {
        let opts = MermaidRenderOptions {
            color_mode: MermaidColorMode::None,
            use_ascii: true,
            max_width: None,
        };
        assert_eq!(render_mermaid_ascii("", &opts), None);
        assert_eq!(render_mermaid_ascii("   \n  ", &opts), None);
    }

    #[test]
    fn ascii_display_width_cjk() {
        // CJK 문자는 2칸
        assert_eq!(ascii_display_width("한글"), 4);
        assert_eq!(ascii_display_width("hello"), 5);
    }
}
```

---

## 8. 부록: omp → oxicode 매핑

| omp 위치 | oxicode 위치 |
|---|---|
| `modes/theme/mermaid-cache.ts` | `oxicode-tui/src/render/mermaid.rs` |
| `resolveMermaidAscii` | `render_mermaid_ascii` |
| `renderMermaidAsciiSafe` (pi-utils) | `render_with_mmdc` / `render_with_builtin` |
| `asciiDisplayWidth` (Bun.stringWidth) | `ascii_display_width` (unicode_width) |
| `adaptToViewport` (TD/LR 전환) | `adapt_to_viewport` |
| `clearMermaidCache` | `clear_mermaid_cache` |
| 마크다운 렌더의 ```mermaid 분기 | `markdown.rs::render_code_block` |
