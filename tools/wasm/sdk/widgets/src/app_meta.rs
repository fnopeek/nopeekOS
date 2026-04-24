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
        let bytes = encode(&m).unwrap();
        assert_eq!(bytes[0], APP_META_WIRE);
        assert_eq!(decode(&bytes).unwrap(), m);
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
