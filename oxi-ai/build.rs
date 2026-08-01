//! Build script for oxi-ai.
//!
//! Compiles the Cursor agent proto (`proto/cursor/agent.proto`, package
//! `agent.v1`, 492 messages) into Rust types via `prost-build`. The proto is
//! self-contained (proto3, no imports, no `google.protobuf` deps), so no
//! include path beyond its own directory is needed.
//!
//! `protoc-bin-vendored` bundles a `protoc` binary so the build does not
//! depend on a system-installed `protoc` (CI images have it, local machines
//! may not). It is wired in by setting the `PROTOC` env var before
//! `compile_protos`, which prost-build honours. This keeps the build hermetic.
//!
//! prost-build names the output file after the proto package — `package
//! agent.v1` → `OUT_DIR/agent.v1.rs`. The provider includes it via
//! `include!(concat!(env!("OUT_DIR"), "/agent.v1.rs"))`.
//!
//! Re-generation is gated on `cargo:rerun-if-changed` for the proto file and
//! this script, so incremental builds skip recompilation when neither moved.

// When the `protobuf` feature is off, `prost-build` and `protoc-bin-vendored`
// are not in the build script's dependency closure, so any reference to them
// is a hard compile error. We isolate the proto-compilation path in a
// #[cfg]-gated inline module so the compiler never even sees the symbols
// when the feature is off.
#[cfg(feature = "protobuf")]
mod proto_build {
    use std::env;
    use std::path::PathBuf;

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
        let proto_dir = manifest_dir.join("proto/cursor");
        let proto_file = proto_dir.join("agent.proto");

        assert!(
            proto_file.exists(),
            "Cursor agent.proto missing at {}",
            proto_file.display()
        );

        // Bundle protoc so the build is hermetic (no system protoc required).
        // prost-build honours the `PROTOC` env var to locate the protoc binary.
        let protoc_path = protoc_bin_vendored::protoc_bin_path()
            .expect("protoc-bin-vendored failed to locate bundled protoc");
        // SAFETY: build scripts are single-threaded; no concurrent env access.
        unsafe {
            env::set_var("PROTOC", &protoc_path);
        }

        let mut config = prost_build::Config::new();
        // proto3 optional needs the experimental flag on older protoc; harmless on new.
        config.protoc_arg("--experimental_allow_proto3_optional");

        config.compile_protos(&[&proto_file], &[&proto_dir])?;

        println!("cargo:rerun-if-changed={}", proto_file.display());
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Always rerun if this script changes.
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(feature = "protobuf")]
    {
        proto_build::run()?;
    }

    Ok(())
}
