//! Terminal image rendering with Kitty and iTerm2 protocol support.
//!
//! Provides:
//! - **Kitty graphics protocol** – full support including image IDs, deletion,
//!   chunked transfer, and cursor control.
//! - **iTerm2 inline images** – base64-encoded images via OSC 1337.
//! - **Image cache** – store images by ID for efficient updates/deletion.
//! - **Size negotiation** – compute cell dimensions and scale images to fit.
//! - **Progressive rendering** – show placeholders while large images load.
//! - **Image formats** – PNG, JPEG, GIF, WebP, BMP via the `image` crate.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use base64::Engine;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum base64 payload per Kitty chunk (recommended safe size).
const KITTY_CHUNK_SIZE: usize = 4096;

/// Kitty escape sequence prefix.
const KITTY_PREFIX: &str = "\x1b_G";
/// Kitty escape sequence terminator.
const KITTY_SUFFIX: &str = "\x1b\\";
/// iTerm2 escape sequence prefix.
const ITERM2_PREFIX: &str = "\x1b]1337;File=";

// ---------------------------------------------------------------------------
// Protocol enum
// ---------------------------------------------------------------------------

/// Which terminal graphics protocol to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ImageProtocol {
    /// Automatically detect from environment.
    #[default]
    Auto,
    /// Kitty graphics protocol.
    Kitty,
    /// iTerm2 inline images.
    Iterm2,
    /// No supported protocol – render a placeholder.
    Fallback,
}

impl fmt::Display for ImageProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageProtocol::Auto => write!(f, "auto"),
            ImageProtocol::Kitty => write!(f, "kitty"),
            ImageProtocol::Iterm2 => write!(f, "iterm2"),
            ImageProtocol::Fallback => write!(f, "fallback"),
        }
    }
}

// ---------------------------------------------------------------------------
// Terminal capabilities
// ---------------------------------------------------------------------------

/// Detected terminal capabilities relevant to image rendering.
#[derive(Debug, Clone)]
pub struct TerminalCapabilities {
    /// Best image protocol available.
    pub protocol: ImageProtocol,
    /// Whether the terminal supports 24-bit true color.
    pub true_color: bool,
    /// Whether the terminal supports OSC 8 hyperlinks.
    pub hyperlinks: bool,
}

impl Default for TerminalCapabilities {
    fn default() -> Self {
        Self {
            protocol: ImageProtocol::Fallback,
            true_color: false,
            hyperlinks: false,
        }
    }
}

/// Detect terminal capabilities from environment variables.
pub fn detect_capabilities() -> TerminalCapabilities {
    let term_program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    let color_term = std::env::var("COLORTERM")
        .unwrap_or_default()
        .to_lowercase();

    // Kitty
    if std::env::var("KITTY_WINDOW_ID").is_ok() || term_program == "kitty" {
        return TerminalCapabilities {
            protocol: ImageProtocol::Kitty,
            true_color: true,
            hyperlinks: true,
        };
    }

    // Ghostty (supports Kitty protocol)
    if term_program == "ghostty"
        || term.contains("ghostty")
        || std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
    {
        return TerminalCapabilities {
            protocol: ImageProtocol::Kitty,
            true_color: true,
            hyperlinks: true,
        };
    }

    // WezTerm (supports Kitty protocol)
    if std::env::var("WEZTERM_PANE").is_ok() || term_program == "wezterm" {
        return TerminalCapabilities {
            protocol: ImageProtocol::Kitty,
            true_color: true,
            hyperlinks: true,
        };
    }

    // iTerm2
    if std::env::var("ITERM_SESSION_ID").is_ok() || term_program == "iterm.app" {
        return TerminalCapabilities {
            protocol: ImageProtocol::Iterm2,
            true_color: true,
            hyperlinks: true,
        };
    }

    let true_color = color_term == "truecolor" || color_term == "24bit";

    TerminalCapabilities {
        protocol: ImageProtocol::Fallback,
        true_color,
        hyperlinks: true,
    }
}

// ---------------------------------------------------------------------------
// Cell & image dimensions
// ---------------------------------------------------------------------------

/// Pixel dimensions of a single terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellDimensions {
    /// Width in pixels.
    pub width_px: u32,
    /// Height in pixels.
    pub height_px: u32,
}

impl Default for CellDimensions {
    fn default() -> Self {
        // Common default; can be updated via terminal query response.
        Self {
            width_px: 9,
            height_px: 18,
        }
    }
}

/// Pixel dimensions of an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    /// Width in pixels.
    pub width_px: u32,
    /// Height in pixels.
    pub height_px: u32,
}

// ---------------------------------------------------------------------------
// Image format helpers
// ---------------------------------------------------------------------------

/// Image MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG image.
    Png,
    /// JPEG image.
    Jpeg,
    /// GIF image.
    Gif,
    /// WebP image.
    WebP,
    /// BMP image.
    Bmp,
}

impl ImageFormat {
    /// Kitty format code for raw pixel transmission.
    /// We use `f=100` (PNG) for PNG data and let the terminal decode it.
    pub fn kitty_format_code(self) -> u32 {
        match self {
            ImageFormat::Png => 100,
            ImageFormat::Jpeg => 24,
            ImageFormat::Gif => 100,  // re-encoded as PNG
            ImageFormat::WebP => 100, // re-encoded as PNG
            ImageFormat::Bmp => 100,  // re-encoded as PNG
        }
    }

    /// MIME type string.
    pub fn mime_type(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Gif => "image/gif",
            ImageFormat::WebP => "image/webp",
            ImageFormat::Bmp => "image/bmp",
        }
    }
}

/// Detect image format from magic bytes.
pub fn detect_format(data: &[u8]) -> Option<ImageFormat> {
    if data.len() < 4 {
        return None;
    }

    // PNG: 89 50 4E 47
    if data[0] == 0x89 && data[1] == 0x50 && data[2] == 0x4E && data[3] == 0x47 {
        return Some(ImageFormat::Png);
    }
    // JPEG: FF D8 FF
    if data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        return Some(ImageFormat::Jpeg);
    }
    // GIF: "GIF8"
    if data.len() >= 6 && &data[0..4] == b"GIF8" {
        return Some(ImageFormat::Gif);
    }
    // WebP: "RIFF"...."WEBP"
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    // BMP: "BM"
    if data.len() >= 2 && data[0] == 0x42 && data[1] == 0x4D {
        return Some(ImageFormat::Bmp);
    }

    None
}

/// Detect format from file extension.
pub fn format_from_extension(path: &Path) -> Option<ImageFormat> {
    match path
        .extension()
        .and_then(|e| e.to_str())?
        .to_lowercase()
        .as_str()
    {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "webp" => Some(ImageFormat::WebP),
        "bmp" => Some(ImageFormat::Bmp),
        _ => None,
    }
}

/// Extract image dimensions using the `image` crate.
pub fn get_image_dimensions(data: &[u8]) -> Option<ImageDimensions> {
    let reader = image::ImageReader::new(std::io::Cursor::new(data));
    let reader = reader.with_guessed_format().ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    Some(ImageDimensions {
        width_px: w,
        height_px: h,
    })
}

/// Load, optionally resize, and re-encode an image to PNG bytes.
///
/// If `max_width` / `max_height` are provided and the image exceeds them,
/// it is scaled down preserving aspect ratio. The output is always PNG.
pub fn prepare_image(
    data: &[u8],
    max_width: Option<u32>,
    max_height: Option<u32>,
) -> Option<Vec<u8>> {
    let img = image::load_from_memory(data).ok()?;
    let (w, h) = (img.width(), img.height());

    let scaled = if let (Some(mw), Some(mh)) = (max_width, max_height) {
        if w > mw || h > mh {
            img.resize(mw, mh, image::imageops::FilterType::Lanczos3)
        } else {
            img
        }
    } else if let Some(mw) = max_width {
        if w > mw {
            let new_h = (h as f64 * (mw as f64 / w as f64)).round() as u32;
            img.resize(mw, new_h, image::imageops::FilterType::Lanczos3)
        } else {
            img
        }
    } else if let Some(mh) = max_height {
        if h > mh {
            let new_w = (w as f64 * (mh as f64 / h as f64)).round() as u32;
            img.resize(new_w, mh, image::imageops::FilterType::Lanczos3)
        } else {
            img
        }
    } else {
        img
    };

    let mut buf = Vec::new();
    scaled
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(buf)
}

// ---------------------------------------------------------------------------
// Image render options
// ---------------------------------------------------------------------------

/// Options controlling how an image is rendered.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Maximum width in terminal columns.
    pub max_width_cells: u32,
    /// Maximum height in terminal rows (0 = auto).
    pub max_height_cells: u32,
    /// Preserve aspect ratio when scaling.
    pub preserve_aspect_ratio: bool,
    /// Kitty image ID for reuse/replacement (None = one-shot).
    pub image_id: Option<u32>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            max_width_cells: 80,
            max_height_cells: 0,
            preserve_aspect_ratio: true,
            image_id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Size negotiation
// ---------------------------------------------------------------------------

/// Calculate how many terminal rows an image occupies at a given column width.
pub fn calculate_image_rows(
    image_dims: ImageDimensions,
    target_width_cells: u32,
    cell_dims: CellDimensions,
) -> u32 {
    let target_width_px = target_width_cells * cell_dims.width_px;
    if image_dims.width_px == 0 {
        return 1;
    }
    let scale = target_width_px as f64 / image_dims.width_px as f64;
    let scaled_height_px = image_dims.height_px as f64 * scale;
    let rows = (scaled_height_px / cell_dims.height_px as f64).ceil() as u32;
    rows.max(1)
}

// ---------------------------------------------------------------------------
// Image cache
// ---------------------------------------------------------------------------

/// Entry stored in the image cache.
#[derive(Debug, Clone)]
struct CacheEntry {
    /// The Kitty image ID.
    image_id: u32,
    /// Pixel dimensions of the cached image.
    dimensions: ImageDimensions,
    /// Number of rows the image occupies.
    rows: u32,
    /// Number of columns the image occupies.
    columns: u32,
}

/// Cache for terminal images, keyed by a user-defined ID.
///
/// Allows reusing Kitty image IDs so that an image can be updated in-place
/// (delete old, place new at same position) without flickering.
#[derive(Debug, Default)]
pub struct ImageCache {
    entries: HashMap<u32, CacheEntry>,
    /// Counter for allocating unique Kitty image IDs.
    next_kitty_id: u32,
}

impl ImageCache {
    /// Create a new empty image cache.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_kitty_id: 1,
        }
    }

    /// Allocate a unique Kitty image ID.
    pub fn allocate_id(&mut self) -> u32 {
        let id = self.next_kitty_id;
        self.next_kitty_id = self.next_kitty_id.wrapping_add(1);
        if self.next_kitty_id == 0 {
            self.next_kitty_id = 1; // skip 0
        }
        id
    }

    /// Insert an image into the cache.
    pub fn insert(
        &mut self,
        key: u32,
        kitty_image_id: u32,
        dimensions: ImageDimensions,
        columns: u32,
        rows: u32,
    ) {
        self.entries.insert(
            key,
            CacheEntry {
                image_id: kitty_image_id,
                dimensions,
                rows,
                columns,
            },
        );
    }

    /// Look up a cached image by key.
    pub fn get(&self, key: u32) -> Option<(u32, ImageDimensions, u32, u32)> {
        self.entries
            .get(&key)
            .map(|e| (e.image_id, e.dimensions, e.columns, e.rows))
    }

    /// Remove a cached image by key. Returns the Kitty image ID if found.
    pub fn remove(&mut self, key: u32) -> Option<u32> {
        self.entries.remove(&key).map(|e| e.image_id)
    }

    /// Number of cached images.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all cached Kitty image IDs.
    pub fn kitty_ids(&self) -> Vec<u32> {
        self.entries.values().map(|e| e.image_id).collect()
    }

    /// Clear the cache.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

// ---------------------------------------------------------------------------
// Kitty protocol encoder
// ---------------------------------------------------------------------------

/// Encode image data for the Kitty graphics protocol.
///
/// Returns the full escape sequence string(s). For large payloads the data
/// is split into chunks of [`KITTY_CHUNK_SIZE`] bytes.
pub fn encode_kitty(
    base64_data: &str,
    columns: u32,
    rows: u32,
    image_id: Option<u32>,
    format_code: u32,
) -> String {
    let mut params: Vec<String> = Vec::new();
    params.push("a=T".to_string()); // transmit and display
    params.push(format!("f={}", format_code));
    params.push("q=2".to_string()); // suppress errors
    params.push(format!("c={}", columns));
    params.push(format!("r={}", rows));
    if let Some(id) = image_id {
        params.push(format!("i={}", id));
    }

    if base64_data.len() <= KITTY_CHUNK_SIZE {
        format!(
            "{}{};{}{}",
            KITTY_PREFIX,
            params.join(","),
            base64_data,
            KITTY_SUFFIX
        )
    } else {
        let mut chunks: Vec<String> = Vec::new();
        let mut offset = 0;
        let mut is_first = true;

        while offset < base64_data.len() {
            let end = (offset + KITTY_CHUNK_SIZE).min(base64_data.len());
            let chunk = &base64_data[offset..end];
            let is_last = end >= base64_data.len();

            if is_first {
                chunks.push(format!(
                    "{}{},m=1;{}{}",
                    KITTY_PREFIX,
                    params.join(","),
                    chunk,
                    KITTY_SUFFIX
                ));
                is_first = false;
            } else if is_last {
                chunks.push(format!("{}m=0;{}{}", KITTY_PREFIX, chunk, KITTY_SUFFIX));
            } else {
                chunks.push(format!("{}m=1;{}{}", KITTY_PREFIX, chunk, KITTY_SUFFIX));
            }

            offset = end;
        }

        chunks.join("")
    }
}

/// Generate a Kitty delete-image escape sequence for a specific ID.
pub fn delete_kitty_image(image_id: u32) -> String {
    format!("{}a=d,d=I,i={}{}", KITTY_PREFIX, image_id, KITTY_SUFFIX)
}

/// Generate a Kitty delete-all-images escape sequence.
pub fn delete_all_kitty_images() -> String {
    format!("{}a=d,d=A{}", KITTY_PREFIX, KITTY_SUFFIX)
}

// ---------------------------------------------------------------------------
// iTerm2 protocol encoder
// ---------------------------------------------------------------------------

/// Encode image data for the iTerm2 inline image protocol.
pub fn encode_iterm2(
    base64_data: &str,
    width: &str,
    height: &str,
    preserve_aspect_ratio: bool,
    name: Option<&str>,
) -> String {
    let mut params: Vec<String> = Vec::new();
    params.push("inline=1".to_string());
    params.push(format!("width={}", width));
    params.push(format!("height={}", height));

    if let Some(n) = name {
        let name_b64 = base64::engine::general_purpose::STANDARD.encode(n.as_bytes());
        params.push(format!("name={}", name_b64));
    }

    if !preserve_aspect_ratio {
        params.push("preserveAspectRatio=0".to_string());
    }

    format!("{}{}:{}\x07", ITERM2_PREFIX, params.join(";"), base64_data)
}

// ---------------------------------------------------------------------------
// Progressive rendering placeholder
// ---------------------------------------------------------------------------

/// A placeholder shown while a real image is being loaded/decoded.
#[derive(Debug, Clone)]
pub struct ImagePlaceholder {
    /// Display width in terminal columns.
    pub width: u32,
    /// Display height in terminal rows.
    pub height: u32,
    /// Label text (e.g. MIME type).
    pub label: String,
}

impl ImagePlaceholder {
    /// Create a new placeholder.
    pub fn new(width: u32, height: u32, label: &str) -> Self {
        Self {
            width,
            height,
            label: label.to_string(),
        }
    }

    /// Render the placeholder as lines of text.
    pub fn render(&self) -> Vec<String> {
        let w = self.width.max(4) as usize;
        let h = self.height.max(3) as usize;
        let inner = w.saturating_sub(2).max(1);

        let mut lines = Vec::new();

        // Top border
        lines.push(format!("┌{}┐", "─".repeat(inner)));

        // Content lines
        let label = format!(" {} ", self.label);
        let padded_label = if label.chars().count() <= inner {
            let pad = inner.saturating_sub(label.chars().count());
            format!("{}{}", label, " ".repeat(pad))
        } else {
            label.chars().take(inner).collect::<String>()
        };
        lines.push(format!("│{}│", padded_label));

        // Loading indicator
        let loading = " ⏳ loading… ".to_string();
        let padded_loading = if loading.chars().count() <= inner {
            let pad = inner.saturating_sub(loading.chars().count());
            format!("{}{}", loading, " ".repeat(pad))
        } else {
            loading.chars().take(inner).collect::<String>()
        };
        if h > 3 {
            lines.push(format!("│{}│", padded_loading));
        }

        // Fill remaining rows
        while lines.len() < h.saturating_sub(1) {
            lines.push(format!("│{}│", " ".repeat(inner)));
        }

        // Bottom border
        lines.push(format!("└{}┘", "─".repeat(inner)));

        lines
    }
}

// ---------------------------------------------------------------------------
// Render result
// ---------------------------------------------------------------------------

/// The result of rendering a terminal image.
#[derive(Debug, Clone)]
pub struct RenderedImage {
    /// The ANSI escape sequence(s) to write to the terminal.
    pub sequence: String,
    /// Number of terminal rows the image occupies.
    pub rows: u32,
    /// Number of terminal columns the image occupies.
    pub columns: u32,
    /// Kitty image ID, if assigned.
    pub image_id: Option<u32>,
}

// ---------------------------------------------------------------------------
// Main render function
// ---------------------------------------------------------------------------

/// Render an image for the terminal.
///
/// This is the primary entry point. It detects the protocol, computes
/// dimensions, prepares the image data, and generates the escape sequences.
pub fn render_terminal_image(
    image_data: &[u8],
    options: &RenderOptions,
    cache: &mut ImageCache,
    cell_dims: CellDimensions,
    force_protocol: Option<ImageProtocol>,
) -> Option<RenderedImage> {
    let protocol = force_protocol.unwrap_or_else(|| {
        let caps = detect_capabilities();
        if caps.protocol == ImageProtocol::Fallback {
            ImageProtocol::Fallback
        } else {
            caps.protocol
        }
    });

    if protocol == ImageProtocol::Fallback {
        return None;
    }

    // Get image dimensions.
    let dims = get_image_dimensions(image_data)?;
    let format = detect_format(image_data).unwrap_or(ImageFormat::Png);

    // Calculate target size in cells.
    let target_columns = options.max_width_cells;
    let target_rows = if options.max_height_cells > 0 {
        options.max_height_cells
    } else {
        calculate_image_rows(dims, target_columns, cell_dims)
    };

    // Prepare image (resize if needed).
    let max_w = Some(target_columns * cell_dims.width_px);
    let max_h = Some(target_rows * cell_dims.height_px);
    let prepared = prepare_image(image_data, max_w, max_h).unwrap_or_else(|| image_data.to_vec());

    // Base64-encode.
    let b64 = base64::engine::general_purpose::STANDARD.encode(&prepared);

    // Determine Kitty image ID.
    let kitty_id = options.image_id.or_else(|| {
        if protocol == ImageProtocol::Kitty {
            Some(cache.allocate_id())
        } else {
            None
        }
    });

    let sequence = match protocol {
        ImageProtocol::Kitty => {
            let format_code = if format == ImageFormat::Png {
                100u32
            } else {
                // After prepare_image, output is always PNG.
                100u32
            };
            encode_kitty(&b64, target_columns, target_rows, kitty_id, format_code)
        }
        ImageProtocol::Iterm2 => encode_iterm2(
            &b64,
            &format!("{}c", target_columns),
            &format!("{}r", target_rows),
            options.preserve_aspect_ratio,
            None,
        ),
        ImageProtocol::Fallback | ImageProtocol::Auto => return None,
    };

    // Cache the image if it has a Kitty ID.
    if let Some(kid) = kitty_id {
        cache.insert(kid, kid, dims, target_columns, target_rows);
    }

    Some(RenderedImage {
        sequence,
        rows: target_rows,
        columns: target_columns,
        image_id: kitty_id,
    })
}

/// Render a fallback placeholder string for when no protocol is available.
pub fn image_fallback(
    mime_type: &str,
    dims: Option<ImageDimensions>,
    filename: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(f) = filename {
        parts.push(f.to_string());
    }
    parts.push(format!("[{}]", mime_type));
    if let Some(d) = dims {
        parts.push(format!("{}x{}", d.width_px, d.height_px));
    }
    format!("[Image: {}]", parts.join(" "))
}

/// Check if a line contains an image escape sequence.
pub fn is_image_line(line: &str) -> bool {
    line.contains(KITTY_PREFIX) || line.contains(ITERM2_PREFIX)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid 1×1 PNG (red pixel).
    fn sample_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, // 8-bit RGB
            0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT
            0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21,
            0xBC, 0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND
            0xAE, 0x42, 0x60, 0x82,
        ]
    }

    // ----- Format detection -----

    #[test]
    fn test_detect_format_png() {
        let data = sample_png();
        assert_eq!(detect_format(&data), Some(ImageFormat::Png));
    }

    #[test]
    fn test_detect_format_jpeg() {
        let data = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(detect_format(&data), Some(ImageFormat::Jpeg));
    }

    #[test]
    fn test_detect_format_gif() {
        let data = b"GIF89a\x00\x00\x00\x00".to_vec();
        assert_eq!(detect_format(&data), Some(ImageFormat::Gif));
    }

    #[test]
    fn test_detect_format_webp() {
        let mut data = b"RIFF".to_vec();
        data.extend_from_slice(&[0, 0, 0, 0]); // file size
        data.extend_from_slice(b"WEBP");
        assert_eq!(detect_format(&data), Some(ImageFormat::WebP));
    }

    #[test]
    fn test_detect_format_bmp() {
        let data = b"BM\x00\x00\x00\x00".to_vec();
        assert_eq!(detect_format(&data), Some(ImageFormat::Bmp));
    }

    #[test]
    fn test_detect_format_unknown() {
        assert_eq!(detect_format(&[0xDE, 0xAD, 0xBE, 0xEF]), None);
    }

    #[test]
    fn test_detect_format_too_small() {
        assert_eq!(detect_format(&[0x89, 0x50]), None);
    }

    // ----- Format from extension -----

    #[test]
    fn test_format_from_extension() {
        assert_eq!(
            format_from_extension(Path::new("photo.png")),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            format_from_extension(Path::new("photo.JPG")),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            format_from_extension(Path::new("photo.gif")),
            Some(ImageFormat::Gif)
        );
        assert_eq!(
            format_from_extension(Path::new("photo.webp")),
            Some(ImageFormat::WebP)
        );
        assert_eq!(
            format_from_extension(Path::new("photo.bmp")),
            Some(ImageFormat::Bmp)
        );
        assert_eq!(format_from_extension(Path::new("file.txt")), None);
        assert_eq!(format_from_extension(Path::new("noext")), None);
    }

    // ----- Kitty protocol -----

    #[test]
    fn test_encode_kitty_small() {
        let b64 = "dGVzdA=="; // "test"
        let result = encode_kitty(b64, 40, 10, None, 100);
        assert!(result.starts_with(KITTY_PREFIX));
        assert!(result.ends_with(KITTY_SUFFIX));
        assert!(result.contains("a=T"));
        assert!(result.contains("f=100"));
        assert!(result.contains("c=40"));
        assert!(result.contains("r=10"));
        assert!(!result.contains("m=")); // single chunk, no m= flag
    }

    #[test]
    fn test_encode_kitty_with_id() {
        let b64 = "dGVzdA==";
        let result = encode_kitty(b64, 20, 5, Some(42), 100);
        assert!(result.contains("i=42"));
    }

    #[test]
    fn test_encode_kitty_chunked() {
        // Create data that will produce a base64 string > 4096 chars
        let b64 = "A".repeat(10_000);
        let result = encode_kitty(&b64, 20, 5, None, 100);
        // Should have multiple chunks
        let chunk_count = result.matches(KITTY_PREFIX).count();
        assert!(chunk_count >= 3); // 10000 / 4096 ≈ 3 chunks
                                   // First chunk: m=1, last chunk: m=0
        assert!(result.contains(",m=1;"));
        assert!(result.contains("m=0;"));
    }

    #[test]
    fn test_delete_kitty_image() {
        let seq = delete_kitty_image(123);
        assert!(seq.starts_with(KITTY_PREFIX));
        assert!(seq.ends_with(KITTY_SUFFIX));
        assert!(seq.contains("a=d"));
        assert!(seq.contains("d=I"));
        assert!(seq.contains("i=123"));
    }

    #[test]
    fn test_delete_all_kitty_images() {
        let seq = delete_all_kitty_images();
        assert!(seq.contains("a=d"));
        assert!(seq.contains("d=A"));
    }

    // ----- iTerm2 protocol -----

    #[test]
    fn test_encode_iterm2() {
        let b64 = "dGVzdA==";
        let result = encode_iterm2(b64, "40c", "10r", true, None);
        assert!(result.starts_with(ITERM2_PREFIX));
        assert!(result.ends_with('\x07'));
        assert!(result.contains("inline=1"));
        assert!(result.contains("width=40c"));
        assert!(result.contains("height=10r"));
    }

    #[test]
    fn test_encode_iterm2_with_name() {
        let b64 = "dGVzdA==";
        let result = encode_iterm2(b64, "20c", "5r", true, Some("test.png"));
        assert!(result.contains("name="));
        // Name is base64-encoded
        let name_b64 = base64::engine::general_purpose::STANDARD.encode("test.png");
        assert!(result.contains(&format!("name={}", name_b64)));
    }

    #[test]
    fn test_encode_iterm2_no_preserve_aspect() {
        let b64 = "dGVzdA==";
        let result = encode_iterm2(b64, "20c", "5r", false, None);
        assert!(result.contains("preserveAspectRatio=0"));
    }

    // ----- Image cache -----

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = ImageCache::new();
        let dims = ImageDimensions {
            width_px: 100,
            height_px: 200,
        };
        cache.insert(1, 10, dims, 40, 10);
        let (id, d, cols, rows) = cache.get(1).unwrap();
        assert_eq!(id, 10);
        assert_eq!(d, dims);
        assert_eq!(cols, 40);
        assert_eq!(rows, 10);
    }

    #[test]
    fn test_cache_remove() {
        let mut cache = ImageCache::new();
        let dims = ImageDimensions {
            width_px: 50,
            height_px: 50,
        };
        cache.insert(1, 10, dims, 20, 5);
        assert_eq!(cache.len(), 1);
        let removed = cache.remove(1);
        assert_eq!(removed, Some(10));
        assert!(cache.get(1).is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_allocate_id() {
        let mut cache = ImageCache::new();
        let id1 = cache.allocate_id();
        let id2 = cache.allocate_id();
        assert_ne!(id1, id2);
        assert!(id1 >= 1);
        assert!(id2 >= 1);
    }

    #[test]
    fn test_cache_kitty_ids() {
        let mut cache = ImageCache::new();
        let dims = ImageDimensions {
            width_px: 10,
            height_px: 10,
        };
        cache.insert(1, 10, dims, 5, 2);
        cache.insert(2, 20, dims, 5, 2);
        let ids = cache.kitty_ids();
        assert!(ids.contains(&10));
        assert!(ids.contains(&20));
    }

    // ----- Size negotiation -----

    #[test]
    fn test_calculate_image_rows() {
        let dims = ImageDimensions {
            width_px: 100,
            height_px: 200,
        };
        let cell = CellDimensions {
            width_px: 10,
            height_px: 20,
        };
        // 10 cells wide → scale factor 1.0 → 200px / 20px per cell = 10 rows
        let rows = calculate_image_rows(dims, 10, cell);
        assert_eq!(rows, 10);
    }

    #[test]
    fn test_calculate_image_rows_upscale() {
        let dims = ImageDimensions {
            width_px: 50,
            height_px: 50,
        };
        let cell = CellDimensions {
            width_px: 10,
            height_px: 20,
        };
        // 20 cells wide → 200px / 50px = 4x → 50 * 4 = 200px / 20 = 10 rows
        let rows = calculate_image_rows(dims, 20, cell);
        assert_eq!(rows, 10);
    }

    #[test]
    fn test_calculate_image_rows_zero_width() {
        let dims = ImageDimensions {
            width_px: 0,
            height_px: 100,
        };
        let cell = CellDimensions::default();
        let rows = calculate_image_rows(dims, 10, cell);
        assert_eq!(rows, 1); // min 1
    }

    // ----- Progressive placeholder -----

    #[test]
    fn test_placeholder_render() {
        let ph = ImagePlaceholder::new(20, 5, "image/png");
        let lines = ph.render();
        assert!(lines[0].starts_with('┌'));
        assert!(lines.last().unwrap().starts_with('└'));
        assert!(lines.iter().any(|l| l.contains("image/png")));
        assert!(lines.iter().any(|l| l.contains("loading")));
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn test_placeholder_min_size() {
        let ph = ImagePlaceholder::new(2, 2, "img");
        let lines = ph.render();
        // Should clamp to minimum 4x3
        assert!(lines.len() >= 3);
        assert!(lines[0].starts_with('┌'));
    }

    // ----- Fallback string -----

    #[test]
    fn test_image_fallback() {
        let result = image_fallback(
            "image/png",
            Some(ImageDimensions {
                width_px: 800,
                height_px: 600,
            }),
            Some("photo.png"),
        );
        assert!(result.contains("photo.png"));
        assert!(result.contains("image/png"));
        assert!(result.contains("800x600"));
    }

    // ----- is_image_line -----

    #[test]
    fn test_is_image_line_kitty() {
        assert!(is_image_line("\x1b_Ga=T,f=100;\x1b\\"));
    }

    #[test]
    fn test_is_image_line_iterm2() {
        assert!(is_image_line("\x1b]1337;File=inline=1:dGVzdA==\x07"));
    }

    #[test]
    fn test_is_image_line_plain() {
        assert!(!is_image_line("Hello, world!"));
    }

    // ----- Image dimensions via image crate -----

    #[test]
    fn test_get_image_dimensions() {
        let png = sample_png();
        let dims = get_image_dimensions(&png);
        assert!(dims.is_some());
        let d = dims.unwrap();
        assert_eq!(d.width_px, 1);
        assert_eq!(d.height_px, 1);
    }

    // ----- Protocol display -----

    #[test]
    fn test_protocol_display() {
        assert_eq!(format!("{}", ImageProtocol::Kitty), "kitty");
        assert_eq!(format!("{}", ImageProtocol::Iterm2), "iterm2");
        assert_eq!(format!("{}", ImageProtocol::Fallback), "fallback");
        assert_eq!(format!("{}", ImageProtocol::Auto), "auto");
    }

    // ----- prepare_image -----

    /// Create a valid 2×2 PNG using the image crate.
    fn make_test_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn test_prepare_image_png_passthrough() {
        let png = make_test_png();
        let result = prepare_image(&png, None, None);
        assert!(result.is_some());
        let out = result.unwrap();
        // Output should be a valid PNG
        assert_eq!(&out[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn test_prepare_image_resize() {
        let png = make_test_png();
        // 2x2 image, max 100x100 – no resize needed
        let result = prepare_image(&png, Some(100), Some(100));
        assert!(result.is_some());
    }

    // ----- Kitty format codes -----

    #[test]
    fn test_kitty_format_codes() {
        assert_eq!(ImageFormat::Png.kitty_format_code(), 100);
        assert_eq!(ImageFormat::Jpeg.kitty_format_code(), 24);
        // Others get re-encoded to PNG
        assert_eq!(ImageFormat::Gif.kitty_format_code(), 100);
        assert_eq!(ImageFormat::WebP.kitty_format_code(), 100);
        assert_eq!(ImageFormat::Bmp.kitty_format_code(), 100);
    }

    // ----- MIME type -----

    #[test]
    fn test_mime_types() {
        assert_eq!(ImageFormat::Png.mime_type(), "image/png");
        assert_eq!(ImageFormat::Jpeg.mime_type(), "image/jpeg");
        assert_eq!(ImageFormat::Gif.mime_type(), "image/gif");
        assert_eq!(ImageFormat::WebP.mime_type(), "image/webp");
        assert_eq!(ImageFormat::Bmp.mime_type(), "image/bmp");
    }

    // ----- TerminalCapabilities -----

    #[test]
    fn test_capabilities_default() {
        let caps = TerminalCapabilities::default();
        assert_eq!(caps.protocol, ImageProtocol::Fallback);
        assert!(!caps.true_color);
        assert!(!caps.hyperlinks);
    }
}
