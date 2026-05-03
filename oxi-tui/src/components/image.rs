//! Image component – renders images inline in the terminal.
//!
//! Supports two protocols:
//! - **Kitty graphics protocol**: high-quality images in Kitty / compatible terminals.
//! - **iTerm2 inline images**: fallback for iTerm2 / compatible terminals.
//!
//! The component detects which protocol to use based on environment variables
//! (`TERM`, `KITTY_WINDOW_ID`, `TERM_PROGRAM`) and falls back gracefully.

use std::fmt;
use std::path::Path;

use crate::cell::Cell;
use crate::component::Component;
use crate::event::Event;
use crate::surface::{Rect, Surface};
use crate::Size;

// ---------------------------------------------------------------------------
// Image data
// ---------------------------------------------------------------------------

/// Which terminal graphics protocol to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageProtocol {
    /// Automatically detect from environment.
    #[default]
    Auto,
    /// Kitty graphics protocol (`\x1b_G…\x1b\\`).
    Kitty,
    /// iTerm2 inline images (`\x1b]1337;File=…\x07`).
    Iterm2,
    /// No supported protocol – render a placeholder.
    Fallback,
}

/// A terminal image ready for rendering.
#[derive(Debug, Clone)]
pub struct Image {
    /// Raw image bytes (PNG, JPEG, etc.).
    data: Vec<u8>,
    /// MIME type (e.g. `"image/png"`).
    mime_type: String,
    /// Optional display width in terminal columns.
    width: Option<u32>,
    /// Optional display height in terminal rows.
    height: Option<u32>,
    /// Force a specific protocol (defaults to `Auto`).
    protocol: ImageProtocol,
    /// Cached base64-encoded data (lazily computed).
    cached_b64: Option<String>,
    /// Dirty flag.
    dirty: bool,
}

impl Image {
    /// Create a new Image from raw bytes and a MIME type.
    pub fn new(data: Vec<u8>, mime_type: &str) -> Self {
        Self {
            data,
            mime_type: mime_type.to_string(),
            width: None,
            height: None,
            protocol: ImageProtocol::Auto,
            cached_b64: None,
            dirty: true,
        }
    }

    /// Set the display width in terminal columns.
    pub fn with_width(mut self, width: u32) -> Self {
        self.width = Some(width);
        self
    }

    /// Set the display height in terminal rows.
    pub fn with_height(mut self, height: u32) -> Self {
        self.height = Some(height);
        self
    }

    /// Force a specific rendering protocol.
    pub fn with_protocol(mut self, protocol: ImageProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Load an image from a file path.
    ///
    /// Detects MIME type from extension. Supports PNG and JPEG.
    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        let mime_type = mime_from_extension(path);
        Ok(Self::new(data, &mime_type))
    }

    /// Load an image from a file path string.
    pub fn from_file_str(path: &str) -> std::io::Result<Self> {
        Self::from_file(Path::new(path))
    }

    /// Get the raw image data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get the MIME type.
    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }

    /// Get the configured display width.
    pub fn width(&self) -> Option<u32> {
        self.width
    }

    /// Get the configured display height.
    pub fn height(&self) -> Option<u32> {
        self.height
    }

    /// Get the base64-encoded data (cached).
    fn base64_data(&mut self) -> &str {
        if self.cached_b64.is_none() {
            use base64::Engine;
            self.cached_b64 = Some(base64::engine::general_purpose::STANDARD.encode(&self.data));
        }
        self.cached_b64.as_ref().unwrap()
    }

    /// Detect which protocol to actually use.
    fn effective_protocol(&self) -> ImageProtocol {
        match self.protocol {
            ImageProtocol::Auto => detect_protocol(),
            other => other,
        }
    }

    // -----------------------------------------------------------------------
    // Kitty graphics protocol
    // -----------------------------------------------------------------------

    /// Render using the Kitty graphics protocol.
    ///
    /// The Kitty protocol sends image data as base64 inside `\x1b_G…\x1b\\`
    /// escape sequences. We chunk the payload to stay under 4096 bytes per
    /// chunk (Kitty's recommended safe chunk size).
    ///
    /// Control parameters:
    /// - `a=t`  : transmit-and-display action
    /// - `f=<N>`: format (100 = PNG, 24 = JPEG → auto-detected from MIME)
    /// - `s=<N>`: total data size in bytes
    /// - `C=1`  : do not move cursor
    /// - `q=2`  : suppress protocol-specific errors
    fn render_kitty(&mut self, cols: u16, rows: u16) -> Vec<String> {
        let display_w = self.width.unwrap_or(cols as u32);
        let display_h = self.height.unwrap_or(rows as u32);
        let data_len = self.data.len();
        let format = match self.mime_type.as_str() {
            "image/jpeg" => 24,
            _ => 100, // PNG and everything else
        };

        let b64 = self.base64_data().to_string();

        let chunk_size = 4096;
        let total_chunks = (b64.len() + chunk_size - 1) / chunk_size;
        let mut lines: Vec<String> = Vec::new();

        // First chunk includes image metadata.
        let first_chunk = &b64[..b64.len().min(chunk_size)];
        let more = if total_chunks > 1 { 1 } else { 0 };

        // Place image with a placeholder cell so the layout knows its size.
        // We emit the Kitty escape on the first row.
        let header = format!(
            "a=t,f={},s={},c={},r={},C=1,q=2,m={};{}",
            format, data_len, display_w, display_h, more, first_chunk,
        );
        lines.push(format!("\x1b_G{}\x1b\\", header));

        // Subsequent chunks (m=1 means more follows, m=0 means last).
        let mut offset = chunk_size;
        while offset < b64.len() {
            let end = (offset + chunk_size).min(b64.len());
            let chunk = &b64[offset..end];
            let more = if end < b64.len() { 1 } else { 0 };
            lines.push(format!("\x1b_Gm={};{}\x1b\\", more, chunk));
            offset = end;
        }

        // Add blank placeholder rows so the surface knows the image height.
        // The actual image is rendered by the terminal overlay; we just need
        // to reserve the space.
        let placeholder_rows = display_h as usize;
        while lines.len() < placeholder_rows {
            lines.push(String::new());
        }

        lines
    }

    // -----------------------------------------------------------------------
    // iTerm2 inline images
    // -----------------------------------------------------------------------

    /// Render using the iTerm2 inline image protocol.
    ///
    /// iTerm2 uses OSC 1337: `\x1b]1337;File=inline=1;width=N;height=N:<base64>\x07`
    fn render_iterm2(&mut self, cols: u16, rows: u16) -> Vec<String> {
        let display_w = self.width.unwrap_or(cols as u32);
        let display_h = self.height.unwrap_or(rows as u32);
        let data_len = self.data.len();
        let b64 = self.base64_data().to_string();

        let payload = format!(
            "\x1b]1337;File=inline=1;size={};width={}{};height={}{}:{}\x07",
            data_len,
            display_w,
            "c", // columns
            display_h,
            "r", // rows
            b64,
        );

        let mut lines: Vec<String> = Vec::new();
        lines.push(payload);

        // Reserve placeholder rows.
        let placeholder_rows = display_h as usize;
        while lines.len() < placeholder_rows {
            lines.push(String::new());
        }

        lines
    }

    // -----------------------------------------------------------------------
    // Fallback (placeholder)
    // -----------------------------------------------------------------------

    /// Render a text placeholder when no image protocol is available.
    fn render_fallback(&self, cols: u16) -> Vec<String> {
        let display_w = self.width.map(|w| w as usize).unwrap_or(cols as usize);
        let display_h = self.height.unwrap_or(3) as usize;

        let mut lines: Vec<String> = Vec::new();

        // Top border: ┌───┐
        let inner = display_w.saturating_sub(2);
        lines.push(format!("┌{}┐", "─".repeat(inner.max(1))));

        // Content line(s): │ [image/...] │
        let label = format!(" {} ", self.mime_type);
        let padded = if label.len() < inner.max(1) {
            let pad = inner.max(1) - label.len();
            format!("{}{}", label, " ".repeat(pad))
        } else {
            label[..inner.max(1)].to_string()
        };
        lines.push(format!("│{}│", padded));

        // Size info if it fits
        let size_str = format!(
            " {}×{} ",
            self.width
                .map(|w| w.to_string())
                .unwrap_or_else(|| "?".to_string()),
            self.height
                .map(|h| h.to_string())
                .unwrap_or_else(|| "?".to_string())
        );
        let padded_size = if size_str.len() < inner.max(1) {
            let pad = inner.max(1) - size_str.len();
            format!("{}{}", size_str, " ".repeat(pad))
        } else {
            size_str[..inner.max(1)].to_string()
        };
        if display_h > 2 {
            lines.push(format!("│{}│", padded_size));
        }

        // Fill remaining rows
        while lines.len() < display_h.saturating_sub(1) {
            lines.push(format!("│{}│", " ".repeat(inner.max(1))));
        }

        // Bottom border: └───┘
        lines.push(format!("└{}┘", "─".repeat(inner.max(1))));

        lines
    }

    // -----------------------------------------------------------------------
    // Raw escape sequence generation
    // -----------------------------------------------------------------------

    /// Produce the raw ANSI escape sequence(s) for this image.
    ///
    /// Returns a list of strings, one per logical row. The first string
    /// contains the actual image escape; subsequent strings are placeholders.
    pub fn escape_sequence(&mut self, cols: u16, rows: u16) -> Vec<String> {
        match self.effective_protocol() {
            ImageProtocol::Kitty => self.render_kitty(cols, rows),
            ImageProtocol::Iterm2 => self.render_iterm2(cols, rows),
            ImageProtocol::Fallback => self.render_fallback(cols),
            ImageProtocol::Auto => unreachable!(),
        }
    }
}

// ---------------------------------------------------------------------------
// Protocol detection
// ---------------------------------------------------------------------------

/// Detect the best available image protocol from the environment.
pub fn detect_protocol() -> ImageProtocol {
    // Kitty terminals set KITTY_WINDOW_ID or have "kitty" in TERM.
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return ImageProtocol::Kitty;
    }
    if let Ok(term) = std::env::var("TERM") {
        if term.contains("kitty") {
            return ImageProtocol::Kitty;
        }
    }

    // Ghostty also supports Kitty protocol.
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        if term_program.contains("ghostty") || term_program.contains("Ghostty") {
            return ImageProtocol::Kitty;
        }
    }

    // iTerm2 sets TERM_PROGRAM=iTerm.app.
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        if term_program == "iTerm.app" || term_program.contains("iTerm") {
            return ImageProtocol::Iterm2;
        }
    }

    // WezTerm supports Kitty protocol.
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        if term_program.contains("WezTerm") {
            return ImageProtocol::Kitty;
        }
    }

    ImageProtocol::Fallback
}

// ---------------------------------------------------------------------------
// MIME detection
// ---------------------------------------------------------------------------

fn mime_from_extension(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "gif" => "image/gif".to_string(),
        "webp" => "image/webp".to_string(),
        "bmp" => "image/bmp".to_string(),
        "svg" => "image/svg+xml".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Component implementation
// ---------------------------------------------------------------------------

impl Component for Image {
    fn name(&self) -> &str {
        "Image"
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn handle_event(&mut self, _event: &Event) -> bool {
        false
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        let lines = self.escape_sequence(area.width, area.height);

        for (row_idx, line) in lines.iter().enumerate() {
            let y = area.y + row_idx as u16;
            if y >= area.y + area.height {
                break;
            }

            for (col_idx, ch) in line.chars().enumerate() {
                let x = area.x + col_idx as u16;
                if x >= area.x + area.width {
                    break;
                }
                surface.set(y, x, Cell::new(ch));
            }
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: 4,
            height: 3,
        }
    }

    fn desired_size(&self) -> Option<Size> {
        let w = self.width.unwrap_or(40) as u16;
        let h = self.height.unwrap_or(10) as u16;
        Some(Size {
            width: w,
            height: h,
        })
    }
}

// ---------------------------------------------------------------------------
// Display for ImageProtocol
// ---------------------------------------------------------------------------

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_png_data() -> Vec<u8> {
        // Minimal valid PNG: 1x1 red pixel
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, // 8-bit RGB
            0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT chunk
            0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21,
            0xBC, 0x33, // compressed data
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND chunk
            0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn test_image_new() {
        let img = Image::new(sample_png_data(), "image/png");
        assert_eq!(img.mime_type(), "image/png");
        assert_eq!(img.data().len(), sample_png_data().len());
        assert!(img.width().is_none());
        assert!(img.height().is_none());
    }

    #[test]
    fn test_image_with_dimensions() {
        let img = Image::new(sample_png_data(), "image/png")
            .with_width(40)
            .with_height(12);
        assert_eq!(img.width(), Some(40));
        assert_eq!(img.height(), Some(12));
    }

    #[test]
    fn test_image_with_protocol() {
        let img = Image::new(sample_png_data(), "image/png").with_protocol(ImageProtocol::Kitty);
        assert_eq!(img.protocol, ImageProtocol::Kitty);
        assert_eq!(img.effective_protocol(), ImageProtocol::Kitty);
    }

    #[test]
    fn test_kitty_render() {
        let mut img = Image::new(sample_png_data(), "image/png")
            .with_protocol(ImageProtocol::Kitty)
            .with_width(20)
            .with_height(5);
        let lines = img.render_kitty(80, 24);
        // First line should start with Kitty escape
        assert!(lines[0].starts_with("\x1b_G"));
        assert!(lines[0].contains("f=100")); // PNG format
        assert!(lines[0].contains("c=20")); // width
        assert!(lines[0].contains("r=5")); // height
                                           // Should have at least 5 rows (placeholder)
        assert!(lines.len() >= 5);
    }

    #[test]
    fn test_kitty_chunking() {
        // Create a large enough image to require chunking
        let large_data = vec![0u8; 10000];
        let mut img = Image::new(large_data, "image/png")
            .with_protocol(ImageProtocol::Kitty)
            .with_width(10)
            .with_height(3);
        let lines = img.render_kitty(80, 24);
        // First line should have m=1 (more chunks follow)
        assert!(lines[0].contains("m=1"));
        // Should have continuation chunks
        assert!(lines.len() >= 2);
    }

    #[test]
    fn test_iterm2_render() {
        let mut img = Image::new(sample_png_data(), "image/png")
            .with_protocol(ImageProtocol::Iterm2)
            .with_width(20)
            .with_height(5);
        let lines = img.render_iterm2(80, 24);
        // First line should contain iTerm2 OSC
        assert!(lines[0].contains("\x1b]1337;File=inline=1"));
        assert!(lines[0].contains("width=20c"));
        assert!(lines[0].contains("height=5r"));
        // Should have placeholder rows
        assert!(lines.len() >= 5);
    }

    #[test]
    fn test_fallback_render() {
        let img = Image::new(sample_png_data(), "image/png")
            .with_protocol(ImageProtocol::Fallback)
            .with_width(20)
            .with_height(4);
        let lines = img.render_fallback(80);
        // Should have box-drawing borders
        assert!(lines[0].starts_with('┌'));
        assert!(lines.last().unwrap().starts_with('└'));
        // Should contain the MIME type
        assert!(lines.iter().any(|l| l.contains("image/png")));
    }

    #[test]
    fn test_mime_detection() {
        assert_eq!(mime_from_extension(Path::new("photo.png")), "image/png");
        assert_eq!(mime_from_extension(Path::new("photo.jpg")), "image/jpeg");
        assert_eq!(mime_from_extension(Path::new("photo.jpeg")), "image/jpeg");
        assert_eq!(mime_from_extension(Path::new("photo.gif")), "image/gif");
        assert_eq!(mime_from_extension(Path::new("photo.webp")), "image/webp");
        assert_eq!(mime_from_extension(Path::new("photo.bmp")), "image/bmp");
        assert_eq!(mime_from_extension(Path::new("icon.svg")), "image/svg+xml");
        assert_eq!(
            mime_from_extension(Path::new("file.unknown")),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_from_file_not_found() {
        let result = Image::from_file(Path::new("/nonexistent/image.png"));
        assert!(result.is_err());
    }

    #[test]
    fn test_from_file_str() {
        let result = Image::from_file_str("/nonexistent/image.png");
        assert!(result.is_err());
    }

    #[test]
    fn test_component_trait() {
        let mut img = Image::new(sample_png_data(), "image/png")
            .with_protocol(ImageProtocol::Fallback)
            .with_width(10)
            .with_height(3);
        assert_eq!(img.name(), "Image");
        assert!(img.is_dirty());
        img.clear_dirty();
        assert!(!img.is_dirty());
        img.request_render();
        assert!(img.is_dirty());

        let min = img.min_size();
        assert_eq!(min.width, 4);
        assert_eq!(min.height, 3);

        let desired = img.desired_size().unwrap();
        assert_eq!(desired.width, 10);
        assert_eq!(desired.height, 3);
    }

    #[test]
    fn test_component_render_to_surface() {
        let mut img = Image::new(sample_png_data(), "image/png")
            .with_protocol(ImageProtocol::Fallback)
            .with_width(10)
            .with_height(3);
        let mut surface = Surface::new(80, 24);
        let area = Rect::new(0, 0, 10, 3);
        img.render(&mut surface, area);
        img.clear_dirty();
        // The top-left corner should be '┌'
        let cell = surface.get(0, 0).unwrap();
        assert_eq!(cell.char, '┌');
    }

    #[test]
    fn test_escape_sequence_dispatches_correctly() {
        let mut img = Image::new(sample_png_data(), "image/png")
            .with_protocol(ImageProtocol::Kitty)
            .with_width(10)
            .with_height(3);
        let lines = img.escape_sequence(80, 24);
        assert!(lines[0].starts_with("\x1b_G"));

        let mut img = Image::new(sample_png_data(), "image/png")
            .with_protocol(ImageProtocol::Iterm2)
            .with_width(10)
            .with_height(3);
        let lines = img.escape_sequence(80, 24);
        assert!(lines[0].contains("\x1b]1337"));
    }

    #[test]
    fn test_jpeg_format_code() {
        let mut img = Image::new(vec![0u8; 100], "image/jpeg")
            .with_protocol(ImageProtocol::Kitty)
            .with_width(10)
            .with_height(3);
        let lines = img.render_kitty(80, 24);
        assert!(lines[0].contains("f=24"));
    }

    #[test]
    fn test_display_protocol() {
        assert_eq!(format!("{}", ImageProtocol::Kitty), "kitty");
        assert_eq!(format!("{}", ImageProtocol::Iterm2), "iterm2");
        assert_eq!(format!("{}", ImageProtocol::Fallback), "fallback");
        assert_eq!(format!("{}", ImageProtocol::Auto), "auto");
    }
}
