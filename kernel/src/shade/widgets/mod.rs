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
pub mod layout;
pub mod palette;
pub mod render;
pub mod raster;

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

    // P10.3+: lay out into an 800×600 window centred on the screen.
    // A full-framebuffer layout makes the first-pass UI disappear on
    // a 4K display; a fixed-size window gives a compact, legible
    // preview. Proper window management comes with shade integration
    // in a follow-up.
    let (fb_w, fb_h) = crate::framebuffer::get_resolution();
    let win_w = 800u32.min(fb_w);
    let win_h = 600u32.min(fb_h);
    let win_x = ((fb_w.saturating_sub(win_w)) / 2) as i32;
    let win_y = ((fb_h.saturating_sub(win_h)) / 2) as i32;
    let window = abi::Rect { x: win_x, y: win_y, w: win_w, h: win_h };

    let layout_tree = layout::layout(&tree, window);
    debug::print_layout(&tree, &layout_tree);

    // P10.5: rasterize the tree into a heap back buffer, then blit
    // the pixels to the framebuffer at the window origin. One
    // RasterTarget covering the whole window — tile subdivision +
    // dirty-diff caching come in P10.6.
    rasterize_and_blit(&tree, &layout_tree, win_x, win_y, win_w, win_h);

    let _ = tree;
    let _ = layout_tree;
    let _: Vec<u8> = Vec::new();
    0
}

/// Allocate a back buffer, rasterize the widget+layout tree into it,
/// blit to the framebuffer. First-pass renderer — CPU only, no tile
/// subdivision, no diff cache.
fn rasterize_and_blit(
    tree:      &abi::Widget,
    layout:    &layout::LayoutNode,
    win_x:     i32,
    win_y:     i32,
    win_w:     u32,
    win_h:     u32,
) {
    // Back buffer: BGRA packed u32 per pixel.
    let pixel_count = (win_w as usize) * (win_h as usize);
    let mut pixels: Vec<u32> = alloc::vec![0u32; pixel_count];

    // Clear to Surface token before rendering — covers areas not
    // touched by any widget.
    let bg = palette::resolve(abi::Token::Surface);
    for p in pixels.iter_mut() { *p = bg; }

    let pal = palette::current();
    {
        let mut target = abi::RasterTarget {
            pixels:  &mut pixels,
            stride:  win_w,
            size:    abi::Size { w: win_w, h: win_h },
            origin:  abi::Point { x: win_x, y: win_y },
            scale:   1,
            palette: &pal,
        };
        let mut rast = raster::cpu::CpuRasterizer::new();
        render::render(&mut rast, &mut target, tree, layout);
    }

    // Push pixels to the framebuffer shadow + blit to MMIO.
    crate::framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();
        let fb_pitch = info.pitch as usize;
        let fb_w = info.width;
        let fb_h = info.height;

        // Clip to screen bounds.
        let x0 = win_x.max(0) as u32;
        let y0 = win_y.max(0) as u32;
        let x1 = (win_x + win_w as i32).max(0) as u32;
        let y1 = (win_y + win_h as i32).max(0) as u32;
        let x1 = x1.min(fb_w);
        let y1 = y1.min(fb_h);
        if x0 >= x1 || y0 >= y1 { return; }

        for dy in y0..y1 {
            let src_y = (dy as i32 - win_y) as usize;
            let dst_off = dy as usize * fb_pitch + (x0 as usize) * 4;
            let src_off = src_y * (win_w as usize) + (x0 as i32 - win_x) as usize;
            let span = (x1 - x0) as usize;
            // SAFETY: shadow is valid for fb_h * fb_pitch; bounds above
            // guarantee dst_off + span*4 stays inside that region.
            unsafe {
                let dst = shadow.add(dst_off) as *mut u32;
                core::ptr::copy_nonoverlapping(
                    pixels.as_ptr().add(src_off),
                    dst,
                    span,
                );
            }
        }

        crate::framebuffer::blit_rect(fb, x0, y0, x1 - x0, y1 - y0);
    });

    crate::kprintln!(
        "[npk] scene_commit: rendered + blit {}x{} @ ({}, {})",
        win_w, win_h, win_x, win_y,
    );
}
