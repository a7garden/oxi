//! MCP server spawn validation policy hook.
//!
//! The SDK owns the [`SpawnValidator`] trait + [`NoopSpawnValidator`]
//! reference impl. Consumers (oxi-cli, oxios) own the *policy* — i.e. which
//! commands are safe to spawn, which environment variables must be stripped,
//! and which paths are allowed.
//!
//! See `docs/oxi-sdk-ownership.md` §2 (MCP transport / MCP spawn validation
//! policy split).
//!
//! # Why a trait and not a config
//!
//! Spawn validation involves multi-step logic (command parsing, shell-metachar
//! scanning, env var whitelisting/blacklisting, path resolution) that varies
//! per consumer. A trait lets each consumer express its own policy without
//! the SDK prescribing a checklist. A `NoopSpawnValidator` is provided for
//! the default case where no policy is needed (preserves existing behavior).

use std::collections::HashMap;

/// Validates MCP server spawn commands and environment.
///
/// Consumers inject domain-specific safety policy (forbidden shells,
/// dangerous env vars, path traversal checks) without modifying the SDK's
/// MCP client. The SDK calls `validate_command` before spawning and
/// `sanitize_env` before passing the environment to the child process.
///
/// This trait is `#[unstable]` initially — the surface may evolve as we
/// learn which signals consumers actually need.
pub trait SpawnValidator: Send + Sync {
    /// Validate the command + args before spawn. Return `Err(reason)` to block
    /// the spawn (the error message is forwarded to the caller as
    /// `McpError::SpawnValidation`).
    fn validate_command(&self, cmd: &str, args: &[String]) -> Result<(), String>;

    /// Sanitize or strip dangerous environment variables before spawn.
    ///
    /// Implementations SHOULD remove known loader-injection vectors
    /// (`LD_PRELOAD`, `LD_LIBRARY_PATH`, `DYLD_INSERT_LIBRARIES`,
    /// `PYTHONPATH`, etc.) and SHOULD resolve any relative paths in
    /// remaining vars to absolute. The default noop leaves the env
    /// untouched.
    fn sanitize_env(&self, env: &mut HashMap<String, String>);
}

/// Default no-op validator — preserves current behavior (no validation, no
/// env scrubbing). Use this when no consumer-supplied policy is registered.
pub struct NoopSpawnValidator;

impl SpawnValidator for NoopSpawnValidator {
    fn validate_command(&self, _cmd: &str, _args: &[String]) -> Result<(), String> {
        Ok(())
    }

    fn sanitize_env(&self, _env: &mut HashMap<String, String>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_validator_accepts_any_command() {
        let v = NoopSpawnValidator;
        assert!(v.validate_command("/bin/anything", &[]).is_ok());
        assert!(
            v.validate_command("/bin/sh", &["-c".into(), "rm -rf /".into()])
                .is_ok()
        );
    }

    #[test]
    fn noop_validator_leaves_env_untouched() {
        let v = NoopSpawnValidator;
        let mut env = HashMap::new();
        env.insert("LD_PRELOAD".into(), "/tmp/evil.so".into());
        env.insert("PATH".into(), "/usr/bin".into());
        v.sanitize_env(&mut env);
        assert_eq!(
            env.get("LD_PRELOAD").map(String::as_str),
            Some("/tmp/evil.so")
        );
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
    }

    /// A test policy that blocks any command containing "sh" and strips
    /// `LD_*` env vars. Verifies the trait can carry a real consumer policy.
    struct TestStrictPolicy;
    impl SpawnValidator for TestStrictPolicy {
        fn validate_command(&self, cmd: &str, _args: &[String]) -> Result<(), String> {
            if cmd.contains("sh") {
                Err(format!("shell not allowed: {cmd}"))
            } else {
                Ok(())
            }
        }
        fn sanitize_env(&self, env: &mut HashMap<String, String>) {
            env.retain(|k, _| !k.starts_with("LD_"));
        }
    }

    #[test]
    fn consumer_policy_can_block_commands() {
        let v = TestStrictPolicy;
        assert!(v.validate_command("/usr/bin/node", &[]).is_ok());
        assert!(v.validate_command("/bin/sh", &[]).is_err());
    }

    #[test]
    fn consumer_policy_can_scrub_env() {
        let v = TestStrictPolicy;
        let mut env = HashMap::new();
        env.insert("LD_PRELOAD".into(), "evil".into());
        env.insert("LD_LIBRARY_PATH".into(), "/evil".into());
        env.insert("PATH".into(), "/usr/bin".into());
        v.sanitize_env(&mut env);
        assert!(!env.contains_key("LD_PRELOAD"));
        assert!(!env.contains_key("LD_LIBRARY_PATH"));
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
    }
}
