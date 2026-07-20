# NOTICE — Vendored Third-Party Code

This file is required by Apache-2.0 §4(d). The oxi project (MIT) vendors
source code from the Apache-2.0 `grok-build` project
(https://github.com/xai-org/grok-build, copyright © 2023-2026 SpaceXAI).
The vendored code retains its Apache-2.0 license; it is NOT relicensed to
MIT (Apache-2.0 is compatible with MIT, but the upstream license and
copyright notice must be preserved on the covered files).

## Source

- Upstream:   https://github.com/xai-org/grok-build
- Commit:     ba69d70c2f7d70a130a323b2becdf137af784c7f
- License:    Apache-2.0 (see `LICENSE-APACHE` and upstream `LICENSE`)
- Copyright:  © 2023-2026 SpaceXAI

## Vendored Crates (verbatim copy, path-only renames)

These crates are copied in full (source + Cargo.toml). Their internal
`use xai_*` paths are rewritten to oxi vendor crate names; otherwise the
source is unchanged.

| oxi crate                              | Upstream crate                | Approx LOC |
|----------------------------------------|-------------------------------|-----------:|
| `oxi-vendor-grok-markdown`             | `xai-grok-markdown`           | ~20,000    |
| `oxi-vendor-grok-markdown-core`        | `xai-grok-markdown-core`      |        n/a |
| `oxi-vendor-ratatui-textarea`          | `xai-ratatui-textarea`        |     ~12,700|
| `oxi-vendor-ratatui-inline`            | `xai-ratatui-inline`          |      ~3,000|

## Vendored Files (partial copy inside `oxi-pager/src/render/grok/`)

Partial selection from `xai-grok-pager-render/src/`. Files are copied
verbatim except for: (a) crate-path rewrites (`crate::` →
`crate::render::grok::` / `crate::render::theme::`), (b) `xai_*` external
paths replaced with oxi vendor crate paths, (c) stubs for modules not
relevant to oxi (image_overlay, gboom, clipboard, etc.). Any substantive
behavioral change to a copied file is marked with a `// OXI-CHANGE: ...`
comment per Apache-2.0 §4(b).

- `render/{color,draw,highlight,line_utils,osc8,renderable,safe_buf,scrollbar,wrapping}.rs`
- `render/theme/{mod,cache,color_support,grokday,groknight,oscura,rosepine,tokyonight,terminal_default,system_appearance,osc11,md_style}.rs`
- `glyphs.rs` (subset — legacy-console glyph fallbacks)
- `syntax.rs`

## Upstream License

```
                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   Copyright 2023-2026 SpaceXAI

   Licensed under the Apache License, Version 2.0 (the "License");
   ...
```

Full text: https://www.apache.org/licenses/LICENSE-2.0
