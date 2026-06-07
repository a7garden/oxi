//! Path resolution for the `~/.oxi` directory layout.

use std::path::PathBuf;

/// Return the conventional `oxi` home directory.
///
/// Resolution order:
/// 1. `$OXI_HOME` environment variable
/// 2. `$HOME/.oxi` (or platform equivalent via `dirs`)
///
/// Returns an error if neither is available.
pub fn home_dir() -> std::io::Result<PathBuf> {
    if let Ok(p) = std::env::var("OXI_HOME")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME not set"))?;
    Ok(home.join(".oxi"))
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
        // Either $OXI_HOME or $HOME must be set in normal environments.
        let d = home_dir();
        if std::env::var("OXI_HOME").is_err() && dirs::home_dir().is_none() {
            assert!(d.is_err());
        } else {
            assert!(d.is_ok());
        }
    }
}
