# Performance Fix: Regex Caching & SSE Buffer Optimization

## Fix 1: Regex Caching Across the Project

### Changes Made

#### 1. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/ui/changelog.rs`
- Added `LazyLock` import and `VERSION_REGEX` static
- Replaced `Regex::new()` call inside `parse_changelog_content()` function

```rust
static VERSION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"##\s+\[?(\d+)\.(\d+)\.(\d+)\]?").unwrap()
});
```

#### 2. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/prompt/templates.rs`
- Added `LazyLock` import and two cached regex statics
- Replaced `Regex::new()` calls inside `substitute_args()` function

```rust
static POSITIONAL_ARG_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\$(\d+)").unwrap()
});

static SLICE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\$\{@:(\d+)(?::(\d+))?\}").unwrap()
});
```

#### 3. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/storage/packages.rs`
- Added `LazyLock` import and `NPM_SPEC_RE` static
- Replaced `Regex::new()` call inside `parse_npm_spec()` function

```rust
static NPM_SPEC_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(@?[^@]+(?:/[^@]+)?)(?:@(.+))?$").expect("valid static regex")
});
```

#### 4. `/Volumes/MERCURY/PROJECTS/oxi/oxi-store/src/model_resolver.rs`
- Added `LazyLock` import and two cached regex statics
- Replaced `Regex::new()` calls inside `is_alias()` and `get_thinking_level_map()` functions

```rust
static DATE_PATTERN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"-\d{8}$").expect("date pattern regex should compile")
});

static DATE_PATTERN_STRIP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"-\d{8}").expect("date pattern strip regex should compile")
});
```

### Already Cached (Pre-existing)

The following files already had proper regex caching with `LazyLock`:

- `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/infra/output_guard.rs` - 16 patterns cached
- `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/agent_loop/retry.rs` - Uses `OnceLock`

### Additional Files Found (Not in original scope)

The grep found additional files with `Regex::new()` calls that were NOT modified:
- `oxi-agent/src/mcp/mod.rs` - Lines 328, 339 (dynamic regex from user input, cannot cache)
- `oxi-cli/src/infra/output_guard.rs` line 223 - Pattern list iteration

---

## Fix 2: SSE Buffer Optimization in proxy.rs

### Changes Made

#### `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/proxy.rs`

Replaced the drain-based approach (which allocates a new Vec for each line) with an index-based approach (no allocation per line):

**Before:**
```rust
while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
    let line = buffer.drain(..=pos).collect::<Vec<_>>();
    let line_str = String::from_utf8_lossy(&line);
    // process...
}
```

**After:**
```rust
let mut start = 0;
while let Some(pos) = buffer[start..].iter().position(|&b| b == b'\n') {
    let end = start + pos;
    let line = &buffer[start..end];  // no allocation
    let line_str = String::from_utf8_lossy(line);
    // process...
    start = end + 1;
}
if start > 0 {
    buffer.drain(..start);  // single drain at the end
}
```

This optimization:
1. Eliminates per-line Vec allocation
2. Uses slice reference instead of `collect()`
3. Only drains once at the end, after processing all complete lines

---

## Verification

All modified files compile without errors related to the changes:

```
(no errors in changelog.rs, templates.rs, packages.rs, model_resolver.rs)
```

**Note:** There are pre-existing errors in `oxi-agent/src/tools/bash.rs` that are unrelated to these performance fixes.