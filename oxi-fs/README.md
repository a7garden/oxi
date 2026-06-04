# oxi-fs

> File-based port implementations for `oxi-sdk`.

`oxi-fs` is the **adapter** layer that lets `oxi-sdk` persist state, read
configuration, manage credentials, and load skills from the conventional
home-directory layout (`~/.oxi/`).

It implements the port traits defined in
[`oxi_sdk::ports`](https://docs.rs/oxi-sdk) — for any other storage
backend (S3, SQLite, in-memory, keychain, …), implement the same
traits directly. `oxi-fs` is **one option among many**, not a
prerequisite for using the SDK.

## Layout

```
~/.oxi/
├── auth.json         — API keys + OAuth tokens (FileAuthProvider)
├── settings.toml     — layered configuration (FileConfigStore)
├── sessions/         — append-only JSON state (FileStateStore)
│   ├── <uuid>.json
│   └── …
├── skills/           — discovered SKILL.md files (FileSkillLoader)
│   ├── <skill-name>/SKILL.md
│   └── …
└── cache/            — reserved for ephemeral state
```

Resolution: `$OXI_HOME` → `$HOME/.oxi` (or platform equivalent).

## Crates

| type | file | description |
|---|---|---|
| `FileStateStore` | `src/session.rs` | Append-only JSON files, atomic write (temp + rename), per-id locking |
| `FileAuthProvider` | `src/auth.rs` | `auth.json` with API keys + OAuth; 7 env var fallbacks |
| `FileConfigStore` | `src/config.rs` | `settings.toml` with dotted-key flattening (`a.b.c`) |
| `FileSkillLoader` | `src/skill.rs` | Discovers `<root>/<name>/SKILL.md`; parses frontmatter |

## Usage

### oxi-cli composition root

```rust,no_run
use std::sync::Arc;
use oxi_sdk::OxiBuilder;
use oxi_fs::{FileStateStore, FileAuthProvider, FileConfigStore, FileSkillLoader};

# async fn run() -> anyhow::Result<()> {
let home = oxi_fs::home_dir()?;

let oxi = OxiBuilder::new()
    .with_builtins()
    .with_state(Arc::new(FileStateStore::new(home.join("sessions"))))
    .with_auth(Arc::new(FileAuthProvider::new(home.join("auth.json"))))
    .with_config(Arc::new(FileConfigStore::new(home.join("settings.toml"))))
    .with_skills(Arc::new(FileSkillLoader::single(home.join("skills"))))
    .build();
# let _ = oxi;
# Ok(()) }
```

### Per-port usage

Each adapter is independent and can be used standalone:

```rust,no_run
use oxi_fs::FileStateStore;
use serde_json::json;

# async fn run() -> anyhow::Result<()> {
let store = FileStateStore::new("/tmp/my-sessions");
let id = store.append(json!({"role": "user", "content": "hello"})).await?;
let entry = store.load(&id).await?;
println!("{entry:?}");
# Ok(()) }
```

## Auth provider: API-key resolution order

`FileAuthProvider::get_api_key(provider)` checks, in order:

1. `auth.json`'s `providers[provider].api_key`
2. `OXI_API_KEY_<UPPER>` environment variable (e.g. `OXI_API_KEY_ANTHROPIC`)
3. Provider-standard environment variable (best-effort, 7 providers):
   - `anthropic` → `ANTHROPIC_API_KEY`
   - `openai`    → `OPENAI_API_KEY`
   - `google`    → `GOOGLE_API_KEY`
   - `gemini`    → `GOOGLE_API_KEY`
   - `deepseek`  → `DEEPSEEK_API_KEY`

OAuth tokens are stored under `providers[provider].oauth` and only
read/written via `set_oauth` / `get_oauth` (no env-var fallback —
OAuth is interactive by nature).

## Config store: dotted keys

`FileConfigStore` flattens TOML tables into dotted-key paths:

```toml
[model]
provider = "anthropic"
name = "claude-sonnet-4-20250514"

[ui]
theme = "dark"
```

```rust
store.get("model.provider") // → Some(json!("anthropic"))
store.get("ui.theme")       // → Some(json!("dark"))
```

Nested objects and arrays round-trip via `serde_json::Value`.

## Skill loader: frontmatter

`FileSkillLoader` scans for `<root>/<name>/SKILL.md` and parses the
optional YAML frontmatter:

```markdown
---
description: write a commit message
version: "1.0"
---

# Commit Skill

When the user asks to commit…
```

```rust
let skills = loader.list().await?;   // → Vec<SkillMeta>
let skill  = loader.load("commit").await?;  // → Option<Skill>
```

Frontmatter is optional — `SKILL.md` without `---` delimiters is loaded
with empty `description` and the body is the entire file.

## When NOT to use oxi-fs

| situation | alternative |
|---|---|
| Multi-tenant cloud | implement `oxi_sdk::ports::StateStore` for your DB |
| Serverless / stateless | use `oxi_sdk::NoopStateStore` (default) |
| Encrypted at rest | wrap `FileStateStore` in an encrypting decorator |
| Concurrent multi-user | implement custom `StateStore` with row-level locking |
| Already have a settings system | implement `oxi_sdk::ports::ConfigStore` for it |

## Testing

Each port impl has unit tests under `#[cfg(test)] mod tests`:

```bash
cargo test -p oxi-fs
```

14 unit tests + 1 doctest covering:

- `FileStateStore`: round-trip, list, delete, missing-id
- `FileAuthProvider`: set/get, delete, oauth round-trip, env-var fallback
- `FileConfigStore`: nested round-trip, missing-key
- `FileSkillLoader`: discovery, body load, missing-skill
- `home_dir`: resolution paths

## License

MIT — same as oxi.
