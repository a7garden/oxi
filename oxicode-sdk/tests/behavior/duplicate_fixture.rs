//! Overlay/replacement rules exercised end-to-end through resolve + install:
//! duplicate model-visible names need an explicit `replaces` declaration;
//! a declared replacement wins and the replaced tool is never installed.
use crate::common::*;
use oxicode_sdk::behavior::{
    BehaviorInstallError, BehaviorPack, BehaviorPackId, BehaviorToolDescriptor, ToolFactory,
};
use std::sync::Arc;

fn pack_with(id: &str, tool: BehaviorToolDescriptor) -> BehaviorPack {
    let name = tool.exposed_name.clone();
    let factory: ToolFactory = Arc::new(move |_| {
        Ok(Arc::new(StubTool { name: name.clone() }) as Arc<dyn oxicode_agent::AgentTool>)
    });
    BehaviorPack::new(BehaviorPackId(id.to_string()), format!("target-{id}"))
        .with_tool(tool, factory)
        .unwrap()
}

fn read_descriptor(id: &str) -> BehaviorToolDescriptor {
    BehaviorToolDescriptor::new(id, "read")
}

#[test]
fn duplicate_exposed_name_without_replaces_fails_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let services = minimal_services(dir.path());
    let mut resolver = BehaviorPackResolver::with_builtin_packs().unwrap();
    resolver
        .register(pack_with("custom", read_descriptor("read.custom.v1")))
        .unwrap();
    let err = resolver
        .resolve(
            &[
                BehaviorPackId::coding_omp_v1(),
                BehaviorPackId("custom".to_string()),
            ],
            &services,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        BehaviorInstallError::DuplicateExposedName { ref existing, ref incoming, .. }
            if existing.0 == "read.file.v1" && incoming.0 == "read.custom.v1"
    ));
}

#[test]
fn declared_replacement_wins_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    let services = minimal_services(&ws);

    let mut resolver = BehaviorPackResolver::with_builtin_packs().unwrap();
    resolver
        .register(pack_with(
            "custom",
            read_descriptor("read.custom.v1").replaces("read.file.v1"),
        ))
        .unwrap();

    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let resolved = resolver
        .resolve(
            &[
                BehaviorPackId::coding_omp_v1(),
                BehaviorPackId("custom".to_string()),
            ],
            &services,
        )
        .unwrap();
    // Exactly one `read` survives, and it is the replacement.
    let reads: Vec<&BehaviorToolDescriptor> = resolved
        .tools
        .iter()
        .map(|t| &t.descriptor)
        .filter(|d| d.exposed_name == "read")
        .collect();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].id.0, "read.custom.v1");

    let manifest = resolved.install(&services, &mut installer).unwrap();
    assert_eq!(manifest.packs.len(), 2);
    // Installer saw exactly one `read` — the replacement, not the original.
    assert_eq!(
        installer.installed.iter().filter(|n| *n == "read").count(),
        1
    );
    // The replaced implementation never reached the installer.
    assert!(!installer.installed.iter().any(|n| n == "read.file.v1"));
    let _ = installer.into_registry();
}
