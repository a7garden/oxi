//! Shared harness for `coding-omp-v1` behavior fixtures (deterministic:
//! scripted provider, in-memory stores, no network / TUI / OMP binary).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

pub use oxicode_sdk::behavior::{
    BehaviorInstallError, BehaviorPackResolver, BehaviorSessionServices, BehaviorToolDescriptor,
    BehaviorToolInstaller,
};
pub use std::sync::Arc;

use oxicode_agent::tools::{AgentTool, ToolError, ToolRegistry};
use oxicode_agent::{AgentToolResult, ToolContext};
use oxicode_ai::{
    Api, AssistantMessage, ContentBlock, Provider, ProviderEvent, StopReason, StreamResult,
    TextContent, ToolCall,
};
use oxicode_hashline::InMemorySnapshotStore;
use parking_lot::Mutex;

/// Shared tool-trace log: entries like `read:call_0:ok:<prefix>` or
/// `bash:call_1:denied`.
pub type Trace = Arc<Mutex<Vec<String>>>;

/// One scripted provider turn.
pub enum ScriptedReply {
    /// Assistant text (end of task).
    Text(String),
    /// Tool calls issued this turn: `(tool_name, args)`.
    ToolCalls(Vec<(String, serde_json::Value)>),
}

/// Provider returning one scripted reply per `stream` call.
pub struct ScriptedProvider {
    replies: Mutex<Vec<ScriptedReply>>,
    calls: AtomicUsize,
}

impl ScriptedProvider {
    pub fn new(replies: Vec<ScriptedReply>) -> Self {
        Self {
            replies: Mutex::new(replies),
            calls: AtomicUsize::new(0),
        }
    }
}

impl Provider for ScriptedProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a oxicode_ai::Model,
        _context: &'a oxicode_ai::Context,
        _options: Option<oxicode_ai::StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let idx = self.calls.fetch_add(1, Ordering::SeqCst);
            let reply = {
                let mut replies = self.replies.lock();
                assert!(
                    !replies.is_empty(),
                    "scripted transcript exhausted at call {idx}"
                );
                replies.remove(0)
            };
            let mut assistant = AssistantMessage::new(Api::AnthropicMessages, "mock", "mock-model");
            let events = match reply {
                ScriptedReply::Text(text) => {
                    assistant.content = vec![ContentBlock::Text(TextContent::new(text))];
                    assistant.stop_reason = StopReason::Stop;
                    vec![
                        ProviderEvent::Start {
                            partial: Arc::new(assistant.clone()),
                        },
                        ProviderEvent::Done {
                            reason: StopReason::Stop,
                            message: assistant,
                        },
                    ]
                }
                ScriptedReply::ToolCalls(calls) => {
                    assistant.content = calls
                        .into_iter()
                        .enumerate()
                        .map(|(i, (name, args))| {
                            ContentBlock::ToolCall(ToolCall::new(format!("call_{i}"), name, args))
                        })
                        .collect();
                    assistant.stop_reason = StopReason::ToolUse;
                    vec![
                        ProviderEvent::Start {
                            partial: Arc::new(assistant.clone()),
                        },
                        ProviderEvent::Done {
                            reason: StopReason::ToolUse,
                            message: assistant,
                        },
                    ]
                }
            };
            Ok(Box::pin(futures::stream::iter(events))
                as Pin<
                    Box<dyn futures::Stream<Item = ProviderEvent> + Send>,
                >)
        })
    }
}

/// How the recording installer wraps each installed tool.
pub enum WrapMode {
    /// Record call order + result prefixes (the host audit wrapper).
    Trace,
    /// Substitute [`DenyTool`] for the named tool; everything else is traced.
    Deny { tool: String },
}

/// Host installer that records every install call, wraps tools, and hands
/// back a registry — proving all registry entries flow through the
/// interceptor (the pack never touches a registry).
pub struct RecordingInstaller {
    pub installed: Vec<String>,
    pub wrapped: Vec<(String, Arc<dyn AgentTool>)>,
    pub trace: Trace,
    pub wrap: WrapMode,
}

impl RecordingInstaller {
    pub fn new(wrap: WrapMode) -> Self {
        Self {
            installed: Vec::new(),
            wrapped: Vec::new(),
            trace: Arc::new(Mutex::new(Vec::new())),
            wrap,
        }
    }

    /// Build a registry from the wrapped tools.
    pub fn into_registry(self) -> ToolRegistry {
        let registry = ToolRegistry::new();
        for (_, tool) in self.wrapped {
            registry.register_arc(tool);
        }
        registry
    }
}

impl BehaviorToolInstaller for RecordingInstaller {
    fn install(
        &mut self,
        descriptor: &BehaviorToolDescriptor,
        tool: Arc<dyn AgentTool>,
    ) -> Result<(), BehaviorInstallError> {
        self.installed.push(descriptor.exposed_name.clone());
        let wrapped: Arc<dyn AgentTool> = match &self.wrap {
            WrapMode::Trace => Arc::new(TraceTool {
                inner: tool,
                trace: self.trace.clone(),
            }),
            WrapMode::Deny { tool: denied } if denied == &descriptor.exposed_name => {
                Arc::new(DenyTool {
                    inner: tool,
                    trace: self.trace.clone(),
                })
            }
            WrapMode::Deny { .. } => Arc::new(TraceTool {
                inner: tool,
                trace: self.trace.clone(),
            }),
        };
        self.wrapped
            .push((descriptor.exposed_name.clone(), wrapped));
        Ok(())
    }
}

/// Tracing proxy: records `{name}:{call_id}:ok|err:{output prefix}`.
struct TraceTool {
    inner: Arc<dyn AgentTool>,
    trace: Trace,
}

fn record(trace: &Trace, entry: String) {
    let mut t = trace.lock();
    t.push(entry);
}

fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(400).collect();
    if text.chars().count() > 400 {
        out.push('…');
    }
    out.replace('\n', "\\n")
}

#[async_trait::async_trait]
impl AgentTool for TraceTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn label(&self) -> &str {
        self.inner.label()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }
    fn essential(&self) -> bool {
        self.inner.essential()
    }
    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        match self.inner.execute(tool_call_id, params, signal, ctx).await {
            Ok(result) => {
                let text = result.to_string();
                let tagged = format!("{}:{}:ok:{}", self.inner.name(), tool_call_id, clip(&text));
                record(&self.trace, tagged);
                Ok(result)
            }
            Err(e) => {
                let tagged = format!("{}:{}:err:{}", self.inner.name(), tool_call_id, clip(&e));
                record(&self.trace, tagged);
                Err(e)
            }
        }
    }
}

/// Structured-deny proxy: the host refused this tool; the model receives a
/// normal, recoverable tool result (design "Host policy boundary").
struct DenyTool {
    inner: Arc<dyn AgentTool>,
    trace: Trace,
}

#[async_trait::async_trait]
impl AgentTool for DenyTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn label(&self) -> &str {
        self.inner.label()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }
    fn essential(&self) -> bool {
        self.inner.essential()
    }
    async fn execute(
        &self,
        tool_call_id: &str,
        _params: serde_json::Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        record(
            &self.trace,
            format!("{}:{}:denied", self.inner.name(), tool_call_id),
        );
        Ok(AgentToolResult::success(
            r#"{"error":"denied by host policy","recoverable":true}"#,
        ))
    }
}

/// Workspace containing one LF-only `lib.rs` (hash-stable for the anchor
/// round-trip).
pub fn workspace_with_lib_rs() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    (dir, file)
}

/// Services with only the snapshot store provided.
pub fn minimal_services(ws: &Path) -> BehaviorSessionServices {
    BehaviorSessionServices::new(ws.to_path_buf())
        .with_snapshot_store(Arc::new(InMemorySnapshotStore::new()))
}

/// Resolve + install the builtin pack through `installer`; returns the manifest.
pub fn install_builtin_pack(
    ws: &Path,
    installer: &mut dyn BehaviorToolInstaller,
) -> oxicode_sdk::behavior::InstalledBehaviorManifest {
    let services = minimal_services(ws);
    let resolver = BehaviorPackResolver::with_builtin_packs().unwrap();
    let resolved = resolver
        .resolve(
            &[oxicode_sdk::behavior::BehaviorPackId::coding_omp_v1()],
            &services,
        )
        .unwrap();
    resolved.install(&services, installer).unwrap()
}

/// Drive one AgentLoop run over `registry` with a scripted transcript and
/// collect AgentEvents. `config` comes from [`loop_config`] so fixtures can
/// thread stores/providers.
pub async fn run_scripted_turns(
    provider: Arc<ScriptedProvider>,
    registry: Arc<ToolRegistry>,
    config: oxicode_agent::AgentLoopConfig,
    prompt: &str,
) -> Vec<oxicode_agent::AgentEvent> {
    use oxicode_agent::{AgentLoop, SharedState};

    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, registry, state);
    let events = Arc::new(Mutex::new(Vec::new()));
    {
        let events = events.clone();
        agent_loop
            .run(prompt.to_string(), move |e| events.lock().push(e))
            .await
            .expect("agent loop run");
    }
    events.lock().clone()
}

/// Minimal in-memory tool for resolver-level fixtures.
pub struct StubTool {
    pub name: String,
}

#[async_trait::async_trait]
impl AgentTool for StubTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn label(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "stub tool"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        Ok(AgentToolResult::success("stub ok"))
    }
}

/// LSP provider stub recording executed actions and replying canned text.
#[derive(Debug, Default)]
pub struct MockLspProvider {
    pub actions: Mutex<Vec<String>>,
}

impl oxicode_agent::tools::LspProvider for MockLspProvider {
    fn ensure_started_background<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
    fn ensure_ready<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn drain_diagnostics<'a>(
        &'a self,
        _timeout: std::time::Duration,
    ) -> Pin<Box<dyn Future<Output = Option<oxicode_agent::tools::DiagnosticsSummary>> + Send + 'a>>
    {
        Box::pin(async { None })
    }
    fn read_diagnostics<'a>(
        &'a self,
        _paths: &'a [PathBuf],
    ) -> Pin<Box<dyn Future<Output = Vec<oxicode_agent::tools::FileDiagnosticEntry>> + Send + 'a>>
    {
        Box::pin(async { Vec::new() })
    }
    fn execute_action<'a>(
        &'a self,
        action: &'a oxicode_agent::tools::LspAction,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        let label = format!("{action:?}");
        self.actions.lock().push(label.clone());
        Box::pin(async move { Ok(format!("mock-lsp:{label}")) })
    }
}

/// Sub-agent runner stub recording prompts and returning canned forks.
#[derive(Debug, Default)]
pub struct MockSubagentRunner {
    pub prompts: Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl oxicode_agent::tools::SubagentRunner for MockSubagentRunner {
    async fn run_isolated(
        &self,
        agent_name: &str,
        task: &str,
        _system_prompt: Option<&str>,
        _model: Option<&str>,
        _tools: &[String],
        _cwd: &Path,
        _depth: u8,
    ) -> anyhow::Result<oxicode_agent::tools::ForkResult> {
        self.prompts
            .lock()
            .push((agent_name.to_string(), task.to_string()));
        Ok(oxicode_agent::tools::ForkResult {
            text: format!("mock result for: {task}"),
            ..Default::default()
        })
    }
}

/// AgentLoop config pre-wired with the fixture workspace and optional services.
pub fn loop_config(
    ws: &Path,
    snapshot_store: Option<Arc<dyn oxicode_hashline::SnapshotStore>>,
    subagent_runner: Option<Arc<dyn oxicode_agent::tools::SubagentRunner>>,
    lsp: Option<Arc<dyn oxicode_agent::tools::LspProvider>>,
) -> oxicode_agent::AgentLoopConfig {
    oxicode_agent::AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        workspace_dir: Some(ws.to_path_buf()),
        snapshot_store,
        subagent_runner,
        lsp,
        ..Default::default()
    }
}

/// Resolve + install the builtin pack with custom services; returns
/// `(manifest, requested patch)`.
pub fn install_pack_with_services(
    services: &BehaviorSessionServices,
    installer: &mut dyn BehaviorToolInstaller,
) -> (
    oxicode_sdk::behavior::InstalledBehaviorManifest,
    oxicode_sdk::behavior::AgentConfigPatch,
) {
    let resolver = BehaviorPackResolver::with_builtin_packs().unwrap();
    let resolved = resolver
        .resolve(
            &[oxicode_sdk::behavior::BehaviorPackId::coding_omp_v1()],
            services,
        )
        .unwrap();
    let patch = resolved.patch.clone();
    let manifest = resolved.install(services, installer).unwrap();
    (manifest, patch)
}
