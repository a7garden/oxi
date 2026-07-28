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
