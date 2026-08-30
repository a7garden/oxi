//! Path resolution for the canonical oxicode home directory.

use std::path::PathBuf;

/// Return the canonical `oxicode` home directory.
///
/// Delegates to [`oxicode_ai::product_env::home_dir`] so the leaf library and
/// the SDK share one resolution path. Resolution order (unified Oxi home):
/// 1. `$OXICODE_HOME` environment variable
/// 2. `$OXI_HOME/oxicode`
/// 3. `$HOME/.oxi/oxicode` (or platform equivalent via `dirs`)
///
/// Legacy `~/.oxicode` installs are handled at the reader layer (read-only
/// fallback via `oxicode_ai::oxi_home::read_path`); this function always
/// returns the canonical, write-target home.
///
/// Returns an error if none of the above is available.
pub fn home_dir() -> std::io::Result<PathBuf> {
    oxicode_ai::product_env::home_dir()
}

/// Ensure a directory exists, creating it (and parents) if missing.
pub async fn ensure_dir(path: &std::path::Path) -> std::io::Result<()> {
    if !path.exists() {
        tokio::fs::create_dir_all(path).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_resolves() {
        // Either $OXICODE_HOME or $HOME must be set in normal environments.
        let d = home_dir();
        if std::env::var("OXICODE_HOME").is_err() && dirs::home_dir().is_none() {
            assert!(d.is_err());
        } else {
            assert!(d.is_ok());
        }
    }
}
