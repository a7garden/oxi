//! EXIF orientation detection and correction for images
//!
//! Reads EXIF orientation tags from JPEG/TIFF images and provides
//! the correct transformation to apply.

/// EXIF orientation values (1-8) per the EXIF specification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExifOrientation {
    /// Normal - no transformation needed
    Normal = 1,
    /// Flip horizontal
    FlipHorizontal = 2,
    /// Rotate 180°
    Rotate180 = 3,
    /// Flip vertical
    FlipVertical = 4,
    /// Rotate 90° CW + flip horizontal
    Rotate90FlipH = 5,
    /// Rotate 90° CW
    Rotate90 = 6,
    /// Rotate 90° CW + flip vertical
    Rotate90FlipV = 7,
    /// Rotate 270° CW
    Rotate270 = 8,
}

impl ExifOrientation {
    /// Parse orientation from EXIF tag value
    pub fn from_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Normal),
            2 => Some(Self::FlipHorizontal),
            3 => Some(Self::Rotate180),
            4 => Some(Self::FlipVertical),
            5 => Some(Self::Rotate90FlipH),
            6 => Some(Self::Rotate90),
            7 => Some(Self::Rotate90FlipV),
            8 => Some(Self::Rotate270),
            _ => None,
        }
    }

    /// Returns the rotation in degrees (0, 90, 180, 270)
    pub fn rotation_degrees(&self) -> u16 {
        match self {
            Self::Normal | Self::FlipHorizontal => 0,
            Self::Rotate180 | Self::FlipVertical => 180,
            Self::Rotate90 | Self::Rotate90FlipH => 90,
            Self::Rotate270 | Self::Rotate90FlipV => 270,
        }
    }

    /// Returns whether the image needs to be flipped horizontally
    pub fn flip_horizontal(&self) -> bool {
        matches!(self, Self::FlipHorizontal | Self::Rotate90FlipH | Self::FlipVertical | Self::Rotate90FlipV)
    }

    /// Returns the dimensions after applying orientation
    pub fn oriented_dimensions(&self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Normal | Self::FlipHorizontal | Self::Rotate180 | Self::FlipVertical => (width, height),
            Self::Rotate90 | Self::Rotate90FlipH | Self::Rotate270 | Self::Rotate90FlipV => (height, width),
        }
    }
}

/// Extract EXIF orientation from JPEG data
pub fn extract_orientation(data: &[u8]) -> Option<ExifOrientation> {
    // Check JPEG SOI marker
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }

    let mut pos = 2;
    while pos + 4 <= data.len() {
        if data[pos] != 0xFF {
            break;
        }

        let marker = data[pos + 1];
        // APP1 marker (EXIF data)
        if marker == 0xE1 {
            let seg_len = ((data[pos + 2] as usize) << 8) | data[pos + 3] as usize;
            let seg_start = pos + 4;
            let seg_end = seg_start + seg_len - 2;

            if seg_end <= data.len() {
                if let Some(orientation) = parse_exif_orientation(&data[seg_start..seg_end]) {
                    return Some(orientation);
                }
            }
        }

        // Skip to next marker
        if marker == 0xDA {
            // SOS marker - image data follows, no more EXIF
            break;
        }

        let seg_len = ((data[pos + 2] as usize) << 8) | data[pos + 3] as usize;
        pos += 2 + seg_len;
    }

    None
}

/// Parse EXIF orientation from APP1 segment
fn parse_exif_orientation(data: &[u8]) -> Option<ExifOrientation> {
    // Check "Exif\0\0" header
    if data.len() < 14 || &data[0..6] != b"Exif\x00\x00" {
        return None;
    }

    // Check TIFF byte order
    let tiff_data = &data[6..];
    if tiff_data.len() < 8 {
        return None;
    }

    let big_endian = &tiff_data[0..2] == b"MM";
    let little_endian = &tiff_data[0..2] == b"II";

    if !big_endian && !little_endian {
        return None;
    }

    let read_u16 = |d: &[u8], off: usize| -> u16 {
        if big_endian {
            ((d[off] as u16) << 8) | d[off + 1] as u16
        } else {
            d[off] as u16 | ((d[off + 1] as u16) << 8)
        }
    };

    let read_u32 = |d: &[u8], off: usize| -> u32 {
        if big_endian {
            ((d[off] as u32) << 24) | ((d[off + 1] as u32) << 16) | ((d[off + 2] as u32) << 8) | d[off + 3] as u32
        } else {
            d[off] as u32 | ((d[off + 1] as u32) << 8) | ((d[off + 2] as u32) << 16) | ((d[off + 3] as u32) << 24)
        }
    };

    // TIFF header: byte order (2) + magic 42 (2) + IFD0 offset (4)
    let magic = read_u16(tiff_data, 2);
    if magic != 42 {
        return None;
    }

    let ifd0_offset = read_u32(tiff_data, 4) as usize;
    if ifd0_offset + 2 > tiff_data.len() {
        return None;
    }

    let num_entries = read_u16(tiff_data, ifd0_offset) as usize;
    let entries_start = ifd0_offset + 2;

    for i in 0..num_entries {
        let entry_off = entries_start + i * 12;
        if entry_off + 12 > tiff_data.len() {
            break;
        }

        let tag = read_u16(tiff_data, entry_off);
        // Orientation tag is 0x0112
        if tag == 0x0112 {
            let _type = read_u16(tiff_data, entry_off + 2);
            let _count = read_u32(tiff_data, entry_off + 4);
            let value = tiff_data[entry_off + 8];
            return ExifOrientation::from_value(value);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orientation_values() {
        assert_eq!(ExifOrientation::from_value(1), Some(ExifOrientation::Normal));
        assert_eq!(ExifOrientation::from_value(6), Some(ExifOrientation::Rotate90));
        assert_eq!(ExifOrientation::from_value(8), Some(ExifOrientation::Rotate270));
        assert_eq!(ExifOrientation::from_value(0), None);
        assert_eq!(ExifOrientation::from_value(9), None);
    }

    #[test]
    fn test_rotation_degrees() {
        assert_eq!(ExifOrientation::Normal.rotation_degrees(), 0);
        assert_eq!(ExifOrientation::Rotate90.rotation_degrees(), 90);
        assert_eq!(ExifOrientation::Rotate180.rotation_degrees(), 180);
        assert_eq!(ExifOrientation::Rotate270.rotation_degrees(), 270);
    }

    #[test]
    fn test_oriented_dimensions() {
        assert_eq!(ExifOrientation::Normal.oriented_dimensions(100, 200), (100, 200));
        assert_eq!(ExifOrientation::Rotate90.oriented_dimensions(100, 200), (200, 100));
        assert_eq!(ExifOrientation::Rotate270.oriented_dimensions(100, 200), (200, 100));
    }

    #[test]
    fn test_extract_orientation_non_jpeg() {
        assert_eq!(extract_orientation(&[0, 0, 0, 0]), None);
        assert_eq!(extract_orientation(&[]), None);
    }
}
