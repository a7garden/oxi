//! Source info for slash commands and other resources

/// Metadata about where a resource was loaded from
#[derive(Debug, Clone)]
pub struct SourceInfo {
    /// Origin description
    pub origin: String,
}
