//! AppMeta — launcher-visible metadata embedded per app as a WASM
//! custom section (`.npk.app_meta`), cached by the installer.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::abi::IconId;

pub const APP_META_WIRE: u8 = 0x01;

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IconRef {
    Builtin(IconId),
    // Appended only.
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppMeta {
    pub display_name: String,
    pub description:  String,
    pub icon:         IconRef,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AppMetaError {
    Empty,
    VersionMismatch { got: u8, want: u8 },
    Postcard,
    Serialize,
}

pub fn encode(meta: &AppMeta) -> Result<Vec<u8>, AppMetaError> {
    let body = postcard::to_allocvec(meta).map_err(|_| AppMetaError::Serialize)?;
    let mut out = Vec::with_capacity(body.len() + 1);
    out.push(APP_META_WIRE);
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<AppMeta, AppMetaError> {
    let (&ver, body) = bytes.split_first().ok_or(AppMetaError::Empty)?;
    if ver != APP_META_WIRE {
        return Err(AppMetaError::VersionMismatch { got: ver, want: APP_META_WIRE });
    }
    match postcard::from_bytes(body) {
        Ok(m) => Ok(m),
        // An app built against a NEWER SDK may name an icon this build has
        // never heard of, and serde rejects the whole record for it. Losing
        // the icon is a blemish; losing the app is a bug — tune shipped with
        // IconId 44 and vanished from the launcher and the dock of every
        // system whose drun predated that icon. The name and the description
        // sit in front of the icon on the wire, so they are still readable.
        Err(_) => decode_lenient(body),
    }
}

/// Read only what every wire version puts first: two length-prefixed
/// strings. Anything after them is left to the strict decoder.
fn decode_lenient(body: &[u8]) -> Result<AppMeta, AppMetaError> {
    let (display_name, rest) = take_str(body)?;
    let (description, _) = take_str(rest)?;
    Ok(AppMeta { display_name, description, icon: IconRef::Builtin(IconId::File) })
}

/// postcard string: varint byte length, then UTF-8.
fn take_str(b: &[u8]) -> Result<(String, &[u8]), AppMetaError> {
    let mut len = 0usize;
    let mut shift = 0u32;
    let mut i = 0usize;
    loop {
        let byte = *b.get(i).ok_or(AppMetaError::Postcard)?;
        i += 1;
        len |= ((byte & 0x7F) as usize) << shift;
        if byte & 0x80 == 0 { break; }
        shift += 7;
        if shift > 28 { return Err(AppMetaError::Postcard); }
    }
    let end = i.checked_add(len).ok_or(AppMetaError::Postcard)?;
    let raw = b.get(i..end).ok_or(AppMetaError::Postcard)?;
    let s = core::str::from_utf8(raw).map_err(|_| AppMetaError::Postcard)?;
    Ok((String::from(s), &b[end..]))
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
        let bytes = encode(&m).unwrap();
        assert_eq!(bytes[0], APP_META_WIRE);
        assert_eq!(decode(&bytes).unwrap(), m);
    }

    /// The exact failure that hid `tune`: an icon the reader has never
    /// heard of must cost the icon, not the app.
    #[test]
    fn unknown_icon_keeps_name_and_description() {
        let blob = b"\x01\x04tune\x0cAudio player\x00\xfa\x01";
        let m = decode(blob).expect("unknown icon must not drop the app");
        assert_eq!(m.display_name, "tune");
        assert_eq!(m.description, "Audio player");
        assert_eq!(m.icon, IconRef::Builtin(IconId::File));
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
