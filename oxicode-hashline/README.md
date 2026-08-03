# oxicode-hashline

Line-anchored patch format for AI-assisted code editing.

`oxicode-hashline` is a pure-function library that parses and applies a
line-anchored patch format designed for LLM-driven code edits: each edit names
the exact original lines it touches, so an agent can express surgical edits
without re-emitting whole files. No filesystem, no agent runtime, no schema
library — the host injects a [`HashlineFs`] implementation.

Ported from omp's `packages/hashline/` (TypeScript): same algorithms and test
contracts, Rust idioms.

## Highlights

- **Parse** a patch into typed sections and edits (`parser`, `grammar`).
- **Apply** edits with line-anchored matching, BOM/line-ending normalization
  (`apply`, `normalize`).
- **Recover** gracefully from shifted anchors via snapshot replay (`recovery`,
  `snapshots`).
- **Preview** the result as a unified diff (`diff_preview`).
- **Validate** edits against the live file and surface precise mismatches
  (`mismatch`, `messages`).

## Feature flags

| Feature          | Default | Description                                            |
|------------------|:-------:|--------------------------------------------------------|
| `block-ops`      |    —    | tree-sitter-based block ops (`SWAP.BLK`, …) — future.  |
| `three-way-merge |    —    | Phase-2 three-way merge backed by `similar`.           |

## Usage

```rust
use oxicode_hashline::{split_patch_input, apply_edits, patcher::HashlineFs};

// Host implements HashlineFs (read/write/snapshot the target files).
let fs: Box<dyn HashlineFs> = /* ... */;
let (patch, _rest) = split_patch_input(&patch_text)?;
apply_edits(&patch, fs.as_ref())?;
```

## License

MIT — see [LICENSE.md](LICENSE.md).
