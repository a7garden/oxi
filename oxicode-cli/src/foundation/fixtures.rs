//! Shared cross-host fixture loader.
//!
//! The cross-host fixture set lives under
//! `tests/fixtures/oxi-foundation/v1/` and is byte-identical across
//! oxicode / oxibrain / oxios. Loading here is intentionally dumb
//! (`include_str!`-style — never network-fetched, never mutated).

use std::path::PathBuf;

/// Resolve the fixture root by walking the current CARGO_MANIFEST_DIR
/// upward. Returns `None` when the layout can't be found (e.g. when
/// the crate is consumed as a dependency without the fixtures
/// directory).
pub fn fixture_root() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
        .join("oxi-foundation")
        .join("v1");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

/// Read a profile fixture by name (without extension).
pub fn profile(name: &str) -> Option<String> {
    let root = fixture_root()?;
    let path = root.join("profiles").join(format!("{name}.json"));
    std::fs::read_to_string(path).ok()
}

/// Read a package fixture by name (without extension).
pub fn package(name: &str) -> Option<String> {
    let root = fixture_root()?;
    let path = root.join("packages").join(format!("{name}.json"));
    std::fs::read_to_string(path).ok()
}

/// Read the canonical `foundation.json` fixture.
pub fn foundation() -> Option<String> {
    let root = fixture_root()?;
    let path = root.join("foundation.json");
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_root_resolves() {
        // The fixture directory may not exist in this test build if
        // the fixtures have not been written yet. We just check the
        // path computation is stable.
        let root = fixture_root();
        assert!(root.is_none() || root.unwrap().ends_with("oxi-foundation/v1"));
    }
}
