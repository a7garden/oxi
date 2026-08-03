//! Pure data types shared across the hashline parser, applier, and patcher.
//! No filesystem, agent runtime, or schema library references — keep it pure.

/// A line-number anchor (1-indexed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    /// The 1-indexed source line this anchor points at.
    pub line: u32,
}

/// Where an `insert` edit should land relative to existing content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cursor {
    /// Insert at the beginning of the file (before line 1).
    Bof,
    /// Insert at the end of the file (after the last line).
    Eof,
    /// Insert immediately before the given anchor line.
    BeforeAnchor(Anchor),
    /// Insert immediately after the given anchor line.
    AfterAnchor(Anchor),
}

/// Insert mode for replacement lowering (SWAP lowers to Insert::Replacement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertMode {
    /// The insert replaces existing content at the cursor (SWAP lowering).
    Replacement,
}

/// Block operations mode (future: block-ops feature).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockMode {
    /// Insert content after the resolved block.
    InsertAfter,
}

/// A single low-level edit produced by the parser and consumed by the applier.
#[derive(Debug, Clone)]
pub enum Edit {
    /// Insert new text at a cursor position.
    Insert {
        /// Where the inserted text lands relative to existing content.
        cursor: Cursor,
        /// The literal text to insert.
        text: String,
        /// 1-indexed line number of this edit in the patch text (for diagnostics).
        line_num: u32,
        /// Position of this edit in the patch's edit list (for stable ordering).
        index: usize,
        /// Lowering mode for this insert, if any (e.g. SWAP-to-replacement).
        mode: Option<InsertMode>,
    },
    /// Delete the line at an anchor.
    Delete {
        /// The 1-indexed line this delete targets.
        anchor: Anchor,
        /// 1-indexed line number of this edit in the patch text (for diagnostics).
        line_num: u32,
        /// Position of this edit in the patch's edit list (for stable ordering).
        index: usize,
        /// Optional content asserted present at the deleted line (reserved for future guards).
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
    /// First line of the range (inclusive).
    pub start: u32,
    /// Last line of the range (inclusive).
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
    /// Maximum unchanged lines kept as context between hunks.
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
    /// First line of the resolved span (inclusive).
    pub start: u32,
    /// Last line of the resolved span (inclusive).
    pub end: u32,
}

/// One block anchor resolution (block-ops feature — future).
#[derive(Debug, Clone)]
pub struct BlockResolution {
    /// 1-indexed line that named the block being resolved.
    pub anchor_line: u32,
    /// First line of the resolved block (inclusive).
    pub start: u32,
    /// Last line of the resolved block (inclusive).
    pub end: u32,
    /// The operation to perform on the resolved block.
    pub op: BlockOp,
}

/// Kind of block operation to perform (block-ops feature — future).
#[derive(Debug, Clone, Copy)]
pub enum BlockOp {
    /// Replace the resolved block's content.
    Replace,
    /// Delete the resolved block.
    Delete,
    /// Insert new content after the resolved block.
    InsertAfter,
}

/// Request handed to a block resolver (block-ops feature — future).
#[derive(Debug, Clone)]
pub struct BlockResolverRequest {
    /// The source text the resolver searches within.
    pub text: String,
    /// 1-indexed line naming the block to resolve.
    pub anchor_line: u32,
}

/// Resolves a block anchor to a line span (block-ops feature — future).
pub type BlockResolver =
    std::sync::Arc<dyn Fn(&BlockResolverRequest) -> Option<BlockSpan> + Send + Sync>;
