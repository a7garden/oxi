# oxibrowser 0.20 Upgrade + Vision Routing Activation

> **Status:** Implemented
> **Date:** 2026-08-10
> **Scope:** `oxicode-ai` (router), `oxicode-agent` + `oxicode-sdk` (deps), `deny.toml` / `.cargo/audit.toml`
> **Trigger:** oxibrowser 0.20.0 published (headless rendering via Blitz, capture, PDF, full JS execution).

## 1. Context

oxicode depended on `oxibrowser` / `oxibrowser-core` **0.16** (`oxicode-agent`) and
**0.17** (`oxicode-sdk` re-export) — a version skew left over from a prior revert
(0.17 was reverted in `oxicode-agent` because it advertised a `browser` feature
that 0.17 didn't expose; `oxicode-sdk` kept 0.17). oxibrowser 0.20 ships:

- A new **`oxibrowser-render`** crate (Blitz / Stylo CSS + Taffy layout + vello_cpu
  paint) that `oxibrowser-core` now hard-depends on. Screenshots are real
  CSS-laid-out PNGs, not the legacy text-bitmap renderer.
- **`capture_screenshot_png` defaults to `full_page: true`** — full-page capture
  is now the default, transparent to callers.
- Full JS execution on navigation (`<script>` runs, fetch/XHR/WebSocket, Canvas,
  Shadow DOM, custom elements) — SPA pages actually render.
- `Page.printToPDF` (CDP-only), multi-tab (`Target.createTarget`), CORS, cookie
  expiry, dialogs.

## 2. Compatibility assessment (0.16 → 0.20)

**Fully compatible — zero oxicode source changes required for the upgrade itself.**
Every oxicode-touched surface is unchanged at 0.20:

| Surface | Status |
|---|---|
| `BrowserEvent` (5 variants, `#[non_exhaustive]`) | unchanged; oxicode's wildcard fallback is forward-compatible |
| `BrowserConfig::headless()` / `builder()` | unchanged (+ new `automation()`, `viewport`, `ssrf_filter`, …) |
| `BrowseResult { url, title, status, markdown, html }` | unchanged |
| `tab::WaitCondition` (Visible/NetworkIdle/DomContentLoaded/Load) | unchanged |
| `Tab::{goto,content,evaluate,evaluate_await,wait_for,screenshot,wait_for_condition}` | unchanged |
| `oxibrowser::search::dispatch` / `SearchResult` / `SearchOutput` | unchanged |

MSRV (1.96) and edition (2024) already satisfied by the workspace.

### Dependency-footprint change

`native-browser` now pulls the entire Blitz stack (`blitz-dom/html/paint`,
`anyrender`, `anyrender_vello_cpu`, `parley`, `peniko`). **The default build is
lighter**, however: `oxibrowser` 0.20 made `default = []` (search-only, no
`boa_engine`), and `oxicode-agent` already uses `default-features = false`.

### Supply-chain cleanup (performed)

- `paste` (RUSTSEC-2024-0436) and `rustybuzz` (RUSTSEC-2026-0206, new via the
  Blitz text-shaping stack) are both **native-browser-only** (invisible to
  `cargo deny`'s default-features scan). Comments updated; `paste` path corrected
  (was `rav1e / boa_engine`; rav1e is gone in 0.20, now `boa_engine → boa_string`).
  `rustybuzz` added to `audit.toml` (sole record; cargo-deny doesn't see it).
- `rav1e` / `ravif` / `libfuzzer-sys` are **gone** — the dead `libfuzzer-sys`
  NCSA license exception in `deny.toml` was removed.

## 3. Vision routing activation (the real improvement)

### 3.1 The gap

The vision-routing infrastructure in `oxicode-ai/src/router/` was **fully built
but never wired into the routing hot path**:

- `signals.rs` — `VisionSignal { recent_image_count, has_image_in_latest_turn,
  image_producing_tools }` with `extract()` / `requires_vision()` / `normalized()`
  + 8 unit tests (incl. one asserting a `browse` screenshot is detected).
- `scoring.rs` — `compute_score(..., vision: Option<&VisionSignal>, ...)` weights
  vision.
- `mod.rs` — `RouterProvider::route_with_vision()` + `ensure_vision_model()`
  (swap to vision fallback / tier upgrade) implemented.

But `RouterProvider::stream()` called only the plain `route()` and **hardcoded**
`is_vision_triggered: false, vision_images: 0` in the recorded decision.
`route_with_vision` had zero callers — dead code. Result: when a `browse`
screenshot produced an image content block, the router never upgraded to a
vision-capable model.

### 3.2 The fix

`stream()` now extracts `VisionSignal` after the classifier settles the final
tier and, when images are present, runs `ensure_vision_model` to swap the
resolved `tier_config` to a vision-capable model (fallback swap → tier upgrade →
warn-and-keep). The decision records the real `is_vision_triggered` /
`vision_images`.

```rust
// router/mod.rs, inside Provider::stream, after the LLM-classifier block:
let vision = VisionSignal::extract(&context.messages, 10);
let is_vision_triggered = vision.requires_vision();
let vision_images = vision.recent_image_count;
let tier_config = if is_vision_triggered {
    self.ensure_vision_model(tier_config, tier, profile_name)
} else {
    tier_config
};
```

The signal/scoring/swap logic is unchanged — only the wiring was missing.

### 3.3 Tests

`router::vision_routing_tests` (new) proves `route_with_vision` performs a real
model swap via the global model registry (nextest process-per-test isolates the
global mutation):

- `route_with_vision_swaps_to_vision_fallback` — image context + text-only
  low-tier model → swaps to the vision fallback (`testvision/sees`).
- `route_with_vision_keeps_model_without_images` — text-only context → no swap.

### 3.4 End-to-end chain (now live)

```
browse tool → ContentBlock::Image (now a real CSS-rendered PNG via Blitz)
  → VisionSignal::extract detects image_producing_tools: ["browse"]
  → stream() runs ensure_vision_model → swaps to a vision-capable model
  → the model actually receives and can reason about the screenshot
```

## 4. Free improvements (no oxicode code change)

- **Screenshot quality** — `Tab::screenshot` now routes through Blitz/vello; the
  old text-bitmap renderer is replaced. `capture_screenshot_png` defaults to
  `full_page: true`, so the existing `screenshot(width)` calls capture the full
  page. (The `width` parameter is vestigial at the oxibrowser-core layer —
  `_viewport_width` — but kept on the oxicode `BrowserTab` trait for stability.)
- **SPA extraction fidelity** — `browse` / `browse_extract` on JS-rendered pages
  now see the post-script DOM (scripts execute on navigation).

## 5. Deferred (with rationale)

### 5.1 PDF export — blocked on an oxibrowser-core API

`Page.printToPDF` exists only as a **CDP domain method**
(`oxibrowser-cdp/src/domains/page.rs`), not as a `Tab`-level Rust API. oxicode
consumes `oxibrowser-core::Tab` (not raw CDP), so it cannot reach PDF generation
without an upstream `Tab::print_to_pdf` addition. **Action:** file an
oxibrowser-core feature request; expose a `pdf` browse action once it lands.

### 5.2 Screenshot API extension (full_page / viewport knobs) — already satisfied

The design considered exposing `CaptureOpts { full_page, viewport }` through the
oxicode `BrowserTab` trait. But oxibrowser-core's `capture_screenshot_png`
**already captures `full_page: true`** and respects the session viewport, so
oxicode gets full-page CSS screenshots for free. Adding oxicode-side knobs would
duplicate control oxibrowser-core doesn't expose on `Tab` (the `width` arg is
already ignored). Not worth the abstraction until `Tab` offers viewport/full_page
parameters.

### 5.3 browse_session multi-tab — high-risk, not runtime-verifiable here

`browse_session_tool.rs` is single-tab (`Option<TabGuard>`). 0.20's real
`Target.createTarget` enables true multi-tab, but converting the ~1300-line tool
to a `HashMap<TabId, TabGuard>` model is a substantial refactor that cannot be
runtime-verified without a live headless browser + network. Declared as
**designed-not-implemented** to avoid shipping an unverified behavioral change.
**Action:** dedicated PR with an acceptance harness (the `acceptance/` suite in
the oxibrowser repo is the model).

### 5.4 Pre-existing: classifier / tier_config tier mismatch (noted, not fixed)

In `stream()`, `tier_config` is resolved from the pre-classifier tier (step 2),
but the LLM classifier (step 3) may change `tier`. The recorded decision reflects
the classifier tier while the streamed model still comes from the pre-classifier
`tier_config`. This predates this work and is orthogonal to vision; left
untouched to keep the change focused. The vision override happens to mitigate it
in the image case (ensure_vision_model can upgrade the tier).

## 6. Verification

- `cargo build -p oxicode-agent --features native-browser` — clean (Blitz stack
  compiles).
- `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` (the CI
  `clippy-native-browser` gate) — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo nextest run --workspace` — 3285 passed, 4 skipped, 0 failed (pre-vision);
  +2 vision routing tests after.
- `cargo deny check` — `advisories ok, bans ok, licenses ok, sources ok`.
- `cargo audit` — exit 0 (rustybuzz unmaintained is informational, ignored).

## 7. Files changed

| File | Change |
|---|---|
| `oxicode-agent/Cargo.toml` | `oxibrowser` / `oxibrowser-core` 0.16 → 0.20 |
| `oxicode-sdk/Cargo.toml` | `oxibrowser-core` 0.17 → 0.20 (skew resolved) |
| `Cargo.lock` | re-resolved; +`oxibrowser-render`/Blitz stack, −rav1e/ravif/libfuzzer-sys |
| `oxicode-ai/src/router/mod.rs` | vision override wired into `stream()`; decision fields populated; +`vision_routing_tests` |
| `deny.toml` | `paste` comment corrected; +`rustybuzz`; −dead `libfuzzer-sys` license exception |
| `.cargo/audit.toml` | `paste` comment corrected; +`rustybuzz` |
