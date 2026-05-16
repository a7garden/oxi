//! CLI parsing integration tests

#[cfg(test)]
mod tests {
    use assert_cmd::Command;
    use predicates::prelude::*;
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        // Walk up from CARGO_MANIFEST_DIR to find the workspace Cargo.toml
        let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        while !dir.join("Cargo.lock").exists() {
            if !dir.pop() {
                panic!("Could not find workspace root");
            }
        }
        dir
    }

    #[test]
    fn test_version_flag() {
        Command::new("cargo")
            .args(["run", "--", "--version"])
            .current_dir(workspace_root())
            .assert()
            .success()
            .stdout(predicate::str::contains("oxi"));
    }

    #[test]
    fn test_help_flag() {
        Command::new("cargo")
            .args(["run", "--", "--help"])
            .current_dir(workspace_root())
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }

    #[test]
    fn test_config_subcommand_exists() {
        Command::new("cargo")
            .args(["run", "--", "config", "show"])
            .current_dir(workspace_root())
            .assert()
            .success();
    }

    #[test]
    fn test_sessions_subcommand_exists() {
        Command::new("cargo")
            .args(["run", "--", "sessions"])
            .current_dir(workspace_root())
            .assert()
            .success();
    }

    #[test]
    fn test_pkg_subcommand_exists() {
        Command::new("cargo")
            .args(["run", "--", "pkg", "list"])
            .current_dir(workspace_root())
            .assert()
            .success();
    }

    #[test]
    fn test_invalid_provider_shows_error() {
        Command::new("cargo")
            .args(["run", "--", "-p", "nonexistent_provider", "test"])
            .current_dir(workspace_root())
            .assert()
            .failure();
    }
}
