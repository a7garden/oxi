# oxicode → oxicode Project Rename — Design

**Date:** 2026-08-03
**Status:** Approved (autonomous execution per user delegation)

## Goal

Rename every "oxicode" reference in the project to "oxicode" — crate names, Rust identifiers, env vars, config paths, binary name, CI workflows, and documentation.

## Rename Mapping

| Context | Old | New |
|---|---|---|
| Crate names | `oxicode-ai`, `oxicode-agent`, `oxicode-cli`, `oxicode-sdk`, `oxicode-hashline`, `oxicode-lsp`, `oxicode-mnemopi`, `oxicode-snapcompact`, `oxicode-vtui`, `oxicode-vtui-compat`, `oxicode-catalog`, `oxicode-api-stability`, `oxicode-tui` | `oxicode-*` |
| Rust crate identifiers | `oxicode_ai`, `oxicode_agent`, ... | `oxicode_ai`, `oxicode_agent`, ... |
| Binary name | `oxicode` | `oxicode` |
| Secondary binary | `oxicode-mnemopi-mcp` | `oxicode-mnemopi-mcp` |
| Env vars | `OXICODE_*` | `OXICODE_*` |
| Config dir | `.oxicode/`, `~/.oxicode/` | `.oxicode/`, `~/.oxicode/` |
| Type names | `Oxicode`, `OxicodeBuilder` | `Oxicode`, `OxicodeBuilder` |
| Function/method names | `from_oxicode`, `build_oxicode_engine`, `.oxicode()` | `from_oxicode`, `build_oxicode_engine`, `.oxicode()` |
| Variable/field names | `let oxicode =`, `self.oxicode` | `let oxicode =`, `self.oxicode` |
| Product name in prose | "oxicode", "Oxicode" | "oxicode", "Oxicode" |
| Directory names | `oxicode-*/` | `oxicode-*/` |

## Exclusions (Must NOT Rename)

| Pattern | Reason |
|---|---|
| `oxibrowser`, `oxibrowser-core` | External crate dependency |
| `oxios` | Sister project (separate repo) |
| `oxipage` | Sister project (separate tool) |
| `oxiline`, `oxinot` | Sister projects |
| `project-oxi` | GitHub org name (user only renamed the repo, not the org) |
| `oxipage.toml` | Config for oxipage tool (unrelated) |
| `earendil-works/pi` | Upstream project reference |

## Strategy

1. **Text replacements via Perl regex** — multi-pass, ordered from most specific to most general, with negative lookahead/lookbehind to exclude false positives.
2. **Directory renames via `git mv`** — after content is updated.
3. **File renames** — `oxicode-mnemopi-mcp.rs` → `oxicode-mnemopi-mcp.rs`, HTML reports.
4. **Build & test** — verify the workspace compiles and tests pass.
5. **Incremental fix** — resolve any missed references found by the compiler.
6. **Commit** — single comprehensive commit.
7. **crates.io publishing** — if `CARGO_TOKEN` is available.

## Regex Strategy

```
Pass 1: OXICODE_          → OXICODE_         (env vars)
Pass 2: oxicode-          → oxicode-         (crate names, paths)
Pass 3: oxicode_          → oxicode_         (Rust identifiers)
Pass 4: _oxicode\b        → _oxicode         (from_oxicode, build_oxicode)
Pass 5: OxicodeBuilder    → OxicodeBuilder   (specific type)
Pass 6: \bOxi\b       → Oxicode          (standalone type)
Pass 7: (?<![\w-])oxicode(?![\w-]) → oxicode (standalone word: variables, commands, config paths, strings)
```

Pass 7's negative lookbehind/lookahead prevents matching:
- `project-oxi` (preceded by `-`)
- `oxibrowser`, `oxios`, `oxipage` (followed by word char)
- `oxicode_ai`, `oxicode-ai` (already handled by earlier passes; followed by `_`/`-`)
