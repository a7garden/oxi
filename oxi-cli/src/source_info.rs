//! Source information tracking for messages and resources.
//!
//! Provides metadata describing where a resource originated.

/// Whether a source comes from the user, the project, or is temporary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceScope {
/// user variant.
    User,
/// project variant.
    Project,
/// temporary variant.
    Temporary,
}

impl Default for SourceScope {
    fn default() -> Self {
        Self::Temporary
    }
}

/// Whether a source is a package or top-level.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceOrigin {
/// package variant.
    Package,
/// top level variant.
    TopLevel,
}

impl Default for SourceOrigin {
    fn default() -> Self {
        Self::TopLevel
    }
}

/// Metadata describing where a resource came from.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    /// File path of the source.
    pub path: String,
    /// Source identifier (e.g. extension name, package name).
    pub source: String,
    /// Scope of the source (user, project, temporary).
    pub scope: SourceScope,
    /// Origin type (package or top-level).
    pub origin: SourceOrigin,
    /// Optional base directory.
    pub base_dir: Option<String>,
}

/// Metadata returned by the package manager, used to construct [`SourceInfo`].
#[derive(Debug, Clone)]
pub struct PathMetadata {
    pub source: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    pub base_dir: Option<String>,
}

/// Create a [`SourceInfo`] from a path and package-manager metadata.
pub fn create_source_info(path: impl Into<String>, metadata: &PathMetadata) -> SourceInfo {
    SourceInfo {
        path: path.into(),
        source: metadata.source.clone(),
        scope: metadata.scope.clone(),
        origin: metadata.origin.clone(),
        base_dir: metadata.base_dir.clone(),
    }
}

/// Create a synthetic [`SourceInfo`] with optional overrides.
///
/// Defaults: `scope` → [`SourceScope::Temporary`], `origin` → [`SourceOrigin::TopLevel`].
pub fn create_synthetic_source_info(
    path: impl Into<String>,
    source: impl Into<String>,
    scope: Option<SourceScope>,
    origin: Option<SourceOrigin>,
    base_dir: Option<String>,
) -> SourceInfo {
    SourceInfo {
        path: path.into(),
        source: source.into(),
        scope: scope.unwrap_or_default(),
        origin: origin.unwrap_or_default(),
        base_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_from_metadata() {
        let meta = PathMetadata {
            source: "my-ext".into(),
            scope: SourceScope::Project,
            origin: SourceOrigin::Package,
            base_dir: Some("/base".into()),
        };
        let info = create_source_info("/path/to/file", &meta);
        assert_eq!(info.path, "/path/to/file");
        assert_eq!(info.source, "my-ext");
        assert_eq!(info.scope, SourceScope::Project);
        assert_eq!(info.origin, SourceOrigin::Package);
        assert_eq!(info.base_dir.as_deref(), Some("/base"));
    }

    #[test]
    fn synthetic_defaults() {
        let info = create_synthetic_source_info("p", "s", None, None, None);
        assert_eq!(info.scope, SourceScope::Temporary);
        assert_eq!(info.origin, SourceOrigin::TopLevel);
        assert!(info.base_dir.is_none());
    }
}
