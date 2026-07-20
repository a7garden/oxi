//! Shim of `xai_grok_shared::clipboard::ImageData` and `mime_to_extension`.

/// Minimal image data type used by the render clipboard module.
#[derive(Debug, Clone)]
pub struct ImageData {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

/// Map a MIME type to a file extension.
pub fn mime_to_extension(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/tiff" => Some("tiff"),
        _ => None,
    }
}
