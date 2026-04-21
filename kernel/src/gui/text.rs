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

// ── Embedded font bytes ───────────────────────────────────────────────

/// Inter Variable v4.1 (OFL). Embedded at build time; swap to npkFS load
/// when the system-fonts content-addressing path lands (not in Phase 10).
///
/// Path resolves relative to this source file.
const INTER_VARIABLE_TTF: &[u8] =
    include_bytes!("../../../sys/fonts/inter-variable.ttf");

/// Frozen BLAKE3 digest of `INTER_VARIABLE_TTF`. Boot-time verification
/// panics on mismatch — catches corruption or an accidental font swap.
/// Recompute with `b3sum sys/fonts/inter-variable.ttf` after updates.
const INTER_VARIABLE_BLAKE3: &str =
    "273f86e03d009a0ba65d109cf6ed8931560e98289ce1da5bede6c27f36758bf9";

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
        TextStyle::Title   => StyleDesc { size_px: 24, weight: 600 },
        TextStyle::Body    => StyleDesc { size_px: 14, weight: 400 },
        TextStyle::Muted   => StyleDesc { size_px: 14, weight: 400 }, // body + 60% alpha at raster
        TextStyle::Caption => StyleDesc { size_px: 11, weight: 500 },
        // Mono routes to Spleen bitmap at raster time; metrics unused.
        TextStyle::Mono    => StyleDesc { size_px: 16, weight: 400 },
    }
}

// ── Global font instance ──────────────────────────────────────────────

static FONT: Mutex<Option<Font>> = Mutex::new(None);
static READY: AtomicBool = AtomicBool::new(false);

/// Glyph cache key: (glyph index, pixel size, weight).
/// `weight` is stored for the v2 variable-axis path; v1 ignores it
/// (fontdue renders default weight) but the key shape is stable.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub glyph:   u16,
    pub size_px: u16,
    pub weight:  u16,
}

/// Rasterized glyph — alpha bitmap + metrics. P10.4 migrates storage
/// into the GGTT glyph region; v1 keeps this heap-backed.
pub struct CachedGlyph {
    pub alpha:   Vec<u8>,
    pub width:   u16,
    pub height:  u16,
    pub xmin:    i16,
    pub ymin:    i16,
    pub advance: f32,
}

static GLYPH_CACHE: Mutex<Option<HashMap<GlyphKey, CachedGlyph>>> = Mutex::new(None);

// ── Init ──────────────────────────────────────────────────────────────

/// Load Inter Variable at boot. Panics on hash mismatch or parse error
/// — the font is not optional, and the embedded bytes are content-
/// addressable. Call after heap init, before shade::init.
pub fn init() {
    // 1. BLAKE3 verify the embedded bytes against the frozen digest.
    let hash = blake3::hash(INTER_VARIABLE_TTF);
    let hex = hash.to_hex();
    if hex.as_str() != INTER_VARIABLE_BLAKE3 {
        crate::kprintln!(
            "[npk] FATAL: Inter Variable BLAKE3 mismatch: got {}, expected {}",
            hex.as_str(), INTER_VARIABLE_BLAKE3,
        );
        panic!("inter-variable.ttf hash mismatch");
    }

    // 2. Parse via fontdue. Default settings; scale is per-call.
    let settings = FontSettings {
        // collection_index: 0 for TTF (not TTC collection).
        collection_index: 0,
        // Raster scale: 1.0 → metrics in px match the size_px argument.
        scale: 40.0,
        ..Default::default()
    };

    let font = match Font::from_bytes(INTER_VARIABLE_TTF, settings) {
        Ok(f) => f,
        Err(e) => {
            crate::kprintln!("[npk] FATAL: fontdue parse failed: {}", e);
            panic!("inter-variable.ttf parse failed");
        }
    };

    // 3. Sanity-log a few metrics so we know the load worked end-to-end.
    let body = style_desc(TextStyle::Body);
    if let Some(lm) = font.horizontal_line_metrics(body.size_px as f32) {
        crate::kprintln!(
            "[npk] Inter Variable loaded: {} glyphs, {} bytes, UPEM {}",
            font.glyph_count(),
            INTER_VARIABLE_TTF.len(),
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
    with_font(|f| f.metrics(ch, d.size_px as f32).advance_width).unwrap_or(0.0)
}

/// Pair kerning correction (left, right) in logical pixels.
/// 0.0 if no kerning pair defined or font not loaded.
pub fn kern(left: char, right: char, style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font(|f| f.horizontal_kern(left, right, d.size_px as f32))
        .flatten()
        .unwrap_or(0.0)
}

/// Measure a string's total advance width (logical px), with kerning.
pub fn measure(s: &str, style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font(|f| {
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
    with_font(|f| f.horizontal_line_metrics(d.size_px as f32).map(|m| m.new_line_size))
        .flatten()
        .unwrap_or(d.size_px as f32 * 1.2) // conservative fallback
}

/// Ascent (baseline → top), always positive.
pub fn ascent(style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font(|f| f.horizontal_line_metrics(d.size_px as f32).map(|m| m.ascent))
        .flatten()
        .unwrap_or(d.size_px as f32)
}

/// Descent (baseline → bottom). Conventionally negative.
pub fn descent(style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font(|f| f.horizontal_line_metrics(d.size_px as f32).map(|m| m.descent))
        .flatten()
        .unwrap_or(0.0)
}

/// Cap height — approximated from 'H' glyph height. Inter's OS/2 table
/// has a precise value, but fontdue v0.9 doesn't surface it; glyph-
/// derived is within 1 px. Used for baseline alignment between Title
/// and Body in mixed rows.
pub fn cap_height(style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font(|f| f.metrics('H', d.size_px as f32).height as f32)
        .unwrap_or(d.size_px as f32 * 0.7)
}

/// X-height — approximated from 'x' glyph height.
pub fn x_height(style: TextStyle) -> f32 {
    let d = style_desc(style);
    with_font(|f| f.metrics('x', d.size_px as f32).height as f32)
        .unwrap_or(d.size_px as f32 * 0.5)
}

// ── Rasterization (used by CpuRasterizer in P10.5) ────────────────────

/// Rasterize a glyph. Returns (metrics, alpha bitmap — 1 byte/px).
/// Falls back to zero-size bitmap if font not loaded.
pub fn rasterize(ch: char, style: TextStyle) -> (Metrics, Vec<u8>) {
    let d = style_desc(style);
    with_font(|f| f.rasterize(ch, d.size_px as f32)).unwrap_or_else(|| {
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
    let font_guard = FONT.lock();
    let font = font_guard.as_ref()?;
    let glyph = font.lookup_glyph_index(ch);
    let key = GlyphKey { glyph, size_px: d.size_px, weight: d.weight };

    let mut cache_guard = GLYPH_CACHE.lock();
    let cache = cache_guard.as_mut()?;

    if !cache.contains_key(&key) {
        let (m, alpha) = font.rasterize_indexed(glyph, d.size_px as f32);
        cache.insert(key, CachedGlyph {
            alpha,
            width:   m.width  as u16,
            height:  m.height as u16,
            xmin:    m.xmin   as i16,
            ymin:    m.ymin   as i16,
            advance: m.advance_width,
        });
    }
    cache.get(&key).map(f)
}

/// Current glyph-cache occupancy (for debug + eviction planning in P10.4).
pub fn cache_len() -> usize {
    GLYPH_CACHE.lock().as_ref().map(|c| c.len()).unwrap_or(0)
}

// ── Internal ──────────────────────────────────────────────────────────

/// Run `f` with the loaded font, or `None` if not yet initialized.
fn with_font<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&Font) -> R,
{
    FONT.lock().as_ref().map(f)
}
