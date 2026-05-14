//! MIME type detection from file magic bytes.
//!
//! Detects supported image MIME types by reading file magic bytes.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Supported image MIME types
const SUPPORTED_IMAGE_MIMES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Number of bytes to read for MIME type sniffing
const FILE_TYPE_SNIFF_BYTES: usize = 4100;

/// Detect supported image MIME type from a file by reading magic bytes.
///
/// Returns the MIME type string if detected and supported, or `None`.
pub fn detect_supported_image_mime_type_from_file(file_path: &Path) -> Option<String> {
    let file = File::open(file_path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buffer = vec![0u8; FILE_TYPE_SNIFF_BYTES];

    let bytes_read = reader.read(&mut buffer).ok()?;
    if bytes_read == 0 {
        return None;
    }

    let buffer = &buffer[..bytes_read];
    let mime_type = detect_mime_from_bytes(buffer)?;

    if SUPPORTED_IMAGE_MIMES.contains(&mime_type.as_str()) {
        Some(mime_type)
    } else {
        None
    }
}

/// Detect MIME type from raw bytes using magic byte matching.
fn detect_mime_from_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 4 {
        return None;
    }

    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png".to_string());
    }

    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg".to_string());
    }

    // GIF87a: 47 49 46 38 37 61
    if bytes.starts_with(b"GIF87a") {
        return Some("image/gif".to_string());
    }

    // GIF89a: 47 49 46 38 39 61
    if bytes.starts_with(b"GIF89a") {
        return Some("image/gif".to_string());
    }

    // WebP: RIFF .... WEBP
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp".to_string());
    }

    // BMP: 42 4D
    if bytes.starts_with(b"BM") {
        return Some("image/bmp".to_string());
    }

    None
}

/// Check if a MIME type is a supported image type
pub fn is_supported_image_mime(mime_type: &str) -> bool {
    SUPPORTED_IMAGE_MIMES.contains(&mime_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported_image_mime() {
        assert!(is_supported_image_mime("image/png"));
        assert!(is_supported_image_mime("image/jpeg"));
        assert!(is_supported_image_mime("image/gif"));
        assert!(is_supported_image_mime("image/webp"));
        assert!(!is_supported_image_mime("image/bmp"));
        assert!(!is_supported_image_mime("text/plain"));
    }

    #[test]
    fn test_detect_mime_from_bytes() {
        // PNG bytes
        let png_bytes = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        ];
        assert_eq!(detect_mime_from_bytes(&png_bytes), Some("image/png".to_string()));

        // JPEG bytes
        let jpeg_bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];
        assert_eq!(detect_mime_from_bytes(&jpeg_bytes), Some("image/jpeg".to_string()));

        // GIF87a
        let gif_bytes = b"GIF87a\x00\x01\x00\x01\x00";
        assert_eq!(detect_mime_from_bytes(gif_bytes), Some("image/gif".to_string()));

        // GIF89a
        let gif_bytes = b"GIF89a\x00\x01\x00\x01\x00";
        assert_eq!(detect_mime_from_bytes(gif_bytes), Some("image/gif".to_string()));

        // WebP
        let webp_bytes = b"RIFF\x00\x00\x00\x00WEBP";
        assert_eq!(detect_mime_from_bytes(webp_bytes), Some("image/webp".to_string()));

        // BMP
        let bmp_bytes = b"BM\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(detect_mime_from_bytes(bmp_bytes), Some("image/bmp".to_string()));

        // Unknown
        let unknown_bytes = b"hello world";
        assert_eq!(detect_mime_from_bytes(unknown_bytes), None);

        // Too short
        let short_bytes = [0x89, 0x50];
        assert_eq!(detect_mime_from_bytes(&short_bytes), None);
    }
}
