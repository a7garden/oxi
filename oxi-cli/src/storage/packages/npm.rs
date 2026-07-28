//! npm registry client for the package system.
//!
//! `NpmPackageInfo` fetches a package's manifest (versions + dist-tags)
//! from `registry.npmjs.org`, exposes the latest version via
//! `latest_version`, and supports best-match semver resolution against
//! the version map. The standalone `get_latest_npm_version` helper is
//! kept for the package manager's quick update check.

use crate::util::http_client::shared_http_client;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Information fetched from the npm registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NpmPackageInfo {
    /// pub.
    pub name: String,
    /// pub.
    pub versions: BTreeMap<String, serde_json::Value>,
    /// pub.
    #[serde(rename = "dist-tags")]
    pub dist_tags: BTreeMap<String, String>,
}

impl NpmPackageInfo {
    /// Fetch package info from the npm registry
    pub async fn fetch(name: &str) -> Result<Self> {
        let url = format!("https://registry.npmjs.org/{}", name);
        let client = shared_http_client();

        let resp = client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .with_context(|| format!("Failed to fetch npm info for '{}'", name))?;

        if !resp.status().is_success() {
            bail!("npm registry returned {} for '{}'", resp.status(), name);
        }

        let info: NpmPackageInfo = resp
            .json()
            .await
            .with_context(|| format!("Failed to parse npm registry response for '{}'", name))?;

        Ok(info)
    }

    /// Get the latest version from dist-tags
    pub fn latest_version(&self) -> Option<&str> {
        self.dist_tags.get("latest").map(|s| s.as_str())
    }

    /// Find the best matching version for a constraint
    pub fn resolve_version(&self, constraint: &str) -> Option<String> {
        if constraint == "latest" || constraint.is_empty() {
            return self.latest_version().map(|s| s.to_string());
        }

        // Try exact match first
        if self.versions.contains_key(constraint) {
            return Some(constraint.to_string());
        }

        // Try semver range matching
        if let Ok(req) = semver::VersionReq::parse(constraint) {
            let mut best: Option<semver::Version> = None;
            for ver_str in self.versions.keys() {
                if let Ok(ver) = semver::Version::parse(ver_str)
                    && req.matches(&ver)
                {
                    match &best {
                        Some(b) if ver > *b => best = Some(ver),
                        None => best = Some(ver),
                        _ => {}
                    }
                }
            }
            if let Some(v) = best {
                return Some(v.to_string());
            }
        }

        None
    }
}

/// Get the latest version of an npm package
pub async fn get_latest_npm_version(name: &str) -> Result<String> {
    let info = NpmPackageInfo::fetch(name).await?;
    info.latest_version()
        .map(|s| s.to_string())
        .context(format!("No latest version found for '{}'", name))
}
