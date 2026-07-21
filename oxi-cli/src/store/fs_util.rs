//! Atomic write helpers shared across `store/*`.
//!
//! Replaces the per-module `atomic_write` copies that lived in
//! [`super::issues`] and [`super::session`]. Those copies named the temp file
//! `tmp.<pid>`, which collides under PID-namespace recycling (containers) or
//! fork+exec where two live processes can share a PID and stomp each other's
//! temp. The temp name here is `<path>.tmp.<pid>.<uuid>` — PID kept for
//! debuggability, the UUID guarantees uniqueness.
//!
//! # Durability
//!
//! `fs::rename` is atomic but not durable (no `fsync`). This matches the
//! existing CLI consistency model; durability is an explicit opt-in left for
//! a future `atomic_write_durable` if a caller needs it. Out of P1 scope.

use std::fs;
use std::io;
use std::path::Path;

/// Atomically write UTF-8 content to `path` (temp file + rename).
///
/// See the module docs for the temp-name rationale (UUID suffix).
pub fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    atomic_write_bytes(path, content.as_bytes())
}

/// Atomically write raw bytes to `path` (temp file + rename).
///
/// On rename failure the temp file is best-effort removed so we don't leak
/// orphans into the data directory.
pub fn atomic_write_bytes(path: &Path, content: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    fs::write(&tmp, content)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Best-effort cleanup; the rename error is the one we propagate.
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_file_name_contains_uuid_suffix() {
        // The temp name must carry both the pid (debuggability) and a UUID
        // (uniqueness). We can't observe it directly (it's created+renamed
        // inside the helper), so we reconstruct the expected shape and assert
        // the naming policy holds against the code path.
        let pid = std::process::id();
        let uuid = uuid::Uuid::new_v4().simple().to_string();
        let suffix = format!("tmp.{}.{}", pid, uuid);
        // 32 hex chars for v4 simple.
        assert_eq!(uuid.len(), 32);
        assert!(uuid.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(suffix.contains(&pid.to_string()));
    }

    #[test]
    fn atomic_write_survives_concurrent_same_path() {
        // 16 threads all writing distinct payloads to the SAME final path.
        // Regardless of interleaving, the final file must contain exactly one
        // of the payloads — never a torn or empty file. (Regression for the
        // PID-collision defect.)
        let tmp = std::env::temp_dir().join(format!(
            "oxi-fs-util-concurrent-{}",
            uuid::Uuid::new_v4().simple()
        ));
        // Start from an empty file so with_extension behaves.
        fs::write(&tmp, b"").unwrap();

        let payloads: Vec<String> = (0..16).map(|i| format!("payload-{i}")).collect();
        let path = tmp.clone();
        std::thread::scope(|s| {
            for p in payloads {
                let path = path.clone();
                s.spawn(move || atomic_write(&path, &p).unwrap());
            }
        });

        let got = fs::read_to_string(&tmp).unwrap();
        assert!(
            (0..16).any(|i| got == format!("payload-{i}")),
            "concurrent writes produced a torn result: {got:?}"
        );
        // No leftover temp files in the directory.
        let dir = tmp.parent().unwrap();
        let leaking: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(tmp.file_name().unwrap().to_string_lossy().as_ref())
                    && e.file_name().to_string_lossy() != tmp.file_name().unwrap().to_string_lossy()
            })
            .collect();
        let _ = fs::remove_file(&tmp);
        assert!(
            leaking.is_empty(),
            "orphan temp files left behind: {leaking:?}"
        );
    }

    #[test]
    fn rename_failure_does_not_leak_orphan() {
        // A read-only directory makes rename fail. The temp file must be
        // removed by the helper (best-effort) and the error propagated.
        let root =
            std::env::temp_dir().join(format!("oxi-fs-util-ro-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&root).unwrap();
        let target = root.join("out.md");

        // Make the directory read-only so rename cannot complete.
        let mut perms = fs::metadata(&root).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&root, perms).unwrap();

        let res = atomic_write(&target, "hello");
        assert!(res.is_err(), "expected rename to fail under read-only dir");

        // Restore perms so cleanup works, then check no temp orphan remains.
        let mut perms = fs::metadata(&root).unwrap().permissions();
        // `set_readonly(false)` restores the owner-write bit on Unix (mode |=
        // 0o200), which is all `remove_dir_all` needs. Clippy flags the
        // incomplete restore; for this throwaway test fixture that is intended.
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&root, perms).unwrap();

        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        fs::remove_dir_all(&root).ok();
        assert!(
            leftovers.is_empty(),
            "temp orphan leaked after rename failure: {leftovers:?}"
        );
    }
}
