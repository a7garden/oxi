//! Source-spec parsing for the package system.
//!
//! Every package source string flows through `ParsedSource::parse`,
//! which recognises npm specs, GitHub shorthand, raw git URLs, archive
//! URLs, and local paths. Helpers (`parse_npm_spec`,
//! `split_git_path_ref`, `parse_git_source`) are pure functions so they

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Parsed package source
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParsedSource {
    /// Variant.
    Npm {
        /// Full spec (e.g. "express@4.18.0")
        spec: String,
        /// Package name without version
        name: String,
        /// Whether a version was pinned
        pinned: bool,
    },
    /// Variant.
    Git {
        /// Full repository URL
        repo: String,
        /// Host (e.g. "github.com")
        host: String,
        /// Path on host (e.g. "org/repo")
        path: String,
        /// Optional ref (branch / tag / commit)
        ref_: Option<String>,
    },
    /// Variant.
    Local {
        /// Local path
        path: String,
    },
    /// Variant.
    Url {
        /// URL to archive
        url: String,
    },
}

impl ParsedSource {
    /// Parse a source string into a ParsedSource
    pub fn parse(source: &str) -> Self {
        if let Some(rest) = source.strip_prefix("npm:") {
            let spec = rest.trim();
            let (name, pinned) = parse_npm_spec(spec);
            return ParsedSource::Npm {
                spec: spec.to_string(),
                name,
                pinned,
            };
        }

        if let Some(rest) = source.strip_prefix("github:") {
            let parts: Vec<&str> = rest.splitn(2, '/').collect();
            if parts.len() == 2 {
                let (path, ref_) = split_git_path_ref(rest);
                return ParsedSource::Git {
                    repo: format!("https://github.com/{}.git", path),
                    host: "github.com".to_string(),
                    path,
                    ref_,
                };
            }
        }

        if source.starts_with("git+") || source.starts_with("git://") || source.starts_with("git@")
        {
            return parse_git_source(source);
        }

        if source.starts_with("https://") || source.starts_with("http://") {
            // Distinguish git URLs from plain archive URLs
            if source.ends_with(".git")
                || source.contains("github.com")
                || source.contains("gitlab.com")
            {
                return parse_git_source(source);
            }
            // Archive URL (.tar.gz, .zip, .tgz)
            if source.ends_with(".tar.gz")
                || source.ends_with(".tgz")
                || source.ends_with(".zip")
                || source.ends_with(".tar.bz2")
            {
                return ParsedSource::Url {
                    url: source.to_string(),
                };
            }
            // Default to git for http(s) URLs that look like repos
            return parse_git_source(source);
        }

        // Local path
        ParsedSource::Local {
            path: source.to_string(),
        }
    }

    /// Get a unique identity key for this source (ignoring version/ref)
    pub fn identity(&self) -> String {
        match self {
            ParsedSource::Npm { name, .. } => format!("npm:{}", name),
            ParsedSource::Git { host, path, .. } => format!("git:{}/{}", host, path),
            ParsedSource::Local { path } => format!("local:{}", path),
            ParsedSource::Url { url } => format!("url:{}", url),
        }
    }

    /// Get a display-friendly name
    pub fn display_name(&self) -> String {
        match self {
            ParsedSource::Npm { name, .. } => name.clone(),
            ParsedSource::Git { host, path, .. } => format!("{}/{}", host, path),
            ParsedSource::Local { path } => path.clone(),
            ParsedSource::Url { url } => url.clone(),
        }
    }
}

/// Cached regex for parsing npm package specs
pub(super) static NPM_SPEC_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(@?[^@]+(?:/[^@]+)?)(?:@(.+))?$").expect("valid static regex")
});

/// Parse an npm spec into (name, pinned)
pub(super) fn parse_npm_spec(spec: &str) -> (String, bool) {
    // Handle scoped packages like @scope/name@version
    if let Some(caps) = NPM_SPEC_RE.captures(spec) {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or(spec);
        let has_version = caps.get(2).is_some();
        return (name.to_string(), has_version);
    }
    (spec.to_string(), false)
}

/// Split a git path like "org/repo@ref" into ("org/repo", Some("ref"))
pub(super) fn split_git_path_ref(input: &str) -> (String, Option<String>) {
    if let Some(at_pos) = input.rfind('@') {
        // Make sure it's not part of an email (don't split if there's no / before @)
        if input[..at_pos].contains('/') {
            return (
                input[..at_pos].to_string(),
                Some(input[at_pos + 1..].to_string()),
            );
        }
    }
    (input.to_string(), None)
}

/// Parse a git source string
pub(super) fn parse_git_source(source: &str) -> ParsedSource {
    // Handle git@host:path format (SSH)
    if let Some(rest) = source.strip_prefix("git@") {
        let colon_pos = rest.find(':').unwrap_or(rest.len());
        let host = &rest[..colon_pos];
        let path_part = rest.get(colon_pos + 1..).unwrap_or("");
        let (path, ref_) = if let Some(hash_pos) = path_part.rfind('#') {
            (
                path_part[..hash_pos].to_string(),
                Some(path_part[hash_pos + 1..].to_string()),
            )
        } else {
            split_git_path_ref(path_part)
        };
        let repo = format!("git@{}:{}", host, path_part);
        let host = host.to_string();
        return ParsedSource::Git {
            repo,
            host,
            path: path.trim_end_matches(".git").to_string(),
            ref_,
        };
    }

    // Handle git+ssh://, git+https://, git:// prefixes
    let url_str = source
        .strip_prefix("git+")
        .unwrap_or(source)
        .strip_prefix("git://")
        .map(|s| format!("https://{}", s))
        .unwrap_or_else(|| source.strip_prefix("git+").unwrap_or(source).to_string());

    // Parse URL to extract host and path
    let url = match url::Url::parse(&url_str) {
        Ok(u) => u,
        Err(_) => {
            return ParsedSource::Local {
                path: source.to_string(),
            };
        }
    };

    let host = url.host_str().unwrap_or("unknown").to_string();
    let full_path = url.path().trim_start_matches('/').to_string();

    // Check for #ref fragment
    let fragment = url.fragment().map(|f| f.to_string());

    let (path, ref_) = if let Some(frag) = fragment {
        (full_path.trim_end_matches(".git").to_string(), Some(frag))
    } else {
        let (p, r) = split_git_path_ref(&full_path);
        (p.trim_end_matches(".git").to_string(), r)
    };

    let repo = url_str.clone();

    ParsedSource::Git {
        repo,
        host,
        path,
        ref_,
    }
}

/// Shell metacharacter rejection. The package system spawns external
/// processes (`npm pack`, `git clone`) with fields from `ParsedSource`
/// as arguments. Anything outside the strict whitelist below can become
/// a shell injection vector through arg-splitting differences and
/// packaging-tool escaping bugs.
///
/// Policy:
/// - npm `name`/`spec`: letters, digits, `.`, `_`, `-`, `/`, `@`, `~`,
///   `=`, `+`, leading dot, internal spaces are NOT allowed.
/// - git `repo`/`host`/`path`/`ref_`: alphanumerics, `.`, `_`, `-`, `:`,
///   `/`, `+`, `=`, `@`, `#`, `~` (no spaces, no backticks, no $).
/// - URL: any URL is treated as opaque — we never spawn `Command::new`
///   on it directly (the HTTP client takes the URL as a single
///   structured argument), so URLs are exempt from the same rule.
const NPM_SAFE_CHARS: &str = r"@a-zA-Z0-9._/+=~";
const GIT_SAFE_CHARS: &str = r"a-zA-Z0-9._:/=+@#-~";

/// Validate a shell-safe npm package spec string (the `name` or full
/// `spec` portion after stripping `npm:`).
pub(crate) fn validate_npm_spec(spec: &str) -> Result<(), String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err("npm spec must not be empty".to_string());
    }
    if trimmed.chars().any(|c| !is_safe_npm_char(c)) {
        return Err(format!(
            "npm spec '{trimmed}' contains shell metacharacters; \
             only {NPM_SAFE_CHARS} are permitted"
        ));
    }
    if trimmed.starts_with('.') {
        return Err(format!("npm spec '{trimmed}' must not start with '.'"));
    }
    Ok(())
}

/// Validate shell-safe fields on a git source.
pub(crate) fn validate_git_source(
    repo: &str,
    host: &str,
    path: &str,
    ref_: Option<&str>,
) -> Result<(), String> {
    fn check(field: &str, label: &str) -> Result<(), String> {
        if field.is_empty() {
            return Err(format!("git {label} must not be empty"));
        }
        if field.chars().any(|c| !is_safe_git_char(c)) {
            return Err(format!(
                "git {label} '{field}' contains shell metacharacters; \
                 only {GIT_SAFE_CHARS} are permitted"
            ));
        }
        Ok(())
    }
    check(repo, "repo")?;
    check(host, "host")?;
    check(path, "path")?;
    if let Some(r) = ref_ {
        check(r, "ref")?;
    }
    Ok(())
}

/// Reject a `ParsedSource` outright if any of its component fields that
/// are about to be passed to `Command::new` (or to `git_clone`'s args)
/// contain shell metacharacters. URLs are exempt because they always go
/// through `reqwest` which takes the URL as one structured argument.
pub(crate) fn validate_parsed_source(parsed: &ParsedSource) -> Result<(), String> {
    match parsed {
        ParsedSource::Npm { spec, name, .. } => {
            validate_npm_spec(spec)?;
            validate_npm_spec(name)?;
        }
        ParsedSource::Git {
            repo,
            host,
            path,
            ref_,
        } => {
            validate_git_source(repo, host, path, ref_.as_deref())?;
        }
        ParsedSource::Local { .. } | ParsedSource::Url { .. } => {}
    }
    Ok(())
}

fn is_safe_npm_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '@' | '+' | '=' | '~')
}

fn is_safe_git_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '.' | '_' | '-' | ':' | '/' | '+' | '=' | '@' | '#' | '~')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_safe_npm_specs() {
        for s in [
            "express",
            "express@4.18.0",
            "@scope/pkg",
            "@scope/pkg@1.2.3",
            "lodash-es@4.17.21+esm",
        ] {
            validate_npm_spec(s).unwrap_or_else(|e| panic!("{s}: {e}"));
        }
    }

    #[test]
    fn rejects_npm_specs_with_metachars() {
        for s in [
            "ex press",
            "$(rm -rf)",
            ";ls",
            "ex&press",
            "ex|press",
            "`ls`",
            "ex\npress",
            ".hidden-start",
            "@scope/pkg@1; rm",
            "npm:foo", // prefix is stripped before validate; just name
        ] {
            assert!(validate_npm_spec(s).is_err(), "{s} should be rejected");
        }
    }

    #[test]
    fn accepts_safe_git_fields() {
        validate_git_source(
            "https://github.com/org/repo.git",
            "github.com",
            "org/repo",
            Some("v1.2.3"),
        )
        .unwrap();
        validate_git_source(
            "https://github.com/org/repo.git",
            "github.com",
            "org/repo",
            None,
        )
        .unwrap();
    }

    #[test]
    fn rejects_git_fields_with_metachars() {
        assert!(
            validate_git_source(
                "https://github.com/org/repo;rm.git",
                "github.com",
                "org/repo",
                None
            )
            .is_err()
        );
        assert!(
            validate_git_source(
                "https://github.com/org/repo.git",
                "github.com|xx",
                "org/repo",
                None
            )
            .is_err()
        );
        assert!(
            validate_git_source(
                "https://github.com/org/repo.git",
                "github.com",
                "org/$(rm)repo",
                None
            )
            .is_err()
        );
        assert!(
            validate_git_source(
                "https://github.com/org/repo.git",
                "github.com",
                "org/repo",
                Some("v1; ls"),
            )
            .is_err()
        );
    }

    #[test]
    fn parsed_source_validation_picks_correct_branch() {
        validate_parsed_source(&ParsedSource::Npm {
            spec: "express".into(),
            name: "express".into(),
            pinned: false,
        })
        .unwrap();
        assert!(
            validate_parsed_source(&ParsedSource::Npm {
                spec: "ex;rm".into(),
                name: "express".into(),
                pinned: false,
            })
            .is_err()
        );

        validate_parsed_source(&ParsedSource::Git {
            repo: "https://github.com/o/r.git".into(),
            host: "github.com".into(),
            path: "o/r".into(),
            ref_: None,
        })
        .unwrap();
        assert!(
            validate_parsed_source(&ParsedSource::Git {
                repo: "https://github.com/o/r.git".into(),
                host: "github.com".into(),
                path: "o/r;ls".into(),
                ref_: None,
            })
            .is_err()
        );

        // URL & Local short-circuit (no spawn-time fields).
        validate_parsed_source(&ParsedSource::Url {
            url: "https://example.com/x.tar.gz?query=`whoami`".into(),
        })
        .unwrap();
        validate_parsed_source(&ParsedSource::Local {
            path: "/tmp/pkg;rm".into(),
        })
        .unwrap();
    }
}
