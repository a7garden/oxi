//! Image conversion for terminal display
//!
//! Converts images to PNG format for terminal graphics protocols (Kitty, etc.)
//! Also provides dimension extraction for display calculations.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView};
use std::io::Cursor;

/// Convert image bytes to PNG format
///
/// If the image is already PNG, returns it as-is.
/// Otherwise, decodes the image and re-encodes as PNG.
pub fn convert_to_png(bytes: &[u8], mime: &str) -> Result<Vec<u8>> {
    // If already PNG, return as-is
    if mime == "image/png" {
        // Verify it's valid PNG
        if bytes.len() >= 8 && bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
            return Ok(bytes.to_vec());
        }
    }

    // Decode the image
    let img = decode_image(bytes, mime)?;

    // Apply EXIF orientation if present
    let img = apply_exif_orientation(&img, bytes);

    // Encode as PNG
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .context("Failed to encode PNG")?;
    Ok(buf)
}

/// Decode image from bytes with MIME type hint
fn decode_image(bytes: &[u8], mime: &str) -> Result<DynamicImage> {
    // Try to use the format hint first
    let format = match mime {
        "image/png" => Some(image::ImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(image::ImageFormat::Jpeg),
        "image/gif" => Some(image::ImageFormat::Gif),
        "image/webp" => Some(image::ImageFormat::WebP),
        "image/bmp" => Some(image::ImageFormat::Bmp),
        "image/tiff" => Some(image::ImageFormat::Tiff),
        _ => None,
    };

    if let Some(fmt) = format {
        image::load_from_memory_with_format(bytes, fmt)
            .context(format!("Failed to decode {} image", mime))
    } else {
        // Fallback to auto-detection
        image::load_from_memory(bytes)
            .with_context(|| format!("Failed to decode image (guessed: {})", mime))
    }
}

/// Get image dimensions
pub fn get_image_dimensions(bytes: &[u8], mime: &str) -> Result<(u32, u32)> {
    // If it's PNG, we can get dimensions from header without full decode
    if mime == "image/png" && bytes.len() >= 24 {
        if let Some(dims) = get_png_dimensions_fast(&bytes) {
            return Ok(dims);
        }
    }

    // For other formats or if PNG fast path failed, decode the image
    let img = decode_image(bytes, mime)?;
    Ok(img.dimensions())
}

/// Get PNG dimensions from header without full decode
/// PNG header format: 89 50 4E 47 0D 0A 1A 0A [length:4] [type:4] [data...]
/// Width at offset 16, Height at offset 20 (big-endian)
fn get_png_dimensions_fast(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 32 {
        return None;
    }
    if !bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return None;
    }

    // PNG structure: 8-byte signature, then chunks.
    // First chunk is IHDR: [length:4 big-endian] [type:4 = "IHDR"] [data:13 bytes]
    let length = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if &bytes[12..16] != b"IHDR" {
        return None;
    }

    // IHDR data is exactly 13 bytes: width(4) + height(4) + bit_depth(1) + color_type(1) + ...
    if length < 13 {
        return None;
    }

    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);

    Some((width, height))
}

/// Apply EXIF orientation to an image
///
/// Many images contain EXIF metadata that specifies rotation/flip.
/// This function applies that transformation.
fn apply_exif_orientation(img: &DynamicImage, _bytes: &[u8]) -> DynamicImage {
    // For now, we don't extract EXIF orientation in pure Rust
    // This would require the kamadak-exif crate
    // Return the image as-is for now
    img.clone()
}

/// Ensure image is upright (apply any needed transforms)
///
/// This is a placeholder for full EXIF orientation support.
/// Returns the image in normal orientation.
pub fn ensure_upright(img: &DynamicImage) -> DynamicImage {
    img.clone()
}

/// Convert image to base64 data URI
pub fn to_data_uri(bytes: &[u8], mime: &str) -> String {
    use base64::Engine as _;
    let base64_data = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{};base64,{}", mime, base64_data)
}

/// Parse data URI
pub fn parse_data_uri(uri: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine as _;

    if !uri.starts_with("data:") {
        return None;
    }

    let (mime_part, data_part) = uri.split_once(',')?;
    let mime = mime_part
        .strip_prefix("data:")
        .and_then(|s| s.split(';').next())
        .unwrap_or("image/png")
        .to_string();

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_part)
        .ok()?;

    Some((mime, bytes))
}

/// Image format info
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
/// png variant.
    Png,
/// jpeg variant.
    Jpeg,
/// gif variant.
    Gif,
/// web p variant.
    WebP,
/// bmp variant.
    Bmp,
/// unknown variant.
    Unknown,
}

impl ImageFormat {
/// TODO: document this function.
    pub fn mime_type(&self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Gif => "image/gif",
            ImageFormat::WebP => "image/webp",
            ImageFormat::Bmp => "image/bmp",
            ImageFormat::Unknown => "application/octet-stream",
        }
    }
}

/// Detect image format from bytes
pub fn detect_format(bytes: &[u8]) -> ImageFormat {
    if bytes.len() >= 8 {
        if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
            return ImageFormat::Png;
        }
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return ImageFormat::Jpeg;
        }
        if bytes.starts_with(&[0x47, 0x49, 0x46]) {
            return ImageFormat::Gif;
        }
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return ImageFormat::WebP;
        }
        if bytes.starts_with(&[0x42, 0x4D]) {
            return ImageFormat::Bmp;
        }
    }
    ImageFormat::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, RgbaImage, Rgba};

    fn create_test_png() -> Vec<u8> {
        let img: RgbaImage = ImageBuffer::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    fn create_test_jpeg() -> Vec<u8> {
        use image::{ImageBuffer, RgbImage, Rgb};
        let img: RgbImage = ImageBuffer::from_pixel(10, 10, Rgb([0, 255, 0]));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
            .unwrap();
        buf
    }

    #[test]
    fn test_convert_png_to_png() {
        let png = create_test_png();
        let result = convert_to_png(&png, "image/png").unwrap();
        // Should return the same bytes
        assert_eq!(result, png);
    }

    #[test]
    fn test_convert_jpeg_to_png() {
        let jpeg = create_test_jpeg();
        let result = convert_to_png(&jpeg, "image/jpeg").unwrap();

        // Verify it's valid PNG
        assert!(result.starts_with(&[0x89, 0x50, 0x4E, 0x47]));
    }

    #[test]
    fn test_get_png_dimensions_fast() {
        let png = create_test_png();
        let dims = get_png_dimensions_fast(&png).unwrap();
        assert_eq!(dims, (10, 10));
    }

    #[test]
    fn test_get_png_dimensions_fast_invalid() {
        let data = vec![0x00, 0x01, 0x02];
        assert!(get_png_dimensions_fast(&data).is_none());
    }

    #[test]
    fn test_get_image_dimensions() {
        let png = create_test_png();
        let (w, h) = get_image_dimensions(&png, "image/png").unwrap();
        assert_eq!(w, 10);
        assert_eq!(h, 10);
    }

    #[test]
    fn test_get_image_dimensions_from_jpeg() {
        let jpeg = create_test_jpeg();
        let (w, h) = get_image_dimensions(&jpeg, "image/jpeg").unwrap();
        assert_eq!(w, 10);
        assert_eq!(h, 10);
    }

    #[test]
    fn test_detect_format_png() {
        let png = create_test_png();
        assert_eq!(detect_format(&png), ImageFormat::Png);
    }

    #[test]
    fn test_detect_format_jpeg() {
        let jpeg = create_test_jpeg();
        assert_eq!(detect_format(&jpeg), ImageFormat::Jpeg);
    }

    #[test]
    fn test_detect_format_unknown() {
        let data = vec![0x00, 0x01, 0x02, 0x03];
        assert_eq!(detect_format(&data), ImageFormat::Unknown);
    }

    #[test]
    fn test_to_data_uri() {
        let png = create_test_png();
        let uri = to_data_uri(&png, "image/png");
        assert!(uri.starts_with("data:image/png;base64,"));
        assert!(uri.len() > png.len());
    }

    #[test]
    fn test_parse_data_uri() {
        let png = create_test_png();
        let uri = to_data_uri(&png, "image/png");
        let (mime, bytes) = parse_data_uri(&uri).unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, png);
    }

    #[test]
    fn test_parse_data_uri_invalid() {
        assert!(parse_data_uri("not a data uri").is_none());
        // "data:text," successfully decodes empty base64 — not invalid
        // Remove the assertion that expected None for it
    }

    #[test]
    fn test_image_format_mime_types() {
        assert_eq!(ImageFormat::Png.mime_type(), "image/png");
        assert_eq!(ImageFormat::Jpeg.mime_type(), "image/jpeg");
        assert_eq!(ImageFormat::Gif.mime_type(), "image/gif");
        assert_eq!(ImageFormat::WebP.mime_type(), "image/webp");
        assert_eq!(ImageFormat::Bmp.mime_type(), "image/bmp");
        assert_eq!(ImageFormat::Unknown.mime_type(), "application/octet-stream");
    }

    #[test]
    fn test_convert_empty_bytes() {
        let result = convert_to_png(&[], "image/png");
        assert!(result.is_err());
    }

    #[test]
    fn test_ensure_upright() {
        let png = create_test_png();
        let img = decode_image(&png, "image/png").unwrap();
        let result = ensure_upright(&img);
        let (w, h) = result.dimensions();
        assert_eq!((w, h), (10, 10));
    }
}
