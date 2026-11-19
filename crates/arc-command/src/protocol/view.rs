//! View rendering over the protocol (CORE-CMD-026): the method `view.render` and the Model
//! Context Protocol tool `render_view`.
//!
//! arc-command cannot depend on the renderer, so a host attaches a [`ViewRenderer`] with
//! [`Server::with_view_renderer`]. This module enforces what comes before the renderer runs (the
//! read permission, the shape of the parameters, the size bounds and, for the file variant, the
//! file permission and the write roots) and encodes the inline image. Without a renderer every
//! request is answered `cmd.render.unavailable`.

use std::io::Write;
use std::path::Path;

use arc_base::Error;
use arc_doc::Snapshot;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::events::require_read;
use super::{Answer, Failure, Server, decode, invalid_params};
use crate::context::Session;
use crate::error::codes;
use crate::spec::Permission;

/// The image size when a request names none, in pixels: width and height.
pub const DEFAULT_SIZE: (u32, u32) = (800, 600);
/// The largest width or height in pixels.
pub const MAX_SIDE: u32 = 4096;
/// The largest number of pixels of an image.
pub const MAX_PIXELS: u64 = 8 << 20;
/// The largest PNG returned inline, in bytes; its base64 text is a third larger again.
pub const MAX_INLINE_BYTES: usize = 12 << 20;
/// The most section planes of a request.
pub const MAX_SECTIONS: usize = 16;
/// The view name of a request that names none.
pub const DEFAULT_VIEW: &str = "default";
/// The display mode of a request that names none.
pub const DEFAULT_DISPLAY_MODE: &str = "shaded";

/// The longest view name and display mode, in bytes.
const MAX_NAME_BYTES: usize = 64;

/// Renders one view of a document snapshot to PNG bytes. The host implements it; it runs on the
/// thread of the session that asked, and it may be called from several sessions at once.
pub trait ViewRenderer: Send + Sync {
    /// Renders `request` to a PNG image of `request.width` by `request.height` pixels.
    ///
    /// # Errors
    ///
    /// `cmd.render.unsupported` for a view, display mode, camera or section state the renderer
    /// does not draw (the detail `feature` names it); the renderer's own errors otherwise.
    fn render(&self, request: &ViewRequest<'_>) -> Result<Vec<u8>, Error>;
}

/// What a `view.render` request asks for, after arc-command checked its shape and bounds.
#[derive(Debug, Clone, Copy)]
pub struct ViewRequest<'a> {
    /// The document, as a snapshot of its current revision.
    pub snapshot: &'a Snapshot,
    /// The view name; [`DEFAULT_VIEW`] when the request names none.
    pub view: &'a str,
    /// The width in pixels, from 1 to [`MAX_SIDE`].
    pub width: u32,
    /// The height in pixels, from 1 to [`MAX_SIDE`].
    pub height: u32,
    /// The display mode; [`DEFAULT_DISPLAY_MODE`] when the request names none.
    pub display_mode: &'a str,
    /// The camera: a JSON object, when the request names one.
    pub camera: Option<&'a Value>,
    /// The section planes: JSON objects, at most [`MAX_SECTIONS`].
    pub sections: &'a [Value],
}

#[derive(Deserialize)]
struct RenderParams {
    document: String,
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default, rename = "displayMode")]
    display_mode: Option<String>,
    #[serde(default)]
    camera: Option<Value>,
    #[serde(default)]
    sections: Option<Value>,
    #[serde(default)]
    path: Option<String>,
}

fn size_exceeded(limit: &'static str) -> Failure {
    Error::new(codes::RENDER_SIZE_EXCEEDED)
        .with_detail("limit", limit)
        .into()
}

fn name_ok(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_NAME_BYTES && !text.chars().any(char::is_control)
}

/// The standard base64 alphabet with padding (RFC 4648).
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            let sextet = (n >> (18 - 6 * i)) & 63;
            if i <= chunk.len() {
                let digit = ALPHABET.get(sextet as usize).copied().unwrap_or(b'=');
                out.push(char::from(digit));
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl Server<'_> {
    /// `view.render`: checks the request, renders it and answers the image inline (`data`,
    /// base64) or written to `path` in a write root.
    pub(super) fn view_render(&self, session: &Session, params: Map<String, Value>) -> Answer {
        const METHOD: &str = "view.render";
        require_read(&session.permissions, METHOD)?;
        let renderer = self
            .renderer
            .ok_or_else(|| Error::new(codes::RENDER_UNAVAILABLE))?;
        let p: RenderParams = decode(METHOD, params)?;
        let (width, height) = (
            p.width.unwrap_or(DEFAULT_SIZE.0),
            p.height.unwrap_or(DEFAULT_SIZE.1),
        );
        if width == 0 {
            return Err(invalid_params(METHOD, "width"));
        }
        if height == 0 {
            return Err(invalid_params(METHOD, "height"));
        }
        if width > MAX_SIDE || height > MAX_SIDE {
            return Err(size_exceeded("side"));
        }
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(size_exceeded("pixels"));
        }
        let view = p.view.unwrap_or_else(|| DEFAULT_VIEW.to_owned());
        if !name_ok(&view) {
            return Err(invalid_params(METHOD, "view"));
        }
        let display_mode = p
            .display_mode
            .unwrap_or_else(|| DEFAULT_DISPLAY_MODE.to_owned());
        if !name_ok(&display_mode) {
            return Err(invalid_params(METHOD, "display_mode"));
        }
        let camera = match &p.camera {
            None => None,
            Some(camera @ Value::Object(_)) => Some(camera),
            Some(_) => return Err(invalid_params(METHOD, "camera")),
        };
        let sections: Vec<Value> = match p.sections {
            None => Vec::new(),
            Some(Value::Array(list))
                if list.len() <= MAX_SECTIONS && list.iter().all(Value::is_object) =>
            {
                list
            }
            Some(_) => return Err(invalid_params(METHOD, "sections")),
        };
        if p.path.is_some() {
            if !session.permissions.contains(&Permission::FileWrite) {
                return Err(Error::new(codes::PERMISSION_DENIED)
                    .with_arg("command", METHOD)
                    .with_detail("permission", "file_write")
                    .into());
            }
            // Refused before the renderer runs when no write root exists at all; a path outside
            // the roots is refused by the write itself, after the render.
            if session.roots.write_roots().is_empty() {
                return Err(Error::new(codes::PATH_OUTSIDE_ROOTS).into());
            }
        }
        let document = self.document(&p.document)?;
        let snapshot = document.snapshot();
        // shortcut: the render blocks the session's reader until it ends, upgrade when renders need cancelling.
        let png = renderer.render(&ViewRequest {
            snapshot: &snapshot,
            view: &view,
            width,
            height,
            display_mode: &display_mode,
            camera,
            sections: &sections,
        })?;
        let bytes = png.len();
        match p.path {
            Some(path) => {
                let mut file = session.roots.create_write(Path::new(&path))?;
                file.write_all(&png)
                    .and_then(|()| file.flush())
                    .map_err(|_| Error::new(codes::PATH_IO).with_detail("reason", "write"))?;
                Ok(
                    json!({"path": path, "mimeType": "image/png", "width": width,
                    "height": height, "bytes": bytes}),
                )
            }
            None => {
                if bytes > MAX_INLINE_BYTES {
                    return Err(Error::new(codes::RENDER_TOO_LARGE)
                        .with_detail("limit", "inline_bytes")
                        .into());
                }
                let data = base64(&png);
                // The envelope around the image takes a little of the message limit.
                if data.len() + 1024 > self.config.max_line() {
                    return Err(Error::new(codes::RENDER_TOO_LARGE)
                        .with_detail("limit", "message_bytes")
                        .into());
                }
                Ok(
                    json!({"mimeType": "image/png", "width": width, "height": height,
                    "bytes": bytes, "data": data}),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
