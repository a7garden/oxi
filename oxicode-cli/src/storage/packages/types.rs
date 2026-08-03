//! Type definitions for the package system.
//!
//! All core data types used across the package subsystem live here:
//! resource kinds, manifests, discovery records, scope/origin enums,
//! resolution paths, progress events, and update/configured-package
//! metadata. Splitting them out keeps the manager focused on lifecycle
//! logic instead of getting buried in field-level structs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Types of resources a package can contribute
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// extension variant.
    Extension,
    /// skill variant.
    Skill,
    /// prompt variant.
    Prompt,
    /// theme variant.
    Theme,
}
impl ResourceKind {
    /// All resource kinds, iteration order is stable.
    pub const ALL: [ResourceKind; 4] = [
        ResourceKind::Extension,
        ResourceKind::Skill,
        ResourceKind::Prompt,
        ResourceKind::Theme,
    ];
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceKind::Extension => write!(f, "extension"),
            ResourceKind::Skill => write!(f, "skill"),
            ResourceKind::Prompt => write!(f, "prompt"),
            ResourceKind::Theme => write!(f, "theme"),
        }
    }
}

// All resource kinds for iteration

/// Package manifest describing bundled resources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    /// Package name (e.g. "@foo/oxicode-tools")
    pub name: String,
    /// Semantic version (e.g. "1.0.0")
    pub version: String,
    /// Extension paths relative to the package root
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Skill names/paths
    #[serde(default)]
    pub skills: Vec<String>,
    /// Prompt template paths
    #[serde(default)]
    pub prompts: Vec<String>,
    /// Theme paths
    #[serde(default)]
    pub themes: Vec<String>,
    /// Optional description
    #[serde(default)]
    pub description: Option<String>,
    /// Package dependencies (name -> version constraint)
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

/// A discovered resource within a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredResource {
    /// Resource type
    pub kind: ResourceKind,
    /// Absolute path to the resource
    pub path: PathBuf,
    /// Relative path within the package
    pub relative_path: String,
}

/// Metadata about a resolved resource path
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathMetadata {
    /// Source specifier
    pub source: String,
    /// Scope (user / project)
    pub scope: SourceScope,
    /// Whether this is a package resource or top-level
    pub origin: ResourceOrigin,
    /// Base directory for resolving relative paths
    pub base_dir: Option<PathBuf>,
}

/// Origin of a resource
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceOrigin {
    /// package variant.
    Package,
    /// top level variant.
    TopLevel,
}

/// Scope for package sources
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    /// user variant.
    User,
    /// project variant.
    Project,
}

impl std::fmt::Display for SourceScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceScope::User => write!(f, "user"),
            SourceScope::Project => write!(f, "project"),
        }
    }
}

/// A resolved resource with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedResource {
    /// Absolute path to the resource
    pub path: PathBuf,
    /// Whether this resource is enabled
    pub enabled: bool,
    /// Metadata about the resource
    pub metadata: PathMetadata,
}

/// Resolved paths for all resource types
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolvedPaths {
    /// pub.
    pub extensions: Vec<ResolvedResource>,
    /// pub.
    pub skills: Vec<ResolvedResource>,
    /// pub.
    pub prompts: Vec<ResolvedResource>,
    /// pub.
    pub themes: Vec<ResolvedResource>,
}

/// Progress events for package operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    /// pub.
    pub event_type: ProgressEventType,
    /// pub.
    pub action: ProgressAction,
    /// pub.
    pub source: String,
    /// pub.
    pub message: Option<String>,
}

/// Progress event type
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressEventType {
    /// start variant.
    Start,
    /// progress variant.
    Progress,
    /// complete variant.
    Complete,
    /// error variant.
    Error,
}

/// Action being performed
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressAction {
    /// install variant.
    Install,
    /// remove variant.
    Remove,
    /// update variant.
    Update,
    /// clone variant.
    Clone,
    /// pull variant.
    Pull,
}

impl std::fmt::Display for ProgressAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgressAction::Install => write!(f, "install"),
            ProgressAction::Remove => write!(f, "remove"),
            ProgressAction::Update => write!(f, "update"),
            ProgressAction::Clone => write!(f, "clone"),
            ProgressAction::Pull => write!(f, "pull"),
        }
    }
}

/// Callback for progress events
pub type ProgressCallback = Box<dyn Fn(ProgressEvent) + Send + Sync>;

/// Information about an available package update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageUpdateInfo {
    /// pub.
    pub source: String,
    /// pub.
    pub display_name: String,
    /// pub.
    pub source_type: String, // "npm" or "git"
    /// pub.
    pub scope: SourceScope,
}

/// A configured package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfiguredPackage {
    /// pub.
    pub source: String,
    /// pub.
    pub scope: SourceScope,
    /// pub.
    pub filtered: bool,
    /// pub.
    pub installed_path: Option<PathBuf>,
}
