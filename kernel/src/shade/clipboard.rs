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
//!               the bytes, so it is unbounded by construction. Reserved,
//!               not yet used (adding it later is no ABI break — it is
//!               just another variant).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Bound on inline `Text`: a sixteenth of the free RAM, so an app cannot
/// balloon kernel memory through the clipboard. Larger text is refused, never
/// stored cut.
fn max_text_bytes() -> usize {
    crate::memory::stats().0.saturating_mul(crate::memory::PAGE_SIZE) / 16
}

#[allow(dead_code)] // `Object` lands with loft file-copy.
pub enum ClipData {
    Text(Vec<u8>),
    Object { hash: [u8; 32], name: String, size: u64 },
}

static CLIPBOARD: Mutex<Option<ClipData>> = Mutex::new(None);

/// Bumped on every write so a future microvm/agent bridge (and apps) can
/// cheaply detect "did the clipboard change?" without diffing bytes.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Replace the clipboard with UTF-8 text; empty input clears it. False,
/// and the clipboard unchanged, when the text is larger than
/// `max_text_bytes` or the memory for it is not there.
pub fn set_text(bytes: &[u8]) -> bool {
    if bytes.len() > max_text_bytes() { return false; }
    let mut data = Vec::new();
    if data.try_reserve_exact(bytes.len()).is_err() { return false; }
    data.extend_from_slice(bytes);
    *CLIPBOARD.lock() = if data.is_empty() { None } else { Some(ClipData::Text(data)) };
    GENERATION.fetch_add(1, Ordering::Release);
    true
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
