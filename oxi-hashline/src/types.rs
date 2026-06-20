//! Pure data types shared across the hashline parser, applier, and patcher.
//! No filesystem, agent runtime, or schema library references — keep it pure.

/// A line-number anchor (1-indexed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub line: u32,
}

/// Where an `insert` edit should land relative to existing content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cursor {
    Bof,
    Eof,
    BeforeAnchor(Anchor),
    AfterAnchor(Anchor),
}

/// Insert mode for replacement lowering (SWAP lowers to Insert::Replacement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertMode {
    Replacement,
}

/// Block operations mode (future: block-ops feature).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockMode {
    InsertAfter,
}

/// A single low-level edit produced by the parser and consumed by the applier.
#[derive(Debug, Clone)]
pub enum Edit {
    Insert {
        cursor: Cursor,
        text: String,
        line_num: u32,
        index: usize,
        mode: Option<InsertMode>,
    },
    Delete {
        anchor: Anchor,
        line_num: u32,
        index: usize,
        old_assertion: Option<String>,
    },
}

impl Edit {
    /// The 1-indexed line number in the patch text where this edit appears
    /// (for diagnostics).
    pub fn line_num(&self) -> u32 {
        match self {
            Edit::Insert { line_num, .. } | Edit::Delete { line_num, .. } => *line_num,
        }
    }

    /// The anchor line this edit targets (for session-chain recovery guards).
    pub fn anchor_line(&self) -> u32 {
        match self {
            Edit::Insert { cursor, .. } => match cursor {
                Cursor::BeforeAnchor(a) | Cursor::AfterAnchor(a) => a.line,
                Cursor::Bof => 0,
                Cursor::Eof => u32::MAX,
            },
            Edit::Delete { anchor, .. } => anchor.line,
        }
    }

    /// Index in the patch's edit list (for stable ordering).
    pub fn index(&self) -> usize {
        match self {
            Edit::Insert { index, .. } | Edit::Delete { index, .. } => *index,
        }
    }
}

/// Result of applying a parsed set of edits to a text body.
#[derive(Debug, Clone, Default)]
pub struct ApplyResult {
    /// The resulting text after all edits are applied.
    pub text: String,
    /// 1-indexed line number of the first changed line, if any.
    pub first_changed_line: Option<u32>,
    /// Warning messages generated during apply (boundary repair, landing
    /// correction, etc.).
    pub warnings: Vec<String>,
}

/// A parsed `start.=end` line range (1-indexed, inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedRange {
    pub start: u32,
    pub end: u32,
}

/// Optional hints for [`crate::parser::split_patch_input`].
#[derive(Debug, Clone, Default)]
pub struct SplitOptions {
    /// Root directory for resolving relative paths.
    pub root: Option<std::path::PathBuf>,
}

/// Result of [`crate::diff_preview::build_compact_diff_preview`].
#[derive(Debug, Clone, Default)]
pub struct CompactDiffPreview {
    /// Textual preview lines.
    pub lines: Vec<String>,
}

/// Optional knobs for compact diff preview.
#[derive(Debug, Clone)]
pub struct CompactDiffOptions {
    pub max_unchanged_context: usize,
}

impl Default for CompactDiffOptions {
    fn default() -> Self {
        Self {
            max_unchanged_context: 3,
        }
    }
}

/// Resolved 1-indexed inclusive line span of a `replace_block N:` target.
/// (block-ops feature — future)
#[derive(Debug, Clone, Copy)]
pub struct BlockSpan {
    pub start: u32,
    pub end: u32,
}

/// One block anchor resolution (block-ops feature — future).
#[derive(Debug, Clone)]
pub struct BlockResolution {
    pub anchor_line: u32,
    pub start: u32,
    pub end: u32,
    pub op: BlockOp,
}

#[derive(Debug, Clone, Copy)]
pub enum BlockOp {
    Replace,
    Delete,
    InsertAfter,
}

/// Request handed to a block resolver (block-ops feature — future).
#[derive(Debug, Clone)]
pub struct BlockResolverRequest {
    pub text: String,
    pub anchor_line: u32,
}

/// Resolves a block anchor to a line span (block-ops feature — future).
pub type BlockResolver =
    std::sync::Arc<dyn Fn(&BlockResolverRequest) -> Option<BlockSpan> + Send + Sync>;
