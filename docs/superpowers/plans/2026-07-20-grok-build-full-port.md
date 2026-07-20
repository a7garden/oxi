# grok-build 전체 스택 이식 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** grok-build(`xai-org/grok-build`)의 TUI + agent 런타임 전체를 oxi 워크스페이스로 verbatim vendoring하고, oxi-ai를 inference adapter로 연결한다.

**Architecture:** ~55개 grok-build crate를 `oxi-vendor-*`로 복사, Cargo.toml 참조 변환, `oxi-grok-bridge`로 oxi-ai 연결, 기존 oxi crate(agent, SDK, TUI, pager 등) 삭제.

**Tech Stack:** Rust 2024, ratatui 0.30, crossterm 0.29, grok-build commit `ba76b0a6`

## Global Constraints

- grok-build source: `/tmp/ref-porter/xai-org-grok-build` (commit `ba76b0a683fa52e4e60685017b85905451be17bc`)
- oxi workspace: `/Volumes/MERCURY/PROJECTS/oxi`
- Vendored crate naming: `xai-grok-foo` → `oxi-vendor-grok-foo`, `xai-*` → `oxi-vendor-*`, third-party 그대로 prefix
- ratatui 0.29→0.30: oxi-vendor-ratatui-inline은 이미 `B: Backend<Error = io::Error>` 패치 완료
- 모든 grok-build vendored crate는 `#![allow(deprecated, dead_code, unused_imports, unused_variables, unused_mut, clippy::all, clippy::pedantic, rustdoc::broken_intra_doc_links)]` 프리앰블 적용
- `oxi-ai`만 oxi 코드베이스에서 유지

---

### Task 1: Vendoring 스크립트 — crate 복사 + Cargo.toml 변환

**Files:**
- Create: `scripts/vendor-grok-build.sh`
- Create: 모든 `oxi-vendor-*` crates (~55개)

**Interfaces:**
- Consumes: grok-build 소스 at `/tmp/ref-porter/xai-org-grok-build`
- Produces: 모든 crate가 `oxi-vendor-*`로 복사되고 Cargo.toml이 변환된 상태

- [ ] **Step 1: 전체 crate 목록 추출**

```bash
cd /tmp/ref-porter/xai-org-grok-build
cargo tree -p xai-grok-pager --prefix none --depth 0 2>&1 | \
  grep -oP '^\S+' | sort -u > /tmp/pager-deps.txt
wc -l /tmp/pager-deps.txt
```

Expected: ~200줄 (external 포함). Internal crate만 필터링:

```bash
grep -E '^(xai-|dagre_rust|graphlib_rust|mermaid-to-svg|ordered_hashmap|prod-mc)' /tmp/pager-deps.txt | sort -u > /tmp/pager-internal.txt
wc -l /tmp/pager-internal.txt
```

- [ ] **Step 2: 복사 스크립트 생성**

`scripts/vendor-grok-build.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

GROK="/tmp/ref-porter/xai-org-grok-build"
OXI="/Volumes/MERCURY/PROJECTS/oxi"
SRC_REV=$(cat "$GROK/SOURCE_REV" 2>/dev/null || echo "ba76b0a6")

# Path mappings: crate name → grok source dir
declare -A CRATE_PATHS
while IFS= read -r crate; do
    # Find the crate in grok workspace
    path=$(find "$GROK/crates" "$GROK/third_party" "$GROK/prod" -maxdepth 4 -name Cargo.toml 2>/dev/null | \
           while read f; do
               if grep -q "name = \"$crate\"" "$f" 2>/dev/null; then
                   dirname "$f"
                   break
               fi
           done | head -1)
    if [ -n "$path" ]; then
        CRATE_PATHS["$crate"]="$path"
    fi
done < /tmp/pager-internal.txt

# Copy each crate
for crate in "${!CRATE_PATHS[@]}"; do
    src="${CRATE_PATHS[$crate]}"
    
    # Naming: xai-grok-foo → oxi-vendor-grok-foo
    vendor_name="oxi-vendor-${crate#xai-}"
    dest="$OXI/$vendor_name"
    
    if [ -d "$dest" ]; then
        echo "SKIP $vendor_name (exists)"
        continue
    fi
    
    echo "COPY $src → $dest"
    cp -r "$src" "$dest"
    
    # Write SOURCE_REV
    echo "$SRC_REV" > "$dest/SOURCE_REV"
done

echo "Done. Vendored ${#CRATE_PATHS[@]} crates."
```

```bash
chmod +x scripts/vendor-grok-build.sh
bash scripts/vendor-grok-build.sh
```

Expected: 각 crate가 `oxi-vendor-*`로 복사됨. 이미 존재하는 crate는 skip.

- [ ] **Step 3: Cargo.toml 변환 스크립트 생성**

`scripts/fix-vendored-toml.py`:

```python
#!/usr/bin/env python3
"""Fix vendored Cargo.toml files: rename crates, fix path refs, inline workspace deps."""
import re, os, sys
from pathlib import Path

GROK = Path("/tmp/ref-porter/xai-org-grok-build")
OXI = Path("/Volumes/MERCURY/PROJECTS/oxi")

# Parse grok workspace deps for version inlining
grok_ws = (GROK / "Cargo.toml").read_text()
grok_deps = {}
for m in re.finditer(r'^(\S+)\s*=\s*(\{.+\}|\".+\")', grok_ws, re.MULTILINE):
    grok_deps[m.group(1)] = m.group(2)

# Build name map: all grok crate names → oxi-vendor names
name_map = {}
for d in os.listdir(OXI):
    if d.startswith('oxi-vendor-') and os.path.isdir(OXI / d):
        suffix = d.replace('oxi-vendor-', '')
        name_map[suffix] = d
        name_map['xai-' + suffix] = d

def fix_toml(path: Path):
    content = path.read_text()
    original = content
    
    # Fix package name
    for grok_name, oxi_name in name_map.items():
        if f'name = "{grok_name}"' in content:
            content = content.replace(f'name = "{grok_name}"', f'name = "{oxi_name}"')
            break
    
    # Fix edition.workspace
    content = content.replace('edition.workspace = true', 'edition = "2024"')
    
    # Fix internal path refs: xai-grok-foo = { workspace = true } → oxi-vendor-grok-foo = { path = "../oxi-vendor-grok-foo" }
    for grok_name, oxi_name in sorted(name_map.items(), key=lambda x: -len(x[0])):  # longest first
        # workspace = true variant
        pattern = f'{grok_name} = {{ workspace = true'
        replacement = f'{oxi_name} = {{ path = "../{oxi_name}"'
        content = content.replace(pattern, replacement)
        
        # workspace = true with features
        content = re.sub(
            rf'{re.escape(grok_name)}\s*=\s*\{{\s*workspace\s*=\s*true\s*,\s*features\s*=',
            f'{oxi_name} = {{ path = "../{oxi_name}", features =',
            content
        )
    
    # Fix remaining workspace = true → inline version from grok workspace
    def replace_workspace(m):
        dep_name = m.group(1)
        features = m.group(2) or ''
        if dep_name in grok_deps:
            val = grok_deps[dep_name]
            if features:
                # Merge features into the version spec
                if '{' in val:
                    return f'{dep_name} = {val.rstrip("}")}, features ={features} }}'
                else:
                    return f'{dep_name} = {{ version = {val}, features ={features} }}'
            return f'{dep_name} = {val}'
        return m.group(0)  # leave as-is if unknown
    
    content = re.sub(
        r'^(\S+)\s*=\s*\{\s*workspace\s*=\s*true\s*(,\s*features\s*=\s*\[[^\]]+\])?\s*\}',
        replace_workspace,
        content,
        flags=re.MULTILINE
    )
    
    if content != original:
        path.write_text(content)
        print(f"  FIXED: {path.relative_to(OXI)}")
    else:
        print(f"  OK:    {path.relative_to(OXI)}")

# Process all vendored crates
for d in sorted(os.listdir(OXI)):
    if d.startswith('oxi-vendor-') and os.path.isdir(OXI / d):
        toml = OXI / d / 'Cargo.toml'
        if toml.exists():
            fix_toml(toml)

print("\nDone.")
```

```bash
python3 scripts/fix-vendored-toml.py
```

- [ ] **Step 4: Add lint-allow preamble to all vendored lib.rs files**

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
PREAMBLE='#![allow(deprecated, dead_code, unused_imports, unused_variables, unused_mut, clippy::all, clippy::pedantic, rustdoc::broken_intra_doc_links)]'

for crate in oxi-vendor-*/; do
    lib="${crate}src/lib.rs"
    if [ -f "$lib" ]; then
        if ! head -1 "$lib" | grep -q 'allow(deprecated'; then
            echo "Adding preamble to $lib"
            printf '%s\n' "$PREAMBLE" "$(cat "$lib")" > "$lib.tmp"
            mv "$lib.tmp" "$lib"
        fi
    fi
done
echo "Done."
```

- [ ] **Step 5: Commit vendored crates**

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
git add scripts/vendor-grok-build.sh scripts/fix-vendored-toml.py
git add oxi-vendor-*/
git commit -m "chore: vendor grok-build TUI + agent runtime (~55 crates)

Source: xai-org/grok-build @ ba76b0a683fa52e4e60685017b85905451be17bc
Vendored via scripts/vendor-grok-build.sh + scripts/fix-vendored-toml.py"
```

---

### Task 2: Workspace 통합 + 컴파일

**Files:**
- Modify: `Cargo.toml` (workspace members + dependencies)
- Modify: 각 `oxi-vendor-*/Cargo.toml` (compile fix loop)

**Interfaces:**
- Consumes: Task 1에서 vendoring된 모든 crate
- Produces: `cargo check --workspace` 성공

- [ ] **Step 1: Register all vendored crates as workspace members**

`Cargo.toml`의 `members`를 모든 vendored crate를 포함하도록 업데이트:

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
# List all vendored crates
VENDORED=$(ls -d oxi-vendor-*/ | sed 's|/$||' | sort | sed 's/.*/"&"/' | paste -sd ',')
echo "Vendored members: $VENDORED"

# Manually update Cargo.toml members line
```

- [ ] **Step 2: Add missing workspace dependencies**

`Cargo.toml`의 `[workspace.dependencies]`에 누락된 external deps 추가:

```toml
# grok-build vendoring — transitive deps
agent-client-protocol = { version = "0.10.4", features = ["unstable"] }
base64 = "0.22"
camino = "1.1.10"
chrono = "0.4"
clap = { version = "4", features = ["derive", "env"] }
clap_complete = "4"
core-foundation = "0.10"
criterion = "0.6"
derive_more = { version = "2", features = ["add", "add_assign", "debug", "deref", "deref_mut", "display", "from", "from_str", "into", "into_iterator", "try_into"] }
dirs = "5.0"
dunce = "1"
enum_delegate = "0.2"
flate2 = "1"
fontdb = "0.23"
image = { version = "0.25.9", default-features = false }
indexmap = { version = "2", features = ["serde"] }
insta = "1"
libc = "0.2"
nix = { version = "0.30", features = ["signal"] }
notify = "8"
obfstr = "0.4"
pretty_assertions = "1"
rand = "0.9"
resvg = { version = "0.47", default-features = false, features = ["text"] }
serde_json = "1"
serial_test = "3"
signal-hook = "0.3"
similar = "2.7"
strip-ansi-escapes = "0.2.1"
tar = "0.4"
tempfile = "3"
tiny-skia = "0.12"
tokio-util = { version = "0.7", features = ["compat"] }
toml = "0.9"
toml_edit = "0.22"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
urlencoding = "2"
uuid = { version = "1", features = ["serde", "v4", "v5"] }
wait-timeout = "0.2"
which = "8"
```

- [ ] **Step 3: First compile attempt**

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
cargo check -p oxi-vendor-grok-pager 2>&1 | tee /tmp/build-errors.txt
grep "^error" /tmp/build-errors.txt | wc -l
```

Expected: 오류 수 기록. ratatui-inline 21개 이외의 오류에 집중.

- [ ] **Step 4: Fix compile errors iteratively**

각 오류 클래스에 대해 수정 적용 후 재컴파일. 오류가 0이 될 때까지 반복.

- [ ] **Step 5: Commit workspace integration**

```bash
git add Cargo.toml
git add -u oxi-vendor-*/
git commit -m "chore: integrate vendored crates into workspace, fix compilation"
```

---

### Task 3: oxi-grok-bridge crate — Inference Adapter

**Files:**
- Create: `oxi-grok-bridge/Cargo.toml`
- Create: `oxi-grok-bridge/src/lib.rs`

**Interfaces:**
- Consumes: `oxi-ai::Provider`, `oxi-ai::ProviderRegistry`, `oxi-ai::Model`
- Produces: grok의 sampling backend trait 구현체

- [ ] **Step 1: Create crate skeleton**

`oxi-grok-bridge/Cargo.toml`:

```toml
[package]
name = "oxi-grok-bridge"
version = "0.1.0"
edition = "2024"
license = "MIT"

[dependencies]
oxi-ai = { path = "../oxi-ai" }
# grok shell's sampling types — vendored
oxi-vendor-grok-shell = { path = "../oxi-vendor-grok-shell" }
# For the sampling backend trait
oxi-vendor-grok-sampling-types = { path = "../oxi-vendor-grok-sampling-types" }
anyhow = "1"
tokio = { version = "1", features = ["sync", "rt"] }
tokio-stream = "0.1"
futures = "0.3"
```

`oxi-grok-bridge/src/lib.rs`:

```rust
//! Bridge: oxi-ai Provider → grok sampling backend.
//!
//! Implements grok's inference trait by delegating to oxi-ai's
//! multi-provider streaming pipeline.

use oxi_ai::{ProviderRegistry, Model, Context, ProviderEvent};
use std::pin::Pin;
use std::sync::Arc;

pub struct OxiSamplingBackend {
    registry: Arc<ProviderRegistry>,
}

impl OxiSamplingBackend {
    pub fn new(registry: Arc<ProviderRegistry>) -> Self {
        Self { registry }
    }
}

// TODO: Implement grok's SamplingBackend trait
// The exact trait name/signature depends on how xai-grok-shell
// defines its inference interface. This will be filled in once
// the vendored crate compiles and we can inspect the trait.
```

- [ ] **Step 2: Inspect grok's sampling trait**

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
# Find the sampling backend trait
grep -rn 'trait.*Sampling' oxi-vendor-grok-shell/src/ | head -5
# Or check sampling-types crate
grep -rn 'trait.*Backend\|trait.*Sampling\|trait.*Inference' oxi-vendor-grok-sampling-types/src/ | head -5
```

- [ ] **Step 3: Implement the trait**

grok의 trait signature에 맞춰 `OxiSamplingBackend` 구현. 기본 구조:

```rust
#[async_trait::async_trait]
impl grok_shell::sampling::SamplingBackend for OxiSamplingBackend {
    async fn stream_completion(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[Tool],
        options: &SamplingOptions,
    ) -> Result<Pin<Box<dyn Stream<Item = SamplingEvent> + Send>>, SamplingError> {
        let model = self.registry.resolve_model(model)?;
        let provider = self.registry.resolve(&model.provider)?;
        let context = convert_context(messages, tools);
        let stream = provider.stream(&model, &context, convert_options(options)).await?;
        Ok(Box::pin(transform_stream(stream)))
    }
}
```

정확한 trait은 compile 후 확인.

- [ ] **Step 4: Verify bridge compiles**

```bash
cargo check -p oxi-grok-bridge
```

- [ ] **Step 5: Commit**

```bash
git add oxi-grok-bridge/
git commit -m "feat: add oxi-grok-bridge — oxi-ai → grok inference adapter"
```

---

### Task 4: Composition Root — oxi-cli 재작성

**Files:**
- Modify: `oxi-cli/Cargo.toml`
- Modify: `oxi-cli/src/main.rs`
- Delete: `oxi-cli/src/tui/` (전체)
- Delete: `oxi-cli/src/bootstrap.rs`
- Modify: `oxi-cli/src/lib.rs`

**Interfaces:**
- Consumes: oxi-grok-bridge, oxi-vendor-grok-pager
- Produces: grok TUI를 실행하는 oxi binary

- [ ] **Step 1: Update oxi-cli dependencies**

`oxi-cli/Cargo.toml`에서 oxi-agent, oxi-sdk, oxi-tui, oxi-pager 의존성 제거. 추가:

```toml
oxi-ai = { path = "../oxi-ai" }
oxi-grok-bridge = { path = "../oxi-grok-bridge" }
oxi-vendor-grok-pager = { path = "../oxi-vendor-grok-pager" }
```

- [ ] **Step 2: Rewrite main.rs**

```rust
// oxi-cli/src/main.rs
use oxi_ai::ProviderRegistry;
use oxi_grok_bridge::OxiSamplingBackend;
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    // Initialize oxi-ai provider registry
    let mut registry = ProviderRegistry::new();
    registry.register_builtins();

    let backend = Arc::new(OxiSamplingBackend::new(Arc::new(registry)));

    // Launch grok pager with oxi backend
    oxi_vendor_grok_pager::app::run(backend)?;

    Ok(())
}
```

정확한 entry point는 `oxi-vendor-grok-pager`의 public API 확인 후 조정.

- [ ] **Step 3: Clean up oxi-cli/src/lib.rs**

`build_system_prompt`, `dispatch_run_mode` 등 미사용 export 제거.

- [ ] **Step 4: Compile oxi-cli**

```bash
cargo check -p oxi-cli
```

- [ ] **Step 5: Commit**

```bash
git add oxi-cli/
git commit -m "refactor: rewrite oxi-cli composition root for grok pager"
```

---

### Task 5: 기존 Crate 제거

**Files:**
- Delete: `oxi-agent/`, `oxi-sdk/`, `oxi-tui/`, `oxi-pager/`
- Delete: `oxi-hashline/`, `oxi-lsp/`, `oxi-mnemopi/`, `oxi-snapcompact/`, `oxi-sandbox/`
- Delete: `oxi-vendor-grok-shim/`
- Modify: `Cargo.toml` (members에서 제거)

- [ ] **Step 1: Remove crate directories**

```bash
cd /Volumes/MERCURY/PROJECTS/oxi
for crate in oxi-agent oxi-sdk oxi-tui oxi-pager oxi-hashline oxi-lsp oxi-mnemopi oxi-snapcompact oxi-sandbox oxi-vendor-grok-shim; do
    rm -rf "$crate"
    echo "Removed $crate"
done
```

- [ ] **Step 2: Update workspace members**

`Cargo.toml`에서 삭제된 crate 제거.

- [ ] **Step 3: Verify workspace compiles**

```bash
cargo check --workspace 2>&1 | grep "^error" | wc -l
```

Expected: 0.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor: remove oxi agent/SDK/TUI — replaced by grok-build"
```

---

### Task 6: Verification

**Files:**
- Modify: `.github/workflows/ci.yml` (필요시)

- [ ] **Step 1: Full workspace check**

```bash
cargo check --workspace
```

Expected: 0 errors.

- [ ] **Step 2: Build release binary**

```bash
cargo build --release -p oxi-cli
```

Expected: binary at `target/release/oxi`.

- [ ] **Step 3: Smoke test — launch TUI**

```bash
# Requires API key configured
./target/release/oxi
```

TUI가 grok-build 디자인으로 정상 실행되는지 확인.

- [ ] **Step 4: Update CI workflow**

`ci.yml`에서 삭제된 crate 참조 제거. vendored crate는 clippy/lint에서 제외 (기존 vendor exclude 패턴 유지).

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "chore: update CI for grok-build port, final verification"
```
