//! Self-update mechanism via GitHub Releases.

use anyhow::Result;

/// Update checker and executor.
#[allow(dead_code)]
pub struct Updater {
    repo_owner: &'static str,
    repo_name: &'static str,
}

impl Updater {
    /// Create a new updater targeting the given GitHub repository.
    pub fn new() -> Self {
        Self {
            repo_owner: "earendil-works",
            repo_name: "oxi",
        }
    }

    /// Check if a newer version is available on GitHub Releases.
    ///
    /// Returns `Some(message)` when an update is available, `None` when
    /// already on the latest version.
    #[cfg(feature = "self-update")]
    pub async fn check_update(&self) -> Result<Option<String>> {
        let repo_owner = self.repo_owner.to_string();
        let repo_name = self.repo_name.to_string();
        let current = env!("CARGO_PKG_VERSION").to_string();

        tokio::task::spawn_blocking(move || -> Result<Option<String>> {
            let releases = self_update::backends::github::ReleaseList::configure()
                .repo_owner(&repo_owner)
                .repo_name(&repo_name)
                .build()?
                .fetch()?;

            if let Some(latest) = releases.into_iter().next() {
                if latest.version != current {
                    return Ok(Some(format!(
                        "Update available: {} -> {}",
                        current, latest.version
                    )));
                }
            }
            Ok(None)
        })
        .await?
    }

    /// Check if a newer version is available (stub when self-update disabled).
    #[cfg(not(feature = "self-update"))]
    pub async fn check_update(&self) -> Result<Option<String>> {
        Ok(None)
    }

    /// Execute the self-update, replacing the current binary.
    ///
    /// Downloads the latest release from GitHub and replaces the running
    /// executable in-place.
    #[cfg(feature = "self-update")]
    pub async fn update(&self) -> Result<()> {
        let repo_owner = self.repo_owner.to_string();
        let repo_name = self.repo_name.to_string();
        let target = self_update::get_target();
        let current = env!("CARGO_PKG_VERSION").to_string();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let status = self_update::backends::github::Update::configure()
                .repo_owner(&repo_owner)
                .repo_name(&repo_name)
                .target(&target)
                .bin_name("oxi")
                .show_download_progress(true)
                .current_version(&current)
                .build()?
                .update()?;
            tracing::info!("Updated to version {}", status.version());
            Ok(())
        })
        .await?
    }

    /// Execute the self-update (stub when self-update disabled).
    #[cfg(not(feature = "self-update"))]
    pub async fn update(&self) -> Result<()> {
        anyhow::bail!("Self-update is not available in this build. Reinstall manually.")
    }
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updater_new() {
        let u = Updater::new();
        assert_eq!(u.repo_owner, "earendil-works");
        assert_eq!(u.repo_name, "oxi");
    }

    #[test]
    fn updater_default() {
        let u = Updater::default();
        assert_eq!(u.repo_owner, "earendil-works");
        assert_eq!(u.repo_name, "oxi");
    }
}
