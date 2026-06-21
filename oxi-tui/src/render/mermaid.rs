//! Mermaid diagram rendering to terminal ASCII art.
//!
//! Pure-Rust subset renderer for the four most common Mermaid diagram types:
//! flowchart (`graph`/`flowchart`), sequence diagrams, state diagrams
//! (`stateDiagram` / `stateDiagram-v2`), and class diagrams. **No external
//! binaries required** — fully self-contained, no `mmdc` / Node dependency.
//!
//! Rendering is a three-stage pipeline:
//! 1. **Parse** the source into a typed intermediate representation.
//! 2. **Layout** — assign 2D coordinates to nodes / lifelines / edges.
//! 3. **Render** the layout into a string of box-drawing characters.
//!
//! Unsupported syntax (pie, gantt, ER, journey, gitGraph, requirement, etc.)
//! returns `None` so callers fall back to displaying the source as a fenced
//! code block.
//!
//! # Known limitations (v1)
//!
//! - Flowchart: only forward edges between adjacent BFS ranks are rendered
//!   as connectors; long-range back edges are recorded but may be dropped.
//! - CJK / wide characters in labels will misalign the box canvas (which is
//!   char-cell based). ASCII labels render perfectly.
//! - Stylistic directives (`classDef`, `style`, `linkStyle`, `click`) are
//!   ignored.
//! - State composite states (`state X { ... }`) are flattened.
//! - Class generics (`Class~T~`) render as plain `Class`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

use parking_lot::RwLock;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

// ===========================================================================
// Public API — options, cache, entry points
// ===========================================================================

/// Color mode for rendered Mermaid diagrams.
///
/// Currently informational only: all output is plain (uncolored) ASCII.
/// Reserved for a future theme-aware mode.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
pub enum MermaidColorMode {
    /// No color — plain ASCII output. This is the default.
    #[default]
    None,
    /// Theme-aware colored output (reserved; currently behaves as `None`).
    Themed,
}

/// Options that control how Mermaid diagrams are rendered.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MermaidRenderOptions {
    /// Color mode for the rendered diagram.
    pub color_mode: MermaidColorMode,
    /// Whether to request ASCII-art output (as opposed to SVG/PNG).
    ///
    /// Always `true` for the pure-Rust renderer; preserved for API
    /// compatibility with the previous `mmdc`-based implementation.
    pub use_ascii: bool,
    /// Maximum terminal width in columns for viewport adaptation.
    ///
    /// When `None`, no width constraint is applied. Currently advisory.
    pub max_width: Option<u16>,
}

impl Default for MermaidRenderOptions {
    fn default() -> Self {
        Self {
            color_mode: MermaidColorMode::None,
            use_ascii: true,
            max_width: None,
        }
    }
}

/// Process-level cache for Mermaid renders.
///
/// Keyed by source text + options; stores `None` for unparseable/unsupported
/// sources so they are not retried on every frame redraw.
static MERMAID_CACHE: LazyLock<RwLock<HashMap<String, Option<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Render Mermaid `source` to ASCII art using the pure-Rust subset renderer.
///
/// Supported diagram types: `graph`/`flowchart`, `sequenceDiagram`,
/// `stateDiagram` / `stateDiagram-v2`, `classDiagram`. Anything else returns
/// `None`.
///
/// Returns `None` when:
///
/// - `source` is empty or whitespace-only,
/// - the diagram type is unsupported,
/// - parsing fails irrecoverably.
///
/// Callers should fall back to displaying the raw code block on `None`.
pub fn render_mermaid_ascii(source: &str, options: &MermaidRenderOptions) -> Option<String> {
    let normalized = source.replace("\r\n", "\n");
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return None;
    }
    let key = cache_key(normalized, options);
    if let Some(cached) = MERMAID_CACHE.read().get(&key) {
        return cached.clone();
    }
    let rendered = render_uncached(normalized);
    MERMAID_CACHE.write().insert(key, rendered.clone());
    rendered
}

/// Clear all cached Mermaid renders.
///
/// After calling this, every diagram will be re-rendered from scratch on the
/// next call to [`render_mermaid_ascii`].
pub fn clear_mermaid_cache() {
    MERMAID_CACHE.write().clear();
}

fn cache_key(source: &str, options: &MermaidRenderOptions) -> String {
    format!(
        "{}\x00{:?}\x00{}",
        source, options.color_mode, options.use_ascii
    )
}

fn render_uncached(source: &str) -> Option<String> {
    let first = first_significant_line(source)?;
    let kind = first.split_whitespace().next()?.to_ascii_lowercase();
    match kind.as_str() {
        "graph" | "flowchart" => render_flowchart(source),
        "sequencediagram" => render_sequence(source),
        "statediagram" | "statediagram-v2" => render_state(source),
        "classdiagram" => render_class(source),
        _ => None,
    }
}

fn first_significant_line(source: &str) -> Option<&str> {
    source
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("%%"))
}

/// Strip a `%%`-introduced Mermaid comment from a line.
fn strip_comment(line: &str) -> &str {
    match line.find("%%") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

// ===========================================================================
// Canvas — char-cell grid for diagram rendering
// ===========================================================================

/// A 2D char-cell canvas. Coordinates are (x: column, y: row).
struct Canvas {
    cells: Vec<Vec<char>>,
    width: usize,
    height: usize,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            cells: vec![vec![' '; width]; height],
            width,
            height,
        }
    }

    /// Place a single char. Out-of-bounds writes are silently dropped.
    fn put(&mut self, x: isize, y: isize, c: char) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            self.cells[y as usize][x as usize] = c;
        }
    }

    /// Horizontal run of `c` from `x0` to `x1` inclusive.
    fn hline(&mut self, x0: isize, x1: isize, y: isize, c: char) {
        let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        for x in lo..=hi {
            self.put(x, y, c);
        }
    }

    /// Vertical run of `c` from `y0` to `y1` inclusive.
    fn vline(&mut self, x: isize, y0: isize, y1: isize, c: char) {
        let (lo, hi) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        for y in lo..=hi {
            self.put(x, y, c);
        }
    }

    /// Write an arbitrary string starting at (x, y). Multibyte chars occupy
    /// exactly one cell — wide CJK glyphs will misalign downstream canvas
    /// features. Acceptable for v1.
    fn write(&mut self, x: isize, y: isize, s: &str) {
        for (i, c) in s.chars().enumerate() {
            self.put(x + i as isize, y, c);
        }
    }

    /// Trim trailing whitespace from every row and join with `\n`.
    fn render(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                let s: String = row.iter().collect();
                s.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn unicode_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Measure the display width of the widest line in an ASCII string.
fn ascii_display_width(ascii: &str) -> usize {
    ascii.lines().map(UnicodeWidthStr::width).max().unwrap_or(0)
}

// ===========================================================================
// Flowchart
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Direction {
    #[default]
    Td,
    Lr,
    Rl,
    Bt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Shape {
    #[default]
    Rect,
    Rounded,
    Stadium,
    Diamond,
    Circle,
    Cylindrical,
    Subroutine,
    Hexagon,
    Parallelogram,
}

#[derive(Debug, Clone, Default)]
struct NodeDecl {
    id: String,
    label: Option<String>,
    shape: Shape,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrowStyle {
    SolidArrow,
    SolidLine,
    DashedArrow,
    DottedLine,
    ThickArrow,
    ThickLine,
    ArrowX,
    ArrowO,
    BidirArrow,
}

#[derive(Debug, Clone)]
struct Arrow {
    style: ArrowStyle,
    label: Option<String>,
}

#[derive(Debug, Clone)]
struct Edge {
    from: usize,
    to: usize,
    arrow: Arrow,
}

#[derive(Debug, Clone, Default)]
struct Flowchart {
    direction: Direction,
    nodes: Vec<NodeDecl>,
    edges: Vec<Edge>,
    node_index: HashMap<String, usize>,
}

fn render_flowchart(source: &str) -> Option<String> {
    let fc = parse_flowchart(source)?;
    if fc.nodes.is_empty() {
        return None;
    }
    let out = layout_and_render_flowchart(&fc);
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

fn parse_flowchart(source: &str) -> Option<Flowchart> {
    let mut fc = Flowchart::default();
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut shapes: HashMap<String, Shape> = HashMap::new();
    let mut known: HashSet<String> = HashSet::new();
    let mut known_order: Vec<String> = Vec::new();
    let mut edge_pairs: Vec<(String, String, Arrow)> = Vec::new();
    let mut saw_header = false;
    let mut subgraph_depth: i32 = 0;

    for raw in source.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();

        // First significant line: `graph` / `flowchart` + optional direction.
        if !saw_header
            && (lower == "graph"
                || lower == "flowchart"
                || lower.starts_with("graph ")
                || lower.starts_with("flowchart "))
        {
            saw_header = true;
            let dir = lower.split_whitespace().nth(1).unwrap_or("td");
            fc.direction = match dir {
                "td" | "tb" | "" => Direction::Td,
                "lr" => Direction::Lr,
                "rl" => Direction::Rl,
                "bt" => Direction::Bt,
                _ => Direction::Td,
            };
            continue;
        }

        // Subgraph open/close — track nesting only (statements inside are
        // parsed as ordinary nodes/edges).
        if lower.starts_with("subgraph") {
            subgraph_depth += 1;
            continue;
        }
        if lower == "end" && subgraph_depth > 0 {
            subgraph_depth -= 1;
            continue;
        }

        // Directives we ignore.
        if lower.starts_with("classdef")
            || lower.starts_with("class ")
            || lower.starts_with("style ")
            || lower.starts_with("linkstyle")
            || lower.starts_with("click")
            || lower.starts_with("direction ")
            || lower.starts_with("defaultlinkstyle")
            || lower.starts_with("title")
            || lower.starts_with("accdesc")
            || lower.starts_with("acc_title")
            || lower.starts_with("acc_descr")
        {
            continue;
        }

        // Allow a top-level `direction LR` mid-stream — updates direction.
        if lower.starts_with("direction ") {
            let dir = lower.split_whitespace().nth(1).unwrap_or("td");
            fc.direction = match dir {
                "td" | "tb" => Direction::Td,
                "lr" => Direction::Lr,
                "rl" => Direction::Rl,
                "bt" => Direction::Bt,
                _ => fc.direction,
            };
            continue;
        }

        for stmt in line.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            parse_flowchart_stmt(
                stmt,
                &mut known,
                &mut known_order,
                &mut labels,
                &mut shapes,
                &mut edge_pairs,
            );
        }
    }

    for id in &known_order {
        let idx = fc.nodes.len();
        let label = labels.get(id).cloned();
        let shape = *shapes.get(id).unwrap_or(&Shape::Rect);
        fc.nodes.push(NodeDecl {
            id: id.clone(),
            label,
            shape,
        });
        fc.node_index.insert(id.clone(), idx);
    }
    for (from, to, arrow) in edge_pairs {
        if let (Some(&f), Some(&t)) = (fc.node_index.get(&from), fc.node_index.get(&to)) {
            fc.edges.push(Edge {
                from: f,
                to: t,
                arrow,
            });
        }
    }
    Some(fc)
}

#[allow(clippy::too_many_arguments)]
fn parse_flowchart_stmt(
    stmt: &str,
    known: &mut HashSet<String>,
    known_order: &mut Vec<String>,
    labels: &mut HashMap<String, String>,
    shapes: &mut HashMap<String, Shape>,
    edges: &mut Vec<(String, String, Arrow)>,
) {
    let chars: Vec<char> = stmt.chars().collect();
    let mut pos;

    let (first_id, first_label, first_shape, consumed) = match parse_node_spec(&chars) {
        Some(v) => v,
        None => return,
    };
    pos = consumed;
    register_node(
        known,
        known_order,
        labels,
        shapes,
        first_id.clone(),
        first_label,
        first_shape,
    );

    let mut current = first_id;
    loop {
        skip_ws(&chars, &mut pos);
        if pos >= chars.len() {
            break;
        }
        // Arrow must begin with `-`, `=`, `.`, or `<`.
        if !"-=.<".contains(chars[pos]) {
            break;
        }
        let arrow_start = pos;
        while pos < chars.len() && "-=.xo<>".contains(chars[pos]) {
            pos += 1;
        }
        let arrow_text: String = chars[arrow_start..pos].iter().collect();
        if arrow_text.trim().is_empty() {
            break;
        }
        skip_ws(&chars, &mut pos);
        let mut label = None;
        if pos < chars.len() && chars[pos] == '|' {
            pos += 1;
            let lstart = pos;
            while pos < chars.len() && chars[pos] != '|' {
                pos += 1;
            }
            if pos < chars.len() {
                let lbl: String = chars[lstart..pos].iter().collect();
                label = Some(lbl.trim().to_string());
                pos += 1;
            }
        }
        let arrow = classify_arrow(arrow_text.trim(), label);

        skip_ws(&chars, &mut pos);
        let (next_id, next_label, next_shape, consumed) = match parse_node_spec(&chars[pos..]) {
            Some(v) => v,
            None => break,
        };
        pos += consumed;
        register_node(
            known,
            known_order,
            labels,
            shapes,
            next_id.clone(),
            next_label,
            next_shape,
        );
        edges.push((current.clone(), next_id.clone(), arrow));
        current = next_id;
    }
}

#[allow(clippy::too_many_arguments)]
fn register_node(
    known: &mut HashSet<String>,
    known_order: &mut Vec<String>,
    labels: &mut HashMap<String, String>,
    shapes: &mut HashMap<String, Shape>,
    id: String,
    label: Option<String>,
    shape: Option<Shape>,
) {
    if !known.contains(&id) {
        known.insert(id.clone());
        known_order.push(id.clone());
    }
    if let Some(l) = label {
        labels.insert(id.clone(), l);
    }
    if let Some(s) = shape {
        shapes.insert(id.clone(), s);
    }
}

fn skip_ws(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_whitespace() {
        *pos += 1;
    }
}

/// Parse a node spec at the start of `chars`.
/// Returns `(id, optional label, optional shape, chars consumed)`.
fn parse_node_spec(chars: &[char]) -> Option<(String, Option<String>, Option<Shape>, usize)> {
    let mut pos = 0;
    skip_ws(chars, &mut pos);
    let id_start = pos;
    while pos < chars.len() && !chars[pos].is_whitespace() && !"([{\"'".contains(chars[pos]) {
        pos += 1;
    }
    if pos == id_start {
        return None;
    }
    let id: String = chars[id_start..pos].iter().collect();
    skip_ws(chars, &mut pos);
    if pos >= chars.len() || !"(|[{\"'".contains(chars[pos]) {
        return Some((id, None, None, pos));
    }

    // Shape decorators: try longest openers first.
    // Each tuple: (opener, closer, shape).
    let patterns: &[(&str, &str, Shape)] = &[
        ("([", "])", Shape::Stadium),
        ("((", "))", Shape::Circle),
        ("{{", "}}", Shape::Hexagon),
        ("[(", ")]", Shape::Cylindrical),
        ("[[", "]]", Shape::Subroutine),
        ("[/", "/]", Shape::Parallelogram),
        ("[\\", "\\]", Shape::Parallelogram),
        ("[", "]", Shape::Rect),
        ("(", ")", Shape::Rounded),
        ("{", "}", Shape::Diamond),
    ];
    let rest: String = chars[pos..].iter().collect();
    for (open, close, shape) in patterns {
        if let Some(after_open) = rest.strip_prefix(open) {
            if let Some(end) = after_open.find(close) {
                let label_str = after_open[..end].trim().to_string();
                let consumed = pos + open.chars().count() + end + close.chars().count();
                let label_opt = if label_str.is_empty() {
                    None
                } else {
                    Some(label_str)
                };
                return Some((id, label_opt, Some(*shape), consumed));
            }
            // Opener without matching closer — treat as bare id.
            return Some((id, None, None, pos));
        }
    }
    Some((id, None, None, pos))
}

fn classify_arrow(text: &str, label: Option<String>) -> Arrow {
    let has_dash = text.contains('-');
    let has_eq = text.contains('=');
    let has_dot = text.contains('.');
    let has_x = text.contains('x') || text.contains('X');
    let has_o = text.contains('o') || text.contains('O');
    let has_lt = text.contains('<');
    let has_gt = text.contains('>');

    let style = if has_lt && has_gt {
        ArrowStyle::BidirArrow
    } else if has_eq && has_gt {
        ArrowStyle::ThickArrow
    } else if has_eq {
        ArrowStyle::ThickLine
    } else if has_dot && has_gt {
        ArrowStyle::DashedArrow
    } else if has_dot {
        ArrowStyle::DottedLine
    } else if has_x {
        ArrowStyle::ArrowX
    } else if has_o {
        ArrowStyle::ArrowO
    } else if has_gt {
        ArrowStyle::SolidArrow
    } else if has_dash {
        ArrowStyle::SolidLine
    } else {
        ArrowStyle::SolidArrow
    };
    Arrow { style, label }
}

// ---- Flowchart layout & render --------------------------------------------

const BOX_HPAD: usize = 1; // spaces on each side of label inside box
const BOX_MIN_INNER: usize = 3; // min label slot width
const LAYER_BOX_GAP: usize = 4; // horizontal gap between boxes in same layer
const LAYER_BAND: usize = 2; // rows between layers (TD) or cols (LR)
const ARROW_HEAD: char = '▼';
const ARROW_HEAD_RIGHT: char = '►';

fn layout_and_render_flowchart(fc: &Flowchart) -> String {
    match fc.direction {
        Direction::Td | Direction::Bt => render_flowchart_td(fc),
        Direction::Lr | Direction::Rl => render_flowchart_lr(fc),
    }
}

/// Longest-path rank assignment via Kahn's BFS.
/// Nodes in cycles (never reach in-degree 0) get rank 0.
fn compute_ranks(fc: &Flowchart) -> Vec<usize> {
    let n = fc.nodes.len();
    let mut ranks = vec![0usize; n];
    let mut in_degree = vec![0usize; n];
    let mut forward: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &fc.edges {
        forward[e.from].push(e.to);
        in_degree[e.to] += 1;
    }
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    while let Some(u) = queue.pop_front() {
        for &v in &forward[u] {
            ranks[v] = ranks[v].max(ranks[u] + 1);
            in_degree[v] -= 1;
            if in_degree[v] == 0 {
                queue.push_back(v);
            }
        }
    }
    ranks
}

/// Render a node as a 3-line ASCII box (top border, label row, bottom border).
fn render_node_box(node: &NodeDecl) -> Vec<String> {
    let label_raw = node.label.clone().unwrap_or_else(|| node.id.clone());
    // Clamp very long labels so a single node can't blow up the canvas.
    let label: String = if label_raw.chars().count() > 32 {
        let mut s: String = label_raw.chars().take(31).collect();
        s.push('…');
        s
    } else {
        label_raw
    };
    // Split multi-line labels on `\\n` (mermaid convention) and `\n`.
    let lines: Vec<String> = label
        .split("\\n")
        .flat_map(|s| s.split('\n'))
        .map(String::from)
        .collect();
    let inner_w = lines
        .iter()
        .map(|l| unicode_width(l))
        .max()
        .unwrap_or(0)
        .max(BOX_MIN_INNER);

    let (tl, tr, bl, br, side, fill) = box_chars(node.shape);

    let total_w = inner_w + 2 * BOX_HPAD;
    let pad = " ".repeat(BOX_HPAD);

    let mut top = String::new();
    top.push_str(tl);
    top.push_str(&fill.repeat(total_w));
    top.push_str(tr);

    let mut rows: Vec<String> = Vec::with_capacity(2 + lines.len());
    rows.push(top);
    for l in &lines {
        let lpad = (inner_w - unicode_width(l)) / 2;
        let rpad = inner_w - unicode_width(l) - lpad;
        let mut row = String::new();
        row.push_str(side);
        row.push_str(&pad);
        row.push_str(&" ".repeat(lpad));
        row.push_str(l);
        row.push_str(&" ".repeat(rpad));
        row.push_str(&pad);
        row.push_str(side);
        rows.push(row);
    }
    let mut bot = String::new();
    bot.push_str(bl);
    bot.push_str(&fill.repeat(total_w));
    bot.push_str(br);
    rows.push(bot);
    rows
}

/// `(top-left, top-right, bottom-left, bottom-right, side, fill)` for shape.
fn box_chars(
    shape: Shape,
) -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
) {
    match shape {
        Shape::Rect => ("┌", "┐", "└", "┘", "│", "─"),
        Shape::Rounded | Shape::Stadium => ("╭", "╮", "╰", "╯", "│", "─"),
        Shape::Cylindrical => ("╭", "╮", "╰", "╯", "│", "─"),
        Shape::Diamond => ("◆", "◆", "◆", "◆", "◆", "─"),
        Shape::Circle => ("◯", "◯", "◯", "◯", "│", "─"),
        Shape::Hexagon => ("⬢", "⬢", "⬢", "⬢", "│", "─"),
        Shape::Parallelogram => ("╱", "╱", "╱", "╱", "│", "─"),
        Shape::Subroutine => ("┌", "┐", "└", "┘", "║", "═"),
    }
}

fn box_width(box_rows: &[String]) -> usize {
    box_rows
        .iter()
        .map(|r| r.chars().count())
        .max()
        .unwrap_or(0)
}

fn render_flowchart_td(fc: &Flowchart) -> String {
    let ranks = compute_ranks(fc);
    let max_rank = ranks.iter().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<usize>> = Vec::with_capacity(max_rank + 1);
    for r in 0..=max_rank {
        let layer: Vec<usize> = (0..fc.nodes.len()).filter(|&i| ranks[i] == r).collect();
        layers.push(layer);
    }
    // Drop empty trailing layers.
    while layers.len() > 1 && layers.last().map(|v| v.is_empty()).unwrap_or(true) {
        layers.pop();
    }
    if layers.is_empty() || layers[0].is_empty() {
        return String::new();
    }

    // Render boxes.
    let boxes: Vec<Vec<String>> = (0..fc.nodes.len())
        .map(|i| render_node_box(&fc.nodes[i]))
        .collect();
    let widths: Vec<usize> = boxes.iter().map(|b| box_width(b)).collect();

    // Compute column offset for each node within its layer.
    let mut node_x: Vec<isize> = vec![0; fc.nodes.len()];
    let mut node_cx: Vec<isize> = vec![0; fc.nodes.len()]; // center x
    let mut layer_widths: Vec<usize> = Vec::with_capacity(layers.len());
    for layer in &layers {
        let mut x = 0isize;
        for (i, &n) in layer.iter().enumerate() {
            if i > 0 {
                x += LAYER_BOX_GAP as isize;
            }
            node_x[n] = x;
            node_cx[n] = x + (widths[n] / 2) as isize;
            x += widths[n] as isize;
        }
        layer_widths.push(x.max(0) as usize);
    }
    let canvas_w = layer_widths.iter().copied().max().unwrap_or(0) + 2;

    // Heights.
    let box_h = 3usize; // all boxes are 3 rows (single-line label); wider boxes have more rows.
    let max_box_h = boxes.iter().map(|b| b.len()).max().unwrap_or(box_h);
    let row_h = max_box_h;
    let n_layers = layers.len();
    let canvas_h = n_layers * row_h + n_layers.saturating_sub(1) * LAYER_BAND + 2;

    let mut canvas = Canvas::new(canvas_w, canvas_h);

    // Draw boxes layer by layer.
    let mut y = 1isize;
    let mut layer_top_y: Vec<isize> = Vec::with_capacity(n_layers);
    let mut layer_bot_y: Vec<isize> = Vec::with_capacity(n_layers);
    for layer in &layers {
        layer_top_y.push(y);
        for &n in layer {
            let rows = &boxes[n];
            for (dy, row) in rows.iter().enumerate() {
                canvas.write(node_x[n], y + dy as isize, row);
            }
        }
        y += row_h as isize;
        layer_bot_y.push(y - 1);
        y += LAYER_BAND as isize;
    }

    // Draw edges (only between adjacent ranks).
    let h_char = '─';
    for edge in &fc.edges {
        let r_from = ranks[edge.from];
        let r_to = ranks[edge.to];
        if r_from + 1 != r_to {
            // Non-adjacent: silently drop in v1.
            continue;
        }
        let band_top = layer_bot_y[r_from] + 1;
        let band_bot = layer_top_y[r_to] - 1;
        let x_from = node_cx[edge.from];
        let x_to = node_cx[edge.to];
        let (line_ch, head_ch) = arrow_chars_td(edge.arrow.style);
        if x_from == x_to {
            canvas.vline(x_from, band_top, band_bot - 1, line_ch);
            canvas.put(x_to, band_bot, head_ch);
        } else {
            // Elbow: down 1 from source, across to target column, down with head.
            canvas.vline(x_from, band_top, band_top, line_ch);
            let turning_top = if x_from < x_to { '┌' } else { '┐' };
            let _ = turning_top;
            // Use simple corner + horizontal + corner + vertical.
            // Band row 0: corner at x_from, hline to x_to, corner.
            let (c_from, c_to) = if x_from < x_to {
                ('└', '┐')
            } else {
                ('┘', '┌')
            };
            canvas.put(x_from, band_top, c_from);
            canvas.hline(x_from.min(x_to) + 1, x_from.max(x_to) - 1, band_top, h_char);
            canvas.put(x_to, band_top, c_to);
            // Band row 1: vertical down at x_to, head at bottom.
            canvas.put(x_to, band_top + 1, line_ch);
            canvas.put(x_to, band_bot, head_ch);
        }
        // Label placement: inside the band, on the horizontal elbow row.
        // For straight vertical edges we drop the label (it would clash with
        // the line); v1 limitation.
        if let Some(lbl) = &edge.arrow.label
            && x_from != x_to
        {
            let mid = (x_from + x_to) / 2;
            let half = (lbl.chars().count() as isize) / 2;
            canvas.write(mid - half, band_top, lbl);
        }
    }

    canvas.render()
}

fn render_flowchart_lr(fc: &Flowchart) -> String {
    // LR layout: layers become columns. Within a column, boxes stacked
    // vertically. Between columns, a horizontal connector band.
    let ranks = compute_ranks(fc);
    let max_rank = ranks.iter().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<usize>> = Vec::with_capacity(max_rank + 1);
    for r in 0..=max_rank {
        let layer: Vec<usize> = (0..fc.nodes.len()).filter(|&i| ranks[i] == r).collect();
        layers.push(layer);
    }
    while layers.len() > 1 && layers.last().map(|v| v.is_empty()).unwrap_or(true) {
        layers.pop();
    }
    if layers.is_empty() || layers[0].is_empty() {
        return String::new();
    }

    let boxes: Vec<Vec<String>> = (0..fc.nodes.len())
        .map(|i| render_node_box(&fc.nodes[i]))
        .collect();
    let widths: Vec<usize> = boxes.iter().map(|b| box_width(b)).collect();

    let max_box_w = widths.iter().copied().max().unwrap_or(0);

    let mut node_y: Vec<isize> = vec![0; fc.nodes.len()];
    let mut node_cy: Vec<isize> = vec![0; fc.nodes.len()];
    let mut layer_heights: Vec<usize> = Vec::with_capacity(layers.len());
    for layer in &layers {
        let mut y = 0isize;
        for (i, &n) in layer.iter().enumerate() {
            if i > 0 {
                y += LAYER_BOX_GAP as isize;
            }
            let h = boxes[n].len() as isize;
            node_y[n] = y;
            node_cy[n] = y + h / 2 + 1;
            y += h;
        }
        layer_heights.push(y.max(0) as usize);
    }
    let canvas_h = layer_heights.iter().copied().max().unwrap_or(0) + 2;
    let col_w = max_box_w;
    let n_layers = layers.len();
    let canvas_w = n_layers * col_w + n_layers.saturating_sub(1) * LAYER_BAND + 2;

    let mut canvas = Canvas::new(canvas_w, canvas_h);

    let mut x = 1isize;
    let mut layer_left_x: Vec<isize> = Vec::with_capacity(n_layers);
    let mut layer_right_x: Vec<isize> = Vec::with_capacity(n_layers);
    for layer in &layers {
        layer_left_x.push(x);
        for &n in layer {
            let rows = &boxes[n];
            for (dy, row) in rows.iter().enumerate() {
                canvas.write(x, node_y[n] + dy as isize + 1, row);
            }
        }
        x += col_w as isize;
        layer_right_x.push(x - 1);
        x += LAYER_BAND as isize;
    }

    let v_char = '│';
    let h_char = '─';
    for edge in &fc.edges {
        let r_from = ranks[edge.from];
        let r_to = ranks[edge.to];
        if r_from + 1 != r_to {
            continue;
        }
        let band_left = layer_right_x[r_from] + 1;
        let band_right = layer_left_x[r_to] - 1;
        let y_from = node_cy[edge.from];
        let y_to = node_cy[edge.to];
        let (line_ch, head_ch) = arrow_chars_lr(edge.arrow.style);
        if y_from == y_to {
            canvas.hline(band_left, band_right - 1, y_from, line_ch);
            canvas.put(band_right, y_from, head_ch);
        } else {
            // Elbow: right 1 from source, vertical to target row, right with head.
            let (c_from, c_to) = if y_from < y_to {
                ('┐', '└')
            } else {
                ('┘', '┌')
            };
            canvas.put(band_left, y_from, c_from);
            canvas.vline(
                band_left + 1,
                y_from.min(y_to) + 1,
                y_from.max(y_to) - 1,
                v_char,
            );
            canvas.put(band_left, y_to, c_to);
            canvas.hline(band_left + 1, band_right - 1, y_to, h_char);
            canvas.put(band_right, y_to, head_ch);
        }
        if let Some(lbl) = &edge.arrow.label {
            let mid = (y_from + y_to) / 2;
            let half = (lbl.chars().count() as isize) / 2;
            canvas.write(band_left + 1 - half, mid, lbl);
        }
    }

    canvas.render()
}

/// `(line_char, head_char)` for TD direction (downward).
fn arrow_chars_td(style: ArrowStyle) -> (char, char) {
    match style {
        ArrowStyle::SolidArrow | ArrowStyle::SolidLine => ('│', ARROW_HEAD),
        ArrowStyle::DashedArrow | ArrowStyle::DottedLine => ('┊', ARROW_HEAD),
        ArrowStyle::ThickArrow | ArrowStyle::ThickLine => ('┃', ARROW_HEAD),
        ArrowStyle::ArrowX => ('│', '✗'),
        ArrowStyle::ArrowO => ('│', '◯'),
        ArrowStyle::BidirArrow => ('│', ARROW_HEAD),
    }
}

/// `(line_char, head_char)` for LR direction (rightward).
fn arrow_chars_lr(style: ArrowStyle) -> (char, char) {
    match style {
        ArrowStyle::SolidArrow | ArrowStyle::SolidLine => ('─', ARROW_HEAD_RIGHT),
        ArrowStyle::DashedArrow | ArrowStyle::DottedLine => ('┄', ARROW_HEAD_RIGHT),
        ArrowStyle::ThickArrow | ArrowStyle::ThickLine => ('━', ARROW_HEAD_RIGHT),
        ArrowStyle::ArrowX => ('─', '✗'),
        ArrowStyle::ArrowO => ('─', '◯'),
        ArrowStyle::BidirArrow => ('─', ARROW_HEAD_RIGHT),
    }
}

// ===========================================================================
// Sequence diagram
// ===========================================================================

#[derive(Debug, Clone)]
struct SeqParticipant {
    label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeqArrowKind {
    Solid,    // ->> solid with arrowhead
    Dashed,   // -->> dashed with arrowhead
    Thin,     // -> solid without arrowhead
    ThinDash, // --> dashed without arrowhead
    Crossed,  // --x dashed with cross
}

#[derive(Debug, Clone)]
struct SeqMessage {
    from: usize,
    to: usize,
    kind: SeqArrowKind,
    text: String,
}

#[derive(Debug, Clone)]
enum SeqNote {
    Over { raw_keys: Vec<String>, text: String },
    LeftOf { raw_key: String, text: String },
    RightOf { raw_key: String, text: String },
}

#[derive(Debug, Clone)]
struct Sequence {
    participants: Vec<SeqParticipant>,
    pindex: HashMap<String, usize>,
    events: Vec<SeqEvent>,
}

#[derive(Debug, Clone)]
enum SeqEvent {
    Message(SeqMessage),
    Note(SeqNote),
}

fn render_sequence(source: &str) -> Option<String> {
    let seq = parse_sequence(source)?;
    if seq.participants.is_empty() {
        return None;
    }
    Some(render_sequence_diagram(&seq))
}

fn parse_sequence(source: &str) -> Option<Sequence> {
    let mut participants: Vec<SeqParticipant> = Vec::new();
    let mut pindex: HashMap<String, usize> = HashMap::new();
    let mut events: Vec<SeqEvent> = Vec::new();
    let mut saw_header = false;
    // Track explicit participant order; auto-create on first message reference.
    let ensure = |participants: &mut Vec<SeqParticipant>,
                  pindex: &mut HashMap<String, usize>,
                  key: &str|
     -> usize {
        if let Some(&i) = pindex.get(key) {
            return i;
        }
        let i = participants.len();
        participants.push(SeqParticipant {
            label: key.to_string(),
        });
        pindex.insert(key.to_string(), i);
        i
    };

    for raw in source.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if !saw_header {
            if line.eq_ignore_ascii_case("sequencediagram") {
                saw_header = true;
                continue;
            }
            // Skip leading preamble lines (autonumber etc.).
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("autonumber") {
                continue;
            }
            // If first significant line isn't the header, abort.
            return None;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("autonumber") {
            continue;
        }
        if lower.starts_with("participant ") || lower.starts_with("actor ") {
            let rest = line.split_once(' ').map(|(_, r)| r).unwrap_or("").trim();
            // Patterns: `A` or `A as "Alice"` or `A as Alice`.
            let (key, label) = split_participant_decl(rest);
            let i = participants.len();
            participants.push(SeqParticipant { label });
            pindex.insert(key, i);
            continue;
        }
        if lower.starts_with("note ") {
            if let Some(note) = parse_seq_note(line) {
                events.push(SeqEvent::Note(note));
            }
            continue;
        }
        // Loop / alt / opt / par — parsed transparently (no framing drawn in v1).
        if lower == "end"
            || lower.starts_with("loop ")
            || lower.starts_with("alt ")
            || lower == "else"
            || lower.starts_with("opt ")
            || lower.starts_with("par ")
            || lower.starts_with("rect ")
            || lower.starts_with("critical ")
            || lower.starts_with("option ")
            || lower.starts_with("break ")
        {
            continue;
        }
        // Activate / deactivate — ignored in v1.
        if lower.starts_with("activate ") || lower.starts_with("deactivate ") {
            continue;
        }
        // Message line.
        if let Some(msg) = parse_seq_message(line, &mut participants, &mut pindex, &ensure) {
            events.push(SeqEvent::Message(msg));
        }
    }
    if !saw_header {
        return None;
    }
    Some(Sequence {
        participants,
        pindex,
        events,
    })
}

fn split_participant_decl(rest: &str) -> (String, String) {
    // Strip surrounding quotes; handle ` as ` alias.
    let cleaned = rest.trim();
    if let Some(idx) = cleaned.find(" as ") {
        let key = cleaned[..idx].trim().trim_matches('"').to_string();
        let label = cleaned[idx + 4..].trim().trim_matches('"').to_string();
        return (key, label);
    }
    let s = cleaned.trim_matches('"').to_string();
    (s.clone(), s)
}

#[allow(clippy::type_complexity)]
fn parse_seq_message(
    line: &str,
    participants: &mut Vec<SeqParticipant>,
    pindex: &mut HashMap<String, usize>,
    ensure: &impl Fn(&mut Vec<SeqParticipant>, &mut HashMap<String, usize>, &str) -> usize,
) -> Option<SeqMessage> {
    // Find the arrow token: a run of `-`, `>`, `.`, `x`, `+`, `)`.
    // Pattern: FROM ARROW TO : TEXT
    // Strategy: locate the arrow (sequence of `-><.x+`), split on it.
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_whitespace() {
        i += 1;
    }
    let from_start = i;
    while i < bytes.len() && !bytes[i].is_whitespace() && !"->.x+".contains(bytes[i]) {
        i += 1;
    }
    if i == from_start {
        return None;
    }
    let from_key: String = bytes[from_start..i].iter().collect();
    while i < bytes.len() && bytes[i].is_whitespace() {
        i += 1;
    }
    // Now read arrow token: chars from the set [-+.>x)(] with at least one `-` or `.`
    let arrow_start = i;
    while i < bytes.len() && "-.>x+)(<".contains(bytes[i]) {
        i += 1;
    }
    if i == arrow_start {
        return None;
    }
    let arrow_text: String = bytes[arrow_start..i].iter().collect();
    while i < bytes.len() && bytes[i].is_whitespace() {
        i += 1;
    }
    let to_start = i;
    while i < bytes.len() && !":".contains(bytes[i]) && !bytes[i].is_whitespace() {
        i += 1;
    }
    if i == to_start {
        return None;
    }
    let to_key: String = bytes[to_start..i].iter().collect();
    // Optional `: text`
    let text = if i < bytes.len() && bytes[i] == ':' {
        let rest: String = bytes[i + 1..].iter().collect();
        rest.trim().to_string()
    } else {
        String::new()
    };

    let from = ensure(participants, pindex, &from_key);
    let to = ensure(participants, pindex, &to_key);
    let kind = classify_seq_arrow(&arrow_text);
    Some(SeqMessage {
        from,
        to,
        kind,
        text,
    })
}

fn classify_seq_arrow(s: &str) -> SeqArrowKind {
    // Mermaid sequence-arrow semantics:
    //   `->`   solid line without arrowhead
    //   `-->`  dashed line without arrowhead
    //   `->>`  solid line with arrowhead
    //   `-->>` dashed line with arrowhead
    //   `-x` / `--x` solid / dashed with cross
    // The number of dashes (>=2 → dashed) is what distinguishes the variants.
    let dash_count = s.chars().filter(|&c| c == '-').count();
    let dashed = dash_count >= 2;
    let has_double_right = s.contains(">>");
    let has_x = s.contains('x');
    if has_x {
        return SeqArrowKind::Crossed;
    }
    if dashed {
        if has_double_right {
            SeqArrowKind::Dashed
        } else {
            SeqArrowKind::ThinDash
        }
    } else if has_double_right {
        SeqArrowKind::Solid
    } else {
        SeqArrowKind::Thin
    }
}

fn parse_seq_note(line: &str) -> Option<SeqNote> {
    // Single-line forms only. Multi-line `Note over A:` ... `end note` blocks
    // are not supported in v1.
    let rest = line
        .strip_prefix("Note")
        .or_else(|| line.strip_prefix("note"))?
        .trim();
    let (kind_str, tail) = if let Some(t) = rest.strip_prefix("left of") {
        ("left", t)
    } else if let Some(t) = rest.strip_prefix("right of") {
        ("right", t)
    } else if let Some(t) = rest.strip_prefix("over") {
        ("over", t)
    } else {
        return None;
    };
    let tail = tail.trim();
    let (refs, text) = match tail.find(':') {
        Some(idx) => (&tail[..idx], tail[idx + 1..].trim().to_string()),
        None => (tail, String::new()),
    };
    let raw_keys: Vec<String> = refs.split(',').map(|s| s.trim().to_string()).collect();
    let note = match kind_str {
        "left" => SeqNote::LeftOf {
            raw_key: raw_keys.into_iter().next().unwrap_or_default(),
            text,
        },
        "right" => SeqNote::RightOf {
            raw_key: raw_keys.into_iter().next().unwrap_or_default(),
            text,
        },
        _ => SeqNote::Over { raw_keys, text },
    };
    Some(note)
}

fn render_sequence_diagram(seq: &Sequence) -> String {
    let n = seq.participants.len();
    // Layout: each participant gets a column of fixed width = max(label_width, 6) + 4.
    let lane_w: Vec<usize> = seq
        .participants
        .iter()
        .map(|p| unicode_width(&p.label).max(6) + 4)
        .collect();
    // Right padding accommodates self-loop labels and centered arrow labels
    // that overflow the lane span.
    let max_label_w: usize = seq
        .events
        .iter()
        .map(|e| match e {
            SeqEvent::Message(m) => unicode_width(&m.text),
            SeqEvent::Note(n) => match n {
                SeqNote::Over { text, .. }
                | SeqNote::LeftOf { text, .. }
                | SeqNote::RightOf { text, .. } => unicode_width(text),
            },
        })
        .max()
        .unwrap_or(0);
    let total_w = lane_w.iter().sum::<usize>() + max_label_w + 8;
    // Per-lane center x (column center of each participant's lane).
    let mut lane_cx: Vec<isize> = Vec::with_capacity(n);
    {
        let mut x = 2isize;
        for w in &lane_w {
            x += (*w as isize) / 2;
            lane_cx.push(x);
            x += (*w as isize) - (*w as isize) / 2;
        }
    }

    // Top: participant header boxes (3 rows).
    // Each event: 2 rows (arrow row + optional text row).
    let header_h = 3usize;
    let event_h = 2usize;
    let h = header_h + seq.events.len() * event_h + 2;
    let mut canvas = Canvas::new(total_w, h);

    // Draw participant headers.
    let header_top = 0isize;
    let header_bot = header_top + (header_h as isize) - 1;
    for (i, p) in seq.participants.iter().enumerate() {
        let w = lane_w[i];
        let left = lane_cx[i] - (w as isize) / 2;
        // Top border.
        canvas.put(left, header_top, '┌');
        canvas.hline(left + 1, left + w as isize - 2, header_top, '─');
        canvas.put(left + w as isize - 1, header_top, '┐');
        // Label row.
        let label = &p.label;
        let lw = unicode_width(label);
        let lpad = (w.saturating_sub(lw + 2)) / 2;
        let rpad = w.saturating_sub(lw + 2) - lpad;
        canvas.put(left, header_top + 1, '│');
        canvas.write(left + 1 + lpad as isize, header_top + 1, label);
        let _ = rpad;
        canvas.put(left + w as isize - 1, header_top + 1, '│');
        // Bottom border.
        canvas.put(left, header_bot, '└');
        canvas.hline(left + 1, left + w as isize - 2, header_bot, '─');
        canvas.put(left + w as isize - 1, header_bot, '┘');
    }

    // Draw lifelines (vertical dashed) under each participant.
    let lifeline_top = header_bot + 1;
    let lifeline_bot = h as isize - 2;
    for cx in &lane_cx {
        let mut y = lifeline_top;
        let mut on = true;
        while y <= lifeline_bot {
            canvas.put(*cx, y, if on { '┊' } else { ' ' });
            // Notes/messages overwrite lifeline cells; that's fine.
            on = !on;
            y += 1;
        }
    }

    // Render events.
    let mut y = header_top + header_h as isize;
    for ev in &seq.events {
        match ev {
            SeqEvent::Message(msg) => {
                let (line_ch, head_ch, tail_ch) = seq_arrow_chars(msg.kind);
                let x0 = lane_cx[msg.from];
                let x1 = lane_cx[msg.to];
                if msg.from == msg.to {
                    // Self-loop: short elbow on the right of the lane.
                    let bx = x0 + 1;
                    canvas.put(x0, y, tail_ch);
                    canvas.hline(x0 + 1, bx + 3, y, line_ch);
                    canvas.put(bx + 3, y, '┐');
                    canvas.vline(bx + 3, y + 1, y + 1, '│');
                    canvas.hline(x0 + 1, bx + 3, y + 1, line_ch);
                    canvas.put(x0 + 1, y + 1, head_ch);
                    if !msg.text.is_empty() {
                        canvas.write(x0 + 5, y, &msg.text);
                    }
                } else if x0 < x1 {
                    canvas.put(x0, y, tail_ch);
                    canvas.hline(x0 + 1, x1 - 1, y, line_ch);
                    canvas.put(x1, y, head_ch);
                    if !msg.text.is_empty() {
                        let mid = (x0 + x1) / 2 - (msg.text.chars().count() as isize) / 2;
                        canvas.write(mid, y + 1, &msg.text);
                    }
                } else {
                    canvas.put(x0, y, tail_ch);
                    canvas.hline(x1 + 1, x0 - 1, y, line_ch);
                    canvas.put(x1, y, head_ch);
                    if !msg.text.is_empty() {
                        let mid = (x0 + x1) / 2 - (msg.text.chars().count() as isize) / 2;
                        canvas.write(mid, y + 1, &msg.text);
                    }
                }
                y += event_h as isize;
            }
            SeqEvent::Note(note) => {
                let (text, center_x) = match note {
                    SeqNote::Over { raw_keys, text } => {
                        let first = raw_keys.first().and_then(|k| seq.pindex.get(k)).copied();
                        let last = raw_keys.last().and_then(|k| seq.pindex.get(k)).copied();
                        let cx = match (first, last) {
                            (Some(f), Some(l)) => (lane_cx[f] + lane_cx[l]) / 2,
                            _ => lane_cx[0],
                        };
                        (text.clone(), cx)
                    }
                    SeqNote::LeftOf { raw_key, text } => {
                        let idx = seq.pindex.get(raw_key).copied().unwrap_or(0);
                        (text.clone(), lane_cx[idx] - 6)
                    }
                    SeqNote::RightOf { raw_key, text } => {
                        let idx = seq.pindex.get(raw_key).copied().unwrap_or(0);
                        (text.clone(), lane_cx[idx] + 6)
                    }
                };
                let tw = unicode_width(&text);
                let left = center_x - (tw as isize) / 2 - 1;
                canvas.put(left, y, '┌');
                canvas.hline(left + 1, left + tw as isize, y, '─');
                canvas.put(left + tw as isize + 1, y, '┐');
                canvas.put(left, y + 1, '│');
                canvas.write(left + 1, y + 1, &text);
                canvas.put(left + tw as isize + 1, y + 1, '│');
                canvas.put(left, y + 2, '└');
                canvas.hline(left + 1, left + tw as isize, y + 2, '─');
                canvas.put(left + tw as isize + 1, y + 2, '┘');
                y += event_h as isize;
            }
        }
    }

    canvas.render()
}

fn seq_arrow_chars(kind: SeqArrowKind) -> (char, char, char) {
    // (line, head-at-target, tail-at-source)
    match kind {
        SeqArrowKind::Solid => ('─', '►', '│'),
        SeqArrowKind::Thin => ('─', '►', '│'),
        SeqArrowKind::Dashed => ('┄', '►', '│'),
        SeqArrowKind::ThinDash => ('┄', '►', '│'),
        SeqArrowKind::Crossed => ('─', '✗', '│'),
    }
}

// ===========================================================================
// State diagram — reuses flowchart renderer with synthetic nodes
// ===========================================================================

fn render_state(source: &str) -> Option<String> {
    let fc = parse_state_as_flowchart(source)?;
    if fc.nodes.is_empty() {
        return None;
    }
    let out = layout_and_render_flowchart(&fc);
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Parse a `stateDiagram-v2` (or v1) source into a [`Flowchart`] by treating
/// each state as a node and each `A --> B : label` as an edge.
///
/// The Mermaid start marker `[*]` becomes a synthetic filled-circle node
/// `__start__`; the end marker becomes `__end__`.
fn parse_state_as_flowchart(source: &str) -> Option<Flowchart> {
    let mut fc = Flowchart {
        direction: Direction::Td,
        ..Default::default()
    };
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut shapes: HashMap<String, Shape> = HashMap::new();
    let mut known: HashSet<String> = HashSet::new();
    let mut known_order: Vec<String> = Vec::new();
    let mut edge_pairs: Vec<(String, String, Arrow)> = Vec::new();
    let mut saw_header = false;

    for raw in source.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if !saw_header {
            let lower = line.to_ascii_lowercase();
            if lower == "statediagram" || lower == "statediagram-v2" {
                saw_header = true;
                continue;
            }
            // Skip note lines before header.
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("note ") || lower == "end note" || lower.starts_with("direction ") {
            continue;
        }
        // Composite state open/close: ignored (flattened).
        if lower.starts_with("state ") && line.contains('{') {
            continue;
        }
        if line == "}" {
            continue;
        }
        // State declaration with label: `state "Label" as S1` or `state S1`
        if lower.starts_with("state ") {
            // Try `"Label" as Id`
            let rest = line["state ".len()..].trim();
            if let Some(close) = rest.find('"')
                && let Some(close2) = rest[close + 1..].find('"')
            {
                let label = &rest[close + 1..close + 1 + close2];
                let after = rest[close + 1 + close2 + 1..].trim();
                if let Some(id) = after.strip_prefix("as ").map(str::trim) {
                    register_state_node(
                        &mut known,
                        &mut known_order,
                        &mut labels,
                        &mut shapes,
                        id.to_string(),
                        Some(label.to_string()),
                        Shape::Rounded,
                    );
                    continue;
                }
            }
            // Bare `state S1`
            let id = rest.trim();
            if !id.is_empty() {
                register_state_node(
                    &mut known,
                    &mut known_order,
                    &mut labels,
                    &mut shapes,
                    id.to_string(),
                    None,
                    Shape::Rounded,
                );
            }
            continue;
        }

        // Transition: `A --> B` or `A --> B : Transition label`.
        if let Some(parsed) = parse_state_transition(line) {
            let (from, to, label) = parsed;
            if from == "[*]" {
                let start_id = "__start__".to_string();
                register_state_node(
                    &mut known,
                    &mut known_order,
                    &mut labels,
                    &mut shapes,
                    start_id.clone(),
                    Some("(*)".to_string()),
                    Shape::Circle,
                );
                edge_pairs.push((
                    start_id,
                    to,
                    Arrow {
                        style: ArrowStyle::SolidArrow,
                        label,
                    },
                ));
            } else if to == "[*]" {
                let end_id = "__end__".to_string();
                register_state_node(
                    &mut known,
                    &mut known_order,
                    &mut labels,
                    &mut shapes,
                    end_id.clone(),
                    Some("(*)".to_string()),
                    Shape::Circle,
                );
                register_state_node(
                    &mut known,
                    &mut known_order,
                    &mut labels,
                    &mut shapes,
                    from.clone(),
                    None,
                    Shape::Rounded,
                );
                edge_pairs.push((
                    from,
                    end_id,
                    Arrow {
                        style: ArrowStyle::SolidArrow,
                        label,
                    },
                ));
            } else {
                register_state_node(
                    &mut known,
                    &mut known_order,
                    &mut labels,
                    &mut shapes,
                    from.clone(),
                    None,
                    Shape::Rounded,
                );
                register_state_node(
                    &mut known,
                    &mut known_order,
                    &mut labels,
                    &mut shapes,
                    to.clone(),
                    None,
                    Shape::Rounded,
                );
                edge_pairs.push((
                    from,
                    to,
                    Arrow {
                        style: ArrowStyle::SolidArrow,
                        label,
                    },
                ));
            }
        }
    }
    if !saw_header {
        return None;
    }

    for id in &known_order {
        let idx = fc.nodes.len();
        let label = labels.get(id).cloned();
        let shape = *shapes.get(id).unwrap_or(&Shape::Rounded);
        fc.nodes.push(NodeDecl {
            id: id.clone(),
            label,
            shape,
        });
        fc.node_index.insert(id.clone(), idx);
    }
    for (from, to, arrow) in edge_pairs {
        if let (Some(&f), Some(&t)) = (fc.node_index.get(&from), fc.node_index.get(&to)) {
            fc.edges.push(Edge {
                from: f,
                to: t,
                arrow,
            });
        }
    }
    Some(fc)
}

#[allow(clippy::too_many_arguments)]
fn register_state_node(
    known: &mut HashSet<String>,
    known_order: &mut Vec<String>,
    labels: &mut HashMap<String, String>,
    shapes: &mut HashMap<String, Shape>,
    id: String,
    label: Option<String>,
    shape: Shape,
) {
    if !known.contains(&id) {
        known.insert(id.clone());
        known_order.push(id.clone());
        shapes.insert(id.clone(), shape);
    }
    if let Some(l) = label {
        labels.insert(id.clone(), l);
    }
}

/// Parse a state transition line. Returns `(from, to, optional label)`.
fn parse_state_transition(line: &str) -> Option<(String, String, Option<String>)> {
    // Find the arrow `-->` (or `-->`, `=>`, etc.).
    let chars: Vec<char> = line.chars().collect();
    let mut pos = 0;
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }
    let from_start = pos;
    while pos < chars.len() && !chars[pos].is_whitespace() && !"-=>".contains(chars[pos]) {
        pos += 1;
    }
    if pos == from_start {
        return None;
    }
    let from: String = chars[from_start..pos].iter().collect();
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }
    if pos >= chars.len() || !"-=".contains(chars[pos]) {
        return None;
    }
    let arrow_start = pos;
    while pos < chars.len() && "-=.".contains(chars[pos]) {
        pos += 1;
    }
    // Optional `>`
    if pos < chars.len() && chars[pos] == '>' {
        pos += 1;
    }
    let _arrow_text: String = chars[arrow_start..pos].iter().collect();
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }
    let to_start = pos;
    while pos < chars.len() && !chars[pos].is_whitespace() && chars[pos] != ':' {
        pos += 1;
    }
    if pos == to_start {
        return None;
    }
    let to: String = chars[to_start..pos].iter().collect();
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }
    let label = if pos < chars.len() && chars[pos] == ':' {
        let rest: String = chars[pos + 1..].iter().collect();
        let trimmed = rest.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else {
        None
    };
    Some((from, to, label))
}

// ===========================================================================
// Class diagram
// ===========================================================================

#[derive(Debug, Clone)]
struct ClassMember {
    visibility: char, // '+', '-', '#', '~', or ' ' (none)
    name: String,
}

#[derive(Debug, Clone, Default)]
struct ClassDecl {
    name: String,
    annotation: Option<String>, // <<interface>>, <<abstract>>, etc.
    attrs: Vec<ClassMember>,
    methods: Vec<ClassMember>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClassRelKind {
    Inheritance, // <|--  (hollow triangle)
    Composition, // *--   (filled diamond)
    Aggregation, // o--    (hollow diamond)
    Association, // --     (line, possibly with arrow)
    Realization, // ..|>   (dashed hollow triangle)
    Dependency,  // ..>    (dashed arrow)
    SolidArrow,  // -->
}

#[derive(Debug, Clone)]
struct ClassRelation {
    from: usize,
    to: usize,
    kind: ClassRelKind,
    label: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ClassDiagram {
    classes: Vec<ClassDecl>,
    cindex: HashMap<String, usize>,
    relations: Vec<ClassRelation>,
}

fn render_class(source: &str) -> Option<String> {
    let cd = parse_class(source)?;
    if cd.classes.is_empty() {
        return None;
    }
    Some(render_class_diagram(&cd))
}

fn parse_class(source: &str) -> Option<ClassDiagram> {
    let mut cd = ClassDiagram::default();
    let mut saw_header = false;
    let mut current_class: Option<usize> = None;
    let mut in_body = false;

    let ensure_class = |cd: &mut ClassDiagram, name: &str| -> usize {
        if let Some(&i) = cd.cindex.get(name) {
            return i;
        }
        let i = cd.classes.len();
        cd.classes.push(ClassDecl {
            name: name.to_string(),
            ..Default::default()
        });
        cd.cindex.insert(name.to_string(), i);
        i
    };
    for raw in source.lines() {
        let line = strip_comment(raw).trim();

        if line.is_empty() {
            continue;
        }
        if !saw_header {
            if line.eq_ignore_ascii_case("classdiagram") {
                saw_header = true;
                continue;
            }
            return None;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("direction ") || lower == "%%" {
            continue;
        }

        // Inside a class body — collect attrs / methods.
        if in_body {
            if line == "}" {
                in_body = false;
                current_class = None;
                continue;
            }
            if let Some(ci) = current_class {
                let member = parse_class_member(line);
                if member.is_method() {
                    cd.classes[ci].methods.push(member.into());
                } else {
                    cd.classes[ci].attrs.push(member.into());
                }
            }
            continue;
        }

        // `class Foo {` or `class Foo`
        if lower.starts_with("class ") {
            let rest = line["class ".len()..].trim();
            let name = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches('{')
                .to_string();
            if name.is_empty() {
                continue;
            }
            // `<<interface>>` annotation: appears on the next line in mermaid,
            // but sometimes inline after the name. We'll handle next-line only.
            let idx = ensure_class(&mut cd, &name);
            current_class = Some(idx);
            if rest.ends_with('{') {
                in_body = true;
            }
            continue;
        }
        // Annotation line for the most recently declared class.
        if line.starts_with("<<") && line.ends_with(">>") {
            let ann = line.trim_matches(&['<', '>'] as &[_]).to_string();
            if let Some(ci) = current_class {
                cd.classes[ci].annotation = Some(ann);
            } else if let Some(last) = cd.classes.last_mut() {
                last.annotation = Some(ann);
            }
            continue;
        }

        // Relation: `A <|-- B`, `A *-- B`, etc., optionally with label.
        if let Some(rel) = parse_class_relation(line) {
            let (from, to, kind, label) = rel;
            ensure_class(&mut cd, &from);
            ensure_class(&mut cd, &to);
            if let (Some(&f), Some(&t)) = (cd.cindex.get(&from), cd.cindex.get(&to)) {
                cd.relations.push(ClassRelation {
                    from: f,
                    to: t,
                    kind,
                    label,
                });
            }
        }
        // Generics `Class~T~` and namespaces are silently ignored.
    }
    if !saw_header {
        return None;
    }
    Some(cd)
}

/// Helper to parse a class member line. Returns a tuple-like that knows
/// whether it's a method (contains `(`).
struct ParsedMember {
    visibility: char,
    name: String,
    is_method: bool,
}

impl ParsedMember {
    fn is_method(&self) -> bool {
        self.is_method
    }
}

impl From<ParsedMember> for ClassMember {
    fn from(p: ParsedMember) -> Self {
        ClassMember {
            visibility: p.visibility,
            name: p.name,
        }
    }
}

fn parse_class_member(line: &str) -> ParsedMember {
    let trimmed = line.trim();
    let (vis, rest) = match trimmed.chars().next() {
        Some(c) if "+-~#".contains(c) => (c, trimmed[1..].trim()),
        _ => (' ', trimmed),
    };
    let is_method = rest.contains('(');
    ParsedMember {
        visibility: vis,
        name: rest.to_string(),
        is_method,
    }
}

fn parse_class_relation(line: &str) -> Option<(String, String, ClassRelKind, Option<String>)> {
    // Tokens for relations. Longest first to avoid prefix clashing.
    let tokens: &[(&str, ClassRelKind, bool /* reversed */)] = &[
        ("<|--", ClassRelKind::Inheritance, true),
        ("..|>", ClassRelKind::Realization, false),
        ("*--", ClassRelKind::Composition, true),
        ("o--", ClassRelKind::Aggregation, true),
        ("..>", ClassRelKind::Dependency, false),
        ("<--", ClassRelKind::SolidArrow, true),
        ("-->", ClassRelKind::SolidArrow, false),
        ("--", ClassRelKind::Association, false),
        ("..", ClassRelKind::Dependency, false),
    ];
    for (tok, kind, reversed) in tokens {
        if let Some(idx) = line.find(tok) {
            let left = line[..idx].trim();
            let right_part = line[idx + tok.len()..].trim();
            // Right part may contain a `: label` suffix.
            let (right, label) = match right_part.find(':') {
                Some(c) => (
                    right_part[..c].trim().to_string(),
                    right_part[c + 1..].trim().to_string(),
                ),
                None => (right_part.to_string(), String::new()),
            };
            if left.is_empty() || right.is_empty() {
                continue;
            }
            let label_opt = if label.is_empty() { None } else { Some(label) };
            let (from, to) = if *reversed {
                // Token like `<|--`: arrow points from right to left (A inherits B).
                // Mermaid semantics: `A <|-- B` means A is parent, B is child.
                // We render the relation as A ← B (B points to A).
                (right.clone(), left.to_string())
            } else {
                (left.to_string(), right)
            };
            return Some((from, to, *kind, label_opt));
        }
    }
    None
}

fn render_class_diagram(cd: &ClassDiagram) -> String {
    // Render each class as a multi-row box: header (name + annotation),
    // attrs section, methods section.
    let boxes: Vec<Vec<String>> = cd.classes.iter().map(render_class_box).collect();
    let widths: Vec<usize> = boxes.iter().map(|b| box_width(b)).collect();

    // Simple grid layout: arrange classes in rows of up to 3 per row.
    let per_row = 3usize;
    let n = cd.classes.len();
    let rows = n.div_ceil(per_row);
    let row_heights: Vec<usize> = (0..rows)
        .map(|r| {
            let lo = r * per_row;
            let hi = (lo + per_row).min(n);
            (lo..hi).map(|i| boxes[i].len()).max().unwrap_or(0)
        })
        .collect();
    let col_widths: Vec<usize> = (0..per_row)
        .map(|c| {
            (0..rows)
                .filter_map(|r| {
                    let idx = r * per_row + c;
                    if idx < n { Some(widths[idx]) } else { None }
                })
                .max()
                .unwrap_or(0)
        })
        .collect();
    let hgap = 6usize;
    let vgap = 3usize;
    let total_w: usize = col_widths.iter().sum::<usize>() + hgap * (per_row - 1) + 4;
    let total_h: usize = row_heights.iter().sum::<usize>() + vgap * (rows - 1) + 4;
    let mut canvas = Canvas::new(total_w.max(8), total_h.max(4));

    // Place classes.
    let mut pos_x: Vec<isize> = vec![0; n];
    let mut pos_y: Vec<isize> = vec![0; n];
    let mut y = 1isize;
    for (r, &rh) in row_heights.iter().enumerate() {
        let mut x = 1isize;
        let lo = r * per_row;
        let hi = (lo + per_row).min(n);
        for (c, &cw) in col_widths[..(hi - lo)].iter().enumerate() {
            let idx = lo + c;
            pos_x[idx] = x;
            pos_y[idx] = y;
            let bw = widths[idx];
            for (dy, row) in boxes[idx].iter().enumerate() {
                canvas.write(x, y + dy as isize, row);
            }
            x += cw.max(bw) as isize + hgap as isize;
        }
        y += rh as isize + vgap as isize;
    }

    // Compute class center and edge anchor points.
    let centers: Vec<(isize, isize)> = (0..n)
        .map(|i| {
            let bw = widths[i] as isize;
            let bh = boxes[i].len() as isize;
            (pos_x[i] + bw / 2, pos_y[i] + bh / 2)
        })
        .collect();

    // Draw relations using box edges (so arrows never overwrite box content).
    for rel in &cd.relations {
        let (cx0, cy0) = centers[rel.from];
        let (cx1, cy1) = centers[rel.to];
        let bw0 = widths[rel.from] as isize;
        let bh0 = boxes[rel.from].len() as isize;
        let bw1 = widths[rel.to] as isize;
        let bh1 = boxes[rel.to].len() as isize;

        // Anchor points on box edges, chosen by relative position.
        let (x0, y0, x1, y1) = if (cx1 - cx0).abs() > (cy1 - cy0).abs() {
            if cx0 < cx1 {
                // Source → right edge; target → left edge.
                (pos_x[rel.from] + bw0, cy0, pos_x[rel.to] - 1, cy1)
            } else {
                (pos_x[rel.from] - 1, cy0, pos_x[rel.to] + bw1, cy1)
            }
        } else if cy0 < cy1 {
            // Source → bottom; target → top.
            (cx0, pos_y[rel.from] + bh0, cx1, pos_y[rel.to] - 1)
        } else {
            (cx0, pos_y[rel.from] - 1, cx1, pos_y[rel.to] + bh1)
        };
        let (line_ch, head_ch, dash) = class_rel_chars(rel.kind);
        let ch = if dash {
            if x0 == x1 { '┊' } else { '┄' }
        } else {
            line_ch
        };
        if y0 == y1 {
            // Pure horizontal: line spans between source anchor and head.
            let (lo, hi) = if x0 < x1 { (x0, x1 - 1) } else { (x1 + 1, x0) };
            canvas.hline(lo, hi, y0, ch);
        } else if x0 == x1 {
            let (lo, hi) = if y0 < y1 { (y0, y1 - 1) } else { (y1 + 1, y0) };
            canvas.vline(x0, lo, hi, ch);
        } else if (x1 - x0).abs() > (y1 - y0).abs() {
            // Horizontal-dominant elbow.
            let (lo, hi) = if x0 < x1 { (x0, x1) } else { (x1, x0) };
            canvas.hline(lo + 1, hi, y0, ch);
            let corner = if (y1 > y0) ^ (x1 < x0) { '┐' } else { '└' };
            canvas.put(x1, y0, corner);
            canvas.vline(x1, y0.min(y1) + 1, y0.max(y1) - 1, ch);
        } else {
            // Vertical-dominant elbow.
            let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
            canvas.vline(x0, lo + 1, hi, ch);
            let corner = if (x1 > x0) ^ (y1 < y0) { '└' } else { '┘' };
            canvas.put(x0, y1, corner);
            canvas.hline(x0.min(x1) + 1, x0.max(x1) - 1, y1, ch);
        }
        canvas.put(x1, y1, head_ch);
        if let Some(lbl) = &rel.label {
            let mid_x = (x0 + x1) / 2;
            let mid_y = (y0 + y1) / 2;
            canvas.write(mid_x, mid_y - 1, lbl);
        }
    }

    canvas.render()
}

fn class_rel_chars(kind: ClassRelKind) -> (char, char, bool /* dashed */) {
    match kind {
        ClassRelKind::Inheritance => ('─', '▽', false),
        ClassRelKind::Composition => ('─', '◆', false),
        ClassRelKind::Aggregation => ('─', '◇', false),
        ClassRelKind::Association => ('─', '►', false),
        ClassRelKind::Realization => ('─', '▽', true),
        ClassRelKind::Dependency => ('─', '►', true),
        ClassRelKind::SolidArrow => ('─', '►', false),
    }
}

fn render_class_box(c: &ClassDecl) -> Vec<String> {
    let name_w = unicode_width(&c.name);
    let ann_w = c
        .annotation
        .as_ref()
        .map(|a| unicode_width(a) + 4)
        .unwrap_or(0); // <<a>>
    let inner = name_w
        .max(ann_w)
        .max(
            c.attrs
                .iter()
                .map(|m| unicode_width(&format_member(m)))
                .max()
                .unwrap_or(0),
        )
        .max(
            c.methods
                .iter()
                .map(|m| unicode_width(&format_member(m)))
                .max()
                .unwrap_or(0),
        )
        .max(8);

    let total_w = inner;
    let mut rows: Vec<String> = Vec::new();

    // Header top.
    rows.push(format!("┌{}┐", "─".repeat(total_w)));
    // Name row (centered).
    let npad = inner - name_w;
    let nleft = npad / 2;
    let nright = npad - nleft;
    rows.push(format!(
        "│{}{}{}│",
        " ".repeat(nleft),
        c.name,
        " ".repeat(nright)
    ));
    // Annotation row.
    if let Some(a) = &c.annotation {
        let ann = format!("‹‹{}››", a);
        let aw = unicode_width(&ann);
        let pad = inner.saturating_sub(aw);
        let left = pad / 2;
        let right = pad - left;
        rows.push(format!(
            "│{}{}{}│",
            " ".repeat(left),
            ann,
            " ".repeat(right)
        ));
    }
    // Separator.
    rows.push(format!("├{}┤", "─".repeat(total_w)));
    // Attrs.
    if c.attrs.is_empty() {
        rows.push(format!("│{}│", " ".repeat(inner)));
    } else {
        for m in &c.attrs {
            let s = format_member(m);
            let pad = inner - unicode_width(&s);
            rows.push(format!("│{}{}│", s, " ".repeat(pad)));
        }
    }
    // Separator.
    rows.push(format!("├{}┤", "─".repeat(total_w)));
    // Methods.
    if c.methods.is_empty() {
        rows.push(format!("│{}│", " ".repeat(inner)));
    } else {
        for m in &c.methods {
            let s = format_member(m);
            let pad = inner - unicode_width(&s);
            rows.push(format!("│{}{}│", s, " ".repeat(pad)));
        }
    }
    // Footer.
    rows.push(format!("└{}┘", "─".repeat(total_w)));
    rows
}

fn format_member(m: &ClassMember) -> String {
    if m.visibility == ' ' {
        m.name.clone()
    } else {
        format!("{} {}", m.visibility, m.name)
    }
}

// ===========================================================================
// Box-border wrapper — preserves the previous public surface
// ===========================================================================

/// Render an ASCII diagram string inside a titled box border.
///
/// Produces [`Line`]s with box-drawing characters wrapping the ASCII content,
/// suitable for use in a `Paragraph`. The border is dimmed so the diagram
/// content stands out.
///
/// # Example output
///
/// ```text
/// ┌─ mermaid ─────┐
/// │  A──►B        │
/// │  B──►C        │
/// └───────────────┘
/// ```
pub fn render_ascii_diagram(ascii: &str) -> Vec<Line<'static>> {
    let border = Style::new().add_modifier(Modifier::DIM);
    // 9 = display width of " mermaid " (the title text).
    let w = ascii_display_width(ascii).max(9);

    let mut lines = Vec::new();

    // Top border: ┌─ mermaid ─...─┐
    let title_fill = w.saturating_sub(9);
    lines.push(Line::from(vec![
        Span::styled("┌─", border),
        Span::styled(" mermaid ", border),
        Span::styled("─".repeat(title_fill), border),
        Span::styled("┐", border),
    ]));

    // Content rows: │ content padded │
    for line in ascii.lines() {
        let line_w = UnicodeWidthStr::width(line);
        let pad = w.saturating_sub(line_w);
        lines.push(Line::from(vec![
            Span::styled("│ ", border),
            Span::raw(line.to_string()),
            Span::raw(" ".repeat(pad)),
            Span::styled("│", border),
        ]));
    }

    // Bottom border: └─...─┘
    lines.push(Line::from(vec![
        Span::styled("└", border),
        Span::styled("─".repeat(w + 1), border),
        Span::styled("┘", border),
    ]));

    lines
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Preserve existing public-API tests --------------------------------

    #[test]
    fn empty_source_returns_none() {
        let opts = MermaidRenderOptions::default();
        assert_eq!(render_mermaid_ascii("", &opts), None);
        assert_eq!(render_mermaid_ascii("   \n  ", &opts), None);
    }

    #[test]
    fn cache_returns_same_result() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let source = "sequenceDiagram\n  A->>B: hello";
        let r1 = render_mermaid_ascii(source, &opts);
        let r2 = render_mermaid_ascii(source, &opts);
        assert_eq!(r1, r2);
    }

    #[test]
    fn clear_cache_empties_cache() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let _ = render_mermaid_ascii("graph TD\n  A-->B", &opts);
        assert!(!MERMAID_CACHE.read().is_empty());
        clear_mermaid_cache();
        assert!(MERMAID_CACHE.read().is_empty());
    }

    #[test]
    fn ascii_display_width_ascii() {
        assert_eq!(ascii_display_width("hello"), 5);
        assert_eq!(ascii_display_width("ab\ncd"), 2);
        assert_eq!(ascii_display_width(""), 0);
    }

    #[test]
    fn ascii_display_width_cjk() {
        assert_eq!(ascii_display_width("한글"), 4);
    }

    #[test]
    fn render_ascii_diagram_has_borders() {
        let lines = render_ascii_diagram("hello");
        assert_eq!(lines.len(), 3);
        assert!(lines[0].spans.iter().any(|s| s.content.starts_with("┌")));
        assert!(lines[2].spans.iter().any(|s| s.content.ends_with('┘')));
    }

    #[test]
    fn render_ascii_diagram_empty_input() {
        let lines = render_ascii_diagram("");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn render_ascii_diagram_multiline() {
        let lines = render_ascii_diagram("A\nBB\nCCC");
        assert_eq!(lines.len(), 5);
        let bottom_dashes: String = lines[4]
            .spans
            .iter()
            .flat_map(|s| s.content.chars())
            .filter(|&c| c == '─')
            .collect();
        assert_eq!(bottom_dashes.chars().count(), 10);
    }

    // ---- Unsupported types fall back to None -------------------------------

    #[test]
    fn unsupported_diagram_type_returns_none() {
        let opts = MermaidRenderOptions::default();
        assert_eq!(
            render_mermaid_ascii("pie\n  \"A\": 50\n  \"B\": 50", &opts),
            None
        );
        assert_eq!(
            render_mermaid_ascii("gantt\n  title X\n  a :1d", &opts),
            None
        );
        assert_eq!(
            render_mermaid_ascii("gitGraph\n  commit\n  commit", &opts),
            None
        );
    }

    // ---- Flowchart ---------------------------------------------------------

    #[test]
    fn flowchart_basic_td() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "graph TD\n  A --> B\n  B --> C";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        // Must contain the labels.
        assert!(out.contains('A'), "out: {out}");
        assert!(out.contains('B'), "out: {out}");
        assert!(out.contains('C'), "out: {out}");
        // Must contain a downward arrow somewhere.
        assert!(out.contains(ARROW_HEAD), "out: {out}");
    }

    #[test]
    fn flowchart_labeled_edge() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "graph LR\n  A -->|hello| B";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains("hello"), "out: {out}");
    }

    #[test]
    fn flowchart_node_shapes() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "graph TD\n  A[Rect]\n  B(Round)\n  C{Diamond}\n  D((Circle))";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains("Rect"));
        assert!(out.contains("Round"));
        assert!(out.contains("Diamond"));
        assert!(out.contains("Circle"));
    }

    #[test]
    fn flowchart_lr_direction() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "flowchart LR\n  X --> Y --> Z";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains('X'));
        assert!(out.contains('Y'));
        assert!(out.contains('Z'));
        assert!(out.contains(ARROW_HEAD_RIGHT));
    }

    #[test]
    fn flowchart_dashed_and_thick_arrows() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "graph TD\n  A -.-> B\n  B ==> C";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains('A'));
        assert!(out.contains('C'));
    }

    #[test]
    fn flowchart_diamond_shape_in_box() {
        // Diamond corners render as ◆.
        let node = NodeDecl {
            id: "x".into(),
            label: Some("Hi".into()),
            shape: Shape::Diamond,
        };
        let rows = render_node_box(&node);
        assert!(rows.iter().any(|r| r.contains('◆')));
    }

    // ---- Sequence ----------------------------------------------------------

    #[test]
    fn sequence_basic() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains("Alice"));
        assert!(out.contains("Bob"));
        assert!(out.contains("Hello"));
        assert!(out.contains("Hi"));
        assert!(out.contains('►'));
    }

    #[test]
    fn sequence_participant_alias() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src =
            "sequenceDiagram\n  participant A as Alice\n  participant B as Bob\n  A->>B: ping";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains("Alice"));
        assert!(out.contains("Bob"));
    }

    #[test]
    fn sequence_self_message() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "sequenceDiagram\n  participant A\n  A->>A: self";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains("self"));
    }

    // ---- State -------------------------------------------------------------

    #[test]
    fn state_basic_v2() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src =
            "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Processing : start\n  Processing --> [*]";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains("Idle"));
        assert!(out.contains("Processing"));
        assert!(out.contains("start"));
    }

    #[test]
    fn state_v1_compatible() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "stateDiagram\n  [*] --> S1\n  S1 --> [*]";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains("S1"));
    }

    // ---- Class -------------------------------------------------------------

    #[test]
    fn class_basic() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "classDiagram\n  class Animal {\n    +String name\n    +eat()\n  }\n  class Dog {\n    +bark()\n  }\n  Animal <|-- Dog";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains("Animal"));
        assert!(out.contains("Dog"));
        assert!(out.contains("eat()"));
        assert!(out.contains("bark()"));
        assert!(out.contains("name"));
    }

    #[test]
    fn class_relation_inheritance_arrow() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let src = "classDiagram\n  class Parent\n  class Child\n  Parent <|-- Child";
        let out = render_mermaid_ascii(src, &opts).expect("must render");
        assert!(out.contains("Parent"));
        assert!(out.contains("Child"));
        assert!(out.contains('▽') || out.contains('▼'));
    }

    // ---- Parser unit tests -------------------------------------------------

    #[test]
    fn classify_arrow_variants() {
        assert_eq!(classify_arrow("-->", None).style, ArrowStyle::SolidArrow);
        assert_eq!(classify_arrow("<-->", None).style, ArrowStyle::BidirArrow);
        assert_eq!(classify_arrow("==>", None).style, ArrowStyle::ThickArrow);
        assert_eq!(classify_arrow("-.->", None).style, ArrowStyle::DashedArrow);
        assert_eq!(classify_arrow("---", None).style, ArrowStyle::SolidLine);
        assert_eq!(classify_arrow("--x", None).style, ArrowStyle::ArrowX);
        assert_eq!(classify_arrow("--o", None).style, ArrowStyle::ArrowO);
    }

    #[test]
    fn parse_node_spec_shapes() {
        let s = |t: &str| -> Vec<char> { t.chars().collect() };
        let (id, lbl, shape, _) = parse_node_spec(&s("A[Hello]")).unwrap();
        assert_eq!(id, "A");
        assert_eq!(lbl.as_deref(), Some("Hello"));
        assert_eq!(shape, Some(Shape::Rect));

        let (id, lbl, shape, _) = parse_node_spec(&s("B(Round)")).unwrap();
        assert_eq!(id, "B");
        assert_eq!(lbl.as_deref(), Some("Round"));
        assert_eq!(shape, Some(Shape::Rounded));

        let (id, lbl, shape, _) = parse_node_spec(&s("C{Diamond}")).unwrap();
        assert_eq!(id, "C");
        assert_eq!(shape, Some(Shape::Diamond));
        assert_eq!(lbl.as_deref(), Some("Diamond"));

        let (id, lbl, shape, _) = parse_node_spec(&s("D((Circle))")).unwrap();
        assert_eq!(id, "D");
        assert_eq!(shape, Some(Shape::Circle));
        assert_eq!(lbl.as_deref(), Some("Circle"));

        // Bare id.
        let (id, lbl, shape, _) = parse_node_spec(&s("E")).unwrap();
        assert_eq!(id, "E");
        assert_eq!(lbl, None);
        assert_eq!(shape, None);
    }

    #[test]
    fn parse_flowgraph_multi_edge_chain() {
        let mut known = HashSet::new();
        let mut order = Vec::new();
        let mut labels = HashMap::new();
        let mut shapes = HashMap::new();
        let mut edges = Vec::new();
        parse_flowchart_stmt(
            "A --> B --> C",
            &mut known,
            &mut order,
            &mut labels,
            &mut shapes,
            &mut edges,
        );
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].0, "A");
        assert_eq!(edges[0].1, "B");
        assert_eq!(edges[1].0, "B");
        assert_eq!(edges[1].1, "C");
    }

    #[test]
    fn parse_state_transition_with_label() {
        let (from, to, label) = parse_state_transition("Idle --> Processing : start").unwrap();
        assert_eq!(from, "Idle");
        assert_eq!(to, "Processing");
        assert_eq!(label.as_deref(), Some("start"));
    }

    #[test]
    fn parse_class_relation_inheritance() {
        let (from, to, kind, _) = parse_class_relation("Animal <|-- Dog").unwrap();
        assert_eq!(from, "Dog"); // reversed: child reported as "from"
        assert_eq!(to, "Animal");
        assert_eq!(kind, ClassRelKind::Inheritance);
    }

    #[test]
    fn parse_class_relation_dependency() {
        let (from, to, kind, label) = parse_class_relation("A ..> B : uses").unwrap();
        assert_eq!(from, "A");
        assert_eq!(to, "B");
        assert_eq!(kind, ClassRelKind::Dependency);
        assert_eq!(label.as_deref(), Some("uses"));
    }

    #[test]
    fn parse_class_member_attr_and_method() {
        let attr = parse_class_member("+String name");
        assert_eq!(attr.visibility, '+');
        assert!(!attr.is_method);
        let m = parse_class_member("-eat()");
        assert_eq!(m.visibility, '-');
        assert!(m.is_method);
    }
}
