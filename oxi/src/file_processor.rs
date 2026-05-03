//! File argument processor
//!
//! Processes @file arguments from CLI, converting them to ContentBlocks
//! for inclusion in messages. Handles both text and image files.

use crate::frontmatter::{parse_frontmatter, Frontmatter};
use crate::image_convert::{convert_to_png, detect_format, get_image_dimensions};
use crate::image_resize::{resize_image, ResizeOptions};
use anyhow::Result;
use base64::Engine as _;
use oxi_ai::{ContentBlock, ImageContent, TextContent};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Default maximum image size for API (in bytes, base64 encoded)
const DEFAULT_MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024; // 4MB

/// Default maximum image dimensions
const DEFAULT_MAX_IMAGE_DIMENSION: u32 = 2000;

/// Options for processing file arguments
#[derive(Debug, Clone)]
pub struct FileProcessorOptions {
    /// Maximum image dimensions
    pub max_image_width: u32,
    pub max_image_height: u32,
    /// Maximum image size after base64 encoding
    pub max_image_bytes: usize,
    /// JPEG quality for resized images
    pub jpeg_quality: u8,
    /// Whether to extract frontmatter
    pub extract_frontmatter: bool,
}

impl Default for FileProcessorOptions {
    fn default() -> Self {
        Self {
            max_image_width: DEFAULT_MAX_IMAGE_DIMENSION,
            max_image_height: DEFAULT_MAX_IMAGE_DIMENSION,
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
            jpeg_quality: 80,
            extract_frontmatter: true,
        }
    }
}

impl FileProcessorOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_image_bytes(mut self, bytes: usize) -> Self {
        self.max_image_bytes = bytes;
        self
    }

    pub fn extract_frontmatter(mut self, extract: bool) -> Self {
        self.extract_frontmatter = extract;
        self
    }
}

/// A processed content block with optional metadata
#[derive(Debug, Clone)]
pub struct ProcessedContent {
    /// The content block(s)
    pub blocks: Vec<ContentBlock>,
    /// Optional frontmatter if extracted
    pub frontmatter: Option<Frontmatter>,
    /// File path (for error messages)
    pub path: PathBuf,
}

/// Detect MIME type from file extension or magic bytes
fn detect_mime_type(path: &PathBuf, data: &[u8]) -> String {
    // Check magic bytes first
    let from_bytes = detect_format(data);
    if from_bytes != crate::image_convert::ImageFormat::Unknown {
        return from_bytes.mime_type().to_string();
    }

    // Fall back to extension
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "txt" | "md" | "rs" | "js" | "ts" | "py" => "text/plain",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Check if a file is likely an image based on extension or magic bytes
fn is_image_file(path: &PathBuf, data: &[u8]) -> bool {
    // Check magic bytes
    let format = detect_format(data);
    if format != crate::image_convert::ImageFormat::Unknown {
        return true;
    }

    // Check extension
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif"
    )
}

/// Process a single file into content blocks
fn process_file(path: &PathBuf, opts: &FileProcessorOptions) -> Result<ProcessedContent> {
    let data = fs::read(path)?;
    let mime = detect_mime_type(path, &data);

    if is_image_file(path, &data) {
        process_image_file(path, &data, &mime, opts)
    } else {
        process_text_file(path, &data, opts)
    }
}

/// Process an image file
fn process_image_file(
    path: &PathBuf,
    data: &[u8],
    mime: &str,
    opts: &FileProcessorOptions,
) -> Result<ProcessedContent> {
    // Get original dimensions
    let (width, height) = get_image_dimensions(data, mime).unwrap_or((0, 0));

    // Check if resize is needed
    let needs_resize = width > opts.max_image_width
        || height > opts.max_image_height
        || data.len() * 4 / 3 > opts.max_image_bytes; // Approximate base64 size

    let (final_data, final_mime) = if needs_resize {
        let resize_opts = ResizeOptions::new(opts.max_image_width, opts.max_image_height)
            .max_bytes(opts.max_image_bytes)
            .jpeg_quality(opts.jpeg_quality);

        let resized = resize_image(data, &resize_opts)?;
        (resized.bytes, resized.mime_type)
    } else {
        (data.to_vec(), mime.to_string())
    };

    // Convert to PNG for terminal display if needed
    let png_data = if final_mime != "image/png" {
        convert_to_png(&final_data, &final_mime)?
    } else {
        final_data.clone()
    };

    // Encode as base64
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&png_data);

    let block = ContentBlock::Image(ImageContent::new(base64_data, final_mime));

    Ok(ProcessedContent {
        blocks: vec![block],
        frontmatter: None,
        path: path.clone(),
    })
}

/// Process a text file
fn process_text_file(
    path: &PathBuf,
    data: &[u8],
    opts: &FileProcessorOptions,
) -> Result<ProcessedContent> {
    // Try to decode as UTF-8
    let content = String::from_utf8_lossy(&data);

    // Parse frontmatter if enabled
    let (frontmatter, body) = if opts.extract_frontmatter {
        parse_frontmatter(&content)
            .map(|(fields, body)| {
                (
                    Some(Frontmatter {
                        fields,
                        body: body.to_string(),
                    }),
                    body,
                )
            })
            .unwrap_or((None, content.as_ref()))
    } else {
        (None, content.as_ref())
    };

    let block = ContentBlock::Text(TextContent::new(body.to_string()));

    Ok(ProcessedContent {
        blocks: vec![block],
        frontmatter,
        path: path.clone(),
    })
}

/// Process file arguments from CLI
///
/// For each @path:
/// - Detect MIME type
/// - If image: read bytes, resize if needed, create ImageContent
/// - If text: read content, optionally with frontmatter
/// - Return vector of content blocks for messages
pub fn process_file_args(paths: &[PathBuf]) -> Result<Vec<ContentBlock>> {
    process_file_args_with_options(paths, &FileProcessorOptions::default())
}

/// Process file arguments with custom options
pub fn process_file_args_with_options(
    paths: &[PathBuf],
    opts: &FileProcessorOptions,
) -> Result<Vec<ContentBlock>> {
    let mut all_blocks = Vec::new();

    for path in paths {
        match process_file(path, opts) {
            Ok(processed) => {
                all_blocks.extend(processed.blocks);
            }
            Err(e) => {
                // Create an error text block instead of failing entirely
                let error_text = format!("[Error reading file: {}: {}]", path.display(), e);
                all_blocks.push(ContentBlock::Text(TextContent::new(error_text)));
            }
        }
    }

    Ok(all_blocks)
}

/// Expand @file references in a message
///
/// Scans the message text for @path patterns and replaces them
/// with references to file content blocks.
pub fn expand_file_references(
    message: &str,
    file_paths: &[PathBuf],
) -> Result<(String, Vec<ContentBlock>)> {
    let mut blocks = Vec::new();
    let mut result = message.to_string();

    // Process all files upfront
    for path in file_paths {
        match process_file(path, &FileProcessorOptions::default()) {
            Ok(processed) => {
                let placeholder = format!("[file:{}]", path.display());
                result = result.replace(&placeholder, &format!("[Attached: {}]", path.display()));
                blocks.extend(processed.blocks);
            }
            Err(e) => {
                let error_text = format!("[Error reading file: {}: {}]", path.display(), e);
                blocks.push(ContentBlock::Text(TextContent::new(error_text)));
            }
        }
    }

    Ok((result, blocks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_file_processor_options_default() {
        let opts = FileProcessorOptions::default();
        assert_eq!(opts.max_image_width, DEFAULT_MAX_IMAGE_DIMENSION);
        assert_eq!(opts.max_image_height, DEFAULT_MAX_IMAGE_DIMENSION);
    }

    #[test]
    fn test_file_processor_options_builder() {
        let opts = FileProcessorOptions::new()
            .max_image_bytes(1024 * 1024)
            .extract_frontmatter(false);
        assert_eq!(opts.max_image_bytes, 1024 * 1024);
        assert!(!opts.extract_frontmatter);
    }

    #[test]
    fn test_detect_mime_type_extension() {
        let path = PathBuf::from("test.txt");
        assert_eq!(detect_mime_type(&path, &[]), "text/plain");

        let path = PathBuf::from("test.png");
        assert_eq!(detect_mime_type(&path, &[]), "image/png");

        let path = PathBuf::from("test.jpg");
        assert_eq!(detect_mime_type(&path, &[]), "image/jpeg");
    }

    #[test]
    fn test_detect_mime_type_unknown() {
        let path = PathBuf::from("test.unknown");
        assert_eq!(detect_mime_type(&path, &[]), "application/octet-stream");
    }

    #[test]
    fn test_is_image_file_by_extension() {
        assert!(is_image_file(&PathBuf::from("test.png"), &[]));
        assert!(is_image_file(&PathBuf::from("test.jpg"), &[]));
        assert!(is_image_file(&PathBuf::from("test.gif"), &[]));
        assert!(is_image_file(&PathBuf::from("test.webp"), &[]));
    }

    #[test]
    fn test_is_image_file_not() {
        assert!(!is_image_file(&PathBuf::from("test.txt"), &[]));
        assert!(!is_image_file(&PathBuf::from("test.rs"), &[]));
        assert!(!is_image_file(&PathBuf::from("test.md"), &[]));
    }

    #[test]
    fn test_process_text_file() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"Hello, World!").unwrap();
        let path = file.path().to_path_buf();

        let result = process_file(&path, &FileProcessorOptions::default()).unwrap();
        assert_eq!(result.blocks.len(), 1);
        assert!(matches!(result.blocks[0], ContentBlock::Text(_)));

        if let ContentBlock::Text(t) = &result.blocks[0] {
            assert_eq!(t.text, "Hello, World!");
        }
    }

    #[test]
    fn test_process_text_file_with_frontmatter() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"---\nname: Test\n---\nContent here")
            .unwrap();
        let path = file.path().to_path_buf();

        let result = process_file(&path, &FileProcessorOptions::default()).unwrap();
        assert!(result.frontmatter.is_some());

        let fm = result.frontmatter.unwrap();
        assert_eq!(fm.fields.get("name").unwrap().as_str().unwrap(), "Test");
    }

    #[test]
    fn test_process_text_file_no_frontmatter() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"Simple content").unwrap();
        let path = file.path().to_path_buf();

        let opts = FileProcessorOptions::new().extract_frontmatter(false);
        let result = process_file(&path, &opts).unwrap();
        assert!(result.frontmatter.is_none());

        if let ContentBlock::Text(t) = &result.blocks[0] {
            assert_eq!(t.text, "Simple content");
        }
    }

    #[test]
    fn test_process_nonexistent_file() {
        let path = PathBuf::from("/nonexistent/file.txt");
        let result = process_file(&path, &FileProcessorOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_process_multiple_files() {
        let mut file1 = NamedTempFile::new().unwrap();
        file1.write_all(b"File 1").unwrap();

        let mut file2 = NamedTempFile::new().unwrap();
        file2.write_all(b"File 2").unwrap();

        let paths = vec![file1.path().to_path_buf(), file2.path().to_path_buf()];
        let blocks = process_file_args(&paths).unwrap();

        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn test_process_file_args_empty() {
        let blocks = process_file_args(&[]).unwrap();
        assert!(blocks.is_empty());
    }
}
