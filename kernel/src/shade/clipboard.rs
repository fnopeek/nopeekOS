//! Cross-app clipboard — a single kernel-owned selection buffer.
//!
//! Written/read by the *focused* widget app only: compositor-internal for
//! TextArea Ctrl+C/X/V (it owns the live buffer), or via the
//! `npk_clipboard_*` host fns for app-driven copy (loft filenames, loop
//! scrollback). Eager, unlike X11/Wayland's lazy ownership model — the
//! data survives the source app closing.
//!
//! Two flavors, picked by data kind (Architecture principle #3:
//! content-addressed, not value-copied, for large data):
//!   - `Text`:   small inline UTF-8 bytes. ~all of copy/paste.
//!   - `Object`: a content-address handle (npkFS BLAKE3 hash + name + size)
//!               for files/large blobs — copies the 32-byte reference, never
//!               the bytes, so it is unbounded by construction. Reserved;
//!               loft's file-copy wires it later (no ABI break — it is just
//!               another variant).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Hard cap on inline `Text` payloads. NOT a UX limit — you never select
/// 4 MiB of text by hand — only an anti-heap-exhaustion guard so a
/// malicious app can't balloon kernel memory. Large data uses `Object`
/// (a reference), never inline bytes.
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

#[allow(dead_code)] // `Object` lands with loft file-copy.
pub enum ClipData {
    Text(Vec<u8>),
    Object { hash: [u8; 32], name: String, size: u64 },
}

static CLIPBOARD: Mutex<Option<ClipData>> = Mutex::new(None);

/// Bumped on every write so a future microvm/agent bridge (and apps) can
/// cheaply detect "did the clipboard change?" without diffing bytes.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Replace the clipboard with UTF-8 text. Truncates at `MAX_TEXT_BYTES`
/// on a char boundary. Empty input clears the clipboard.
pub fn set_text(bytes: &[u8]) {
    let data = if bytes.len() > MAX_TEXT_BYTES {
        // Back up to a UTF-8 boundary so `Text` never holds a split codepoint.
        let mut end = MAX_TEXT_BYTES;
        while end > 0 && (bytes[end] & 0xC0) == 0x80 { end -= 1; }
        &bytes[..end]
    } else {
        bytes
    };
    *CLIPBOARD.lock() = if data.is_empty() {
        None
    } else {
        Some(ClipData::Text(data.to_vec()))
    };
    GENERATION.fetch_add(1, Ordering::Release);
}

/// Current clipboard text, or `None` if empty / holding a non-text kind.
pub fn get_text() -> Option<Vec<u8>> {
    match &*CLIPBOARD.lock() {
        Some(ClipData::Text(v)) => Some(v.clone()),
        _ => None,
    }
}

/// Byte length of the current text payload (0 if empty / non-text). Lets
/// an app size its receive buffer before calling `npk_clipboard_get`.
pub fn text_len() -> usize {
    match &*CLIPBOARD.lock() {
        Some(ClipData::Text(v)) => v.len(),
        _ => 0,
    }
}

/// Monotonic write counter (reserved for the microvm clipboard bridge).
#[allow(dead_code)]
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}
