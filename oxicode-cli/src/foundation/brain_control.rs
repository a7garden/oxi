//! oxibrain daemon lifecycle control — bring a `brain·down` daemon back.
//!
//! The TUI's health chip only *observes* (`brain.rs` probes); this module
//! *acts*. When the daemon is installed but stopped, `/brain` revives it:
//!
//! 1. **launchd** (macOS, plist present): `launchctl bootstrap` a service
//!    that is not loaded, `launchctl kickstart` one that is. launchd keeps
//!    supervising it afterwards (KeepAlive), which is the correct home for
//!    a daemon — `oxibrain serve --daemon` deliberately does not fork.
//! 2. **Detached spawn** (no plist / non-macOS): start
//!    `oxibrain serve --daemon --socket <canonical>` in its own process
//!    group with null stdio. The orphan survives oxicode's exit.
//!
//! When the binary is missing entirely, revival is impossible — the
//! caller surfaces install guidance instead of pretending.

use std::path::PathBuf;
use std::process::Stdio;

/// launchd service label used by the oxibrain plist.
pub const BRAIN_SERVICE_LABEL: &str = "com.oxi.oxibrain";

/// Canonical plist location (`~/Library/LaunchAgents/com.oxi.oxibrain.plist`).
pub fn brain_plist_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut p = PathBuf::from(home);
    p.push("Library");
    p.push("LaunchAgents");
    p.push(format!("{BRAIN_SERVICE_LABEL}.plist"));
    Some(p)
}

/// What the revive step decided to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviveAction {
    /// Service exists but is not loaded — bootstrap the plist into launchd.
    BootstrapPlist(PathBuf),
    /// Service is loaded but stopped — kickstart it.
    KickstartService,
    /// No launchd supervision available — spawn detached.
    SpawnDetached(PathBuf),
    /// No oxibrain binary — cannot revive.
    InstallNeeded,
}

/// Environment facts the plan is derived from (injectable for tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainControlReport {
    /// Resolved `oxibrain` binary, if any.
    pub binary: Option<PathBuf>,
    /// Existing launchd plist path, if any.
    pub plist: Option<PathBuf>,
    /// Whether the service is currently loaded into launchd.
    pub service_loaded: bool,
}

/// Decide how to revive from a control report. Pure.
pub fn revive_plan(report: &BrainControlReport) -> ReviveAction {
    let Some(binary) = report.binary.clone() else {
        return ReviveAction::InstallNeeded;
    };
    match &report.plist {
        Some(plist) if !report.service_loaded => ReviveAction::BootstrapPlist(plist.clone()),
        Some(_) => ReviveAction::KickstartService,
        None => ReviveAction::SpawnDetached(binary),
    }
}

/// Locate the `oxibrain` binary: `~/.cargo/bin` first (cargo-installed),
/// then `PATH`.
pub fn find_oxibrain_binary() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".cargo");
        p.push("bin");
        p.push("oxibrain");
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("oxibrain");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Whether the launchd service is loaded (macOS). Non-macOS: false.
pub fn service_loaded() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let uid = std::process::id();
    let _ = uid;
    let user = whoami_uid();
    let out = std::process::Command::new("launchctl")
        .args(["print", &format!("gui/{user}/{BRAIN_SERVICE_LABEL}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(out, Ok(st) if st.success())
}

fn whoami_uid() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "501".to_string())
}

/// Probe the environment for a control report.
pub fn probe_control() -> BrainControlReport {
    BrainControlReport {
        binary: find_oxibrain_binary(),
        plist: brain_plist_path().filter(|p| p.is_file()),
        service_loaded: service_loaded(),
    }
}

/// Run the plan and wait for the daemon to answer a ping.
/// Returns human-readable outcome lines on success.
pub async fn revive() -> Result<String, String> {
    let report = probe_control();
    let plan = revive_plan(&report);
    match plan {
        ReviveAction::InstallNeeded => Err(
            "oxibrain binary not found — install it first (e.g. `cargo install oxibrain` \
             from the oxibrain repo), then run /brain again"
                .to_string(),
        ),
        ReviveAction::BootstrapPlist(plist) => {
            let user = whoami_uid();
            let domain = format!("gui/{user}");
            let status = std::process::Command::new("launchctl")
                .args(["bootstrap", &domain])
                .arg(&plist)
                .status()
                .map_err(|e| format!("launchctl bootstrap failed: {e}"))?;
            if !status.success() {
                return Err(format!(
                    "launchctl bootstrap exited {} — check `launchctl print {domain}/{BRAIN_SERVICE_LABEL}`",
                    status.code().unwrap_or(-1)
                ));
            }
            wait_for_ping("bootstrapped the launchd service").await
        }
        ReviveAction::KickstartService => {
            let user = whoami_uid();
            let target = format!("gui/{user}/{BRAIN_SERVICE_LABEL}");
            let status = std::process::Command::new("launchctl")
                .args(["kickstart", &target])
                .status()
                .map_err(|e| format!("launchctl kickstart failed: {e}"))?;
            if !status.success() {
                return Err(format!(
                    "launchctl kickstart exited {} — check `launchctl print {target}`",
                    status.code().unwrap_or(-1)
                ));
            }
            wait_for_ping("kicked the launchd service").await
        }
        ReviveAction::SpawnDetached(binary) => {
            let socket = crate::foundation::brain::default_socket_path();
            let mut cmd = std::process::Command::new(&binary);
            cmd.args(["serve", "--daemon"])
                .arg("--socket")
                .arg(&socket)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            cmd.spawn()
                .map_err(|e| format!("spawning oxibrain failed: {e}"))?;
            wait_for_ping("spawned a detached daemon").await
        }
    }
}

/// Poll the daemon until it answers or the budget runs out.
async fn wait_for_ping(action: &str) -> Result<String, String> {
    let backend = crate::foundation::brain::BrainMemoryBackend::new(
        crate::foundation::brain::default_socket_path(),
    );
    for _ in 0..20 {
        if backend.ping().await.is_ok() {
            return Ok(format!("{action} — daemon is answering pings"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Err(format!(
        "{action}, but the daemon did not answer within 5s — check the log at \
         ~/.oxi/brain/daemon.log"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin() -> PathBuf {
        PathBuf::from("/usr/local/bin/oxibrain")
    }

    #[test]
    fn plan_without_binary_requires_install() {
        let report = BrainControlReport {
            binary: None,
            plist: Some(PathBuf::from("/Library/LaunchAgents/x.plist")),
            service_loaded: false,
        };
        assert_eq!(revive_plan(&report), ReviveAction::InstallNeeded);
    }

    #[test]
    fn plan_bootstraps_an_unloaded_service() {
        let report = BrainControlReport {
            binary: Some(bin()),
            plist: Some(PathBuf::from("/Library/LaunchAgents/x.plist")),
            service_loaded: false,
        };
        assert_eq!(
            revive_plan(&report),
            ReviveAction::BootstrapPlist(PathBuf::from("/Library/LaunchAgents/x.plist"))
        );
    }

    #[test]
    fn plan_kickstarts_a_loaded_service() {
        let report = BrainControlReport {
            binary: Some(bin()),
            plist: Some(PathBuf::from("/Library/LaunchAgents/x.plist")),
            service_loaded: true,
        };
        assert_eq!(revive_plan(&report), ReviveAction::KickstartService);
    }

    #[test]
    fn plan_spawns_detached_without_launchd() {
        let report = BrainControlReport {
            binary: Some(bin()),
            plist: None,
            service_loaded: false,
        };
        assert_eq!(revive_plan(&report), ReviveAction::SpawnDetached(bin()));
    }
}
