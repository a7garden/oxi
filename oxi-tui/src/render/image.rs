//! Terminal image protocols — Kitty and iTerm2.
//!
//! Encodes images for inline display in terminal emulators that support
//! the Kitty graphics protocol or iTerm2 inline images protocol.

/// Image dimension information detected from file data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Options for image rendering.
#[derive(Debug, Clone)]
pub struct ImageOptions {
    /// Unique image ID for Kitty protocol (for lifecycle management).
    pub id: Option<u32>,
    /// Display width in cells (None = auto).
    pub width_cells: Option<u16>,
    /// Display height in cells (None = auto).
    pub height_cells: Option<u16>,
    /// X offset in cells.
    pub x: u16,
    /// Y offset in cells (row number).
    pub y: u16,
    /// Whether to place the image using cursor positioning.
    pub place: bool,
}

impl Default for ImageOptions {
    fn default() -> Self {
        ImageOptions {
            id: None,
            width_cells: None,
            height_cells: None,
            x: 0,
            y: 0,
            place: true,
        }
    }
}

/// Detect image dimensions from raw file data.
///
/// Supports PNG, JPEG, GIF, and WebP by reading header bytes.
/// No external dependencies required — parses format headers directly.
pub fn detect_dimensions(data: &[u8]) -> Option<ImageDimensions> {
    if data.len() < 10 {
        return None;
    }

    // PNG: bytes 16-23 contain width and height (big-endian u32)
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) && data.len() >= 24 {
        let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some(ImageDimensions { width, height });
    }

    // JPEG: SOF0 marker (0xFF 0xC0) contains dimensions
    if data[0] == 0xFF && data[1] == 0xD8 {
        // Scan for SOF0 marker
        let mut i = 2;
        while i + 9 <= data.len() {
            if data[i] == 0xFF {
                let marker = data[i + 1];
                // SOF0 or SOF2 (progressive)
                if marker == 0xC0 || marker == 0xC2 {
                    let height = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                    let width = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                    return Some(ImageDimensions { width, height });
                }
                // Skip to next marker
                if i + 3 <= data.len() && marker != 0x00 && marker != 0xFF {
                    let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                    i += 2 + seg_len;
                } else {
                    i += 2;
                }
            } else {
                i += 1;
            }
        }
    }

    // GIF: bytes 6-9 contain width and height (little-endian u16)
    if (data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) && data.len() >= 10 {
        let width = u16::from_le_bytes([data[6], data[7]]) as u32;
        let height = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some(ImageDimensions { width, height });
    }

    // WebP: RIFF header + VP8/VP8L
    if data.starts_with(b"RIFF") && data.len() >= 30 && data[8..12].starts_with(b"WEBP") {
        // VP8 lossy
        if data[12..16].starts_with(b"VP8 ") && data.len() >= 30 {
            let width = (u16::from_le_bytes([data[26], data[27]]) & 0x3FFF) as u32;
            let height = (u16::from_le_bytes([data[28], data[29]]) & 0x3FFF) as u32;
            return Some(ImageDimensions { width, height });
        }
        // VP8L lossless
        if data[12..16].starts_with(b"VP8L") && data.len() >= 25 {
            let bits = u32::from_le_bytes([data[21], data[22], data[23], data[24]]);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            return Some(ImageDimensions { width, height });
        }
    }

    None
}

/// Encode an image using the Kitty graphics protocol.
///
/// The Kitty protocol sends images as base64-encoded data in 4096-byte chunks.
/// Each chunk is sent as an escape sequence with control flags.
///
/// Protocol: `ESC _ G a=<action>,<key=value>,...; <base64_data> ESC \`
pub fn encode_kitty(base64_data: &str, opts: &ImageOptions) -> String {
    let mut sequences = Vec::new();
    let chunk_size = 4096;
    let total_len = base64_data.len();
    let mut offset = 0;

    // Build control parameters
    let mut params = vec![
        "a=T".to_string(),   // action = transmit
        "f=100".to_string(), // format = PNG (auto-detected)
        "q=2".to_string(),   // compression = none
    ];

    if let Some(id) = opts.id {
        params.push(format!("i={}", id));
    }

    if opts.width_cells.is_some() || opts.height_cells.is_some() {
        let w = opts.width_cells.map_or(0, |w| w);
        let h = opts.height_cells.map_or(0, |h| h);
        if w > 0 && h > 0 {
            params.push(format!("c={},r={}", w, h));
        } else if w > 0 {
            params.push(format!("c={}", w));
        } else if h > 0 {
            params.push(format!("r={}", h));
        }
    }

    while offset < total_len {
        let end = (offset + chunk_size).min(total_len);
        let chunk = &base64_data[offset..end];
        let is_first = offset == 0;
        let _is_last = end >= total_len;

        // m=0: first/single chunk, m=1: continuation
        let m = if is_first { 0 } else { 1 };

        let mut chunk_params = params.clone();
        if !is_first {
            // Remove a=T for continuation chunks — only send data
            chunk_params.retain(|p| !p.starts_with("a=") && !p.starts_with("f="));
        }
        chunk_params.push(format!("m={}", m));

        let param_str = chunk_params.join(",");
        sequences.push(format!("\x1b_G{};{}\x1b\\", param_str, chunk));

        offset = end;
    }

    // If we need to place the image at a specific position
    if opts.place && (opts.x > 0 || opts.y > 0) {
        // Use cursor positioning
        sequences.insert(0, format!("\x1b[{};{}H", opts.y + 1, opts.x + 1));
    }

    sequences.join("")
}

/// Encode an image using the iTerm2 inline image protocol.
///
/// Protocol: `ESC ] 1337 ; File=<key=value>,...:<base64_data> BEL`
pub fn encode_iterm2(base64_data: &str, opts: &ImageOptions) -> String {
    let mut params = vec![
        "inline=1".to_string(),
        format!("size={}", base64_data.len()),
    ];

    if let Some(w) = opts.width_cells {
        params.push(format!("width={}c", w));
    }
    if let Some(h) = opts.height_cells {
        params.push(format!("height={}c", h));
    }

    let param_str = params.join(";");

    // Position cursor if needed
    let mut result = String::new();
    if opts.place && (opts.x > 0 || opts.y > 0) {
        result.push_str(&format!("\x1b[{};{}H", opts.y + 1, opts.x + 1));
    }

    result.push_str(&format!("\x1b]1337;File={}:{}\x07", param_str, base64_data));

    result
}

/// Delete a Kitty image by ID.
pub fn kitty_delete_image(id: u32) -> String {
    format!("\x1b_Ga=d,d=i,i={}\x1b\\", id)
}

/// Delete all Kitty images.
pub fn kitty_delete_all() -> String {
    "\x1b_Ga=d,d=A\x1b\\".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_png_dimensions() {
        // Minimal valid PNG header (8-byte signature + IHDR chunk)
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        // IHDR chunk: length (4) + type (4) + data (13) + crc (4)
        png.extend_from_slice(&[0, 0, 0, 13]); // length
        png.extend_from_slice(b"IHDR"); // type
        png.extend_from_slice(&[0, 0, 0, 100]); // width = 100
        png.extend_from_slice(&[0, 0, 0, 200]); // height = 200
        png.extend_from_slice(&[8]); // bit depth
        png.extend_from_slice(&[2]); // color type
        png.extend_from_slice(&[0, 0, 0]); // compression, filter, interlace
        png.extend_from_slice(&[0, 0, 0, 0]); // crc (dummy)

        let dims = detect_dimensions(&png).unwrap();
        assert_eq!(dims.width, 100);
        assert_eq!(dims.height, 200);
    }

    #[test]
    fn test_detect_gif_dimensions() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[100, 0]); // width = 100 (LE)
        gif.extend_from_slice(&[50, 0]); // height = 50 (LE)
        gif.extend_from_slice(&[0x80]); // GCT flag

        let dims = detect_dimensions(&gif).unwrap();
        assert_eq!(dims.width, 100);
        assert_eq!(dims.height, 50);
    }

    #[test]
    fn test_detect_too_small() {
        let data = [0u8; 5];
        assert!(detect_dimensions(&data).is_none());
    }

    #[test]
    fn test_detect_unknown_format() {
        let data = vec![0u8; 100];
        assert!(detect_dimensions(&data).is_none());
    }

    #[test]
    fn test_encode_kitty_single_chunk() {
        let base64 = "dGVzdA=="; // "test"
        let opts = ImageOptions::default();
        let result = encode_kitty(base64, &opts);
        assert!(result.contains("\x1b_G"));
        assert!(result.contains("a=T"));
        assert!(result.contains(base64));
        assert!(result.contains("\x1b\\"));
    }

    #[test]
    fn test_encode_kitty_with_id() {
        let base64 = "dGVzdA==";
        let opts = ImageOptions {
            id: Some(42),
            ..Default::default()
        };
        let result = encode_kitty(base64, &opts);
        assert!(result.contains("i=42"));
    }

    #[test]
    fn test_encode_kitty_multi_chunk() {
        // 5000 chars = 2 chunks (4096 + 904)
        let base64: String = "A".repeat(5000);
        let opts = ImageOptions::default();
        let result = encode_kitty(&base64, &opts);
        // Should contain two \x1b_G sequences
        let count = result.matches("\x1b_G").count();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_encode_iterm2() {
        let base64 = "dGVzdA==";
        let opts = ImageOptions {
            width_cells: Some(20),
            height_cells: Some(10),
            ..Default::default()
        };
        let result = encode_iterm2(base64, &opts);
        assert!(result.contains("\x1b]1337;File="));
        assert!(result.contains("width=20c"));
        assert!(result.contains("height=10c"));
        assert!(result.contains(base64));
        assert!(result.contains("\x07"));
    }

    #[test]
    fn test_kitty_delete_image() {
        let result = kitty_delete_image(42);
        assert!(result.contains("a=d"));
        assert!(result.contains("i=42"));
    }

    #[test]
    fn test_kitty_delete_all() {
        let result = kitty_delete_all();
        assert!(result.contains("a=d"));
        assert!(result.contains("d=A"));
    }
}
