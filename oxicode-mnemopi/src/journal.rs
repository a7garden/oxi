//! Filesystem-aware SQLite journal-mode selection.
//!
//! Ported from grok-build `xai-sqlite-journal` (Apache-2.0).
//!
//! WAL keeps its wal-index in an mmap'd `-shm` file and relies on coherent
//! shared memory plus reliable POSIX locks — guarantees network filesystems
//! do not provide. When `$HOME` (and thus `~/.oxicode`) is NFS-mounted on
//! several machines at once, a peer host truncating/rebuilding the `-shm`
//! during WAL recovery or close rips the backing out from under our mapping
//! and the next wal-index read dies with SIGBUS. On such mounts we use a
//! rollback journal instead (SQLite's documented "WAL does not work over
//! a network filesystem" limitation), and each host opens its own per-host
//! DB file so no peer — including pre-fix binaries that would flip a shared
//! DB back to WAL — ever shares the file.
//!
//! ## Gate
//!
//! `OXICODE_SQLITE_JOURNAL_MODE=wal|truncate` overrides filesystem detection.
//! An invalid value logs a warning and falls back to detection.

use std::ffi::CString;
use std::path::{Path, PathBuf};

/// Wait for peers' locks instead of failing instantly; matches what every
/// consumer historically set.
const BUSY_TIMEOUT_MS: u32 = 5000;

/// Kill-switch env var name.
pub const ENV_VAR: &str = "OXICODE_SQLITE_JOURNAL_MODE";

/// Journal mode chosen for a SQLite database based on where it lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalMode {
    /// Write-ahead logging — the historical default, local filesystems only.
    Wal,
    /// Rollback journal truncated (not unlinked) at commit — safe on network
    /// filesystems, and cheaper there than DELETE mode: no per-commit
    /// create/unlink namespace round-trips and no NFS `.nfsXXXX`
    /// silly-rename litter.
    Truncate,
}

impl JournalMode {
    /// Pick the journal mode for a database at `db_path`.
    ///
    /// Classifies the parent directory (the DB file itself may not exist
    /// yet), so callers must create it first. `OXICODE_SQLITE_JOURNAL_MODE`
    /// (`wal`|`truncate`) overrides detection as a field kill-switch.
    pub fn for_db_path(db_path: &Path) -> Self {
        match mode_from_env() {
            EnvOverride::Mode(mode) => {
                tracing::info!(
                    db = %db_path.display(),
                    mode = mode.as_str(),
                    source = "env",
                    env = ENV_VAR,
                    "sqlite journal mode forced"
                );
                mode
            }
            EnvOverride::Invalid(value) => {
                tracing::warn!(
                    value = %value,
                    env = ENV_VAR,
                    accepted = "wal, truncate",
                    "invalid journal-mode override; using detection"
                );
                Self::detect(db_path)
            }
            EnvOverride::Unset => Self::detect(db_path),
        }
    }

    fn detect(db_path: &Path) -> Self {
        let dir = match db_path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let mode = if is_network_fs(dir) {
            Self::Truncate
        } else {
            Self::Wal
        };
        tracing::debug!(
            db = %db_path.display(),
            mode = mode.as_str(),
            source = "statfs",
            "sqlite journal mode"
        );
        mode
    }

    /// The `PRAGMA journal_mode` value for this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wal => "WAL",
            Self::Truncate => "TRUNCATE",
        }
    }

    /// The `PRAGMA busy_timeout` value (ms) paired with this mode.
    pub fn busy_timeout_ms(self) -> u32 {
        BUSY_TIMEOUT_MS
    }

    /// The path actually opened for `db_path` under this mode.
    ///
    /// **Safe default for durable stores.** Always returns `db_path`
    /// unchanged — the file is shared across hosts so concurrent readers
    /// and writers (under `busy_timeout`) see the same data. Only the
    /// `PRAGMA journal_mode` is adjusted (WAL on local FS, TRUNCATE on
    /// network FS to avoid SIGBUS from mmap'd `-shm`).
    ///
    /// For rebuildable caches where each host may safely start fresh,
    /// use [`Self::per_host_db_path`] instead — it rewrites to a
    /// per-host sibling on network filesystems so old binaries that
    /// would flip a shared DB back to WAL cannot corrupt the no-WAL
    /// invariant.
    pub fn effective_db_path(&self, db_path: &Path) -> PathBuf {
        // Durable-safe: never rewrite. Only the journal mode changes.
        db_path.to_path_buf()
    }

    /// Per-host DB path for rebuildable caches.
    ///
    /// `Wal` (local): unchanged. `Truncate` (network): a per-host sibling
    /// (`worktrees.db` → `worktrees.h-<host>.db`). Journal mode is a
    /// database-wide property, so a live pre-fix binary on a peer host (or
    /// this host) can flip a *shared* DB back to WAL at any time and our
    /// long-lived connections would silently adopt it, re-creating the
    /// mmap'd `-shm`. Old binaries never know the per-host name, so the
    /// no-WAL invariant — and the end of cross-host sharing, the root
    /// hazard — holds by construction.
    ///
    /// **Only use this for rebuildable caches** (indexes, job queues,
    /// materialized views). For durable primary stores where data must
    /// be coherent across hosts, use [`Self::effective_db_path`] — it
    /// keeps the shared path and accepts TRUNCATE-only safety.
    ///
    /// Idempotent (an already-suffixed path is returned unchanged) so
    /// callers may pre-resolve the path for sidecar file operations.
    /// Falls back to `db_path` unchanged (still TRUNCATE) if no hostname
    /// is available.
    pub fn per_host_db_path(&self, db_path: &Path) -> PathBuf {
        match self {
            Self::Wal => db_path.to_path_buf(),
            Self::Truncate => per_host_path(db_path).unwrap_or_else(|| db_path.to_path_buf()),
        }
    }
}

enum EnvOverride {
    Mode(JournalMode),
    Invalid(String),
    Unset,
}

fn mode_from_env() -> EnvOverride {
    match std::env::var(ENV_VAR) {
        Ok(raw) => {
            let trimmed = raw.trim();
            match trimmed {
                "wal" | "WAL" => EnvOverride::Mode(JournalMode::Wal),
                "truncate" | "TRUNCATE" => EnvOverride::Mode(JournalMode::Truncate),
                _ => EnvOverride::Invalid(raw),
            }
        }
        Err(_) => EnvOverride::Unset,
    }
}

/// Build the per-host DB path. Returns `None` if the hostname is unavailable
/// or the input has no file stem to suffix.
fn per_host_path(db_path: &Path) -> Option<PathBuf> {
    let host = hostname()?;
    let stem = db_path.file_stem()?.to_str()?;
    // Idempotency: skip paths already suffixed.
    if stem.starts_with("h-") || stem.contains(".h-") {
        return Some(db_path.to_path_buf());
    }
    let ext = db_path.extension();
    let parent = db_path.parent();

    let mut name = format!("{stem}.h-{host}");
    if let Some(ext) = ext {
        name.push('.');
        name.push_str(ext.to_str().unwrap_or_default());
    }

    let mut path = parent.map(|p| p.to_path_buf()).unwrap_or_default();
    path.push(name);
    Some(path)
}

/// Best-effort hostname (short form). Returns `None` on any failure —
/// callers fall back to the shared path.
fn hostname() -> Option<String> {
    #[cfg(unix)]
    {
        // libc::gethostname is the most portable; avoids a `hostname` crate
        // dependency. The buffer is sized to `HOST_NAME_MAX + 1` (POSIX
        // permits 255).
        let mut buf = [0u8; 256];
        let ret = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if ret != 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        let raw = std::str::from_utf8(&buf[..end]).ok()?;
        let short = raw.split('.').next().unwrap_or(raw);
        let trimmed = short.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Returns `true` when `dir` is on a network filesystem.
///
/// Platform-specific:
/// - **macOS**: `statfs()` and check `MNT_LOCAL` flag — network mounts clear it.
/// - **Linux**: `statfs()` and compare `f_type` against known network FS magics
///   (NFS, SMB/CIFS, FUSE). Any unknown FS is treated as local (fail-safe).
/// - **Windows / other**: always returns `false` (no detection).
fn is_network_fs(dir: &Path) -> bool {
    #[cfg(all(unix, target_os = "macos"))]
    {
        macos_is_network_fs(dir)
    }
    #[cfg(all(unix, target_os = "linux"))]
    {
        linux_is_network_fs(dir)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = dir;
        false
    }
}

#[cfg(all(unix, target_os = "macos"))]
fn macos_is_network_fs(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let c_path = match CString::new(dir.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return false,
    };
    // macOS statfs: <sys/mount.h>. The struct layout differs from Linux.
    // MNT_LOCAL = 0x00001000 — if ABSENT, the mount is remote.
    #[allow(non_camel_case_types)]
    type statfs_t = libc::statfs;
    let mut buf: statfs_t = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut buf) };
    if rc != 0 {
        // Cannot determine — assume local (fail-safe; WAL risk acceptable).
        return false;
    }
    // f_flags is `u32` on macOS. `MNT_LOCAL = 0x00001000` — if ABSENT, the
    // mount is remote.
    const MNT_LOCAL: u32 = 0x0000_1000;
    (buf.f_flags & MNT_LOCAL) == 0
}

#[cfg(all(unix, target_os = "linux"))]
fn linux_is_network_fs(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let c_path = match CString::new(dir.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut buf) };
    if rc != 0 {
        return false;
    }
    // Known network filesystem magic numbers from <linux/magic.h>.
    const NFS_SUPER_MAGIC: u64 = 0x6969;
    const SMB_SUPER_MAGIC: u64 = 0x517B;
    const SMB2_MAGIC_NUMBER: u64 = 0xFE534D42; // "SMB2" little-endian, also CIFS
    const CIFS_MAGIC_NUMBER: u64 = 0xFF534D42;
    const FUSE_SUPER_MAGIC: u64 = 0x65735546;
    // Some FUSE mounts are local (e.g. sshfs is remote). Conservative: treat
    // FUSE as remote so users get the safe TRUNCATE default.
    let t = buf.f_type as u64;
    matches!(
        t,
        NFS_SUPER_MAGIC
            | SMB_SUPER_MAGIC
            | SMB2_MAGIC_NUMBER
            | CIFS_MAGIC_NUMBER
            | FUSE_SUPER_MAGIC
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    /// Serialize tests that mutate process-global env vars. Nextest runs
    /// tests in parallel; without this guard, two env-touching tests would
    /// race on `OXICODE_SQLITE_JOURNAL_MODE` and silently flake.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    /// Acquire the env-test guard. Held until end of test.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        // Panicking on poison would only hide the real failure; ignore it.
        ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Clear the env override. Edition 2024 marks env mutation unsafe.
    fn clear_env() {
        // SAFETY: process-global state; serialized via ENV_GUARD.
        unsafe { env::remove_var(ENV_VAR) }
    }

    /// Set the env override. Edition 2024 marks env mutation unsafe.
    fn set_env(value: &str) {
        // SAFETY: process-global state; serialized via ENV_GUARD.
        unsafe { env::set_var(ENV_VAR, value) }
    }

    #[test]
    fn wal_mode_strings() {
        assert_eq!(JournalMode::Wal.as_str(), "WAL");
        assert_eq!(JournalMode::Truncate.as_str(), "TRUNCATE");
    }

    #[test]
    fn busy_timeout_constant() {
        assert_eq!(JournalMode::Wal.busy_timeout_ms(), 5_000);
        assert_eq!(JournalMode::Truncate.busy_timeout_ms(), 5_000);
    }

    #[test]
    fn env_override_unset_falls_back_to_detection() {
        let _guard = lock_env();
        clear_env();
        // On the dev host (macOS local filesystem) detection returns Wal.
        let mode = JournalMode::for_db_path(Path::new("/tmp/oxicode-test-detect.db"));
        // /tmp may be local or a tmpfs; both report as non-network on macOS.
        assert_eq!(mode, JournalMode::Wal);
    }

    #[test]
    fn env_override_wal_forces_wal() {
        let _guard = lock_env();
        set_env("wal");
        let mode = JournalMode::for_db_path(Path::new("/definitely/nonexistent.db"));
        clear_env();
        assert_eq!(mode, JournalMode::Wal);
    }

    #[test]
    fn env_override_truncate_forces_truncate() {
        let _guard = lock_env();
        set_env("truncate");
        let mode = JournalMode::for_db_path(Path::new("/definitely/nonexistent.db"));
        clear_env();
        assert_eq!(mode, JournalMode::Truncate);
    }

    #[test]
    fn env_override_case_insensitive() {
        let _guard = lock_env();
        set_env("WAL");
        assert_eq!(
            JournalMode::for_db_path(Path::new("/x.db")),
            JournalMode::Wal
        );
        set_env("TRUNCATE");
        assert_eq!(
            JournalMode::for_db_path(Path::new("/x.db")),
            JournalMode::Truncate
        );
        clear_env();
    }

    #[test]
    fn env_override_invalid_falls_back_to_detection() {
        let _guard = lock_env();
        set_env("bogus");
        let mode = JournalMode::for_db_path(Path::new("/tmp/oxicode-test-invalid-env.db"));
        clear_env();
        // Falls back to detection; on local FS this is Wal.
        assert_eq!(mode, JournalMode::Wal);
    }

    #[test]
    fn effective_path_wal_unchanged() {
        let p = Path::new("/tmp/foo.db");
        assert_eq!(JournalMode::Wal.effective_db_path(p), p.to_path_buf());
    }

    #[test]
    fn effective_path_truncate_also_unchanged() {
        // Durable-safe default: even under Truncate the path is NOT
        // rewritten. Only the PRAGMA changes.
        let p = Path::new("/tmp/foo.db");
        assert_eq!(JournalMode::Truncate.effective_db_path(p), p.to_path_buf());
    }

    #[test]
    fn per_host_path_wal_unchanged() {
        let p = Path::new("/tmp/foo.db");
        assert_eq!(JournalMode::Wal.per_host_db_path(p), p.to_path_buf());
    }

    #[test]
    fn per_host_path_truncate_suffixed_when_host_available() {
        // Per-host path requires a hostname. On unix dev hosts this is
        // always available; on no-host targets the function falls back.
        let p = Path::new("/tmp/foo.db");
        let eff = JournalMode::Truncate.per_host_db_path(p);
        if let Some(host) = hostname() {
            let s = eff.to_string_lossy();
            let needle = format!(".h-{host}.");
            assert!(
                s.contains(&needle),
                "expected per-host suffix '{needle}' in {s}"
            );
        } else {
            assert_eq!(eff, p.to_path_buf());
        }
    }

    #[test]
    fn per_host_path_truncate_idempotent() {
        // If the path is already per-host suffixed, we don't re-suffix.
        let host = hostname().unwrap_or_else(|| "x".to_string());
        let p_str = format!("/tmp/foo.h-{host}.db");
        let p = Path::new(&p_str);
        let eff = JournalMode::Truncate.per_host_db_path(p);
        assert_eq!(eff, p);
    }

    #[test]
    fn per_host_path_truncate_preserves_extension() {
        let eff = JournalMode::Truncate.per_host_db_path(Path::new("/tmp/data.sqlite"));
        let s = eff.to_string_lossy();
        assert!(s.ends_with(".sqlite"), "extension preserved in {s}");
    }

    #[test]
    fn detection_with_empty_parent_uses_cwd() {
        let _guard = lock_env();
        clear_env();
        // Bare filename — parent is "". Should not panic.
        let _ = JournalMode::for_db_path(Path::new("bare.db"));
    }

    #[test]
    fn detection_with_root_parent() {
        let _guard = lock_env();
        clear_env();
        // `/foo.db` has parent `/` — non-empty, classified normally.
        let _ = JournalMode::for_db_path(Path::new("/foo.db"));
    }

    /// Integration: `MnemopiDb::open` on a local tmpdir actually applies WAL.
    #[test]
    fn db_open_applies_journal_mode_pragma() {
        let _guard = lock_env();
        clear_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("journal_test.db");
        let db = crate::MnemopiDb::open(&db_path).expect("open");
        let guard = db.lock();
        let mode: String = guard
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("query journal_mode");
        drop(guard);
        drop(db);
        // On a local tmpdir this is wal. Per-host path rewrite is only for
        // network FS, so the path we asked for is what was opened.
        assert!(
            mode.eq_ignore_ascii_case("wal"),
            "expected WAL on local FS, got {mode}"
        );
    }

    /// Integration: env override propagate into `MnemopiDb::open`.
    #[test]
    fn db_open_honors_truncate_env_override() {
        let _guard = lock_env();
        set_env("truncate");
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("journal_override.db");
        let db = crate::MnemopiDb::open(&db_path).expect("open");
        let guard = db.lock();
        let mode: String = guard
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("query journal_mode");
        drop(guard);
        drop(db);
        clear_env();
        // TRUNCATE is persistent only if SQLite honors it; on some configs
        // it may fall back to `delete` (also safe). What we are checking
        // is that WAL was NOT forced when the env said truncate.
        assert!(
            !mode.eq_ignore_ascii_case("wal"),
            "expected non-WAL mode under truncate override, got {mode}"
        );
    }
}
