//! CLI parsing integration tests

#[cfg(test)]
mod tests {
    use assert_cmd::Command;
    use predicates::prelude::*;

    #[test]
    fn test_version_flag() {
        Command::new("cargo")
            .args(&["run", "--", "--version"])
            .current_dir("/Volumes/MERCURY/PROJECTS/oxi")
            .assert()
            .success()
            .stdout(predicate::str::contains("oxi"));
    }

    #[test]
    fn test_help_flag() {
        Command::new("cargo")
            .args(&["run", "--", "--help"])
            .current_dir("/Volumes/MERCURY/PROJECTS/oxi")
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }

    #[test]
    fn test_config_subcommand_exists() {
        Command::new("cargo")
            .args(&["run", "--", "config", "show"])
            .current_dir("/Volumes/MERCURY/PROJECTS/oxi")
            .assert()
            .success();
    }

    #[test]
    fn test_sessions_subcommand_exists() {
        Command::new("cargo")
            .args(&["run", "--", "sessions"])
            .current_dir("/Volumes/MERCURY/PROJECTS/oxi")
            .assert()
            .success();
    }

    #[test]
    fn test_pkg_subcommand_exists() {
        Command::new("cargo")
            .args(&["run", "--", "pkg", "list"])
            .current_dir("/Volumes/MERCURY/PROJECTS/oxi")
            .assert()
            .success();
    }

    #[test]
    fn test_invalid_provider_shows_error() {
        Command::new("cargo")
            .args(&["run", "--", "-p", "nonexistent_provider", "test"])
            .current_dir("/Volumes/MERCURY/PROJECTS/oxi")
            .assert()
            .failure();
    }
}
