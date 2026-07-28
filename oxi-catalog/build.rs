//! Build script for oxi-catalog.
//!
//! The catalog owns the embedded models.dev snapshot
//! (`data/catalog/_snapshot.json.gz`), included via `include_bytes!` at
//! compile time in `catalog/materialize.rs`. No runtime IO is needed for the
//! catalog.
//!
//! # CI / reproducible builds
//!
//! Set `OXI_CATALOG_SNAPSHOT=path` to inject a specific snapshot file
//! (e.g., from CI artifacts). Otherwise, the committed `_snapshot.json.gz`
//! is used by default.
//!
//! Model data © [models.dev](https://models.dev) (MIT).

use std::env;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");

    // Optional snapshot override (CI / release builds).
    // If OXI_CATALOG_SNAPSHOT is set, copy that file over the committed
    // snapshot so the `include_bytes!` in `catalog/materialize.rs` picks it up.
    // This lets CI inject a specific snapshot (e.g., from a fresh fetch)
    // without modifying the repo.
    if let Ok(custom_path) = env::var("OXI_CATALOG_SNAPSHOT") {
        let custom = Path::new(&custom_path);
        assert!(
            custom.exists(),
            "OXI_CATALOG_SNAPSHOT not found: {}",
            custom.display()
        );
        let committed = Path::new(&manifest_dir).join("data/catalog/_snapshot.json.gz");
        if custom.canonicalize().ok() != committed.canonicalize().ok() {
            std::fs::copy(custom, &committed)
                .unwrap_or_else(|e| panic!("failed to copy snapshot: {e}"));
            println!(
                "cargo:warning=OXI_CATALOG_SNAPSHOT applied: {}",
                custom.display()
            );
        }
        println!("cargo:rerun-if-env-changed=OXI_CATALOG_SNAPSHOT");
    }

    // Ensure the committed snapshot exists (it should — it's checked into git).
    // The `include_bytes!` in `catalog/materialize.rs` would also fail at
    // compile time if missing; this assert gives a clearer message.
    let committed = Path::new(&manifest_dir).join("data/catalog/_snapshot.json.gz");
    assert!(
        committed.exists(),
        "Committed snapshot not found at {}. Run `gzip -c < api.json > data/catalog/_snapshot.json.gz`.",
        committed.display()
    );

    println!("cargo:rerun-if-changed=data/catalog/_snapshot.json.gz");
    println!("cargo:rerun-if-changed=build.rs");
}
