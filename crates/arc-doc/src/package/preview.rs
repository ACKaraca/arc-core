//! Thumbnail and preview images (CORE-DOC-051): PNG images the application renders and supplies
//! at save, stored in `arc_preview`.

use arc_base::{Error, Result};

use crate::error::codes;

/// The kind of the thumbnail: PNG, 256 × 256 pixels.
pub const THUMBNAIL: &str = "thumbnail";
/// The kind of the preview: PNG, longest side 1,024 pixels.
pub const PREVIEW: &str = "preview";
/// The prefix of a sheet preview's kind, `sheet:<id>`: PNG, longest side at most 1,024 pixels.
pub const SHEET_PREFIX: &str = "sheet:";
/// The only media type of previews.
pub const PNG: &str = "image/png";
/// The largest encoded preview, in bytes.
pub const MAX_PREVIEW_BYTES: usize = 16 * 1024 * 1024;
/// The longest sheet identifier, in bytes.
const MAX_SHEET_ID_BYTES: usize = 256;
/// The PNG signature followed by the length and type of the `IHDR` chunk, which comes first.
const PNG_START: &[u8; 16] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";

/// A thumbnail or preview image: its kind ([`THUMBNAIL`], [`PREVIEW`] or [`SHEET_PREFIX`] and
/// a sheet identifier), media type ([`PNG`]), pixel size and encoded bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// `thumbnail`, `preview` or `sheet:<id>`.
    pub kind: String,
    /// `image/png`.
    pub media_type: String,
    /// The width in pixels, equal to the image's own.
    pub width: u32,
    /// The height in pixels, equal to the image's own.
    pub height: u32,
    /// The PNG file.
    pub bytes: Vec<u8>,
}

/// Checks a preview: the kind, the media type, the PNG signature and `IHDR` size equal to the
/// stated size, the size the kind prescribes and at most [`MAX_PREVIEW_BYTES`] bytes. The image
/// data itself is not decoded. Fails with `doc.package.preview_invalid` (argument `kind`,
/// detail `reason`).
pub(crate) fn check(preview: &Preview) -> Result<()> {
    let invalid = |reason: &'static str| {
        Error::new(codes::PACKAGE_PREVIEW_INVALID)
            .with_arg("kind", preview.kind.clone())
            .with_detail("reason", reason)
    };
    let (width, height) = (preview.width, preview.height);
    let longest = width.max(height);
    let size_ok = match preview.kind.as_str() {
        THUMBNAIL => width == 256 && height == 256,
        PREVIEW => longest == 1024 && width > 0 && height > 0,
        kind => match kind.strip_prefix(SHEET_PREFIX) {
            Some(id)
                if !id.is_empty()
                    && id.len() <= MAX_SHEET_ID_BYTES
                    && !id.chars().any(char::is_control) =>
            {
                longest <= 1024 && width > 0 && height > 0
            }
            _ => return Err(invalid("kind")),
        },
    };
    if preview.media_type != PNG {
        return Err(invalid("media_type"));
    }
    if preview.bytes.len() > MAX_PREVIEW_BYTES {
        return Err(invalid("bytes"));
    }
    let word = |at: usize| {
        preview
            .bytes
            .get(at..at + 4)
            .and_then(|word| word.try_into().ok())
            .map(u32::from_be_bytes)
    };
    if preview.bytes.get(..16) != Some(PNG_START.as_slice())
        || word(16) != Some(width)
        || word(20) != Some(height)
    {
        return Err(invalid("png"));
    }
    if !size_ok {
        return Err(invalid("size"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = PNG_START.to_vec();
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());
        bytes.extend([1, 0, 0, 0, 0]);
        bytes
    }

    fn preview(kind: &str, width: u32, height: u32) -> Preview {
        Preview {
            kind: kind.into(),
            media_type: PNG.into(),
            width,
            height,
            bytes: png(width, height),
        }
    }

    fn reason(preview: &Preview) -> Option<String> {
        check(preview)
            .err()
            .and_then(|error| error.details().get("reason").map(|r| format!("{r:?}")))
    }

    #[test]
    fn each_kind_has_its_size() {
        assert!(check(&preview(THUMBNAIL, 256, 256)).is_ok());
        assert!(check(&preview(PREVIEW, 1024, 700)).is_ok());
        assert!(check(&preview("sheet:A-101", 300, 200)).is_ok());
        assert!(reason(&preview(THUMBNAIL, 255, 256)).is_some());
        assert!(reason(&preview(PREVIEW, 1000, 700)).is_some());
        assert!(reason(&preview("sheet:A", 1025, 1)).is_some());
        assert!(reason(&preview("sheet:", 10, 10)).is_some());
        assert!(reason(&preview("sheet:\n", 10, 10)).is_some());
        assert!(reason(&preview("other", 10, 10)).is_some());
    }

    #[test]
    fn the_image_must_be_a_png_of_the_stated_size() {
        let mut wrong = preview(THUMBNAIL, 256, 256);
        wrong.media_type = "image/jpeg".into();
        assert!(check(&wrong).is_err());
        let mut lying = preview(THUMBNAIL, 256, 256);
        lying.bytes = png(255, 256);
        assert!(check(&lying).is_err());
        let mut short = preview(THUMBNAIL, 256, 256);
        short.bytes.truncate(18);
        assert!(check(&short).is_err());
        let mut huge = preview(THUMBNAIL, 256, 256);
        huge.bytes.resize(MAX_PREVIEW_BYTES + 1, 0);
        assert!(check(&huge).is_err());
    }
}
