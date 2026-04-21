//! Widget pipeline — declarative GUI for WASM apps.
//!
//! Apps describe **what** to render (widget tree); Shade owns **how**
//! (layout, rasterization, GPU compositing, animation, theming).
//!
//! See PHASE10_WIDGETS.md for the full spec. This module is built up in
//! phases — P10.0 lands only the frozen ABI + tile constants.
//!
//! Phase map:
//!   P10.0 (here) — abi, tile, check_abi, ggtt_layout constants
//!   P10.1 — SDK crate + font metrics (gui/text.rs)
//!   P10.2 — npk_scene_commit host fn + deserialize + serial dump
//!   P10.3 — layout (flexbox-lite) with real font metrics
//!   P10.4 — GGTT slab allocator
//!   P10.5 — tile + comp-layer rasterization (first visible window)
//!   P10.6 — diff + per-app cache
//!   P10.7 — event routing
//!   P10.8 — animation (fixed-point Q16.16)
//!   P10.9 — icon atlas
//!   P10.10 — Canvas escape hatch
//!   P10.11 — first real app (file browser)

pub mod abi;
pub mod tile;
pub mod debug;

// Compile-time ABI ordering guard. Module exists solely for its
// const-asserts and exhaustive-match functions.
mod check_abi;

// ── Scene commit (P10.2) ──────────────────────────────────────────────

use alloc::vec::Vec;

/// Deserialize a wire-framed widget tree from an app's commit payload.
///
/// Expected layout: `[ version: u8 ][ postcard-serialized Widget ]`.
/// Returns -1 on version mismatch, -2 on deserialize failure.
/// Prints the decoded tree to serial on success (P10.2 deliverable).
pub fn scene_commit(bytes: &[u8]) -> i32 {
    let (&version, body) = match bytes.split_first() {
        Some(v) => v,
        None => {
            crate::kprintln!("[npk] scene_commit: empty payload");
            return -1;
        }
    };
    if version != abi::WIRE_VERSION {
        crate::kprintln!(
            "[npk] scene_commit: wire version mismatch (got {:#x}, want {:#x})",
            version, abi::WIRE_VERSION,
        );
        return -1;
    }
    let tree: abi::Widget = match postcard::from_bytes(body) {
        Ok(t) => t,
        Err(e) => {
            crate::kprintln!("[npk] scene_commit: postcard decode failed: {:?}", e);
            return -2;
        }
    };
    crate::kprintln!("[npk] scene_commit: {} bytes → tree decoded", bytes.len());
    debug::print_tree(&tree);
    // P10.3+ stores the tree into a per-app scene slot; P10.2 just logs
    // and drops. Swallow unused-warning via explicit discard.
    let _ = tree;
    let _: Vec<u8> = Vec::new();
    0
}
