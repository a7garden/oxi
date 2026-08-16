# Documentation Index

Top-level map of `/oxicode/docs/` (~175 markdown files). Subdirectories are
intentionally preserved as written; this index exists to make them findable.

> **Canonical design system.** The unified `DESIGN.md` lives at
> `project-oxi/.github/DESIGN.md` (single source of truth). The docs under
> this `docs/` tree are project-specific design notes that coexist with —
> but do NOT replace — that canonical source.

## Subdirectory map

| Subdir | Count | Purpose |
| --- | ---: | --- |
| `audits/` | 1 | One-off coverage / quality audits (e.g. `2026-06-30-sdk-coverage`). |
| `design/` | 12 | Long-form design docs on Rust-2024 modernization, oxios migration, multi-provider routing, TUI widget improvements, extension-system WASM, etc. |
| `designs/` | 54 | Date-stamped decision documents and feature designs. Includes the `omp-adoption/` and `omp-adoption-2/` program plans. |
| `production-audit/` | 8 | Six area briefs (`01-doc-debt` … `06-extension-safety`) plus `AREA-SELECTION.md` and an index README. |
| `proposals/` | 4 | Forward-looking feature proposals (MCP disk-path customization, fallback-event observability, SDK consumer requirements). |
| `ref-porter/` | 2 | External porter reference material (`xai-org-grok-build*`). |
| `rfcs/` | 11 | Numbered RFCs (RFC-001 TUI parity through RFC-008 graceful loop termination) plus reviews and the editor evaluation. |
| `sdk-redesign/` | 9 | Multi-part SDK redesign plan (`00-overview.md` through `06-integration.md`) with attached session prompts. |
| `superpowers/` | 48 | Working memory for SDD-style execution: handoffs, remaining-work snapshots, plans, specs, and active research notes. |

> **Subdirectory doc counts include both files at the subdir root and one
> level deep.** The numbers reflect total `.md` files inside each folder.

## Highlighted key docs

### Architecture & design system
- `oxicode-design.md` — top-level oxicode design narrative.
- `oxicode-architecture.md` — repo-wide architecture overview.
- `superpowers/specs/2026-08-17-oxi-foundation-contract.md` — Oxi Foundation v1 contract (oxicode host side).
- `superpowers/plans/2026-08-17-oxi-foundation-integration.md` — implementation plan for the Foundation host cutover.
- `extensions.md` — extension system design.

### SDK
- `sdk-redesign/00-overview.md` … `06-integration.md` — seven-part SDK plan.
- `oxicode-sdk-ownership.md` — operational guidance for SDK maintainers.
- `sdk-stabilization-roadmap.md` — stabilization timeline.
- `rfc-sdk-improvements.md` — proposed improvements (RFC-style).

### TUI
- `tui-architecture-design.md`, `tui-improvements-design.md` — TUI design.
- `design-fix-tui-overlays.md`, `design-pi-mono-alignment.md` —
  project-specific TUI design notes.
- `superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md` — render-pipeline spec.

### RFCs (number-prefixed documents)
- `rfcs/RFC-001-TUI-PARITY.md`, `RFC-002-AI-PROVIDER-COVERAGE.md` (+ `RFC-002-REVIEW`),
  `RFC-003-AGENT-TOOL-SUPERIORITY.md`, `RFC-004-EXTENSION-SKILLS.md`,
  `RFC-005-CI-CD-INFRA.md`, `RFC-006-SDK-MULTI-AGENT.md`,
  `RFC-007-BROWSE-PROGRESS-ENRICHMENT.md`, `RFC-008-GRACEFUL-LOOP-TERMINATION.md`.

### Production audit brief set
- `production-audit/01-doc-debt/BRIEF.md` … `06-extension-safety/BRIEF.md`.

### Process & release
- `release-process.md` — release pipeline.
- `oxicode-sdk-ownership.md` — SDK ownership / hand-off rules.

## Transient & version-sprawl handling

- `oxicode-sdk/DESIGN_IMPROVEMENTS_V2.md` is the **latest** of the trio. The
  earlier `DESIGN_IMPROVEMENTS.md` and `DESIGN_IMPROVEMENTS_REVIEW.md`
  have been moved to `oxicode-sdk/docs/archive/` (superseded by V2; V2
  itself supersedes both).
- Transient status files (`progress.md`, `.release-prep-status.md`) have
  been moved into `docs/archive/transient/`.

## Top-level docs (`*.md` directly under `/oxicode/docs/`)

```
LINE_COUNT_AUDIT.md
MODELS_DEV_SYNC.md
PORT_GUIDE.md
REFACTORING_DESIGN.md
REMAINING.md
ROUTING_ANALYSIS.md
ROUTING_DESIGN.md
THEME_GUIDE.md
custom-providers.md
deep-comparison-report.md
design-agent-delegation.md
design-fix-tui-overlays.md
design-github-sync.md
design-native-browser-resurrection.md
design-pi-mono-alignment.md
extensions.md
oxicode-architecture.md
oxicode-design.md
oxicode-sdk-ownership.md
release-process.md
rfc-browser-interactive-sessions.md
rfc-oxios-requirements.md
rfc-sdk-improvements.md
sdk-stabilization-roadmap.md
tui-architecture-design.md
tui-improvements-design.md
```

These are the working docs that don't belong to any subdirectory —
design notes (`design-*.md`), RFC drafts (`rfc-*.md`), analyses
(`*_ANALYSIS.md`, `*_AUDIT.md`), and architecture references.
