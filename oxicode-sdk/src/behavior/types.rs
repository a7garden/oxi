//! Behavior-pack core data types: identifiers, classification enums, tool
//! descriptors, extension specs, and the declarative `BehaviorPack` itself.
//!
//! `BehaviorPack` is declarative at selection time; installation mechanics
//! (services, interception, manifests) live in [`super::installer`].

use std::collections::HashMap;
use std::sync::Arc;

use oxicode_agent::AgentTool;

use super::installer::BehaviorSessionServices;
use super::ledger::CompatibilityContract;

/// Stable behavior-pack identifier (e.g. `coding-omp-v1`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BehaviorPackId(pub String);

impl BehaviorPackId {
    /// Identifier of the reference coding pack.
    pub fn coding_omp_v1() -> Self {
        BehaviorPackId("coding-omp-v1".to_string())
    }
}

impl std::fmt::Display for BehaviorPackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Product-facing implementation identity — the stability key that must not
/// drift when the model-visible tool name stays fixed (design: "Tool names
/// are model API. Implementation identities are product-facing stability
/// keys.").
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolImplementationId(pub String);

impl std::fmt::Display for ToolImplementationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Coarse capability class — advisory input to host policy, never
/// authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityClass {
    /// Filesystem reads.
    FsRead,
    /// Filesystem writes.
    FsWrite,
    /// Content search (grep/find/AST).
    Search,
    /// Process execution.
    Process,
    /// Network access.
    Network,
    /// Language-server operations.
    Lsp,
    /// Memory backends.
    Memory,
    /// Child-agent delegation.
    Delegation,
    /// Session UI state (todo, ask).
    Ui,
}

/// Side-effect classification — advisory input to host policy; it can never
/// authorize an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffectClass {
    /// No side effects.
    ReadOnly,
    /// Mutates workspace/session state.
    Mutating,
    /// Performs network I/O.
    Networked,
    /// Spawns or drives processes.
    ProcessSpawning,
}

/// Where a tool's mutable state lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStateScope {
    /// No mutable state between calls.
    Stateless,
    /// Hashline snapshot state for the session (`hashline::SnapshotStore`).
    HashlineSession,
    /// Persistent shell environment for the session.
    ShellSession,
    /// Persistent eval kernel state for the session + language.
    EvalKernel,
    /// A debug target process (workspace + debug target).
    DebugTarget,
    /// Workspace-scoped files.
    Workspace,
}

/// A port/service a tool needs at execution time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortRequirementKind {
    /// Hashline snapshot store (file/edit-anchor state).
    HashlineSnapshotStore,
    /// LSP provider capability.
    LspProvider,
    /// TTSR stream-rule engine.
    TtsrEngine,
    /// Internal URL resolver (`skill://`, `issue://`, ...).
    UrlResolver,
    /// In-process sub-agent runner.
    SubagentRunner,
    /// Memory backend.
    MemoryBackend,
    /// Session todo state.
    TodoStateProvider,
    /// Persistent shell session.
    ShellSession,
    /// Persistent eval kernel.
    EvalKernel,
    /// DAP debug service.
    DebugService,
}

/// A declared port dependency with required/optional weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRequirement {
    /// Which port.
    pub kind: PortRequirementKind,
    /// Required tools hard-fail resolution/install when missing; optional
    /// tools degrade with a structured record.
    pub required: bool,
}

/// Model-facing prompt fragment the pack asks the host to install.
#[derive(Debug, Clone)]
pub struct PromptLayerSpec {
    /// Stable layer id (manifest records installed ids).
    pub id: String,
    /// Prompt body the host prepends to (or merges into) the system prompt.
    pub body: String,
}

/// Runtime extension kinds declared by packs (design "Coding extensions").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtensionKind {
    /// Bounded anchored snapshots for reads and safe patch/edit verification.
    HashlineState,
    /// LSP capability discovery, diagnostics, navigation, lifecycle cleanup.
    LspHost,
    /// Persistent command environment with cancellation and output bounds.
    ShellSession,
    /// Persistent Python/Bun kernel state with bounded execution.
    EvalKernel,
    /// Real DAP session lifecycle with breakpoint/control/output events.
    DebugService,
    /// Rule evaluation + retry/repair metadata without hidden model calls.
    TtsrEngine,
    /// Typed child-task lifecycle with inherited limits and cancellation.
    Delegation,
}

impl ExtensionKind {
    /// Stable slug used in manifests and degradation records.
    pub fn slug(&self) -> &'static str {
        match self {
            ExtensionKind::HashlineState => "hashline-state",
            ExtensionKind::LspHost => "lsp-host",
            ExtensionKind::ShellSession => "shell-session",
            ExtensionKind::EvalKernel => "eval-kernel",
            ExtensionKind::DebugService => "debug-service",
            ExtensionKind::TtsrEngine => "ttsr-engine",
            ExtensionKind::Delegation => "delegation",
        }
    }
    /// The port kind this extension backs, for affected-tool computation.
    pub fn port(&self) -> PortRequirementKind {
        match self {
            ExtensionKind::HashlineState => PortRequirementKind::HashlineSnapshotStore,
            ExtensionKind::LspHost => PortRequirementKind::LspProvider,
            ExtensionKind::TtsrEngine => PortRequirementKind::TtsrEngine,
            ExtensionKind::Delegation => PortRequirementKind::SubagentRunner,
            ExtensionKind::ShellSession => PortRequirementKind::ShellSession,
            ExtensionKind::EvalKernel => PortRequirementKind::EvalKernel,
            ExtensionKind::DebugService => PortRequirementKind::DebugService,
        }
    }
}

/// Declared lifetime of an extension requirement — one variant per
/// design-table "Scope" value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionScope {
    /// Lives with the session, scoped to the workspace.
    SessionWorkspace,
    /// Lives with the workspace.
    Workspace,
    /// Lives with the session, per language.
    SessionLanguage,
    /// Lives with the workspace and a debug target.
    WorkspaceDebugTarget,
    /// Lives for one agent turn.
    Turn,
    /// Lives across child-agent lifecycles.
    ChildAgentLifecycle,
}

/// A runtime extension requirement with an explicit lifetime.
#[derive(Debug, Clone)]
pub struct RuntimeExtensionSpec {
    /// Which extension.
    pub kind: ExtensionKind,
    /// Declared lifetime.
    pub scope: ExtensionScope,
    /// Required extensions fail pack resolution when the host cannot supply
    /// them; optional extensions degrade with a structured record.
    pub required: bool,
}

/// Model-visible tool contract: both the implementation identity and the
/// exposed (model API) name, plus advisory classification for host policy.
#[derive(Debug, Clone)]
pub struct BehaviorToolDescriptor {
    /// Stable implementation identity (e.g. `edit.hashline.v1`).
    pub id: ToolImplementationId,
    /// Model-visible tool name (e.g. `edit`).
    pub exposed_name: String,
    /// Advisory capability class.
    pub capability: CapabilityClass,
    /// Advisory side-effect class.
    pub side_effect: SideEffectClass,
    /// Port dependencies, each required or optional.
    pub required_ports: Vec<PortRequirement>,
    /// Where the tool's mutable state lives.
    pub state_scope: ToolStateScope,
    /// Mirrors `AgentTool::essential`: a tool the host cannot skip; rejection
    /// fails the pack instead of degrading it.
    pub essential: bool,
    /// When set, this descriptor explicitly replaces the named implementation
    /// (overlay rule; the replaced pack must declare schema compatibility).
    pub replaces: Option<ToolImplementationId>,
}

impl BehaviorToolDescriptor {
    /// Minimal descriptor; refine with the builder methods.
    pub fn new(id: &str, exposed_name: &str) -> Self {
        BehaviorToolDescriptor {
            id: ToolImplementationId(id.to_string()),
            exposed_name: exposed_name.to_string(),
            capability: CapabilityClass::FsRead,
            side_effect: SideEffectClass::ReadOnly,
            required_ports: Vec::new(),
            state_scope: ToolStateScope::Stateless,
            essential: false,
            replaces: None,
        }
    }

    /// Set the advisory capability class.
    pub fn capability(mut self, capability: CapabilityClass) -> Self {
        self.capability = capability;
        self
    }

    /// Set the advisory side-effect class.
    pub fn side_effect(mut self, side_effect: SideEffectClass) -> Self {
        self.side_effect = side_effect;
        self
    }

    /// Set the state scope.
    pub fn state_scope(mut self, scope: ToolStateScope) -> Self {
        self.state_scope = scope;
        self
    }

    /// Declare a port dependency.
    pub fn port(mut self, kind: PortRequirementKind, required: bool) -> Self {
        self.required_ports.push(PortRequirement { kind, required });
        self
    }

    /// Mark the tool essential (cannot be host-disabled; rejection fails the pack).
    pub fn essential(mut self) -> Self {
        self.essential = true;
        self
    }

    /// Declare this descriptor an explicit replacement of `id`.
    pub fn replaces(mut self, id: &str) -> Self {
        self.replaces = Some(ToolImplementationId(id.to_string()));
        self
    }
}

/// Constructs one canonical tool from the host service inventory.
pub type ToolFactory = Arc<
    dyn Fn(&BehaviorSessionServices) -> Result<Arc<dyn AgentTool>, BehaviorInstallError>
        + Send
        + Sync,
>;

/// A versioned, portable behavior contract (design "Public model").
///
/// Declarative at selection time. `install` may allocate per-session
/// resources but cannot consult model output or widen host authority.
#[derive(Clone)]
pub struct BehaviorPack {
    /// Pack identity.
    pub id: BehaviorPackId,
    /// Pack schema version this SDK understands (currently `1`).
    pub schema_version: u32,
    /// Prompt fragments the host should install.
    pub prompt_layers: Vec<PromptLayerSpec>,
    /// Runtime extension requirements with declared lifetimes.
    pub extensions: Vec<RuntimeExtensionSpec>,
    /// Tool descriptors; declaration order = install order.
    pub tools: Vec<BehaviorToolDescriptor>,
    /// Machine-readable compatibility claims.
    pub compatibility: CompatibilityContract,
    pub(crate) factories: HashMap<ToolImplementationId, ToolFactory>,
}

/// Failure modes across pack registration, resolution, and installation.
#[derive(Debug, Clone)]
pub enum BehaviorInstallError {
    /// Resolution named a pack the resolver does not know.
    UnknownPack(BehaviorPackId),
    /// The same pack id was registered twice.
    DuplicatePackId(BehaviorPackId),
    /// The pack's schema version is not understood by this SDK.
    UnsupportedSchemaVersion {
        /// The offending pack.
        pack: BehaviorPackId,
        /// The version the pack declared.
        got: u32,
    },
    /// One implementation id registered twice within a pack.
    DuplicateToolImplementation {
        /// The offending pack.
        pack: BehaviorPackId,
        /// The duplicated implementation id.
        id: ToolImplementationId,
    },
    /// Two resolved descriptors expose the same model-visible name without
    /// an explicit `replaces` declaration.
    DuplicateExposedName {
        /// The colliding model-visible name.
        exposed_name: String,
        /// The descriptor already holding the name.
        existing: ToolImplementationId,
        /// The descriptor that tried to take it.
        incoming: ToolImplementationId,
    },
    /// A required extension has no satisfying service.
    RequiredExtensionMissing {
        /// The pack declaring the required extension.
        pack: BehaviorPackId,
        /// The unavailable extension.
        kind: ExtensionKind,
    },
    /// An essential tool's required port is missing.
    RequiredServiceMissing {
        /// The essential tool missing its service.
        descriptor: ToolImplementationId,
        /// The missing port.
        kind: PortRequirementKind,
    },
    /// The tool factory itself failed.
    FactoryFailed {
        /// The tool whose factory failed.
        descriptor: ToolImplementationId,
        /// Failure detail.
        reason: String,
    },
    /// The host installer rejected a tool; for essential tools this fails the
    /// pack, otherwise it degrades.
    HostRejected {
        /// The rejected tool's implementation id.
        descriptor: ToolImplementationId,
        /// The rejected tool's model-visible name.
        exposed_name: String,
        /// Rejection detail from the host installer.
        reason: String,
    },
}

impl std::fmt::Display for BehaviorInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BehaviorInstallError::UnknownPack(id) => write!(f, "unknown behavior pack: {id}"),
            BehaviorInstallError::DuplicatePackId(id) => {
                write!(f, "behavior pack registered twice: {id}")
            }
            BehaviorInstallError::UnsupportedSchemaVersion { pack, got } => {
                write!(f, "pack {pack} declares unsupported schema version {got}")
            }
            BehaviorInstallError::DuplicateToolImplementation { pack, id } => {
                write!(f, "pack {pack} registers tool implementation twice: {id}")
            }
            BehaviorInstallError::DuplicateExposedName {
                exposed_name,
                existing,
                incoming,
            } => write!(
                f,
                "duplicate exposed tool name '{exposed_name}': {existing} vs {incoming} (declare `replaces` for a compatible replacement)"
            ),
            BehaviorInstallError::RequiredExtensionMissing { pack, kind } => {
                write!(
                    f,
                    "pack {pack} requires unavailable extension: {}",
                    kind.slug()
                )
            }
            BehaviorInstallError::RequiredServiceMissing { descriptor, kind } => {
                write!(
                    f,
                    "tool {descriptor} requires unavailable service: {kind:?}"
                )
            }
            BehaviorInstallError::FactoryFailed { descriptor, reason } => {
                write!(f, "tool factory failed for {descriptor}: {reason}")
            }
            BehaviorInstallError::HostRejected {
                exposed_name,
                reason,
                ..
            } => write!(f, "host rejected tool '{exposed_name}': {reason}"),
        }
    }
}

impl std::error::Error for BehaviorInstallError {}

impl BehaviorPack {
    /// Create a pack at schema version 1. `target` pins the compatibility
    /// baseline (release or commit — never a moving "latest").
    pub fn new(id: BehaviorPackId, target: String) -> Self {
        BehaviorPack {
            id,
            schema_version: 1,
            prompt_layers: Vec::new(),
            extensions: Vec::new(),
            tools: Vec::new(),
            compatibility: CompatibilityContract {
                target,
                entries: Vec::new(),
            },
            factories: HashMap::new(),
        }
    }

    /// Add a prompt layer.
    pub fn with_prompt_layer(mut self, spec: PromptLayerSpec) -> Self {
        self.prompt_layers.push(spec);
        self
    }

    /// Add an extension requirement.
    pub fn with_extension(mut self, spec: RuntimeExtensionSpec) -> Self {
        self.extensions.push(spec);
        self
    }

    /// Replace the compatibility contract (used when assembling packs).
    pub fn with_compatibility(mut self, compatibility: CompatibilityContract) -> Self {
        self.compatibility = compatibility;
        self
    }

    /// Register a descriptor together with its factory.
    ///
    /// Duplicate implementation ids within one pack error.
    pub fn with_tool(
        mut self,
        descriptor: BehaviorToolDescriptor,
        factory: ToolFactory,
    ) -> Result<Self, BehaviorInstallError> {
        if self.factories.contains_key(&descriptor.id) {
            return Err(BehaviorInstallError::DuplicateToolImplementation {
                pack: self.id.clone(),
                id: descriptor.id.clone(),
            });
        }
        self.factories.insert(descriptor.id.clone(), factory);
        self.tools.push(descriptor);
        Ok(self)
    }

    pub(crate) fn factory_for(&self, id: &ToolImplementationId) -> Option<ToolFactory> {
        self.factories.get(id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_builder_sets_fields() {
        let d = BehaviorToolDescriptor::new("edit.hashline.v1", "edit")
            .capability(CapabilityClass::FsWrite)
            .side_effect(SideEffectClass::Mutating)
            .state_scope(ToolStateScope::HashlineSession)
            .port(PortRequirementKind::HashlineSnapshotStore, true)
            .essential();
        assert_eq!(d.id.0, "edit.hashline.v1");
        assert!(d.essential);
        assert_eq!(d.required_ports.len(), 1);
        assert!(d.required_ports[0].required);
        assert!(d.replaces.is_none());
    }

    #[test]
    fn with_tool_rejects_duplicate_ids() {
        let pack = BehaviorPack::new(BehaviorPackId("p".to_string()), "omp@test".to_string());
        let mk = || BehaviorToolDescriptor::new("t.v1", "tool");
        let factory: ToolFactory = Arc::new(|_| {
            Err(BehaviorInstallError::FactoryFailed {
                descriptor: ToolImplementationId("t.v1".to_string()),
                reason: "unused".to_string(),
            })
        });
        let pack = pack.with_tool(mk(), factory.clone()).unwrap();
        assert!(pack.with_tool(mk(), factory).is_err());
    }

    #[test]
    fn extension_slug_and_port_agree() {
        assert_eq!(ExtensionKind::ShellSession.slug(), "shell-session");
        assert_eq!(
            ExtensionKind::ShellSession.port(),
            PortRequirementKind::ShellSession
        );
        assert_eq!(
            ExtensionKind::HashlineState.port(),
            PortRequirementKind::HashlineSnapshotStore
        );
    }

    #[test]
    fn error_display_is_informative() {
        let e = BehaviorInstallError::DuplicateExposedName {
            exposed_name: "read".to_string(),
            existing: ToolImplementationId("read.file.v1".to_string()),
            incoming: ToolImplementationId("read.custom.v1".to_string()),
        };
        let text = e.to_string();
        assert!(text.contains("read"));
        assert!(text.contains("replaces"));
    }
}
