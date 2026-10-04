//! fonts.rs — the browser's font faces + per-run face selection.
//!
//! Six embedded faces: Inter Regular/Bold/Italic/BoldItalic for body text
//! (matching the compositor's Inter) and Noto Sans Mono Regular/Bold for
//! `<code>`/`<pre>`/`font-family:monospace`, plus faces a page loads via
//! `@font-face`. Layout measures and the raster draws through the same
//! `pick`, so glyph advances agree with the glyphs actually painted.

use alloc::vec::Vec;
use fontdue::{Font, FontSettings};

use crate::gsub::Ligatures;

/// A face as the text path needs it: the outlines and the ligature table
/// that belongs to them.
///
/// `Copy` and `Deref<Target = Font>` on purpose: existing `font.metrics(…)`
/// and `measure(font, …)` call sites keep working, and the ligatures ride
/// along to measuring and painting without extra parameters.
///
/// Measuring and painting must use the same table; a run shaped in the
/// painter but not the measurer lands in a box never reserved for it.
#[derive(Clone, Copy)]
pub struct Face<'a> {
    pub font: &'a Font,
    lig: &'a Ligatures,
}

impl core::ops::Deref for Face<'_> {
    type Target = Font;
    fn deref(&self) -> &Font {
        self.font
    }
}

impl<'a> Face<'a> {
    /// The ligature table, or `None` when this face has none — which is every
    /// face we ship (they are subsetted) and most web fonts. The `None` is what
    /// lets `measure` keep its allocation-free per-character loop.
    pub fn ligatures(&self) -> Option<&'a Ligatures> {
        (!self.lig.is_empty()).then_some(self.lig)
    }

    /// Average and maximum character advance at `size`, from the font's own
    /// tables (`OS/2.xAvgCharWidth`, `head` bounding box).
    ///
    /// Used for the intrinsic width of `<input size=n>` and
    /// `<textarea cols=n>`, which browsers derive from the average advance,
    /// not the width of "0". `None` if the font lacks the tables; the caller
    /// then falls back to "0".
    pub fn char_widths(&self, size: f32) -> Option<(f32, f32)> {
        let (a, m) = (self.lig.avg_char(), self.lig.max_char());
        (a > 0.0 && m > 0.0).then(|| (a * size, m * size))
    }

    /// The glyphs this text becomes, each with the byte range of the source it
    /// covers. Line breaking walks offsets into the original string, not glyph
    /// counts, so the ranges are what keeps it working.
    pub fn shape(&self, s: &str) -> Vec<(u16, usize, usize)> {
        let mut raw: Vec<(u16, usize, usize)> = Vec::with_capacity(s.len());
        for (i, c) in s.char_indices() {
            raw.push((self.font.lookup_glyph_index(c), i, c.len_utf8()));
        }
        let Some(lig) = self.ligatures() else { return raw };
        let ids: Vec<u16> = raw.iter().map(|(g, _, _)| *g).collect();
        let mut out: Vec<(u16, usize, usize)> = Vec::with_capacity(raw.len());
        let mut k = 0usize;
        while k < raw.len() {
            match lig.apply(&ids, k) {
                Some((g, take)) => {
                    let start = raw[k].1;
                    let end = raw[k + take - 1].1 + raw[k + take - 1].2;
                    out.push((g, start, end - start));
                    k += take;
                }
                None => {
                    out.push(raw[k]);
                    k += 1;
                }
            }
        }
        out
    }
}

/// A face the page brought (`@font-face`).
pub struct WebFace {
    /// Hash of the family name, the same number `ComputedStyle` carries
    /// (`style::family_hash`).
    pub family: u32,
    /// 100..900. Selection takes the nearest, not an exact match.
    pub weight: u16,
    pub italic: bool,
    pub font: Font,
    /// Read when the font arrives: an icon font's ligatures are its glyphs.
    pub lig: Ligatures,
}

/// An embedded face, parsed on first use.
///
/// fontdue has no lazy path: `Font::from_bytes` builds the outline of every
/// cmap-reachable glyph, several MB of heap per face, while a typical page
/// uses two of the six. The allocator never returns pages, so an eager
/// startup peak would become permanent.
///
/// `OnceCell`, not `RefCell`: read through `&self` (see `pick`), built
/// exactly once. Single-threaded; `Fonts` lives once in `Engine`.
struct LazyFace {
    bytes: &'static [u8],
    font: core::cell::OnceCell<Font>,
    lig: core::cell::OnceCell<Ligatures>,
}

impl LazyFace {
    const fn new(bytes: &'static [u8]) -> LazyFace {
        LazyFace { bytes, font: core::cell::OnceCell::new(), lig: core::cell::OnceCell::new() }
    }

    fn get(&self) -> &Font {
        self.font.get_or_init(|| {
            // `load_substitutions` only feeds fontdue's glyph-index API: it makes
            // ligature glyphs rasterisable, it does not substitute (fontdue has no
            // shaper; `gsub.rs` does that). The embedded faces are subsetted and
            // carry no GSUB, so leaving it on would only outline dead glyphs.
            let settings = FontSettings { load_substitutions: false, ..FontSettings::default() };
            Font::from_bytes(self.bytes, settings).expect("embedded font is valid TrueType")
        })
    }

    fn ligatures(&self) -> &Ligatures {
        self.lig.get_or_init(|| Ligatures::read(self.bytes, 0))
    }

    fn face(&self) -> Face<'_> {
        Face { font: self.get(), lig: self.ligatures() }
    }
}

/// The loaded set of faces. Built once (in `raster::Engine::new`) and shared
/// by reference into layout + paint.
pub struct Fonts {
    /// The page's faces. Appended, never replacing the embedded ones, which
    /// remain the fallback chain.
    web: Vec<WebFace>,
    regular: LazyFace,
    bold: LazyFace,
    italic: LazyFace,
    bold_italic: LazyFace,
    mono: LazyFace,
    mono_bold: LazyFace,
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}

impl Fonts {
    pub fn new() -> Fonts {
        Fonts {
            web: Vec::new(),
            regular: LazyFace::new(include_bytes!("../assets/inter.ttf")),
            bold: LazyFace::new(include_bytes!("../assets/inter-bold.ttf")),
            italic: LazyFace::new(include_bytes!("../assets/inter-italic.ttf")),
            bold_italic: LazyFace::new(include_bytes!("../assets/inter-bolditalic.ttf")),
            mono: LazyFace::new(include_bytes!("../assets/mono.ttf")),
            mono_bold: LazyFace::new(include_bytes!("../assets/mono-bold.ttf")),
        }
    }

    /// How many of the six embedded faces were actually parsed (for the log).
    pub fn loaded_faces(&self) -> usize {
        [&self.regular, &self.bold, &self.italic,
         &self.bold_italic, &self.mono, &self.mono_bold]
            .iter().filter(|f| f.font.get().is_some()).count()
    }

    /// Add a page font. Returns false if the bytes are not a readable font;
    /// the caller reports that.
    pub fn add_web(&mut self, family: u32, weight: u16, italic: bool, bytes: &[u8]) -> bool {
        // Unlike the embedded faces, a page font needs `load_substitutions`: it
        // brings its own GSUB, and without this the ligature glyph would exist
        // but have no outline to rasterise.
        let settings = FontSettings { load_substitutions: true, ..FontSettings::default() };
        match Font::from_bytes(bytes, settings) {
            Ok(font) => {
                let lig = Ligatures::read(bytes, 0);
                self.web.push(WebFace { family, weight, italic, font, lig });
                true
            }
            Err(_) => false,
        }
    }

    pub fn web_count(&self) -> usize { self.web.len() }

    /// Did the page bring this family?
    pub fn has_web(&self, family: u32) -> bool {
        family != 0 && self.web.iter().any(|w| w.family == family)
    }

    /// The best face of this family for weight and slant: nearest, not exact.
    /// A page often loads only Regular and Bold and still asks for 600; Bold
    /// is then the answer, not the embedded font.
    fn web_pick(&self, family: u32, bold: bool, italic: bool) -> Option<Face<'_>> {
        if family == 0 { return None }
        let want = if bold { 700u16 } else { 400 };
        let mut best: Option<(&WebFace, i32)> = None;
        for w in self.web.iter().filter(|w| w.family == family) {
            // Slant outweighs weight: substituting an upright face for an italic
            // looks more wrong than a stroke that is too thin.
            let cost = (w.weight as i32 - want as i32).abs()
                     + if w.italic == italic { 0 } else { 1000 };
            if best.is_none_or(|(_, c)| cost < c) { best = Some((w, cost)); }
        }
        best.map(|(w, _)| Face { font: &w.font, lig: &w.lig })
    }

    /// The face for a run's style. Monospace ships no italic face, so
    /// `mono + italic` renders upright mono rather than a proportional italic.
    pub fn pick(&self, bold: bool, italic: bool, mono: bool, family: u32) -> Face<'_> {
        if let Some(f) = self.web_pick(family, bold, italic) { return f }
        match (mono, bold, italic) {
            (true, false, _) => self.mono.face(),
            (true, true, _) => self.mono_bold.face(),
            (false, false, false) => self.regular.face(),
            (false, true, false) => self.bold.face(),
            (false, false, true) => self.italic.face(),
            (false, true, true) => self.bold_italic.face(),
        }
    }

    /// The regular body face, for size-agnostic estimates (line-box height
    /// around floats, intrinsic auto-sizing) where one reference face is fine.
    pub fn regular(&self) -> Face<'_> {
        self.regular.face()
    }

    /// A stable key per face for the raster's glyph cache, so two faces never
    /// collide on the same `(char, size)`. Page fonts include the family, or
    /// the second page font would paint the first one's glyphs.
    pub fn face_key(bold: bool, italic: bool, mono: bool, family: u32) -> u32 {
        if family != 0 { return family | 0x8000_0000 }
        Self::face_id(bold, italic, mono)
    }

    /// A stable id per embedded face (style bits only).
    pub fn face_id(bold: bool, italic: bool, mono: bool) -> u32 {
        match (mono, bold, italic) {
            (true, false, _) => 4,
            (true, true, _) => 5,
            (false, false, false) => 0,
            (false, true, false) => 1,
            (false, false, true) => 2,
            (false, true, true) => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded faces carry no ligatures (they are subsetted by
    /// `assets/subset.sh`). `measure` relies on this to keep its
    /// allocation-free path.
    #[test]
    fn the_embedded_faces_carry_no_ligatures() {
        let f = Fonts::new();
        for (b, i, m) in [(false, false, false), (true, false, false), (false, true, false),
                          (true, true, false), (false, false, true), (true, false, true)] {
            assert!(f.pick(b, i, m, 0).ligatures().is_none());
        }
    }

    /// Without ligatures `shape` is the identity: one glyph per character, and
    /// the byte ranges cover the string without gaps or overlap. Line breaking
    /// runs on these ranges.
    #[test]
    fn shaping_without_ligatures_keeps_every_byte_range() {
        let f = Fonts::new();
        let face = f.pick(false, false, false, 0);
        let s = "Grüße, Welt";
        let sh = face.shape(s);
        assert_eq!(sh.len(), s.chars().count());
        let mut at = 0usize;
        for (_, start, n) in &sh {
            assert_eq!(*start, at);
            at += n;
        }
        assert_eq!(at, s.len());
    }
}
