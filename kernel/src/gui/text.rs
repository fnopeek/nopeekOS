//! Inter Variable text rendering + metrics (Phase 10).
//!
//! Owns the system UI font — `Inter Variable` — loaded at boot, BLAKE3-
//! verified against a frozen hash, parsed via `fontdue`. Provides real
//! metrics (advance width, line height, ascent/descent, x-height, cap-
//! height) so the widget layout engine can measure text without
//! rasterizing glyphs.
//!
//! Inter Variable ships weights 100–900 in one file. fontdue v0.9 reads
//! the default instance (weight 400); weight-axis switching per
//! TextStyle is a v2 task (needs ttf-parser + custom outline extraction
//! or rustybuzz). All metrics returned here reflect the default weight
//! but use the real font's `hhea` / `OS/2` tables — no hardcoded values.
//!
//! Glyph atlas is heap-backed in P10.1 (HashMap); P10.4 migrates it into
//! the GGTT glyph region (see `gpu/ggtt_layout.rs`).
//!
//! P10.1 scope: loader + metrics + scaffold cache. P10.5 wires it into
//! `CpuRasterizer`.

#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use fontdue::{Font, FontSettings, Metrics};
use hashbrown::HashMap;
use spin::Mutex;

use crate::shade::widgets::abi::TextStyle;

// ── Font source ───────────────────────────────────────────────────────

/// npkFS path of the system UI font — Inter Variable v4.1 (OFL).
///
/// Seeded on fresh install by `install::bundled_assets::bootstrap_into_npkfs`,
/// thereafter updatable via the OTA path (`intent::install`). The kernel
/// binary itself does **not** embed the font — this keeps normal kernel
/// releases small and makes font updates free of kernel rebuilds.
const FONT_FS_PATH: &str = "sys/fonts/inter-variable";

/// Frozen BLAKE3 digest of the expected font bytes. Checked at load time
/// — a mismatch means the on-disk font was tampered or replaced with an
/// unexpected version; loading refuses.
///
/// Recompute after a font update with `b3sum sys/fonts/inter-variable.ttf`
/// and ship the new hash alongside the new font in a coordinated release.
const INTER_VARIABLE_BLAKE3: &str =
    "273f86e03d009a0ba65d109cf6ed8931560e98289ce1da5bede6c27f36758bf9";

/// IBM Plex Mono Regular — the face behind `TextStyle::Mono`. Inter is
/// proportional, so before this landed every "mono" run (clock digits,
/// file-size columns, source code) was drawn with variable advances and
/// nothing lined up. Shipped UNMODIFIED: the OFL reserves the font name
/// "Plex" for unmodified versions, and subsetting would count as a
/// modification, forcing a rename. See sys/fonts/LICENSE-IBM-Plex.txt.
const MONO_FS_PATH: &str = "sys/fonts/ibm-plex-mono";
const IBM_PLEX_MONO_BLAKE3: &str =
    "51ac7ac25d0fdec160d6d810dd2a58a6b2eaa241c89e1224befcce187a99b5f4";

/// Which face a style is drawn in. Only `Mono` takes the monospace face.
pub const fn is_mono(style: TextStyle) -> bool {
    matches!(style, TextStyle::Mono)
}

// ── TextStyle → (size, weight) mapping ────────────────────────────────

/// Logical pixel size + OpenType weight for a TextStyle. Frozen per
/// PHASE10_WIDGETS.md "Typography" table.
///
/// NOTE: fontdue v0.9 renders the default weight (~400) regardless of
/// the `weight` field — variable-axis switching is deferred to v2.
/// Returned metrics still use the real font (Inter), so layout is
/// correct; only visual weight differentiation is temporarily missing.
#[derive(Clone, Copy, Debug)]
pub struct StyleDesc {
    pub size_px: u16,
    pub weight:  u16,
}

pub const fn style_desc(style: TextStyle) -> StyleDesc {
    match style {
        // Per the Typography table in PHASE10_WIDGETS.md.
        TextStyle::Title   => StyleDesc { size_px: 22, weight: 600 },
        TextStyle::Heading => StyleDesc { size_px: 15, weight: 500 },
        TextStyle::Body    => StyleDesc { size_px: 13, weight: 400 },
        TextStyle::Muted   => StyleDesc { size_px: 13, weight: 400 }, // body + 60% alpha at raster
        TextStyle::Caption => StyleDesc { size_px: 10, weight: 500 },
        TextStyle::Mono    => StyleDesc { size_px: 13, weight: 400 },
    }
}

// ── Global font instance ──────────────────────────────────────────────

static FONT: Mutex<Option<Font>> = Mutex::new(None);
/// The monospace face. `None` until `init` loads it; metrics then fall
/// back to the proportional font so a missing mono file degrades to the
/// pre-0.231 look instead of blank text.
static MONO: Mutex<Option<Font>> = Mutex::new(None);
static READY: AtomicBool = AtomicBool::new(false);

/// Glyph cache key: (glyph index, pixel size, weight).
/// `weight` is stored for the v2 variable-axis path; v1 ignores it
/// (fontdue renders default weight) but the key shape is stable.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub glyph:   u16,
    pub size_px: u16,
    pub weight:  u16,
    /// Which face the glyph id belongs to. Without it, Inter's glyph 42
    /// and Plex Mono's glyph 42 collide in the cache and text renders as
    /// the wrong letters.
    pub mono:    bool,
}

/// Rasterized glyph — alpha bitmap + metrics.
///
/// P10.4: each cached glyph reserves a slot in the GGTT CompSmall4K
/// bucket via the slab allocator. `ggtt_offset` is the slot's address;
/// the alpha bitmap stays heap-resident for now (the CPU rasterizer in
/// P10.5 reads from the heap copy). The GGTT slot is an address
/// reservation so later phases can upload bytes there without
/// re-keying the cache. LRU eviction happens on the slab side.
pub struct CachedGlyph {
    pub alpha:       Vec<u8>,
    pub width:       u16,
    pub height:      u16,
    pub xmin:        i16,
    pub ymin:        i16,
    pub advance:     f32,
    pub ggtt_offset: u32,
}

static GLYPH_CACHE: Mutex<Option<HashMap<GlyphKey, CachedGlyph>>> = Mutex::new(None);

// ── Init ──────────────────────────────────────────────────────────────

/// Load Inter Variable from npkFS. Call after npkfs::mount has succeeded
/// (post-login) and before shade widget pipeline starts raster passes.
///
/// Degrades gracefully — if the font is missing or hash mismatches, logs
/// and returns without installing a font. Widget-side metrics fall back
/// to conservative defaults and rasterization becomes a no-op. The rest
/// of the system (login screen, terminals using Spleen) is unaffected.
pub fn init() {
    if !crate::npkfs::is_mounted() {
        crate::kprintln!("[npk] text::init skipped — npkFS not mounted");
        return;
    }

    // 1. Fetch the font bytes from npkFS.
    let bytes = match crate::npkfs::fetch(FONT_FS_PATH) {
        Ok((data, _cap)) => data,
        Err(e) => {
            crate::kprintln!(
                "[npk] text::init: font not found at {} ({:?}); UI will fall back",
                FONT_FS_PATH, e,
            );
            return;
        }
    };

    // 2. BLAKE3 verify against the frozen digest.
    let hex = blake3::hash(&bytes).to_hex();
    if hex.as_str() != INTER_VARIABLE_BLAKE3 {
        crate::kprintln!(
            "[npk] text::init: Inter Variable hash mismatch (got {}, want {}); refusing to load",
            hex.as_str(), INTER_VARIABLE_BLAKE3,
        );
        return;
    }

    // 3. Parse via fontdue.
    let settings = FontSettings {
        collection_index: 0,
        scale: 40.0,
        ..Default::default()
    };

    let font_len = bytes.len();
    let font = match Font::from_bytes(bytes, settings) {
        Ok(f) => f,
        Err(e) => {
            crate::kprintln!("[npk] text::init: fontdue parse failed: {}", e);
            return;
        }
    };

    // 4. Sanity-log a few metrics so we know the load worked end-to-end.
    let body = style_desc(TextStyle::Body);
    if let Some(lm) = font.horizontal_line_metrics(body.size_px as f32) {
        crate::kprintln!(
            "[npk] Inter Variable loaded: {} glyphs, {} bytes, UPEM {}",
            font.glyph_count(),
            font_len,
            font.units_per_em() as u32,
        );
        crate::kprintln!(
            "[npk] Inter metrics (Body 14px): ascent {:.1}, descent {:.1}, line {:.1}",
            lm.ascent, lm.descent, lm.new_line_size,
        );
    } else {
        crate::kprintln!("[npk] WARN: Inter horizontal_line_metrics returned None");
    }

    *FONT.lock() = Some(font);

    // Monospace face — optional. A miss logs and leaves MONO empty; every
    // metric path then falls back to the proportional font, so the UI
    // still renders (just without aligned columns).
    match crate::npkfs::fetch(MONO_FS_PATH) {
        Ok((bytes, _)) => {
            let hex = blake3::hash(&bytes).to_hex();
            if hex.as_str() != IBM_PLEX_MONO_BLAKE3 {
                crate::kprintln!(
                    "[npk] text::init: IBM Plex Mono hash mismatch (got {}); refusing to load",
                    hex.as_str());
            } else {
                let settings = FontSettings {
                    collection_index: 0,
                    scale: 40.0,
                    ..Default::default()
                };
                match Font::from_bytes(bytes, settings) {
                    Ok(m) => {
                        crate::kprintln!("[npk] IBM Plex Mono loaded: {} glyphs", m.glyph_count());
                        *MONO.lock() = Some(m);
                    }
                    Err(e) => crate::kprintln!("[npk] text::init: mono parse failed: {}", e),
                }
            }
        }
        Err(_) => crate::kprintln!(
            "[npk] text::init: {} not found; mono falls back to Inter", MONO_FS_PATH),
    }
    *GLYPH_CACHE.lock() = Some(HashMap::new());
    READY.store(true, Ordering::Release);
}

/// True once `init` has stored a valid Font. Checked by metrics callers
/// to fail-safe before boot completes.
pub fn is_ready() -> bool {
    READY.load(Ordering::Acquire)
}

// ── Metrics API ───────────────────────────────────────────────────────

/// Horizontal advance of a character in logical pixels (1× HiDPI scale).
/// Returns 0.0 if font not yet loaded or glyph missing.
pub fn advance_width(ch: char, style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font_for(style, |f| f.metrics(ch, d.size_px as f32).advance_width).unwrap_or(0.0)
}

/// Pair kerning correction (left, right) in logical pixels.
/// 0.0 if no kerning pair defined or font not loaded.
pub fn kern(left: char, right: char, style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font_for(style, |f| f.horizontal_kern(left, right, d.size_px as f32))
        .flatten()
        .unwrap_or(0.0)
}

/// Measure a string's total advance width (logical px), with kerning.
pub fn measure(s: &str, style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font_for(style, |f| {
        let size = d.size_px as f32;
        let mut total = 0.0f32;
        let mut prev: Option<char> = None;
        for ch in s.chars() {
            if let Some(p) = prev {
                if let Some(k) = f.horizontal_kern(p, ch, size) {
                    total += k;
                }
            }
            total += f.metrics(ch, size).advance_width;
            prev = Some(ch);
        }
        total
    }).unwrap_or(0.0)
}

/// Line height (ascent − descent + line_gap) in logical pixels.
pub fn line_height(style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font_for(style, |f| f.horizontal_line_metrics(d.size_px as f32).map(|m| m.new_line_size))
        .flatten()
        .unwrap_or(d.size_px as f32 * 1.2) // conservative fallback
}

/// Ascent (baseline → top), always positive.
pub fn ascent(style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font_for(style, |f| f.horizontal_line_metrics(d.size_px as f32).map(|m| m.ascent))
        .flatten()
        .unwrap_or(d.size_px as f32)
}

/// Descent (baseline → bottom). Conventionally negative.
pub fn descent(style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font_for(style, |f| f.horizontal_line_metrics(d.size_px as f32).map(|m| m.descent))
        .flatten()
        .unwrap_or(0.0)
}

/// Cap height — approximated from 'H' glyph height. Inter's OS/2 table
/// has a precise value, but fontdue v0.9 doesn't surface it; glyph-
/// derived is within 1 px. Used for baseline alignment between Title
/// and Body in mixed rows.
pub fn cap_height(style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font_for(style, |f| f.metrics('H', d.size_px as f32).height as f32)
        .unwrap_or(d.size_px as f32 * 0.7)
}

/// X-height — approximated from 'x' glyph height.
pub fn x_height(style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font_for(style, |f| f.metrics('x', d.size_px as f32).height as f32)
        .unwrap_or(d.size_px as f32 * 0.5)
}

// ── Rasterization (used by CpuRasterizer in P10.5) ────────────────────

/// Rasterize a glyph. Returns (metrics, alpha bitmap — 1 byte/px).
/// Falls back to zero-size bitmap if font not loaded.
pub fn rasterize(ch: char, style: TextStyle) -> (Metrics, Vec<u8>) {
    let d = style_desc(style);
    with_font_for(style, |f| f.rasterize(ch, d.size_px as f32)).unwrap_or_else(|| {
        (Metrics::default(), Vec::new())
    })
}

/// Cached variant of `rasterize`. P10.4 replaces the heap Vec with a
/// GGTT offset; the API stays stable.
pub fn rasterize_cached<F, R>(ch: char, style: TextStyle, f: F) -> Option<R>
where
    F: FnOnce(&CachedGlyph) -> R,
{
    let d = style_desc(style);
    let mono_guard = MONO.lock();
    let sans_guard = FONT.lock();
    // Mono styles take the monospace face when it loaded, else the
    // proportional one — same fallback as `with_font_for`.
    let (font, is_mono_face) = match (is_mono(style), mono_guard.as_ref()) {
        (true, Some(m)) => (m, true),
        _ => (sans_guard.as_ref()?, false),
    };
    let glyph = font.lookup_glyph_index(ch);
    let key = GlyphKey { glyph, size_px: d.size_px, weight: d.weight, mono: is_mono_face };

    let mut cache_guard = GLYPH_CACHE.lock();
    let cache = cache_guard.as_mut()?;

    if !cache.contains_key(&key) {
        let (m, alpha) = font.rasterize_indexed(glyph, d.size_px as f32);

        // Reserve a GGTT slot for the alpha bitmap. CompSmall4K fits
        // every glyph we care about (UI text at 11–24 px, max ~32×32
        // = 1 KB). Slab handles LRU eviction if the bucket fills up.
        let ggtt_offset = crate::gpu::ggtt_slab::alloc(
            crate::gpu::ggtt_layout::BucketKind::CompSmall4K,
        )
        .map(|s| s.ggtt_offset())
        .unwrap_or(0);

        cache.insert(key, CachedGlyph {
            alpha,
            width:       m.width  as u16,
            height:      m.height as u16,
            xmin:        m.xmin   as i16,
            ymin:        m.ymin   as i16,
            advance:     m.advance_width,
            ggtt_offset,
        });
    } else {
        // Keep warm glyphs alive in LRU. Cheap — linear on the
        // bucket's VecDeque but hit rate is high on typical text.
        if let Some(cg) = cache.get(&key) {
            if cg.ggtt_offset != 0 {
                // Rebuild a SlotId from the offset for the LRU touch.
                let kind = crate::gpu::ggtt_layout::BucketKind::CompSmall4K;
                let base = crate::gpu::ggtt_layout::BUCKET_BASES[kind as usize];
                let size = crate::gpu::ggtt_layout::BUCKET_SIZES[kind as usize] as u32;
                if cg.ggtt_offset >= base && size > 0 {
                    let idx = (cg.ggtt_offset - base) / size;
                    crate::gpu::ggtt_slab::touch(
                        crate::gpu::ggtt_slab::SlotId { kind, idx },
                    );
                }
            }
        }
    }
    cache.get(&key).map(f)
}

/// Current glyph-cache occupancy (for debug + eviction planning in P10.4).
pub fn cache_len() -> usize {
    GLYPH_CACHE.lock().as_ref().map(|c| c.len()).unwrap_or(0)
}

// ── Internal ──────────────────────────────────────────────────────────

/// Run `f` with the face this style is drawn in, or `None` if it isn't
/// loaded. A missing mono face falls back to the proportional one.
fn with_font_for<F, R>(style: TextStyle, f: F) -> Option<R>
where
    F: FnOnce(&Font) -> R,
{
    if is_mono(style) {
        let guard = MONO.lock();
        if let Some(font) = guard.as_ref() { return Some(f(font)); }
    }
    FONT.lock().as_ref().map(f)
}
