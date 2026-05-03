//! Image resize for API limits
//!
//! Resizes images to fit within API size limits (dimensions and encoded bytes).
//! Uses the `image` crate for decoding and resizing.

use anyhow::{Context, Result};
use image::io::Reader as ImageReader;
use image::{imageops::FilterType, DynamicImage, GenericImageView, ImageBuffer, Rgba, RgbaImage};
use std::io::Cursor;

/// Options for image resizing
#[derive(Debug, Clone)]
pub struct ResizeOptions {
    /// Maximum width in pixels
    pub max_width: u32,
    /// Maximum height in pixels
    pub max_height: u32,
    /// Maximum bytes after base64 encoding
    pub max_bytes: usize,
    /// JPEG quality (1-100), default 80
    pub jpeg_quality: u8,
}

impl Default for ResizeOptions {
    fn default() -> Self {
        Self {
            max_width: 2000,
            max_height: 2000,
            max_bytes: 4 * 1024 * 1024, // 4MB (below 5MB API limit)
            jpeg_quality: 80,
        }
    }
}

impl ResizeOptions {
    /// Create new options with max dimensions
    pub fn new(max_width: u32, max_height: u32) -> Self {
        Self {
            max_width,
            max_height,
            max_bytes: 4 * 1024 * 1024,
            jpeg_quality: 80,
        }
    }

    /// Set maximum encoded bytes
    pub fn max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// Set JPEG quality
    pub fn jpeg_quality(mut self, quality: u8) -> Self {
        self.jpeg_quality = quality.clamp(1, 100);
        self
    }
}

/// Result of resize operation
#[derive(Debug, Clone)]
pub struct ResizedImage {
    /// Resized image bytes
    pub bytes: Vec<u8>,
    /// MIME type of the output
    pub mime_type: String,
    /// Original width
    pub original_width: u32,
    /// Original height
    pub original_height: u32,
    /// Resulting width
    pub width: u32,
    /// Resulting height
    pub height: u32,
    /// Whether the image was resized
    pub was_resized: bool,
}

/// Decode image from bytes
fn decode_image(bytes: &[u8]) -> Result<DynamicImage> {
    image::load_from_memory(bytes).context("Failed to decode image")
}

/// Get image dimensions without full decode
pub fn get_image_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("Failed to guess image format")?;
    let (width, height) = reader.into_dimensions().context("Failed to get image dimensions")?;
    Ok((width, height))
}

/// Encode image as PNG
fn encode_png(img: &DynamicImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .context("Failed to encode PNG")?;
    Ok(buf)
}

/// Encode image as JPEG
fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    img.write_with_encoder(encoder)
        .context("Failed to encode JPEG")?;
    Ok(buf)
}

/// Calculate target dimensions while maintaining aspect ratio
fn calculate_target_dimensions(
    width: u32,
    height: u32,
    max_width: u32,
    max_height: u32,
) -> (u32, u32) {
    let mut target_width = width;
    let mut target_height = height;

    if target_width > max_width {
        let ratio = max_width as f64 / target_width as f64;
        target_width = max_width;
        target_height = (target_height as f64 * ratio) as u32;
    }

    if target_height > max_height {
        let ratio = max_height as f64 / target_height as f64;
        target_height = max_height;
        target_width = (target_width as f64 * ratio) as u32;
    }

    // Ensure at least 1x1
    (target_width.max(1), target_height.max(1))
}

/// Try to resize and encode at given dimensions
fn try_encode(
    img: &DynamicImage,
    width: u32,
    height: u32,
    jpeg_quality: u8,
) -> Result<(Vec<u8>, String, usize)> {
    // Resize the image
    let resized = img.resize_exact(width, height, FilterType::Lanczos3);

    // Try PNG first
    let png_bytes = encode_png(&resized)?;
    let png_base64_size = png_bytes.len() * 4 / 3; // Approximate base64 size
    let png_encoded_size = png_base64_size; // UTF-8 bytes after base64 encoding

    // Try JPEG
    let jpeg_bytes = encode_jpeg(&resized, jpeg_quality)?;
    let jpeg_base64_size = jpeg_bytes.len() * 4 / 3;
    let jpeg_encoded_size = jpeg_base64_size;

    // Pick the smaller one
    if png_encoded_size <= jpeg_encoded_size {
        Ok((png_bytes, "image/png".to_string(), png_encoded_size))
    } else {
        Ok((jpeg_bytes, "image/jpeg".to_string(), jpeg_encoded_size))
    }
}

/// Resize image to fit within size limits
///
/// Strategy:
/// 1. If already within limits, return as-is
/// 2. Resize to fit max dimensions
/// 3. Try both PNG and JPEG, pick smaller
/// 4. If still too large, progressively reduce dimensions
/// 5. Return None if cannot fit within max_bytes
pub fn resize_image(bytes: &[u8], opts: &ResizeOptions) -> Result<ResizedImage> {
    let img = decode_image(bytes)?;
    let (original_width, original_height) = img.dimensions();

    // Check if already within limits
    let current_base64_size = bytes.len() * 4 / 3;
    if original_width <= opts.max_width
        && original_height <= opts.max_height
        && current_base64_size < opts.max_bytes
    {
        // Determine format
        let format = if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
            "image/png"
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            "image/jpeg"
        } else {
            "image/png"
        };

        return Ok(ResizedImage {
            bytes: bytes.to_vec(),
            mime_type: format.to_string(),
            original_width,
            original_height,
            width: original_width,
            height: original_height,
            was_resized: false,
        });
    }

    // Calculate target dimensions
    let (target_width, target_height) = calculate_target_dimensions(
        original_width,
        original_height,
        opts.max_width,
        opts.max_height,
    );

    let mut current_width = target_width;
    let mut current_height = target_height;

    // Quality steps to try for JPEG
    let quality_steps = [opts.jpeg_quality, 85, 70, 55, 40];

    // Progressive size reduction loop
    loop {
        let candidates = try_encode(&img, current_width, current_height, opts.jpeg_quality)?;

        if candidates.2 < opts.max_bytes {
            return Ok(ResizedImage {
                bytes: candidates.0,
                mime_type: candidates.1,
                original_width,
                original_height,
                width: current_width,
                height: current_height,
                was_resized: true,
            });
        }

        // If we're already at 1x1, try different JPEG qualities
        if current_width == 1 && current_height == 1 {
            // Try with lower qualities
            for quality in &quality_steps[1..] {
                if let Ok((jpeg_bytes, _, encoded_size)) = try_encode(&img, 1, 1, *quality) {
                    if encoded_size < opts.max_bytes {
                        return Ok(ResizedImage {
                            bytes: jpeg_bytes,
                            mime_type: "image/jpeg".to_string(),
                            original_width,
                            original_height,
                            width: 1,
                            height: 1,
                            was_resized: true,
                        });
                    }
                }
            }
            break;
        }

        // Reduce dimensions by 75%
        let next_width = if current_width == 1 {
            1
        } else {
            (current_width as f64 * 0.75) as u32
        }
        .max(1);
        let next_height = if current_height == 1 {
            1
        } else {
            (current_height as f64 * 0.75) as u32
        }
        .max(1);

        if next_width == current_width && next_height == current_height {
            break;
        }

        current_width = next_width;
        current_height = next_height;
    }

    anyhow::bail!(
        "Cannot resize image to fit within {} bytes (current: {}x{})",
        opts.max_bytes,
        original_width,
        original_height
    )
}

/// Format a dimension note for the model
pub fn format_dimension_note(result: &ResizedImage) -> Option<String> {
    if !result.was_resized {
        return None;
    }

    let scale = result.original_width as f64 / result.width as f64;
    Some(format!(
        "[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {:.2} to map to original image.]",
        result.original_width, result.original_height, result.width, result.height, scale
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resize_options_default() {
        let opts = ResizeOptions::default();
        assert_eq!(opts.max_width, 2000);
        assert_eq!(opts.max_height, 2000);
        assert_eq!(opts.jpeg_quality, 80);
    }

    #[test]
    fn test_resize_options_builder() {
        let opts = ResizeOptions::new(1000, 1000)
            .max_bytes(1024 * 1024)
            .jpeg_quality(90);
        assert_eq!(opts.max_width, 1000);
        assert_eq!(opts.max_height, 1000);
        assert_eq!(opts.max_bytes, 1024 * 1024);
        assert_eq!(opts.jpeg_quality, 90);
    }

    #[test]
    fn test_resize_options_jpeg_quality_clamp() {
        let opts = ResizeOptions::default().jpeg_quality(150);
        assert_eq!(opts.jpeg_quality, 100);

        let opts = ResizeOptions::default().jpeg_quality(0);
        assert_eq!(opts.jpeg_quality, 1);
    }

    #[test]
    fn test_calculate_target_dimensions_width() {
        // Image wider than max
        let (w, h) = calculate_target_dimensions(4000, 1000, 2000, 2000);
        assert_eq!(w, 2000);
        assert_eq!(h, 500);
    }

    #[test]
    fn test_calculate_target_dimensions_height() {
        // Image taller than max
        let (w, h) = calculate_target_dimensions(1000, 4000, 2000, 2000);
        assert_eq!(w, 500);
        assert_eq!(h, 2000);
    }

    #[test]
    fn test_calculate_target_dimensions_both() {
        // Image larger than both limits
        let (w, h) = calculate_target_dimensions(4000, 4000, 2000, 1000);
        assert_eq!(w, 1000);
        assert_eq!(h, 1000);
    }

    #[test]
    fn test_calculate_target_dimensions_already_small() {
        // Image already within limits
        let (w, h) = calculate_target_dimensions(500, 500, 2000, 2000);
        assert_eq!(w, 500);
        assert_eq!(h, 500);
    }

    #[test]
    fn test_calculate_target_dimensions_minimum() {
        // Image very small
        let (w, h) = calculate_target_dimensions(10, 10, 2000, 2000);
        assert_eq!(w, 10);
        assert_eq!(h, 10);
    }

    #[test]
    fn test_get_image_dimensions() {
        // Create a small 10x10 PNG
        let img = RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();

        let (w, h) = get_image_dimensions(&buf).unwrap();
        assert_eq!(w, 10);
        assert_eq!(h, 10);
    }

    #[test]
    fn test_format_dimension_note_not_resized() {
        let result = ResizedImage {
            bytes: vec![],
            mime_type: "image/png".to_string(),
            original_width: 100,
            original_height: 100,
            width: 100,
            height: 100,
            was_resized: false,
        };
        assert!(format_dimension_note(&result).is_none());
    }

    #[test]
    fn test_format_dimension_note_resized() {
        let result = ResizedImage {
            bytes: vec![],
            mime_type: "image/png".to_string(),
            original_width: 4000,
            original_height: 4000,
            width: 1000,
            height: 1000,
            was_resized: true,
        };
        let note = format_dimension_note(&result).unwrap();
        assert!(note.contains("4000x4000"));
        assert!(note.contains("1000x1000"));
        assert!(note.contains("4.00")); // Scale factor
    }

    #[test]
    fn test_resize_small_image() {
        // Create a small 10x10 PNG
        let img = RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        let bytes = buf;

        let opts = ResizeOptions::new(2000, 2000).max_bytes(1024 * 1024);
        let result = resize_image(&bytes, &opts).unwrap();

        assert!(!result.was_resized);
        assert_eq!(result.width, 10);
        assert_eq!(result.height, 10);
    }
}
