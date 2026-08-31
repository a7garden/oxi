//! Installation mechanics: the per-session service inventory, the
//! host-controlled installer interception point, the requested config patch,
//! the installed manifest, and `BehaviorPack::install()`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use oxicode_agent::AgentTool;
use oxicode_agent::agent_loop::ttsr::TtsrEngine;
use oxicode_agent::tools::{
    LspProvider, MemoryBackend, SubagentRunner, TodoStateProvider, UrlResolver,
};
use oxicode_hashline::SnapshotStore;

use super::ledger::{CompatibilityContract, FeatureStatus};
use super::types::{
    BehaviorInstallError, BehaviorPack, BehaviorPackId, BehaviorToolDescriptor, ExtensionKind,
    PortRequirementKind, PromptLayerSpec, ToolImplementationId,
};

/// Per-session service inventory a host offers to the pack installer.
///
/// `None`/empty fields mean the host does not provide the capability; the
/// installer degrades optional dependencies and fails required ones. This is
/// the host's policy boundary: the pack can never acquire a service the host
/// did not hand over.
#[derive(Clone)]
pub struct BehaviorSessionServices {
    /// Workspace root handed to canonical file/process tools.
    pub workspace_root: PathBuf,
    /// Model-visible tool names the host refuses (mirrors `--tools` /
    /// `disabled_tools` settings).
    pub disabled_tools: Vec<String>,
    /// Hashline snapshot store (file/edit-anchor state).
    pub snapshot_store: Option<Arc<dyn SnapshotStore>>,
    /// LSP provider capability.
    pub lsp: Option<Arc<dyn LspProvider>>,
    /// TTSR stream-rule engine.
    pub ttsr_engine: Option<Arc<TtsrEngine>>,
    /// Internal URL resolver (`skill://`, `issue://`, ...).
    pub url_resolver: Option<Arc<dyn UrlResolver>>,
    /// In-process sub-agent runner.
    pub subagent_runner: Option<Arc<dyn SubagentRunner>>,
    /// Memory backend.
    pub memory: Option<Arc<dyn MemoryBackend>>,
    /// Session todo state.
    pub todo: Option<Arc<dyn TodoStateProvider>>,
    /// Persistent shell session (none ships yet — see `oxicode_agent::runtime`).
    pub shell_session: Option<Arc<dyn oxicode_agent::runtime::ShellSession>>,
    /// Persistent eval kernels, one per language (none ship yet).
    pub eval_kernels: Vec<Arc<dyn oxicode_agent::runtime::EvalKernel>>,
    /// DAP debug service (none ships yet).
    pub debug_service: Option<Arc<dyn oxicode_agent::runtime::DebugService>>,
}

impl std::fmt::Debug for BehaviorSessionServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BehaviorSessionServices")
            .field("workspace_root", &self.workspace_root)
            .field("disabled_tools", &self.disabled_tools)
            .field(
                "snapshot_store",
                &self.snapshot_store.as_ref().map(|_| "<dyn SnapshotStore>"),
            )
            .field("lsp", &self.lsp.as_ref().map(|_| "<dyn LspProvider>"))
            .field(
                "ttsr_engine",
                &self.ttsr_engine.as_ref().map(|_| "<TtsrEngine>"),
            )
            .field(
                "url_resolver",
                &self.url_resolver.as_ref().map(|_| "<dyn UrlResolver>"),
            )
            .field(
                "subagent_runner",
                &self
                    .subagent_runner
                    .as_ref()
                    .map(|_| "<dyn SubagentRunner>"),
            )
            .field(
                "memory",
                &self.memory.as_ref().map(|_| "<dyn MemoryBackend>"),
            )
            .field(
                "todo",
                &self.todo.as_ref().map(|_| "<dyn TodoStateProvider>"),
            )
            .field(
                "shell_session",
                &self.shell_session.as_ref().map(|_| "<dyn ShellSession>"),
            )
            .field("eval_kernels", &self.eval_kernels.len())
            .field(
                "debug_service",
                &self.debug_service.as_ref().map(|_| "<dyn DebugService>"),
            )
            .finish()
    }
}

impl BehaviorSessionServices {
    /// Inventory with every capability unset.
    pub fn new(workspace_root: PathBuf) -> Self {
        BehaviorSessionServices {
            workspace_root,
            disabled_tools: Vec::new(),
            snapshot_store: None,
            lsp: None,
            ttsr_engine: None,
            url_resolver: None,
            subagent_runner: None,
            memory: None,
            todo: None,
            shell_session: None,
            eval_kernels: Vec::new(),
            debug_service: None,
        }
    }

    /// Provide the hashline snapshot store.
    pub fn with_snapshot_store(mut self, store: Arc<dyn SnapshotStore>) -> Self {
        self.snapshot_store = Some(store);
        self
    }

    /// Set host-refused tool names.
    pub fn with_disabled_tools(mut self, disabled: Vec<String>) -> Self {
        self.disabled_tools = disabled;
        self
    }

    /// Provide the LSP provider.
    pub fn with_lsp(mut self, lsp: Arc<dyn LspProvider>) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// Provide the TTSR engine.
    pub fn with_ttsr_engine(mut self, engine: Arc<TtsrEngine>) -> Self {
        self.ttsr_engine = Some(engine);
        self
    }

    /// Provide the internal URL resolver.
    pub fn with_url_resolver(mut self, resolver: Arc<dyn UrlResolver>) -> Self {
        self.url_resolver = Some(resolver);
        self
    }

    /// Provide the in-process sub-agent runner.
    pub fn with_subagent_runner(mut self, runner: Arc<dyn SubagentRunner>) -> Self {
        self.subagent_runner = Some(runner);
        self
    }

    /// Provide the memory backend.
    pub fn with_memory(mut self, memory: Arc<dyn MemoryBackend>) -> Self {
        self.memory = Some(memory);
        self
    }

    /// Provide the session todo state.
    pub fn with_todo(mut self, todo: Arc<dyn TodoStateProvider>) -> Self {
        self.todo = Some(todo);
        self
    }

    /// Provide a persistent shell session.
    pub fn with_shell_session(
        mut self,
        shell: Arc<dyn oxicode_agent::runtime::ShellSession>,
    ) -> Self {
        self.shell_session = Some(shell);
        self
    }

    /// Provide a persistent eval kernel.
    pub fn with_eval_kernel(mut self, kernel: Arc<dyn oxicode_agent::runtime::EvalKernel>) -> Self {
        self.eval_kernels.push(kernel);
        self
    }

    /// Provide the DAP debug service.
    pub fn with_debug_service(
        mut self,
        debug: Arc<dyn oxicode_agent::runtime::DebugService>,
    ) -> Self {
        self.debug_service = Some(debug);
        self
    }

    /// Whether the host supplies `kind`.
    pub fn port_available(&self, kind: PortRequirementKind) -> bool {
        match kind {
            PortRequirementKind::HashlineSnapshotStore => self.snapshot_store.is_some(),
            PortRequirementKind::LspProvider => self.lsp.is_some(),
            PortRequirementKind::TtsrEngine => self.ttsr_engine.is_some(),
            PortRequirementKind::UrlResolver => self.url_resolver.is_some(),
            PortRequirementKind::SubagentRunner => self.subagent_runner.is_some(),
            PortRequirementKind::MemoryBackend => self.memory.is_some(),
            PortRequirementKind::TodoStateProvider => self.todo.is_some(),
            PortRequirementKind::ShellSession => self.shell_session.is_some(),
            PortRequirementKind::EvalKernel => !self.eval_kernels.is_empty(),
            PortRequirementKind::DebugService => self.debug_service.is_some(),
        }
    }

    /// Whether the host supplies the service backing `kind`.
    pub fn extension_available(&self, kind: ExtensionKind) -> bool {
        self.port_available(kind.port())
    }
}

/// Host-controlled interception point.
///
/// The pack never calls `ToolRegistry::register*` behind the host's back:
/// every tool passes through `install`, where the host may wrap it (access
/// gate, audit, approval, telemetry), register it, or reject it.
pub trait BehaviorToolInstaller: Send {
    /// Install (possibly wrapped) `tool` for `descriptor`. `Err` = host
    /// rejection: essential tools fail the pack, optional tools degrade.
    fn install(
        &mut self,
        descriptor: &BehaviorToolDescriptor,
        tool: Arc<dyn AgentTool>,
    ) -> Result<(), BehaviorInstallError>;
}

/// The pack's request for standard `AgentConfig` fields.
///
/// A request the host validates against policy before applying — never
/// unrestricted mutable access (design: "Behavior-pack resolution").
#[derive(Clone, Default)]
pub struct AgentConfigPatch {
    /// Hashline snapshot store wiring.
    pub snapshot_store: Option<Arc<dyn SnapshotStore>>,
    /// LSP provider wiring.
    pub lsp: Option<Arc<dyn LspProvider>>,
    /// TTSR engine wiring.
    pub ttsr_engine: Option<Arc<TtsrEngine>>,
    /// Internal URL resolver wiring.
    pub url_resolver: Option<Arc<dyn UrlResolver>>,
    /// Sub-agent runner wiring.
    pub subagent_runner: Option<Arc<dyn SubagentRunner>>,
    /// Memory backend wiring.
    pub memory: Option<Arc<dyn MemoryBackend>>,
    /// Todo state wiring.
    pub todo: Option<Arc<dyn TodoStateProvider>>,
    /// Prompt layers to prepend/merge into the system prompt.
    pub prompt_layers: Vec<PromptLayerSpec>,
}

impl std::fmt::Debug for AgentConfigPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfigPatch")
            .field(
                "snapshot_store",
                &self.snapshot_store.as_ref().map(|_| "<dyn SnapshotStore>"),
            )
            .field("lsp", &self.lsp.as_ref().map(|_| "<dyn LspProvider>"))
            .field(
                "ttsr_engine",
                &self.ttsr_engine.as_ref().map(|_| "<TtsrEngine>"),
            )
            .field(
                "url_resolver",
                &self.url_resolver.as_ref().map(|_| "<dyn UrlResolver>"),
            )
            .field(
                "subagent_runner",
                &self
                    .subagent_runner
                    .as_ref()
                    .map(|_| "<dyn SubagentRunner>"),
            )
            .field(
                "memory",
                &self.memory.as_ref().map(|_| "<dyn MemoryBackend>"),
            )
            .field(
                "todo",
                &self.todo.as_ref().map(|_| "<dyn TodoStateProvider>"),
            )
            .field(
                "prompt_layers",
                &self
                    .prompt_layers
                    .iter()
                    .map(|l| l.id.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// One tool actually offered to (and accepted by) the host installer.
#[derive(Debug, Clone)]
pub struct InstalledToolRecord {
    /// Implementation identity.
    pub descriptor: ToolImplementationId,
    /// Model-visible name registered.
    pub exposed_name: String,
}

/// Why an expected capability is degraded.
#[derive(Debug, Clone)]
pub enum DegradationReason {
    /// A tool's (optional) port has no service.
    ServiceUnavailable(PortRequirementKind),
    /// A declared optional extension has no service.
    ExtensionUnavailable(ExtensionKind),
    /// The host listed the tool as disabled.
    DisabledByHost,
    /// The host installer rejected the tool.
    HostRejected {
        /// The model-visible name of the rejected tool.
        tool: String,
        /// Rejection detail from the host installer.
        reason: String,
    },
}

/// A structured degradation record (design: "reject an unavailable optional
/// tool and return a structured degradation record").
#[derive(Debug, Clone)]
pub struct DegradationRecord {
    /// Feature slug (`ExtensionKind::slug()`) or the exposed tool name for
    /// per-tool cases.
    pub feature: String,
    /// Why it degraded.
    pub reason: DegradationReason,
    /// Model-visible names affected.
    pub affected_tools: Vec<String>,
}

/// Result of a successful pack install: the actual tools offered, disabled
/// capabilities, extension status, and the compatibility level for the turn.
///
/// Distinct from `oxicode_sdk::lifecycle::ToolManifest`, which records
/// state-snapshot registry metadata.
#[derive(Debug, Clone)]
pub struct InstalledBehaviorManifest {
    /// Pack ids that produced this manifest.
    pub packs: Vec<BehaviorPackId>,
    /// Pack schema version.
    pub schema_version: u32,
    /// Tools accepted by the host installer.
    pub tools: Vec<InstalledToolRecord>,
    /// Structured degradation records.
    pub degraded: Vec<DegradationRecord>,
    /// Installed prompt-layer ids.
    pub prompt_layers: Vec<String>,
    /// Resolved compatibility claims.
    pub compatibility: CompatibilityContract,
}

impl InstalledBehaviorManifest {
    /// Worst ledger status — what the host may advertise (and no more).
    pub fn compatibility_level(&self) -> FeatureStatus {
        self.compatibility.rollup()
    }
}

/// Shared install loop backing single-pack and multi-pack (resolved) installs.
///
/// Order: descriptor order. Per tool:
/// 1. host-disabled non-essential → `DisabledByHost` degradation (essential
///    disabled → hard error);
/// 2. required port missing → essential ? hard error : degradation;
/// 3. factory failure → hard error;
/// 4. installer rejection → essential ? hard error : degradation.
///
/// Required extensions are checked before any tool is offered; optional,
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_descriptors(
    pack_ids: Vec<BehaviorPackId>,
    schema_version: u32,
    prompt_layer_ids: Vec<String>,
    compatibility: CompatibilityContract,
    extensions: &[super::types::RuntimeExtensionSpec],
    descriptors: &[BehaviorToolDescriptor],
    factories: &std::collections::HashMap<ToolImplementationId, super::types::ToolFactory>,
    services: &BehaviorSessionServices,
    installer: &mut dyn BehaviorToolInstaller,
) -> Result<InstalledBehaviorManifest, BehaviorInstallError> {
    if schema_version != 1 {
        return Err(BehaviorInstallError::UnsupportedSchemaVersion {
            pack: pack_ids
                .first()
                .cloned()
                .unwrap_or_else(|| BehaviorPackId("<unknown>".to_string())),
            got: schema_version,
        });
    }
    let disabled: HashSet<&str> = services.disabled_tools.iter().map(String::as_str).collect();
    let mut manifest = InstalledBehaviorManifest {
        packs: pack_ids,
        schema_version,
        tools: Vec::new(),
        degraded: Vec::new(),
        prompt_layers: prompt_layer_ids,
        compatibility,
    };
    // Required extensions first — fail before any tool is installed.
    for ext in extensions {
        if ext.required && !services.extension_available(ext.kind) {
            return Err(BehaviorInstallError::RequiredExtensionMissing {
                pack: manifest
                    .packs
                    .first()
                    .cloned()
                    .unwrap_or_else(|| BehaviorPackId("<unknown>".to_string())),
                kind: ext.kind,
            });
        }
    }
    for d in descriptors {
        if disabled.contains(d.exposed_name.as_str()) {
            // The host's explicit disable list / --tools selection always
            // wins — recorded as a structured degradation so the manifest
            // shows an essential tool was intentionally withheld. (Installer
            // *rejection* of an essential tool still fails the pack below.)
            manifest.degraded.push(DegradationRecord {
                feature: d.exposed_name.clone(),
                reason: DegradationReason::DisabledByHost,
                affected_tools: vec![d.exposed_name.clone()],
            });
            continue;
        }
        let mut missing_required: Option<PortRequirementKind> = None;
        for port in &d.required_ports {
            if port.required && !services.port_available(port.kind) {
                missing_required = Some(port.kind);
                break;
            }
        }
        if let Some(kind) = missing_required {
            if d.essential {
                return Err(BehaviorInstallError::RequiredServiceMissing {
                    descriptor: d.id.clone(),
                    kind,
                });
            }
            manifest.degraded.push(DegradationRecord {
                feature: d.exposed_name.clone(),
                reason: DegradationReason::ServiceUnavailable(kind),
                affected_tools: vec![d.exposed_name.clone()],
            });
            continue;
        }
        let factory = factories
            .get(&d.id)
            .ok_or_else(|| BehaviorInstallError::FactoryFailed {
                descriptor: d.id.clone(),
                reason: "no factory registered".to_string(),
            })?;
        let tool = factory(services)?;
        match installer.install(d, tool) {
            Ok(()) => manifest.tools.push(InstalledToolRecord {
                descriptor: d.id.clone(),
                exposed_name: d.exposed_name.clone(),
            }),
            Err(e) => {
                if d.essential {
                    return Err(BehaviorInstallError::HostRejected {
                        descriptor: d.id.clone(),
                        exposed_name: d.exposed_name.clone(),
                        reason: e.to_string(),
                    });
                }
                manifest.degraded.push(DegradationRecord {
                    feature: d.exposed_name.clone(),
                    reason: DegradationReason::HostRejected {
                        tool: d.exposed_name.clone(),
                        reason: e.to_string(),
                    },
                    affected_tools: vec![d.exposed_name.clone()],
                });
            }
        }
    }
    // Optional, unsatisfied extensions → one degradation each.
    for ext in extensions {
        if !ext.required && !services.extension_available(ext.kind) {
            let affected: Vec<String> = descriptors
                .iter()
                .filter(|t| t.required_ports.iter().any(|p| p.kind == ext.kind.port()))
                .map(|t| t.exposed_name.clone())
                .collect();
            manifest.degraded.push(DegradationRecord {
                feature: ext.kind.slug().to_string(),
                reason: DegradationReason::ExtensionUnavailable(ext.kind),
                affected_tools: affected,
            });
        }
    }
    Ok(manifest)
}

impl BehaviorPack {
    /// Create canonical tools and hand each to the host installer.
    ///
    /// See [`install_descriptors`] for ordering and degradation rules.
    pub fn install(
        &self,
        services: &BehaviorSessionServices,
        installer: &mut dyn BehaviorToolInstaller,
    ) -> Result<InstalledBehaviorManifest, BehaviorInstallError> {
        install_descriptors(
            vec![self.id.clone()],
            self.schema_version,
            self.prompt_layers.iter().map(|l| l.id.clone()).collect(),
            self.compatibility.clone(),
            &self.extensions,
            &self.tools,
            &self.factories,
            services,
            installer,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior::types::{BehaviorPack, CapabilityClass, SideEffectClass, ToolFactory};
    use parking_lot::Mutex;
    #[derive(Debug)]
    struct RecordingInstaller {
        accepted: Mutex<Vec<String>>,
        reject: Vec<String>,
    }

    impl RecordingInstaller {
        fn new(reject: Vec<String>) -> Self {
            Self {
                accepted: Mutex::new(Vec::new()),
                reject,
            }
        }
    }

    impl BehaviorToolInstaller for RecordingInstaller {
        fn install(
            &mut self,
            descriptor: &BehaviorToolDescriptor,
            _tool: Arc<dyn AgentTool>,
        ) -> Result<(), BehaviorInstallError> {
            if self.reject.contains(&descriptor.exposed_name) {
                return Err(BehaviorInstallError::HostRejected {
                    descriptor: descriptor.id.clone(),
                    exposed_name: descriptor.exposed_name.clone(),
                    reason: "test rejection".to_string(),
                });
            }
            self.accepted.lock().push(descriptor.exposed_name.clone());
            Ok(())
        }
    }

    #[derive(Debug)]
    struct TestTool {
        name: String,
    }

    #[async_trait::async_trait]
    impl AgentTool for TestTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn label(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "test tool"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &oxicode_agent::ToolContext,
        ) -> Result<oxicode_agent::AgentToolResult, String> {
            Ok(oxicode_agent::AgentToolResult::success("ok"))
        }
    }

    fn simple_tool_factory(name: &'static str) -> ToolFactory {
        Arc::new(move |_| {
            Ok(Arc::new(TestTool {
                name: name.to_string(),
            }) as Arc<dyn AgentTool>)
        })
    }

    fn test_pack() -> BehaviorPack {
        BehaviorPack::new(
            BehaviorPackId("test-pack".to_string()),
            "omp@test".to_string(),
        )
        .with_prompt_layer(PromptLayerSpec {
            id: "layer-1".to_string(),
            body: "body".to_string(),
        })
        .with_extension(crate::behavior::types::RuntimeExtensionSpec {
            kind: ExtensionKind::HashlineState,
            scope: crate::behavior::types::ExtensionScope::SessionWorkspace,
            required: true,
        })
        .with_extension(crate::behavior::types::RuntimeExtensionSpec {
            kind: ExtensionKind::ShellSession,
            scope: crate::behavior::types::ExtensionScope::SessionWorkspace,
            required: false,
        })
        .with_tool(
            BehaviorToolDescriptor::new("essential.tool.v1", "essential_tool")
                .capability(CapabilityClass::FsWrite)
                .side_effect(SideEffectClass::Mutating)
                .port(PortRequirementKind::HashlineSnapshotStore, true)
                .essential(),
            simple_tool_factory("essential_tool"),
        )
        .unwrap()
        .with_tool(
            BehaviorToolDescriptor::new("optional.tool.v1", "optional_tool")
                .port(PortRequirementKind::ShellSession, false),
            simple_tool_factory("optional_tool"),
        )
        .unwrap()
    }

    fn services_with_store() -> BehaviorSessionServices {
        BehaviorSessionServices::new(std::env::temp_dir())
            .with_snapshot_store(Arc::new(oxicode_hashline::InMemorySnapshotStore::new()))
    }

    #[test]
    fn install_degrades_optional_missing_service() {
        let pack = test_pack();
        let services = services_with_store();
        let mut installer = RecordingInstaller::new(Vec::new());
        let manifest = pack.install(&services, &mut installer).unwrap();
        assert_eq!(manifest.tools.len(), 2);
        assert_eq!(manifest.tools[0].exposed_name, "essential_tool");
        assert_eq!(manifest.prompt_layers, vec!["layer-1".to_string()]);
        // Optional extension (ShellSession) unavailable → slug degradation
        // naming the affected tool.
        let degraded: Vec<&str> = manifest
            .degraded
            .iter()
            .map(|d| d.feature.as_str())
            .collect();
        assert_eq!(degraded, vec!["shell-session"]);
        assert_eq!(manifest.degraded[0].affected_tools, vec!["optional_tool"]);
        assert_eq!(installer.accepted.lock().len(), 2);
    }

    #[test]
    fn install_fails_when_essential_required_service_missing() {
        // No required extensions here — the essential tool's port is the
        // thing under test.
        let pack = BehaviorPack::new(BehaviorPackId("p".to_string()), "t".to_string())
            .with_tool(
                BehaviorToolDescriptor::new("essential.tool.v1", "essential_tool")
                    .capability(CapabilityClass::FsWrite)
                    .side_effect(SideEffectClass::Mutating)
                    .port(PortRequirementKind::HashlineSnapshotStore, true)
                    .essential(),
                simple_tool_factory("essential_tool"),
            )
            .unwrap();
        let services = BehaviorSessionServices::new(std::env::temp_dir());
        let mut installer = RecordingInstaller::new(Vec::new());
        let err = pack.install(&services, &mut installer).unwrap_err();
        assert!(matches!(
            err,
            BehaviorInstallError::RequiredServiceMissing { ref kind, .. }
                if *kind == PortRequirementKind::HashlineSnapshotStore
        ));
    }

    #[test]
    fn install_fails_when_required_extension_missing() {
        let pack = BehaviorPack::new(BehaviorPackId("p".to_string()), "t".to_string())
            .with_extension(crate::behavior::types::RuntimeExtensionSpec {
                kind: ExtensionKind::HashlineState,
                scope: crate::behavior::types::ExtensionScope::SessionWorkspace,
                required: true,
            })
            .with_tool(
                BehaviorToolDescriptor::new("t.v1", "tool"),
                simple_tool_factory("tool"),
            )
            .unwrap();
        let services = BehaviorSessionServices::new(std::env::temp_dir());
        let mut installer = RecordingInstaller::new(Vec::new());
        let err = pack.install(&services, &mut installer).unwrap_err();
        assert!(matches!(
            err,
            BehaviorInstallError::RequiredExtensionMissing {
                kind: ExtensionKind::HashlineState,
                ..
            }
        ));
    }

    #[test]
    fn install_degrades_host_rejected_optional_tool() {
        let pack = test_pack();
        let services = services_with_store();
        let mut installer = RecordingInstaller::new(vec!["optional_tool".to_string()]);
        let manifest = pack.install(&services, &mut installer).unwrap();
        assert_eq!(manifest.tools.len(), 1);
        assert!(manifest.degraded.iter().any(|d| matches!(
            d.reason,
            DegradationReason::HostRejected { ref tool, .. } if tool == "optional_tool"
        )));
    }

    #[test]
    fn install_fails_when_host_rejects_essential_tool() {
        let pack = test_pack();
        let services = services_with_store();
        let mut installer = RecordingInstaller::new(vec!["essential_tool".to_string()]);
        let err = pack.install(&services, &mut installer).unwrap_err();
        assert!(
            matches!(err, BehaviorInstallError::HostRejected { ref exposed_name, .. } if exposed_name == "essential_tool")
        );
    }

    #[test]
    fn install_respects_host_disabled_list() {
        let pack = test_pack();
        let services = services_with_store().with_disabled_tools(vec!["optional_tool".to_string()]);
        let mut installer = RecordingInstaller::new(Vec::new());
        let manifest = pack.install(&services, &mut installer).unwrap();
        assert!(
            manifest
                .degraded
                .iter()
                .any(|d| matches!(d.reason, DegradationReason::DisabledByHost))
        );
        assert_eq!(manifest.tools.len(), 1);
    }

    #[test]
    fn install_rejects_unsupported_schema_version() {
        let mut pack = test_pack();
        pack.schema_version = 99;
        let services = services_with_store();
        let mut installer = RecordingInstaller::new(Vec::new());
        let err = pack.install(&services, &mut installer).unwrap_err();
        assert!(matches!(
            err,
            BehaviorInstallError::UnsupportedSchemaVersion { got: 99, .. }
        ));
    }
}
