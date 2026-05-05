# oxi-cli Architecture

This document describes the internal architecture of the `oxi-cli` crate.

## Session System

The session system manages conversation history with branching support.

### Session File Format

Sessions are stored as newline-delimited JSON (JSONL):

```
~/.oxi/sessions/{session_id}.jsonl
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
│  4. Environment variables (OXI_*)       │
├─────────────────────────────────────────┤
│  3. Project config (.oxi/settings.toml)│
├─────────────────────────────────────────┤
│  2. Global config (~/.oxi/settings.toml) │
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
    pub stream_responses: bool,
    pub extensions_enabled: bool,
    pub auto_compaction: bool,
    pub tool_timeout_seconds: u64,
}
```

### Environment Variables

| Variable | Setting |
|----------|---------|
| `OXI_MODEL` | `default_model` |
| `OXI_PROVIDER` | `default_provider` |
| `OXI_THINKING` | `thinking_level` |
| `OXI_THEME` | `theme` |
| `OXI_MAX_TOKENS` | `max_tokens` |
| `OXI_TEMPERATURE` | `default_temperature` |
| `OXI_SESSION_DIR` | `session_dir` |
| `OXI_STREAM` | `stream_responses` |
| `OXI_AUTO_COMPACTION` | `auto_compaction` |
| `OXI_TOOL_TIMEOUT` | `tool_timeout_seconds` |

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

Slash commands provide in-session shortcuts:

```rust
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub handler: Box<dyn Fn(&str, &mut AgentSession) -> Result<()>>,
}
```

### Built-in Commands

| Command | Description |
|---------|-------------|
| `/model <model>` | Switch model |
| `/provider <provider>` | Switch provider |
| `/session` | Show current session info |
| `/clear` | Clear conversation |
| `/compact` | Trigger compaction |
| `/save <name>` | Save conversation |
| `/load <name>` | Load conversation |

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