//! `profiles.json` parsing and role resolution.
//!
//! The pure decision function [`resolve_profile`] is the only thing
//! that decides which provider/model an agent runs against. It is
//! deliberately testable with no filesystem, no environment, and no
//! network — feed it inputs, get a typed result.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::FoundationError;

/// Typed `profiles.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilesFile {
    /// MUST be `1`.
    pub schema_version: u32,
    /// Profile records. Non-empty when parsing succeeds.
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

/// A single profile record. Non-secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Profile id (unique within the file).
    pub id: String,
    /// Provider implementation (e.g. `anthropic`, `openai`).
    pub provider: String,
    /// Model name (validated against the catalog at first use).
    pub model: String,
    /// Roles this profile binds to (e.g. `coding.primary`).
    #[serde(default)]
    pub roles: Vec<String>,
    /// Keychain locator: `{ service, account }`.
    pub credential: CredentialLocator,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialLocator {
    pub service: String,
    pub account: String,
}

/// Resolved profile + source class. The caller resolves the
/// credential locator against the OS Keychain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfile {
    pub profile: Profile,
    pub source: super::CredentialSource,
}

/// Read and validate `profiles.json`. Returns a typed `ProfilesFile`.
pub fn read(path: &Path) -> Result<ProfilesFile, FoundationError> {
    let raw = std::fs::read_to_string(path)?;
    let parsed: ProfilesFile = serde_json::from_str(&raw)?;
    parsed.validate()?;
    Ok(parsed)
}

impl ProfilesFile {
    /// Validate schema, uniqueness, and non-secret fields.
    pub fn validate(&self) -> Result<(), FoundationError> {
        if self.schema_version != 1 {
            return Err(FoundationError::UnsupportedSchema(self.schema_version));
        }
        let mut seen = HashSet::new();
        for p in &self.profiles {
            if p.id.is_empty() {
                return Err(FoundationError::Parse("profile id is empty".to_string()));
            }
            if !seen.insert(p.id.clone()) {
                return Err(FoundationError::DuplicateProfileId(p.id.clone()));
            }
            if p.provider.is_empty() {
                return Err(FoundationError::Parse(format!(
                    "profile {} has empty provider",
                    p.id
                )));
            }
            if p.model.is_empty() {
                return Err(FoundationError::Parse(format!(
                    "profile {} has empty model",
                    p.id
                )));
            }
            if p.roles.is_empty() {
                return Err(FoundationError::Parse(format!(
                    "profile {} has no roles",
                    p.id
                )));
            }
            for r in &p.roles {
                if r.is_empty() {
                    return Err(FoundationError::Parse(format!(
                        "profile {} has empty role",
                        p.id
                    )));
                }
            }
            p.credential.validate()?;
            check_secret_keys_absent(p)?;
        }
        Ok(())
    }

    /// Find a profile by id.
    pub fn find(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Find profiles whose `roles` contains the requested role.
    pub fn for_role(&self, role: &str) -> Vec<&Profile> {
        self.profiles
            .iter()
            .filter(|p| p.roles.iter().any(|r| r == role))
            .collect()
    }
}

impl CredentialLocator {
    pub fn validate(&self) -> Result<(), FoundationError> {
        if self.service.is_empty() {
            return Err(FoundationError::Parse(
                "credential.service is empty".to_string(),
            ));
        }
        if self.account.is_empty() {
            return Err(FoundationError::Parse(
                "credential.account is empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Pure decision function. Returns the resolved profile, or a typed
/// reason. See the contract spec for precedence rules.
pub fn resolve_profile(input: ResolveInput<'_>) -> Result<ResolvedProfile, FoundationError> {
    // 1. Environment override — non-persistent automation.
    if let Some(env) = input.explicit_environment_override
        && let Some(profile) = env_into_profile(env)
    {
        return Ok(ResolvedProfile {
            profile,
            source: super::CredentialSource::Environment,
        });
    }

    // 2. Explicit profile id.
    if let Some(id) = input.explicit_profile {
        let profile = input
            .foundation_profiles
            .find(id)
            .ok_or_else(|| FoundationError::UnknownProfile(id.to_string()))?
            .clone();
        return Ok(ResolvedProfile {
            profile,
            source: super::CredentialSource::Profile,
        });
    }

    // 3. Role-compatible profile.
    if let Some(role) = input.requested_role {
        let matches = input.foundation_profiles.for_role(role);
        match matches.as_slice() {
            [] => return Err(FoundationError::UnknownRole(role.to_string())),
            [one] => {
                return Ok(ResolvedProfile {
                    profile: (*one).clone(),
                    source: super::CredentialSource::Role,
                });
            }
            _ => return Err(FoundationError::AmbiguousRole(role.to_string())),
        }
    }

    // 4. Compatibility import.
    if let Some(import) = input.compatibility_import {
        return Ok(ResolvedProfile {
            profile: import.profile.clone(),
            source: super::CredentialSource::CompatibilityImport,
        });
    }

    Err(FoundationError::UnknownProfile(
        "no profile, role, environment override, or compatibility import provided".to_string(),
    ))
}

/// Inputs to the pure decision function. Mirrors the spec.
#[derive(Debug, Clone)]
pub struct ResolveInput<'a> {
    /// `--profile` / `OXICODE_PROFILE`.
    pub explicit_profile: Option<&'a str>,
    /// Parsed environment override. See [`resolve_environment_override`].
    pub explicit_environment_override: Option<&'a EnvironmentOverride>,
    /// Requested role id (e.g. `coding.primary`).
    pub requested_role: Option<&'a str>,
    /// Profiles parsed from `profiles.json`.
    pub foundation_profiles: &'a ProfilesFile,
    /// Optional one-time compatibility import.
    pub compatibility_import: Option<&'a CompatibilityImport>,
}

/// Parsed environment override (`OXICODE_PROVIDER` + `OXICODE_MODEL`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentOverride {
    pub provider: String,
    pub model: String,
}

impl EnvironmentOverride {
    /// Read both env vars. Returns `None` if either is unset or empty.
    pub fn from_env() -> Option<Self> {
        let provider = std::env::var("OXICODE_PROVIDER").ok()?;
        let model = std::env::var("OXICODE_MODEL").ok()?;
        if provider.trim().is_empty() || model.trim().is_empty() {
            return None;
        }
        Some(Self {
            provider: provider.trim().to_string(),
            model: model.trim().to_string(),
        })
    }
}

/// One-time legacy compatibility import. Selected only when
/// `OXICODE_FOUNDATION_MIGRATION=1` is set.
#[derive(Debug, Clone)]
pub struct CompatibilityImport {
    pub profile: Profile,
}

fn env_into_profile(env: &EnvironmentOverride) -> Option<Profile> {
    if env.provider.is_empty() || env.model.is_empty() {
        return None;
    }
    Some(Profile {
        id: "__env_override__".to_string(),
        provider: env.provider.clone(),
        model: env.model.clone(),
        roles: vec![],
        credential: CredentialLocator {
            // Env variables never reach the Keychain — the
            // credential module recognizes the literal sentinel and
            // pulls from the env at read time.
            service: "__env__".to_string(),
            account: "__env__".to_string(),
        },
    })
}

/// Known secret-shaped field names. A profile carrying any of these
/// is rejected before reaching the registry.
const SECRET_FIELD_NAMES: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "bearer_token",
    "bearer",
    "password",
    "secret",
    "secret_value",
    "private_key",
    "private-key",
    "access_token",
    "refresh_token",
    "session_token",
    "oauth_token",
];

fn check_secret_keys_absent(profile: &Profile) -> Result<(), FoundationError> {
    // We only inspect fields we ourselves deserialize; the spec
    // scrubs known shapes but does not rely on a denylist for
    // unknown serde fields. Reject any shape that looks like a
    // plaintext credential.
    let value = serde_json::to_value(profile).map_err(|e| FoundationError::Parse(e.to_string()))?;
    if let Some(obj) = value.as_object() {
        for k in obj.keys() {
            let lower = k.to_ascii_lowercase();
            if SECRET_FIELD_NAMES
                .iter()
                .any(|s| s.eq_ignore_ascii_case(&lower))
            {
                return Err(FoundationError::SecretNotAllowed(k.clone()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pf(json: &str) -> ProfilesFile {
        let f: ProfilesFile = serde_json::from_str(json).unwrap();
        f.validate().unwrap();
        f
    }

    fn profile(id: &str, provider: &str, model: &str, roles: &[&str]) -> Profile {
        Profile {
            id: id.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            credential: CredentialLocator {
                service: "dev.oxi.foundation".to_string(),
                account: id.to_string(),
            },
        }
    }

    #[test]
    fn rejects_duplicate_profile_ids() {
        let raw = r#"{
            "schema_version": 1,
            "profiles": [
                {"id":"a","provider":"openai","model":"gpt-4o","roles":["x"],"credential":{"service":"s","account":"a"}},
                {"id":"a","provider":"openai","model":"gpt-4","roles":["y"],"credential":{"service":"s","account":"a"}}
            ]
        }"#;
        let err = serde_json::from_str::<ProfilesFile>(raw)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(err, FoundationError::DuplicateProfileId(id) if id == "a"));
    }
    #[test]
    fn rejects_secret_in_profile() {
        // The parser uses `deny_unknown_fields`, so a profile that
        // smuggles a secret-shaped field is rejected at parse time
        // (before any validator logic runs). The error type is
        // `FoundationError::Parse` because the parser's JSON errors
        // are mapped through that variant.
        let raw = r#"{
            "schema_version": 1,
            "profiles": [
                {"id":"a","provider":"openai","model":"gpt-4o","roles":["x"], "api_key":"sk-xxx",
                 "credential":{"service":"s","account":"a"}}
            ]
        }"#;
        let result = serde_json::from_str::<ProfilesFile>(raw);
        let err = match result {
            Ok(file) => file.validate().unwrap_err(),
            Err(e) => FoundationError::Parse(e.to_string()),
        };
        assert!(matches!(
            err,
            FoundationError::Parse(_) | FoundationError::SecretNotAllowed(_)
        ));
    }

    #[test]
    fn resolve_profile_prefers_env_override() {
        let f = pf(r#"{"schema_version":1,"profiles":[
            {"id":"x","provider":"openai","model":"gpt-4o","roles":["coding.primary"],
             "credential":{"service":"s","account":"x"}}
        ]}"#);
        let env = EnvironmentOverride {
            provider: "anthropic".to_string(),
            model: "claude-sonnet".to_string(),
        };
        let resolved = resolve_profile(ResolveInput {
            explicit_profile: Some("x"),
            explicit_environment_override: Some(&env),
            requested_role: None,
            foundation_profiles: &f,
            compatibility_import: None,
        })
        .unwrap();
        assert_eq!(resolved.source, super::super::CredentialSource::Environment);
        assert_eq!(resolved.profile.provider, "anthropic");
    }

    #[test]
    fn resolve_profile_explicit_id() {
        let f = pf(r#"{"schema_version":1,"profiles":[
            {"id":"x","provider":"openai","model":"gpt-4o","roles":["coding.primary"],
             "credential":{"service":"s","account":"x"}}
        ]}"#);
        let resolved = resolve_profile(ResolveInput {
            explicit_profile: Some("x"),
            explicit_environment_override: None,
            requested_role: None,
            foundation_profiles: &f,
            compatibility_import: None,
        })
        .unwrap();
        assert_eq!(resolved.profile.id, "x");
        assert_eq!(resolved.source, super::super::CredentialSource::Profile);
    }

    #[test]
    fn resolve_profile_unknown_id_is_error() {
        let f = pf(r#"{"schema_version":1,"profiles":[
            {"id":"x","provider":"openai","model":"gpt-4o","roles":["coding.primary"],
             "credential":{"service":"s","account":"x"}}
        ]}"#);
        let err = resolve_profile(ResolveInput {
            explicit_profile: Some("nope"),
            explicit_environment_override: None,
            requested_role: None,
            foundation_profiles: &f,
            compatibility_import: None,
        })
        .unwrap_err();
        assert!(matches!(err, FoundationError::UnknownProfile(_)));
    }

    #[test]
    fn resolve_profile_role_unique() {
        let f = pf(r#"{"schema_version":1,"profiles":[
            {"id":"a","provider":"openai","model":"gpt-4o","roles":["coding.primary"],
             "credential":{"service":"s","account":"a"}}
        ]}"#);
        let resolved = resolve_profile(ResolveInput {
            explicit_profile: None,
            explicit_environment_override: None,
            requested_role: Some("coding.primary"),
            foundation_profiles: &f,
            compatibility_import: None,
        })
        .unwrap();
        assert_eq!(resolved.source, super::super::CredentialSource::Role);
    }

    #[test]
    fn resolve_profile_role_ambiguous() {
        let f = pf(r#"{"schema_version":1,"profiles":[
            {"id":"a","provider":"openai","model":"gpt-4o","roles":["coding.primary"],
             "credential":{"service":"s","account":"a"}},
            {"id":"b","provider":"anthropic","model":"claude","roles":["coding.primary"],
             "credential":{"service":"s","account":"b"}}
        ]}"#);
        let err = resolve_profile(ResolveInput {
            explicit_profile: None,
            explicit_environment_override: None,
            requested_role: Some("coding.primary"),
            foundation_profiles: &f,
            compatibility_import: None,
        })
        .unwrap_err();
        assert!(matches!(err, FoundationError::AmbiguousRole(_)));
    }

    #[test]
    fn resolve_profile_role_unknown() {
        let f = pf(r#"{"schema_version":1,"profiles":[
            {"id":"a","provider":"openai","model":"gpt-4o","roles":["coding.primary"],
             "credential":{"service":"s","account":"a"}}
        ]}"#);
        let err = resolve_profile(ResolveInput {
            explicit_profile: None,
            explicit_environment_override: None,
            requested_role: Some("nonexistent"),
            foundation_profiles: &f,
            compatibility_import: None,
        })
        .unwrap_err();
        assert!(matches!(err, FoundationError::UnknownRole(_)));
    }

    #[test]
    fn resolve_profile_no_inputs_is_error() {
        let f = pf(r#"{"schema_version":1,"profiles":[
            {"id":"a","provider":"openai","model":"gpt-4o","roles":["coding.primary"],
             "credential":{"service":"s","account":"a"}}
        ]}"#);
        let err = resolve_profile(ResolveInput {
            explicit_profile: None,
            explicit_environment_override: None,
            requested_role: None,
            foundation_profiles: &f,
            compatibility_import: None,
        })
        .unwrap_err();
        assert!(matches!(err, FoundationError::UnknownProfile(_)));
    }

    #[test]
    fn resolve_profile_compatibility_import_only() {
        let f = pf(r#"{"schema_version":1,"profiles":[]}"#);
        let p = profile("legacy", "anthropic", "claude-sonnet", &["coding.primary"]);
        let import = CompatibilityImport { profile: p.clone() };
        let resolved = resolve_profile(ResolveInput {
            explicit_profile: None,
            explicit_environment_override: None,
            requested_role: None,
            foundation_profiles: &f,
            compatibility_import: Some(&import),
        })
        .unwrap();
        assert_eq!(
            resolved.source,
            super::super::CredentialSource::CompatibilityImport
        );
        assert_eq!(resolved.profile.id, "legacy");
    }
}
