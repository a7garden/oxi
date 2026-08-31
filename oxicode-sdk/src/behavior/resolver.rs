//! Deterministic behavior-pack resolution: pack registry, overlay
//! replacement rules, and the `ResolvedBehavior` install entry point.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use super::installer::{
    AgentConfigPatch, BehaviorSessionServices, BehaviorToolInstaller, DegradationRecord,
    InstalledBehaviorManifest, install_descriptors,
};
use super::ledger::CompatibilityContract;
use super::types::{
    BehaviorInstallError, BehaviorPack, BehaviorPackId, BehaviorToolDescriptor, PromptLayerSpec,
    ToolFactory, ToolImplementationId,
};

/// Deterministic pack registry and resolution entry point.
#[derive(Default)]
pub struct BehaviorPackResolver {
    packs: BTreeMap<BehaviorPackId, BehaviorPack>,
}

/// One resolved tool: its (possibly overlay-replaced) descriptor and factory.
#[derive(Clone)]
pub struct ResolvedTool {
    /// The winning descriptor.
    pub descriptor: BehaviorToolDescriptor,
    /// How the canonical tool is constructed at install time.
    pub factory: ToolFactory,
}

/// Resolution output: validated descriptors plus the requested config patch.
///
/// Installation happens later, through the host's installer — resolution
/// never constructs tools.
pub struct ResolvedBehavior {
    /// Resolved pack ids, request order.
    pub packs: Vec<BehaviorPackId>,
    /// Winning descriptors in resolve order (replacements applied).
    pub tools: Vec<ResolvedTool>,
    /// Requested `AgentConfig` adjustments for host validation.
    pub patch: AgentConfigPatch,
    /// Prompt layers in pack order.
    pub prompt_layers: Vec<PromptLayerSpec>,
    /// Degradations recorded during resolution (currently none — degradations
    /// are computed at install time; kept for manifest aggregation symmetry).
    pub degradations: Vec<DegradationRecord>,
    /// Merged compatibility contract.
    pub compatibility: CompatibilityContract,
    /// Packs backing this resolution, resolve order (private: install input).
    resolved_packs: Vec<Arc<BehaviorPack>>,
}

/// Manual [`Debug`]: tool factories are opaque closures.
impl std::fmt::Debug for ResolvedBehavior {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBehavior")
            .field("packs", &self.packs)
            .field(
                "tools",
                &self
                    .tools
                    .iter()
                    .map(|t| t.descriptor.exposed_name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("patch", &self.patch)
            .field(
                "prompt_layers",
                &self
                    .prompt_layers
                    .iter()
                    .map(|l| l.id.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("degradations", &self.degradations)
            .field("compatibility", &self.compatibility)
            .field("resolved_packs", &self.resolved_packs.len())
            .finish()
    }
}

impl BehaviorPackResolver {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pack. Duplicate ids error.
    pub fn register(&mut self, pack: BehaviorPack) -> Result<(), BehaviorInstallError> {
        if self.packs.contains_key(&pack.id) {
            return Err(BehaviorInstallError::DuplicatePackId(pack.id));
        }
        self.packs.insert(pack.id.clone(), pack);
        Ok(())
    }

    /// Resolver preloaded with every builtin reference pack.
    pub fn with_builtin_packs() -> Result<Self, BehaviorInstallError> {
        let mut resolver = Self::new();
        resolver.register(super::packs::coding_omp_v1::pack()?)?;
        Ok(resolver)
    }

    /// Look up a registered pack.
    pub fn pack(&self, id: &BehaviorPackId) -> Option<&BehaviorPack> {
        self.packs.get(id)
    }

    /// Resolve `requested` packs in request order (duplicates deduplicated).
    ///
    /// Overlay rule: a later descriptor may replace an earlier one only via
    /// an explicit `replaces` declaration naming the replaced implementation
    /// id; any other duplicate model-visible name errors.
    pub fn resolve(
        &self,
        requested: &[BehaviorPackId],
        services: &BehaviorSessionServices,
    ) -> Result<ResolvedBehavior, BehaviorInstallError> {
        let mut order: Vec<BehaviorPackId> = Vec::new();
        for id in requested {
            if !order.contains(id) {
                order.push(id.clone());
            }
        }
        let mut resolved_packs: Vec<Arc<BehaviorPack>> = Vec::new();
        let mut tools: Vec<ResolvedTool> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut replaced_ids: HashSet<ToolImplementationId> = HashSet::new();
        let mut compatibility: Option<CompatibilityContract> = None;
        let mut prompt_layers: Vec<PromptLayerSpec> = Vec::new();
        for id in &order {
            let pack = self
                .packs
                .get(id)
                .ok_or_else(|| BehaviorInstallError::UnknownPack(id.clone()))?;
            if pack.schema_version != 1 {
                return Err(BehaviorInstallError::UnsupportedSchemaVersion {
                    pack: id.clone(),
                    got: pack.schema_version,
                });
            }
            for d in &pack.tools {
                if let Some(existing_id) = d.replaces.as_ref()
                    && let Some(pos) = tools.iter().position(|t| &t.descriptor.id == existing_id)
                {
                    tools.remove(pos);
                    replaced_ids.insert(existing_id.clone());
                    by_name.retain(|_, v| {
                        if *v > pos {
                            *v -= 1;
                            true
                        } else {
                            *v != pos
                        }
                    });
                }
                if let Some(existing) = by_name.get(&d.exposed_name) {
                    return Err(BehaviorInstallError::DuplicateExposedName {
                        exposed_name: d.exposed_name.clone(),
                        existing: tools[*existing].descriptor.id.clone(),
                        incoming: d.id.clone(),
                    });
                }
                let factory =
                    pack.factory_for(&d.id)
                        .ok_or_else(|| BehaviorInstallError::FactoryFailed {
                            descriptor: d.id.clone(),
                            reason: "no factory registered".to_string(),
                        })?;
                by_name.insert(d.exposed_name.clone(), tools.len());
                tools.push(ResolvedTool {
                    descriptor: d.clone(),
                    factory,
                });
            }
            compatibility = Some(match compatibility {
                Some(c) => c.merge(&pack.compatibility),
                None => pack.compatibility.clone(),
            });
            prompt_layers.extend(pack.prompt_layers.iter().cloned());
            resolved_packs.push(Arc::new(pack.clone()));
        }
        let patch = AgentConfigPatch {
            snapshot_store: services.snapshot_store.clone(),
            lsp: services.lsp.clone(),
            ttsr_engine: services.ttsr_engine.clone(),
            url_resolver: services.url_resolver.clone(),
            subagent_runner: services.subagent_runner.clone(),
            memory: services.memory.clone(),
            todo: services.todo.clone(),
            prompt_layers: prompt_layers.clone(),
        };
        Ok(ResolvedBehavior {
            packs: order,
            tools,
            patch,
            degradations: Vec::new(),
            compatibility: compatibility.unwrap_or(CompatibilityContract {
                target: String::new(),
                entries: Vec::new(),
            }),
            resolved_packs,
            prompt_layers,
        })
    }
}

impl ResolvedBehavior {
    /// Observable contract: exactly one installer call per resolved tool, in
    /// resolve order; degradation and error semantics identical to
    /// `BehaviorPack::install`; the manifest aggregates all packs.
    pub fn install(
        &self,
        services: &BehaviorSessionServices,
        installer: &mut dyn BehaviorToolInstaller,
    ) -> Result<InstalledBehaviorManifest, BehaviorInstallError> {
        let mut factories: HashMap<ToolImplementationId, ToolFactory> = HashMap::new();
        let mut extensions = Vec::new();
        for pack in &self.resolved_packs {
            factories.extend(pack.factories.iter().map(|(k, v)| (k.clone(), v.clone())));
            extensions.extend(pack.extensions.iter().cloned());
        }
        let descriptors: Vec<BehaviorToolDescriptor> =
            self.tools.iter().map(|t| t.descriptor.clone()).collect();
        install_descriptors(
            self.packs.clone(),
            1,
            self.prompt_layers.iter().map(|l| l.id.clone()).collect(),
            self.compatibility.clone(),
            &extensions,
            &descriptors,
            &factories,
            services,
            installer,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior::types::{CapabilityClass, SideEffectClass};
    use oxicode_agent::AgentTool;

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

    fn factory(name: String) -> ToolFactory {
        Arc::new(move |_| {
            let name = name.clone();
            Ok(Arc::new(TestTool { name }) as Arc<dyn AgentTool>)
        })
    }

    fn pack_with(id: &str, tool: BehaviorToolDescriptor) -> BehaviorPack {
        let name = tool.exposed_name.clone();
        BehaviorPack::new(BehaviorPackId(id.to_string()), format!("target-{id}"))
            .with_tool(tool, factory(name))
            .unwrap()
    }

    fn read_descriptor(id: &str) -> BehaviorToolDescriptor {
        BehaviorToolDescriptor::new(id, "read")
            .capability(CapabilityClass::FsRead)
            .side_effect(SideEffectClass::ReadOnly)
    }

    fn services() -> BehaviorSessionServices {
        BehaviorSessionServices::new(std::env::temp_dir())
    }

    #[test]
    fn resolve_unknown_pack_errors() {
        let resolver = BehaviorPackResolver::new();
        let err = resolver
            .resolve(&[BehaviorPackId("nope".to_string())], &services())
            .unwrap_err();
        assert!(matches!(err, BehaviorInstallError::UnknownPack(_)));
    }

    #[test]
    fn duplicate_exposed_name_requires_replaces() {
        let mut resolver = BehaviorPackResolver::new();
        resolver
            .register(pack_with("a", read_descriptor("read.file.v1")))
            .unwrap();
        resolver
            .register(pack_with("b", read_descriptor("read.custom.v1")))
            .unwrap();
        let err = resolver
            .resolve(
                &[
                    BehaviorPackId("a".to_string()),
                    BehaviorPackId("b".to_string()),
                ],
                &services(),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            BehaviorInstallError::DuplicateExposedName { ref existing, ref incoming, .. }
                if existing.0 == "read.file.v1" && incoming.0 == "read.custom.v1"
        ));
    }

    #[test]
    fn declared_replacement_wins_and_old_tool_dropped() {
        let mut resolver = BehaviorPackResolver::new();
        resolver
            .register(pack_with("a", read_descriptor("read.file.v1")))
            .unwrap();
        resolver
            .register(pack_with(
                "b",
                read_descriptor("read.custom.v1").replaces("read.file.v1"),
            ))
            .unwrap();
        let resolved = resolver
            .resolve(
                &[
                    BehaviorPackId("a".to_string()),
                    BehaviorPackId("b".to_string()),
                ],
                &services(),
            )
            .unwrap();
        assert_eq!(resolved.tools.len(), 1);
        assert_eq!(resolved.tools[0].descriptor.id.0, "read.custom.v1");
        let mut installer = RecordingInstaller::default();
        let manifest = resolved.install(&services(), &mut installer).unwrap();
        assert_eq!(manifest.packs.len(), 2);
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.tools[0].descriptor.0, "read.custom.v1");
        assert_eq!(
            manifest.compatibility.target, "target-a + target-b",
            "contracts merge in resolve order"
        );
    }

    #[test]
    fn patch_carries_host_services_and_prompt_layers() {
        let mut resolver = BehaviorPackResolver::new();
        resolver
            .register(
                BehaviorPack::new(BehaviorPackId("p".to_string()), "t".to_string())
                    .with_prompt_layer(PromptLayerSpec {
                        id: "l1".to_string(),
                        body: "b".to_string(),
                    })
                    .with_tool(read_descriptor("read.file.v1"), factory("read".to_string()))
                    .unwrap(),
            )
            .unwrap();
        let resolved = resolver
            .resolve(&[BehaviorPackId("p".to_string())], &services())
            .unwrap();
        assert!(resolved.patch.snapshot_store.is_none());
        assert_eq!(resolved.patch.prompt_layers.len(), 1);
        assert_eq!(resolved.patch.prompt_layers[0].id, "l1");
        assert_eq!(resolved.prompt_layers.len(), 1);
    }

    #[derive(Default)]
    struct RecordingInstaller {
        installed: Vec<String>,
    }

    impl BehaviorToolInstaller for RecordingInstaller {
        fn install(
            &mut self,
            descriptor: &BehaviorToolDescriptor,
            _tool: Arc<dyn AgentTool>,
        ) -> Result<(), BehaviorInstallError> {
            self.installed.push(descriptor.exposed_name.clone());
            Ok(())
        }
    }
}
