# oxicode-cli Architecture

This document describes the internal architecture of the `oxicode-cli` crate.

## Session System

The session system manages conversation history with branching support.

### Session File Format

Sessions are stored as newline-delimited JSON (JSONL):

```
~/.oxicode/sessions/{session_id}.jsonl
```

Each line is a `SessionEntry`:

```rust
pub struct SessionEntry {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,     // Branch parent
    pub message: AgentMessage,
    pub timestamp: i64,
    pub metadata: SessionMetadata,
}
```

### Entry Types

```rust
pub enum AgentMessage {
    User {
        content: ContentValue,
    },
    Assistant {
        content: Vec<AssistantContentBlock>,
        provider: Option<String>,
        model_id: Option<String>,
        usage: Option<Usage>,
        stop_reason: Option<StopReason>,
    },
    ToolResult {
        tool_name: String,
        content: ContentValue,
    },
}
```

### Session Tree Structure

```
Session A
├── Entry 1 (User)
├── Entry 2 (Assistant)
├── Entry 3 (User)
└── Entry 4 (Assistant)
    │
    ├── Branch B
    │   ├── Entry 5 (User) ── parent: Entry 4
    │   └── Entry 6 (Assistant)
    │
    └── Branch C
        ├── Entry 7 (User) ── parent: Entry 4
        └── Entry 8 (Assistant)
```

### Branching

```rust
impl Session {
    /// Fork a new session from a specific entry
    pub fn fork(&mut self, entry_id: &Uuid, new_parent_id: &Uuid) -> Result<Uuid> {
        // Create new session with entries up to and including entry_id
        let entries: Vec<SessionEntry> = self.entries()
            .take_while(|e| e.id != *entry_id)
            .cloned()
            .collect();
        
        let new_id = Uuid::new_v4();
        let new_session = Session::new(new_id, entries, Some(*new_parent_id));
        Ok(new_id)
    }
}
```

### Session Migration

Sessions support version migration:

```rust
pub const SESSION_VERSION: u32 = 2;

impl Session {
    fn migrate(&mut self) -> Result<()> {
        match self.version {
            0 => self.migrate_v0_to_v1()?,
            1 => self.migrate_v1_to_v2()?,
            SESSION_VERSION => { /* current */ }
            _ => bail!("Unknown session version"),
        }
    }
}
```

## Extension System

### Extension Lifecycle

```
load()  ──►  on_load()  ──►  running  ──►  on_unload()  ──►  unload()
              │
              ▼
         register_tools()
```

### Extension Trait

```rust
#[async_trait]
pub trait Extension: Send + Sync {
    /// Extension metadata
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    
    /// Lifecycle hooks
    async fn on_load(&self, ctx: &ExtensionContext) -> Result<()>;
    async fn on_unload(&self) -> Result<()>;
    
    /// Register tools
    fn register_tools(&self, registry: &ToolRegistry);
    
    /// Permission requirements
    fn permissions(&self) -> Vec<Permission>;
}
```

### Permissions

```rust
pub enum Permission {
    FileRead,
    FileWrite,
    Network,
    ExecuteCommand,
    ReadEnvironment,
}
```

### Extension Context

```rust
pub struct ExtensionContext {
    pub settings: Arc<Settings>,
    pub session: Arc<Session>,
    pub tools: Arc<ToolRegistry>,
    pub emit: Arc<dyn Fn(ExtensionEvent) + Send + Sync>,
}
```

### Loading Extensions

```rust
impl ExtensionLoader {
    /// Load extension from path
    pub async fn load(&self, path: &Path) -> Result<Arc<dyn Extension>> {
        let lib = unsafe { Library::new(path)? };
        
        // Find and call extension factory
        let factory: libloading::Symbol<CreateExtension> = lib.get(b"create_extension")?;
        let ext = factory();
        
        // Initialize
        ext.on_load(&self.context).await?;
        
        Ok(ext)
    }
}
```

## Settings Layering

Settings are applied in layers (later overrides earlier):

```
┌─────────────────────────────────────────┐
│  5. CLI arguments (highest priority)   │
├─────────────────────────────────────────┤
│  4. Environment variables (OXICODE_*)       │
├─────────────────────────────────────────┤
│  3. Project config (.oxicode/settings.toml)│
├─────────────────────────────────────────┤
│  2. Global config (~/.oxicode/settings.toml) │
├─────────────────────────────────────────┤
│  1. Built-in defaults (lowest)         │
└─────────────────────────────────────────┘
```

### Load Order

```rust
impl Settings {
    pub fn load() -> Result<Self> {
        let mut settings = Settings::default();        // 1. Defaults
        
        if let Some(path) = Self::settings_path() {
            settings = Self::layer_file(&settings, &path)?;  // 2. Global
        }
        
        if let Some(path) = Self::find_project_settings(&cwd) {
            settings = Self::layer_file(&settings, &path)?;  // 3. Project
        }
        
        settings.apply_env();                          // 4. Environment
        
        // 5. CLI handled separately via merge_cli()
        settings
    }
}
```

### Settings Structure

```rust
pub struct Settings {
    pub version: u32,
    
    // LLM settings
    pub thinking_level: ThinkingLevel,
    pub default_model: Option<String>,
    pub default_provider: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    
    // Session
    pub session_dir: Option<PathBuf>,
    pub session_history_size: usize,
    
    // Resources
    pub extensions: Vec<String>,
    pub skills: Vec<String>,
    pub prompts: Vec<String>,
    pub themes: Vec<String>,
    
    // Behavior
    pub extensions_enabled: bool,
    pub auto_compaction: bool,
    pub tool_timeout_seconds: u64,
}
```

### Environment Variables

| Variable | Setting |
|----------|---------|
| `OXICODE_MODEL` | `default_model` |
| `OXICODE_PROVIDER` | `default_provider` |
| `OXICODE_THINKING` | `thinking_level` |
| `OXICODE_THEME` | `theme` |
| `OXICODE_MAX_TOKENS` | `max_tokens` |
| `OXICODE_TEMPERATURE` | `default_temperature` |
| `OXICODE_SESSION_DIR` | `session_dir` |
| `OXICODE_AUTO_COMPACTION` | `auto_compaction` |
| `OXICODE_TOOL_TIMEOUT` | `tool_timeout_seconds` |

## Oxi Foundation host

oxicode is an **Oxi Foundation v1 host**. It reads the versioned contract
under `~/.oxi/foundation/v1/` (gated by `$OXI_FOUNDATION_HOME`), resolves
provider profiles and package sources through it, and uses
[`oxibrain`](https://github.com/project-oxi/oxibrain) as its only durable
memory authority. The wired engine exposes:

```
services::build_oxicode_with_catalog(paths, catalog, embedding, hooks)
└── foundation::discover(home)?
    ├── compatibility compliant?
    ├── profiles::resolve(explicit?, env?, role?) → ResolvedProfile
    ├── credentials::resolve(&profile) → KeychainCredential
    └── packages::verify(lockfile) → TrustedPackages
└── brain::connect() → Option<BrainClient>  (degraded if absent)
```

### Resolution precedence

1. **Environment override** (`OXICODE_PROVIDER` / `OXICODE_MODEL`) — non-persistent automation override; never logs the value.
2. **Explicit profile** (`--profile` / `OXICODE_PROFILE`) — the selected profile id.
3. **Role-compatible profile** — first profile in `profiles.json` whose `roles` contains the requested role. Ambiguous matches fail visibly.
4. **Compatibility import** — one-time legacy import, gated by `OXICODE_FOUNDATION_MIGRATION=1`. Disabled by default.

### Capability mapping

Foundation packages declare abstract requirements. oxicode maps each to
its existing policy:

| Requirement | Gate |
|---|---|
| `workspace.read` | `AccessGate::allow_workspace_read` + workspace approval |
| `workspace.patch` | `AccessGate::allow_workspace_write` + run approval |
| `shell.execute` | `AccessGate::allow_shell` + `ToolPolicy::bash` |
| `browser.navigate` | `ToolPolicy::web_search` + `native-browser` feature |
| `brain.query` | scoped `BrainClient` already installed at composition root |
| `schedule.manage` | `CronScheduler` port per active scope |

A verified package is **not** automatically authorized. Every requirement
must pass oxicode's existing policy.

### Credentials

Plaintext `~/.oxicode/auth.json` is no longer the durable credential
authority. Profiles carry a `{ service, account }` Keychain locator;
`foundation::credentials::KeychainAuthProvider` resolves the locator
on demand and retypes unavailable/locked/not-found without ever
exposing the value through `Debug` or `Display`. A one-time legacy
importer moves a legacy secret into the Keychain after explicit
acknowledgement, then archives the source file outside the active
credential path.

### Memory

The `MemoryBackend` implemented in `oxicode-agent` is bound at the
composition root to a `BrainMemoryBackend` that wraps a typed
`oxibrain_client::BrainClient`. Agent memory tools return Brain IDs
and citations; consolidation is a Brain request, not a local summary
rebuild. Loss of daemon connectivity is a surfaced degraded state —
no local fallback.

### Compatibility matrix

| Host | Owns |
|---|---|
| **oxibrain** | durable memory, retrieval, projection, consolidation |
| **oxicode** | code execution, workspace policy, tool invocation, package compilation |
| **oxios** | orchestration, experience, persona composition (embeds `oxicode-sdk`) |

oxios MUST NOT spawn the oxicode CLI as a child process for normal
operation; when it needs the CLI surface, it ships its own binary.

See [`docs/superpowers/specs/2026-08-17-oxi-foundation-contract.md`](../docs/superpowers/specs/2026-08-17-oxi-foundation-contract.md)
for the full contract.

## AgentSession

The `AgentSession` ties together the agent runtime with session persistence:

```rust
pub struct AgentSession {
    pub agent: Arc<Agent>,
    pub session: Arc<Session>,
    pub settings: Arc<Settings>,
    pub tools: Arc<ToolRegistry>,
}
```

### Session Events

```rust
impl AgentSession {
    /// Run with session persistence
    pub async fn run(&mut self, prompt: String) -> Result<String> {
        // Add user message to session
        self.session.add_entry(AgentMessage::User { content: prompt.into() });
        
        // Run agent
        let (response, events) = self.agent.run(prompt).await?;
        
        // Persist assistant response
        self.session.add_entry(AgentMessage::Assistant { ... });
        
        Ok(response.content)
    }
}
```

## CLI Arguments

```rust
pub struct CliArgs {
    pub command: Option<Commands>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt: Vec<String>,
    pub interactive: bool,
    pub thinking: Option<String>,
    pub extensions: Vec<PathBuf>,
    pub mode: Option<String>,
    pub tools: Option<String>,
    pub append_system_prompt: Option<PathBuf>,
    pub print: bool,
    pub no_session: bool,
}
```

### Subcommands

```rust
pub enum Commands {
    Sessions,                                    // List sessions
    Tree { session_id: String },                 // Show session tree
    Fork { parent_id: String, entry_id: String }, // Branch session
    Delete { session_id: String },               // Delete session
    Pkg { action: PkgCommands },                 // Package management
    Config { action: ConfigCommands },           // Config management
}
```

## Package Management

### Package Sources

Packages can be loaded from:
- Local paths: `/path/to/package`
- npm packages: `npm:@scope/package-name`

### Package Installation

```rust
pub enum PkgCommands {
    Install { source: String },
    List,
    Uninstall { name: String },
    Update { name: Option<String> },
}
```

### Package Discovery

```rust
impl PackageManager {
    pub async fn install(&self, source: &str) -> Result<Package> {
        if source.starts_with("npm:") {
            self.install_npm(&source[4..]).await
        } else {
            self.install_local(Path::new(source))
        }
    }
}
```

## Slash Commands

Slash commands provide in-session shortcuts. Defined in `util/slash_commands.rs`,
handled in `tui/slash.rs`.

### Built-in Commands

| Command | Description |
|---------|-------------|
| `/help` | Show help and available commands |
| `/quit` | Quit oxicode (aliases: /exit, /q) |
| `/model [id]` | Switch or show model |
| `/scoped-models` | Set/get models for Ctrl+P cycling |
| `/router` | Configure model router |
| `/router pin <tier>` | Pin router tier (low/medium/high/off) |
| `/router disable` | Switch away from router |
| `/router enable` | Switch to router/auto |
| `/skill` | List skills with active status |
| `/skill <name>` | Activate a skill |
| `/skill off <name>` | Deactivate a skill |
| `/compact [instr]` | Manually compact context |
| `/tools [name]` | List active tools or toggle on/off |
| `/extensions` | List extensions & WASM tools |
| `/export [path]` | Export session to HTML |
| `/import <path>` | Import session from JSONL |
| `/share` | Share session as GitHub Gist |
| `/copy` | Copy code block / last reply to clipboard |
| `/new` | Start a new session |
| `/clone` | Duplicate current session |
| `/resume` | Resume a different session |
| `/fork [id]` | Fork from a previous message |
| `/tree` | Show session tree structure |
| `/session` | Show session info and stats |
| `/name <name>` | Set session display name |
| `/provider` | Configure API key for a provider |
| `/logout` | Remove provider authentication |
| `/settings` | Show current settings |
| `/reload` | Reload settings, theme, and extensions |
| `/changelog` | Show changelog entries |
| `/hotkeys` | Show all keyboard shortcuts |

Extensions can register additional slash commands via `Extension::register_commands()`.

## Telemetry

```rust
pub struct Telemetry {
    events: Vec<TelemetryEvent>,
    flush_interval: Duration,
}

pub enum TelemetryEvent {
    SessionStart { session_id: Uuid },
    MessageSent { tokens: usize },
    MessageReceived { tokens: usize },
    ToolUsed { tool: String, duration_ms: u64 },
    Error { error: String },
}
```

## Error Recovery

```rust
pub struct RetryConfig {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub backoff_multiplier: f32,
}

pub enum RetryableError {
    NetworkError,
    RateLimit { retry_after: Duration },
    Timeout,
    ProviderError,
}
```

### Retry Strategy

```rust
impl RetryStrategy {
    fn should_retry(&self, error: &Error, attempt: u32) -> bool {
        match error {
            Error::Retryable(r) => attempt < self.max_attempts,
            _ => false,
        }
    }
    
    fn next_delay(&self, attempt: u32) -> Duration {
        let delay = self.base_delay * self.backoff_multiplier.powi(attempt as i32);
        delay.min(self.max_delay)
    }
}
```