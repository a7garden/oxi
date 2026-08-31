//! Behavior packs — portable, versioned coding-behavior contracts.
//!
//! A pack is a declarative contract (tool descriptors, runtime extension
//! requirements, prompt layers, compatibility ledger) plus a canonical
//! installer that hands tools to a host-controlled interception point.
//!
//! Design: `docs/designs/2026-08-31-omp-compatible-behavior-pack-design.md`.

pub mod installer;
pub mod ledger;
pub mod packs;
pub mod resolver;
pub mod types;
pub use installer::{
    AgentConfigPatch, BehaviorSessionServices, BehaviorToolInstaller, DegradationReason,
    DegradationRecord, InstalledBehaviorManifest, InstalledToolRecord,
};
pub use ledger::{CompatibilityContract, FeatureStatus, LedgerEntry};
pub use types::{
    BehaviorInstallError, BehaviorPack, BehaviorPackId, BehaviorToolDescriptor, CapabilityClass,
    ExtensionKind, ExtensionScope, PortRequirement, PortRequirementKind, PromptLayerSpec,
    RuntimeExtensionSpec, SideEffectClass, ToolFactory, ToolImplementationId, ToolStateScope,
};

pub use resolver::{BehaviorPackResolver, ResolvedBehavior, ResolvedTool};
