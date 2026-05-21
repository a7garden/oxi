# UTF-8 Safety Fixes

## Summary

Fixed byte-based string slicing that would panic on multibyte characters (UTF-8) by replacing with character-based truncation using `char_indices()`.

## Files Modified

### 1. oxi-cli/src/main.rs

**Function: `truncate()`**

Changed from byte-based slicing:
```rust
// BEFORE (UNSAFE - panics on multibyte chars)
format!("{}...", &s[..max_len.saturating_sub(3)])
```

To character-safe version using char_indices:
```rust
// AFTER (SAFE - respects UTF-8 character boundaries)
let boundary = s.char_indices()
    .take_while(|(i, _)| *i <= max_len.saturating_sub(3))
    .last()
    .map(|(i, c)| i + c.len_utf8())
    .unwrap_or(0);
format!("{}...", &s[..boundary])
```

### 2. oxi-cli/src/context/auto_compaction.rs

**Message truncation in `build_summarization_prompt()`:**

Changed from:
```rust
// BEFORE (UNSAFE)
format!("{}...", &msg.content[..500])
```

To:
```rust
// AFTER (SAFE)
let boundary = msg.content.char_indices()
    .take_while(|(i, _)| *i <= 500)
    .last()
    .map(|(i, c)| i + c.len_utf8())
    .unwrap_or(0);
format!("{}...", &msg.content[..boundary])
```

**Token estimation in `estimate_tokens()`:**

Changed from byte-based to character-based counting:
```rust
// BEFORE: (chars / 4) using len()
// AFTER: (chars / 3) using chars().count()
let chars = msg.content.chars().count();
(chars / 3).max(1)
```

### 3. oxi-ai/src/compaction.rs

Added helper function `safe_truncate()` at module level, used in multiple functions:

```rust
fn safe_truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars { return s.to_string(); }
    let boundary = s.char_indices()
        .take_while(|(i, _)| *i <= max_chars)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}...", &s[..boundary])
}
```

**Updated locations:**

1. `build_summarize_prompt()` - 500 char truncation for message previews
2. `compact_fallback()` - 200 char truncation for first/last message previews  
3. `summarize_branch()` - 300 char truncation for branch message previews

## Technical Details

The `char_indices()` approach iterates over character boundaries, finds the last index within the limit, then includes the full UTF-8 bytes for that character. This prevents:
- Panics from partial character slicing
- Visual corruption from split multibyte characters
- Incorrect truncation of non-ASCII text (emoji, CJK, accented characters)

## Verification

Run `cargo build` in each crate to verify changes compile correctly.
