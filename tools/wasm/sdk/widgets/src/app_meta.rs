//! App metadata — icon, display name, description, surfaced by launchers.
//!
//! Each app embeds a postcard-encoded [`AppMeta`] into a WASM custom
//! section named `.npk.app_meta`. The kernel's install flow extracts the
//! section, caches the bytes in npkFS as `sys/meta/<name>`, and exposes
//! them via the `npk_app_meta` host fn so launchers (drun, later files
//! etc.) can render a proper row per app without the user having to
//! memorize module names.
//!
//! # Wire shape
//!
//! ```text
//! [ version: u8 = APP_META_WIRE ][ postcard-serialized AppMeta ]
//! ```
//!
//! `APP_META_WIRE` is independent of the widget-tree `WIRE_VERSION`: app
//! metadata evolves at a different cadence than the UI ABI.
//!
//! # Evolution
//!
//! All fieldful enums here are `#[non_exhaustive]` with an explicit
//! "Appended only" comment — postcard serializes variants by
//! declaration position, so any reorder is a silent wire break. New
//! variants land at the bottom; if a breaking change is ever needed,
//! bump [`APP_META_WIRE`].

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::abi::IconId;

/// Wire protocol version byte for `AppMeta` payloads.
pub const APP_META_WIRE: u8 = 0x01;

/// Reference to the icon a launcher should render for this app. v1 only
/// points into the kernel-bundled Phosphor atlas via [`IconId`]; v2 is
/// intended to carry app-supplied raster/SVG data so third-party apps
/// can ship their own glyphs. Always append — never reorder.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IconRef {
    Builtin(IconId),
    // Appended only. Reserved for v2+:
    //   EmbeddedAlpha { width: u16, height: u16, data: Vec<u8> },
    //   EmbeddedSvg(String),
}

/// Metadata an app publishes about itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppMeta {
    /// Human-readable name, e.g. `"Drun"`. Shown as the row title in
    /// launchers. Prefer title case over lowercase module names.
    pub display_name: String,
    /// One-line description, e.g. `"App launcher"`. Shown as a subtitle.
    /// Keep ≤ 40 chars to avoid wrapping in the common launcher layout.
    pub description:  String,
    /// Icon the launcher should draw next to the row.
    pub icon:         IconRef,
}

/// Errors from encoding / decoding an [`AppMeta`] wire payload.
#[derive(Debug, PartialEq, Eq)]
pub enum AppMetaError {
    Empty,
    VersionMismatch { got: u8, want: u8 },
    Postcard,
    Serialize,
}

/// Encode an [`AppMeta`] into a wire buffer (version byte + postcard).
/// Suitable for embedding via `#[link_section = ".npk.app_meta"]`.
pub fn encode(meta: &AppMeta) -> Result<Vec<u8>, AppMetaError> {
    let body = postcard::to_allocvec(meta).map_err(|_| AppMetaError::Serialize)?;
    let mut out = Vec::with_capacity(body.len() + 1);
    out.push(APP_META_WIRE);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a wire buffer back into an [`AppMeta`]. Verifies the version
/// byte first.
pub fn decode(bytes: &[u8]) -> Result<AppMeta, AppMetaError> {
    let (&ver, body) = bytes.split_first().ok_or(AppMetaError::Empty)?;
    if ver != APP_META_WIRE {
        return Err(AppMetaError::VersionMismatch { got: ver, want: APP_META_WIRE });
    }
    postcard::from_bytes(body).map_err(|_| AppMetaError::Postcard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn roundtrip_basic() {
        let m = AppMeta {
            display_name: "Drun".to_string(),
            description:  "App launcher".to_string(),
            icon:         IconRef::Builtin(IconId::MagnifyingGlass),
        };
        let bytes = encode(&m).expect("encode");
        assert_eq!(bytes[0], APP_META_WIRE);
        let decoded = decode(&bytes).expect("decode");
        assert_eq!(decoded, m);
    }

    #[test]
    fn rejects_wrong_version() {
        let mut bytes = encode(&AppMeta {
            display_name: "X".to_string(),
            description:  "".to_string(),
            icon:         IconRef::Builtin(IconId::None),
        }).unwrap();
        bytes[0] = 0xFF;
        assert!(matches!(decode(&bytes), Err(AppMetaError::VersionMismatch { .. })));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(decode(&[]), Err(AppMetaError::Empty));
    }
}
