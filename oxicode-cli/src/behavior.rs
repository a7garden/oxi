//! CLI behavior-pack composition (`coding-omp-v1` reference consumer).
//!
//! The pack provides the canonical coding tool set through the SDK's host
//! installer interception point; the CLI registers pack tools as-is to
//! preserve today's composition. Hosts that need per-tool audit/approval
//! wrap the tool in their [`oxicode_sdk::behavior::BehaviorToolInstaller`]
//! before registration (design "Host policy boundary").

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use oxicode_agent::{AgentTool, ToolRegistry};
use oxicode_hashline::{InMemorySnapshotStore, SnapshotStore};
use oxicode_sdk::behavior::{
    AgentConfigPatch, BehaviorInstallError, BehaviorPackId, BehaviorPackResolver,
    BehaviorSessionServices, BehaviorToolDescriptor, BehaviorToolInstaller,
    InstalledBehaviorManifest,
};

/// Manifest plus the requested AgentConfig patch produced by installing packs.
#[derive(Clone)]
pub struct BehaviorComposition {
    /// What was actually installed (and degraded).
    pub manifest: InstalledBehaviorManifest,
    /// Requested config adjustments; the host validates before applying.
    pub patch: AgentConfigPatch,
}

struct CliToolInstaller<'a> {
    tools: &'a ToolRegistry,
}

impl BehaviorToolInstaller for CliToolInstaller<'_> {
    fn install(
        &mut self,
        descriptor: &BehaviorToolDescriptor,
        tool: Arc<dyn AgentTool>,
    ) -> Result<(), BehaviorInstallError> {
        self.tools.register_arc(tool);
        tracing::debug!(
            tool = %descriptor.exposed_name,
            id = %descriptor.id.0,
            "behavior pack tool installed"
        );
        Ok(())
    }
}

/// Install `coding-omp-v1` into `tools`, overwriting the legacy instances of
/// the same names with pack-constructed equivalents.
///
/// `allow` mirrors the `--tools` filter (already split and trimmed): pack
/// tools not named are host-disabled. `disabled_tools` mirrors the
/// `--no-*`/settings disable list. Returns `None` on resolution/install
/// failure — the legacy builtin composition keeps running (logged loudly).
pub fn install_coding_omp_v1(
    tools: &ToolRegistry,
    cwd: &Path,
    allow: Option<&[String]>,
    disabled_tools: &[String],
) -> Option<BehaviorComposition> {
    let resolver = match BehaviorPackResolver::with_builtin_packs() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("behavior pack registry failed: {e}");
            return None;
        }
    };
    let pack_id = BehaviorPackId::coding_omp_v1();
    let Some(pack) = resolver.pack(&pack_id) else {
        tracing::warn!("coding-omp-v1 missing from builtin packs");
        return None;
    };
    let mut disabled: Vec<String> = disabled_tools.to_vec();
    if let Some(allow) = allow {
        let allowed: HashSet<&str> = allow.iter().map(String::as_str).collect();
        for t in &pack.tools {
            if !allowed.contains(t.exposed_name.as_str()) {
                disabled.push(t.exposed_name.clone());
            }
        }
    }
    let services = BehaviorSessionServices::new(cwd.to_path_buf())
        .with_snapshot_store(Arc::new(InMemorySnapshotStore::new()) as Arc<dyn SnapshotStore>)
        .with_disabled_tools(disabled);
    let resolved = match resolver.resolve(&[pack_id], &services) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("behavior pack resolve failed: {e}");
            return None;
        }
    };
    let patch = resolved.patch.clone();
    let mut installer = CliToolInstaller { tools };
    match resolved.install(&services, &mut installer) {
        Ok(manifest) => Some(BehaviorComposition { manifest, patch }),
        Err(e) => {
            tracing::warn!("behavior pack install failed: {e}");
            None
        }
    }
}

#[cfg(test)]
mod behavior_tests {
    use super::*;
    use oxicode_sdk::behavior::{DegradationReason, FeatureStatus};

    #[test]
    fn pack_names_are_subset_of_legacy_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = ToolRegistry::with_builtins_cwd(tmp.path().to_path_buf(), &[]);
        let names: HashSet<String> = legacy.names().into_iter().collect();
        let resolver = BehaviorPackResolver::with_builtin_packs().unwrap();
        let pack = resolver.pack(&BehaviorPackId::coding_omp_v1()).unwrap();
        for t in &pack.tools {
            assert!(
                names.contains(&t.exposed_name),
                "pack tool '{}' missing from legacy builtins",
                t.exposed_name
            );
        }
    }

    #[test]
    fn composition_installs_manifest_and_overwrites_names() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ToolRegistry::new();
        let comp =
            install_coding_omp_v1(&registry, tmp.path(), None, &[]).expect("install succeeds");
        assert_eq!(comp.manifest.packs, vec![BehaviorPackId::coding_omp_v1()]);
        assert_eq!(comp.manifest.tools.len(), 16);
        for t in &comp.manifest.tools {
            assert!(
                registry.get(&t.exposed_name).is_some(),
                "{} not registered",
                t.exposed_name
            );
        }
        let degraded: HashSet<&str> = comp
            .manifest
            .degraded
            .iter()
            .map(|d| d.feature.as_str())
            .collect();
        let expected: HashSet<&str> = [
            "shell-session",
            "eval-kernel",
            "debug-service",
            "ttsr-engine",
            "lsp-host",
            "delegation",
        ]
        .into();
        assert_eq!(degraded, expected);
        assert_eq!(comp.manifest.compatibility_level(), FeatureStatus::Partial);
        assert_eq!(comp.patch.prompt_layers.len(), 1);
    }

    #[test]
    fn allow_filter_disables_unselected_pack_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ToolRegistry::new();
        let allow = vec!["read".to_string(), "grep".to_string()];
        let comp = install_coding_omp_v1(&registry, tmp.path(), Some(&allow), &[])
            .expect("install succeeds");
        assert!(registry.get("read").is_some() && registry.get("grep").is_some());
        assert!(
            registry.get("bash").is_none(),
            "non-allowed tools must not be installed"
        );
        assert!(
            comp.manifest
                .degraded
                .iter()
                .any(|d| matches!(d.reason, DegradationReason::DisabledByHost))
        );
    }
}
