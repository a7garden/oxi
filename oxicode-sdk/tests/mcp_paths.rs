//! MCP disk-path customization integration tests.
//!
//! Verifies that `OxicodeBuilder::with_mcp_paths` propagates the supplied
//! cache/consent paths to the spawned `McpManager`, and that the
//! `MetadataCache` re-export resolves from the SDK root.

use std::path::PathBuf;

use oxicode_sdk::{McpConfig, MetadataCache, OxicodeBuilder};
use tempfile::TempDir;

#[tokio::test]
async fn builder_with_mcp_paths_propagates_to_manager() {
    let dir = TempDir::new().unwrap();
    let cache_p: PathBuf = dir.path().join("c.json");
    let consent_p: PathBuf = dir.path().join("consent.json");

    let oxicode = OxicodeBuilder::new()
        .with_mcp_config(McpConfig::default()) // 빈 config → eager 서버 없음
        .with_mcp_paths(cache_p.clone(), consent_p.clone())
        .build();

    let mgr = oxicode.mcp().expect("mcp enabled");
    assert_eq!(mgr.cache().path(), cache_p);
    assert_eq!(mgr.consent().path(), consent_p);
}

#[tokio::test]
async fn builder_without_paths_still_enables_mcp() {
    // 호환성: with_mcp_paths 없이 build → mcp()는 Some (기본 경로 사용).
    let oxicode = OxicodeBuilder::new()
        .with_mcp_config(McpConfig::default())
        .build();
    assert!(oxicode.mcp().is_some());
}

#[tokio::test]
async fn builder_with_mcp_disabled_returns_none() {
    // with_mcp(false)가 paths/config에 우선하여 MCP를 완전히 끈다.
    let dir = TempDir::new().unwrap();
    let oxicode = OxicodeBuilder::new()
        .with_mcp_config(McpConfig::default())
        .with_mcp_paths(dir.path().join("c.json"), dir.path().join("consent.json"))
        .with_mcp(false)
        .build();
    assert!(oxicode.mcp().is_none());
}

#[test]
fn metadata_cache_re_export_resolves() {
    // R3: MetadataCache가 oxicode_sdk 루트에서 접근 가능하고 with_path가 동작한다.
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("c.json");
    let c = MetadataCache::with_path(p.clone());
    assert_eq!(c.path(), p);
}
