# GitHub Sync — Design Document

> **Status:** Designed, **not implemented**. Phase 6 of the local issue system.
> When implementation starts, this document is the source of truth for
> shape and trade-offs. Do not re-derive from scratch.

## Context

The local issue system is implemented in Phases 1–5:

- **Phase 1**: `FileIssueStore` — markdown + YAML frontmatter in `.oxi/issues/`.
- **Phase 1.5**: `oxi issue …` CLI subcommand.
- **Phase 2**: `IssueTool` agent tool.
- **Phase 3**: TUI overlay panel + `/issue` slash command.
- **Phase 4**: Status-bar indicator.
- **Phase 5**: `@issue-N` inline expansion + session linking.

Every issue already carries a `github: { repo, number, url }` field
(nullable) in its frontmatter, designed for round-trip tracking. This
document defines what that field actually does.

## Goal

Allow issues to flow between local markdown and GitHub Issues so that:

- A user working locally can publish their tracker to a team using GitHub.
- A user whose team uses GitHub Issues can mirror a subset locally to
  work on them through `oxi`.

## Non-goals (v1)

- **Real-time push via webhooks.** Sync is on-demand only.
- **Multi-vendor** (GitLab, Linear, Jira). The shape leaves room for
  another backend later; only GitHub is implemented.
- **Authoring comment threads back to GitHub.** Pull is append-only on
  the local side; we don't push comments.
- **Cross-repo sync.** One local repo maps to one GitHub repo.
- **GitHub Enterprise Server.** `api.github.com` only.

## Recommended phasing

| Phase | What | Value | Cost |
|-------|------|-------|------|
| **6.1 Push** | `issue push <id>` publishes local → GH; stores `number`. | Lets a local-first user share work with a team. | ~400 lines. |
| **6.2 Pull** | `issue pull <number>` fetches GH → local. | Lets a GitHub-first user work locally. | ~500 lines. |
| **6.3 Bidirectional** | `issue sync` with per-field conflict detection. | Two-way mirror, robust to offline work. | ~1500 lines. |
| **6.4 Optional** | Webhook-driven push. | Real-time multi-user. | Significant. |

We start with 6.1. 6.3 is gated on a real-world reason to need it
(teams that actively edit both sides).

## Data model additions

The `github` field is already in `IssueMeta`:

```yaml
github:
  repo: owner/repo
  number: 42
  url: https://github.com/owner/repo/issues/42
```

For sync tracking (Phases 6.2+), extend it in place:

```yaml
github:
  repo: owner/repo
  number: 42
  url: https://github.com/owner/repo/issues/42
  # New in 6.2+:
  pushed_at: 2026-06-17T...           # last successful push
  pulled_at: 2026-06-17T...           # last successful pull
  last_remote_updated_at: 2026-06-17T...  # GitHub's `updated_at` at last sync
```

Per-issue sync state is **computed, not stored**, derived from the above:

- `LocalOnly` — `github == null`
- `Pushed` — `github.number` set, no `pulled_at`
- `Pulled` — `github.number` set, `pulled_at` set
- `InSync` — `pushed_at` and `pulled_at` set, neither side changed since
- `Conflict` — both sides changed since last sync
- `Deleted` — local: `github.number` set but file gone; remote: 404

## Field mapping (1:1, transform, or local-only)

| Local | GitHub | Notes |
|-------|--------|-------|
| `id` | — | Local-only. Not synced. |
| `title` | `title` | 1:1. |
| `status` | `state` | open/closed ↔ open/closed. 1:1. |
| `body` | `body` | Markdown. GitHub uses GFM; local may use a permissive superset. Best-effort 3-way merge on conflict (Phase 6.3). |
| `labels` | `labels[]` | Match by `name`. On push, missing labels are created (requires `repo` scope). |
| `priority` | label `priority/<low\|medium\|high\|critical>` | Synthetic label family. Configurable prefix; default `priority/`. |
| `assignee` | `assignee.login` | Push only if a local-to-GitHub user map exists; otherwise drop. |
| `sessions[]` | — | Local-only. **Never synced.** |
| `assigned_to` | — | Local-only. **Never synced.** |
| `created_at` | `created_at` | Set on first create. Never re-synced after that. |
| `updated_at` | `updated_at` | Conflict-resolution signal. |
| `closed_at` | `closed_at` (when `state=closed`) | 1:1. |
| `github.*` | (the link) | Self-referential. |

## Operations

### CLI

```sh
oxi issue push <id>            # local → GitHub (creates or updates)
oxi issue pull <number>        # GitHub → local (creates or updates)
oxi issue sync [<id>]          # bidirectional; one or all
oxi issue where <id>           # show sync state of an issue
oxi issue status               # show sync state of all linked issues
```

All commands support `--dry-run` and `--repo owner/name` (override the
project default).

### Agent tool

Add a new `action` to the existing `issue` tool:

```json
{"action": "sync", "id": 12, "direction": "push"}
{"action": "sync_status", "id": 12}
{"action": "resolve_conflict", "id": 12, "field": "body", "choice": "local"}
```

The `sync` action is **opt-in**: the tool is only registered if the
project has a `github_repo` configured (e.g. via
`oxi issue init --repo owner/name`).

## Conflict resolution (Phase 6.3)

Per-field, with explicit timestamps. For each field, if both `local`
and `remote` updated it after the last sync timestamp → conflict.

| Field type | Default resolution | User override |
|------------|-------------------|---------------|
| `body` (markdown) | 3-way merge with `<<<<<<< local` markers | `choice: local \| remote \| merged` |
| `status`, `priority`, `labels` | Last-write-wins + warning notification | `choice: local \| remote` |
| `title` | Last-write-wins | `choice: local \| remote` |
| `assignee` | Local-wins (single-user env) | n/a |

The `sync` action returns a structured report (in the agent's tool
result) listing conflicts and chosen resolutions, so the user can
decide to override before the write is committed.

## Auth & rate limits

Reuse the existing `gh` CLI authentication (already in use by the
`github` tool — see `oxi-agent/src/tools/github.rs`). No new auth
surface.

- Authenticated rate limit: 5000 requests/hour.
- Sync operations are batched: 1 read + 1 write per issue in the
  common case.
- The status-bar indicator is **not** used for sync state in v1 to
  keep the footer uncluttered.

`gh` is sufficient for 6.1 and 6.2. For 6.3 (conflict detection
needs GitHub's exact `updated_at`), we may need to bypass `gh` and
hit the REST API directly. Decision deferred to 6.3.

## Phasing details

### Phase 6.1 — Push only

```
issue push <id>:
  load issue, validate
  if github.number is null:
    POST /repos/{owner}/{repo}/issues   → fill in github.{number,url}
    create priority/, … labels if missing
  else:
    PATCH /repos/{owner}/{repo}/issues/{number}
  update github.pushed_at, return summary
```

Edge cases:

- 401 → bail, suggest `gh auth login`.
- 403 (insufficient scope) → bail, name the missing scope.
- 422 (validation) → return GitHub's error verbatim.
- Network error → leave local untouched; surface the error to the
  user; they retry.

### Phase 6.2 — Pull

```
issue pull <number>:
  GET /repos/{owner}/{repo}/issues/{number}
  if local file with that github.number exists:
    CAS update using the same hash protocol (Phase 1) — external
    editor edits are caught the same way
  else:
    create new file with next id, store github.number
  update github.pulled_at, github.last_remote_updated_at
```

### Phase 6.3 — Bidirectional sync

```
issue sync:
  for each linked issue:
    local = read from disk
    remote = GET /repos/.../issues/{number}
    if remote.updated_at > last_remote AND local.updated_at > last_local:
      per-field conflict detection (table above)
      apply default resolution or user override
    elif remote changed:
      pull
    elif local changed:
      push
    elif neither: noop
  report counts: synced, conflicts, errors
```

The 3-way merge for `body` uses a simple line-based LCS. No `merge3`
crate; implement locally — ~100 lines. Or use `diffy` if it's already
in the dependency graph.

## Risks

- **`gh` availability** — already required by the `github` tool; no new
  dependency.
- **API rate limits** — batch + on-demand keeps us well under 5000/hr.
- **Network mid-sync** — atomic writes (Phase 1) protect local files.
  Remote writes either fully succeed (200) or fail and are retried by
  the user; no partial state.
- **Markdown differences** — local MD is a superset of GFM; GitHub
  silently drops unsupported syntax. Document, don't try to migrate.
- **Concurrent local + remote edits** — handled by conflict detection
  in 6.3.

## Decisions still to make (before 6.1)

1. **Where does `owner/repo` come from?**
   - `git remote get-url origin` parsed?
   - Per-project `oxi issue init --repo owner/name`?
   - Global setting?
   - *Default: parse `git remote get-url origin`; override with init flag.*

2. **What's the priority-label prefix?**
   - `priority/`, `prio/`, `p/`?
   - Configurable?
   - *Default: `priority/`. Configurable per project.*

3. **Auto-push on `start`/`close`?**
   - Always manual to start; revisit if users complain.
   - *Default: manual only.*

4. **What about issues that the user closed locally that were already
   closed on GitHub?** Noop with an informational note. No data loss.

5. **Should `update` on a `Linked` issue auto-push?**
   - Probably no — the agent's "I edited this issue" intent is distinct
     from "this should be on GitHub". Make `push` explicit.
   - *Default: explicit push only.*

## What's not in this design (and why)

- **Comments as a first-class concept** — out of v1 scope. The local
  issue body is monolithic. If users want comment threads, that's a
  separate design (`IssueComment` model + storage + UI).
- **PR linking** — GitHub issues can reference PRs; we don't track
  that. Add later if needed.
- **Reaction tracking** — GitHub reactions, not in scope.
- **Sub-issues** (GitHub Projects sub-issues) — not in v1.
- **Project boards** (GitHub Projects v2) — different API surface
  entirely; not in v1.
