//! Process-liveness tracking via OS advisory locks.
//!
//! Each session holds an exclusive `flock` on `.oxicode/issues/.alive/<session_id>`.
//! The lock is released by the OS when the process exits (including crashes
//! and `kill -9`). This lets us answer "is session X still alive?" without
//! any wall-clock timeout, PID-recycling heuristics, or heartbeats.

use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// Path of the alive-lock file for `session_id` under `issues_dir`.
pub fn alive_path(issues_dir: &Path, session_id: &str) -> PathBuf {
    issues_dir.join(".alive").join(session_id)
}

/// Try to acquire (and hold) an exclusive advisory lock for `session_id`.
///
/// The returned [`AliveGuard`] releases the lock when dropped — so callers
/// must keep it alive for the whole session. Opening with write+create and
/// calling `flock(LOCK_EX | LOCK_NB)` is atomic enough for our purposes:
/// failure to acquire means another live process holds it.
pub fn acquire(issues_dir: &Path, session_id: &str) -> io::Result<AliveGuard> {
    let dir = issues_dir.join(".alive");
    fs::create_dir_all(&dir)?;
    let path = dir.join(session_id);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    let fd = file.as_raw_fd();
    // Failure (EWOULDBLOCK/EAGAIN) means another live process holds it.
    try_flock_exclusive(fd)?;
    // Record *who* holds the lock, in the lock file itself. Best-effort:
    // the flock is the source of truth for liveness; this payload only adds
    // human-readable provenance (pid/host/cwd/started) for display surfaces.
    write_owner_info(&file, session_id);
    Ok(AliveGuard { _file: file, path })
}

/// Provenance of a live alive-lock holder, persisted inside the lock file.
///
/// Written by [`acquire`] right after the exclusive flock is taken; read
/// back with [`read_owner_info`]. The lock file doubles as the storage so
/// there is exactly one artifact per session — the existing orphan reaper
/// ([`reap_orphans`]) and the RAII unlink in [`AliveGuard::drop`] clean it
/// up with no extra bookkeeping.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OwnerInfo {
    /// Owning session id (same as the lock-file name).
    pub session: String,
    /// OS pid of the holder.
    pub pid: u32,
    /// Hostname of the holder (empty when gethostname fails).
    pub host: String,
    /// Working directory at acquisition time.
    pub cwd: String,
    /// Unix seconds at acquisition time.
    pub started: u64,
}

/// Read back the [`OwnerInfo`] recorded by [`acquire`] for `session_id`.
///
/// Returns `None` when the lock file is missing (dead session) or holds no
/// parseable payload (written by an older oxicode, before this metadata
/// existed) — callers must treat `None` as "unknown holder", not "dead".
pub fn read_owner_info(issues_dir: &Path, session_id: &str) -> Option<OwnerInfo> {
    let data = fs::read_to_string(alive_path(issues_dir, session_id)).ok()?;
    serde_json::from_str(&data).ok()
}

/// Overwrite the lock file's payload with the [`OwnerInfo`] JSON.
///
/// Best-effort — any I/O error is swallowed because liveness comes from the
/// flock, never from the payload.
fn write_owner_info(file: &fs::File, session_id: &str) {
    use std::io::{Seek, SeekFrom, Write};
    let info = OwnerInfo {
        session: session_id.to_string(),
        pid: std::process::id(),
        host: hostname(),
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        started: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let Ok(bytes) = serde_json::to_vec(&info) else {
        return;
    };
    let mut f: &fs::File = file;
    let _ = f.seek(SeekFrom::Start(0));
    let _ = file.set_len(0);
    let _ = f.write_all(&bytes);
}

/// Local hostname, or `""` when gethostname fails/truncates.
fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a valid 256-byte array; gethostname writes at most
    // `buf.len()` bytes and NUL-terminates (truncating the name if needed).
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Returns `true` iff a live process currently holds the alive-lock for
/// `session_id`. Used to decide whether an [`crate::issues::Assignment`]
/// is still valid.
pub fn is_session_alive(issues_dir: &Path, session_id: &str) -> bool {
    let path = alive_path(issues_dir, session_id);
    if !path.exists() {
        return false;
    }
    // Try to acquire a *shared* lock non-blockingly. If we can't, someone
    // holds an exclusive lock → alive. If we can, no one holds it → dead.
    let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
        return false;
    };
    let fd = file.as_raw_fd();
    // Ok = nobody holds exclusive (dead); Err = held by a live process (alive).
    probe_flock_shared(fd).is_err()
}

// ── flock helpers (#11: centralize the two unsafe call sites) ────────
//
// Both take a raw fd that the caller obtained from a live `File` via
// `as_raw_fd()`, so fd validity is guaranteed by construction. Naming
// them (with SAFETY docs) keeps the `unsafe` surface to these two spots
// instead of being scattered through the liveness logic.

/// Try a non-blocking exclusive flock on `fd`.
///
/// `Ok` on success; `Err` on contention (`EWOULDBLOCK`/`EAGAIN` — another
/// live process holds it) or any other OS error.
///
/// `fd` must be a valid open file descriptor.
fn try_flock_exclusive(fd: i32) -> io::Result<()> {
    // SAFETY: `fd` is a valid, owned descriptor (caller passes
    // `File::as_raw_fd()` from a live `File`). `LOCK_NB` never blocks.
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Probe liveness by attempting a non-blocking shared flock.
///
/// `Ok` if no one holds an exclusive lock (we acquired and released a
/// shared one); `Err` if someone holds exclusive (a live process).
///
/// `fd` must be a valid open file descriptor.
fn probe_flock_shared(fd: i32) -> io::Result<()> {
    // SAFETY: `fd` is a valid, owned descriptor (see `try_flock_exclusive`).
    let rc = unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) };
    if rc == 0 {
        // SAFETY: releasing the shared lock we just acquired on a valid fd.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

// ── Orphan reaping (#8) ─────────────────────────────────────────────

/// Minimum age (seconds) a dead alive-lock file must reach before reaping.
///
/// The age gate is the TOCTOU mitigation: a reaper checks `is_session_alive`,
/// and a process could acquire the lock in the gap before `remove_file`.
/// Only reaping files older than this threshold leaves a wide margin for
/// any session that is actively starting up, while still clearing the
/// steady-state accumulation of zombies from crashed/killed processes.
pub const ORPHAN_AGE_SECS: u64 = 3600; // 1 hour

/// Best-effort, idempotent cleanup of dead alive-lock files under
/// `<issues_dir>/.alive/`.
///
/// Two guards keep it safe:
/// 1. **Holder check** — files whose session still holds an exclusive flock
///    ([`is_session_alive`] → `true`) are never touched.
/// 2. **Age gate** — even dead files younger than [`ORPHAN_AGE_SECS`] are
///    skipped, so a process racing to acquire can't lose its lock file.
///
/// Returns the number of files removed. Missing `.alive/` is `Ok(0)`.
pub fn reap_orphans(issues_dir: &Path) -> io::Result<usize> {
    let dir = issues_dir.join(".alive");
    let rd = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in rd.flatten() {
        let sid = entry.file_name();
        let sid = sid.to_string_lossy();
        if is_session_alive(issues_dir, &sid) {
            continue; // (1) someone holds it — never reap
        }
        let mtime = entry.metadata().and_then(|m| m.modified()).unwrap_or(now);
        let age = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
        if age < ORPHAN_AGE_SECS {
            continue; // (2) too young — TOCTOU margin
        }
        if fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// RAII guard for an acquired alive-lock.
#[derive(Debug)]
pub struct AliveGuard {
    _file: fs::File,
    path: PathBuf,
}

impl AliveGuard {
    /// Path of the alive-lock file this guard holds.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AliveGuard {
    fn drop(&mut self) {
        // Drop closes the fd → OS releases the lock. Best-effort unlink.
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_then_alive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let sid = "s1";
        let _g = acquire(&dir, sid).unwrap();
        assert!(is_session_alive(&dir, sid));
        drop(_g);
        assert!(!is_session_alive(&dir, sid));
    }

    #[test]
    fn second_acquire_fails_while_held() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let sid = "s2";
        let g = acquire(&dir, sid).unwrap();
        let second = acquire(&dir, sid);
        assert!(second.is_err(), "second acquire should fail while held");
        assert!(is_session_alive(&dir, sid));
        drop(g);
        assert!(acquire(&dir, sid).is_ok(), "after drop, acquire succeeds");
    }

    // ── Phase 4: orphan reap (#8) ──

    /// Helper: backdate a file's mtime by `secs` so it crosses the age gate.
    fn backdate(path: &std::path::Path, secs: u64) {
        use std::fs::FileTimes;
        let then = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
        let f = std::fs::File::open(path)
            .or_else(|_| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false) // open-or-create without erasing (clippy::suspicious_open_options)
                    .open(path)
            })
            .unwrap();
        f.set_times(FileTimes::new().set_modified(then)).unwrap();
    }

    #[test]
    fn reap_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        // No `.alive/` at all.
        assert_eq!(reap_orphans(&dir).unwrap(), 0);
        fs::create_dir_all(dir.join(".alive")).unwrap();
        // Empty dir, repeated calls stay at 0.
        assert_eq!(reap_orphans(&dir).unwrap(), 0);
        assert_eq!(reap_orphans(&dir).unwrap(), 0);
    }

    #[test]
    fn reap_skips_recent_dead_files() {
        // A dead (unheld) orphan younger than ORPHAN_AGE_SECS must be
        // preserved — the age gate is the TOCTOU mitigation.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::create_dir_all(dir.join(".alive")).unwrap();
        let recent = dir.join(".alive").join("dead-recent");
        fs::write(&recent, b"").unwrap();
        // mtime ~ now.
        assert_eq!(reap_orphans(&dir).unwrap(), 0);
        assert!(
            recent.exists(),
            "recent dead orphan must be preserved by the age gate"
        );
    }

    #[test]
    fn reap_removes_old_dead_and_keeps_alive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        // A genuinely live lock — must never be reaped.
        let _g_live = acquire(&dir, "alive-session").unwrap();
        // An old dead orphan (no holder, mtime > threshold).
        fs::create_dir_all(dir.join(".alive")).unwrap();
        let old = dir.join(".alive").join("dead-old");
        fs::write(&old, b"").unwrap();
        backdate(&old, ORPHAN_AGE_SECS + 60);

        let removed = reap_orphans(&dir).unwrap();
        assert_eq!(removed, 1, "only the old dead orphan should be reaped");
        assert!(!old.exists(), "old dead orphan must be removed");
        // Live holder is still alive and its file untouched.
        assert!(
            is_session_alive(&dir, "alive-session"),
            "live lock must survive reap"
        );
    }

    #[test]
    fn acquire_writes_readable_owner_info() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let g = acquire(&dir, "owner-check").unwrap();
        let info = read_owner_info(&dir, "owner-check").expect("owner info must be recorded");
        assert_eq!(info.session, "owner-check");
        assert_eq!(info.pid, std::process::id());
        assert!(info.started > 0, "started must be unix seconds");
        // Dropping the guard unlinks the lock file — owner info goes with it.
        drop(g);
        assert!(read_owner_info(&dir, "owner-check").is_none());
    }

    #[test]
    fn read_owner_info_missing_or_garbage_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        assert!(read_owner_info(&dir, "nope").is_none());
        fs::create_dir_all(dir.join(".alive")).unwrap();
        fs::write(dir.join(".alive").join("junk"), b"not json").unwrap();
        assert!(read_owner_info(&dir, "junk").is_none());
    }
}
