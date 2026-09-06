//! fonts.rs — the browser's font faces + per-run face selection.
//!
//! Six embedded faces: Inter Regular/Bold/Italic/BoldItalic for body text
//! (matching the compositor's Inter) and Noto Sans Mono Regular/Bold for
//! `<code>`/`<pre>`/`font-family:monospace`. Layout MEASURES and the raster
//! DRAWS through the same `pick(bold, italic, mono)`, so glyph advances agree
//! with the glyphs actually painted. This replaces the earlier single-face
//! approach that faked weight (a 1px horizontal smear) and slant (a shear) —
//! real faces render correctly and, for monospace, at the right advance width.

use alloc::vec::Vec;
use fontdue::{Font, FontSettings};

/// The loaded set of faces. Built once (in `raster::Engine::new`) and shared
/// by reference into layout + paint.
/// Ein Gesicht, das die SEITE mitgebracht hat (`@font-face`).
pub struct WebFace {
    /// Streuwert des Familiennamens — dieselbe Zahl, die `ComputedStyle`
    /// traegt (`style::family_hash`).
    pub family: u32,
    /// 100..900. Gewaehlt wird das naechstliegende, nicht das gleiche.
    pub weight: u16,
    pub italic: bool,
    pub font: Font,
}

pub struct Fonts {
    /// Die Gesichter der Seite. Werden EINGEHAENGT, nicht ersetzt: die
    /// eingebauten bleiben die Ersatzkette.
    web: Vec<WebFace>,
    regular: Font,
    bold: Font,
    italic: Font,
    bold_italic: Font,
    mono: Font,
    mono_bold: Font,
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}

impl Fonts {
    pub fn new() -> Fonts {
        fn load(bytes: &[u8]) -> Font {
            // `load_substitutions` only feeds fontdue's glyph-INDEX API; we
            // rasterise by char, and the subsetted faces carry no GSUB anyway
            // (assets/subset.sh). Leaving it on just outlines dead glyphs.
            let settings = FontSettings { load_substitutions: false, ..FontSettings::default() };
            Font::from_bytes(bytes, settings).expect("embedded font is valid TrueType")
        }
        Fonts {
            web: Vec::new(),
            regular: load(include_bytes!("../assets/inter.ttf")),
            bold: load(include_bytes!("../assets/inter-bold.ttf")),
            italic: load(include_bytes!("../assets/inter-italic.ttf")),
            bold_italic: load(include_bytes!("../assets/inter-bolditalic.ttf")),
            mono: load(include_bytes!("../assets/mono.ttf")),
            mono_bold: load(include_bytes!("../assets/mono-bold.ttf")),
        }
    }

    /// The face for a run's style. Monospace ships no italic face (Noto Sans
    /// Mono has none), so `mono + italic` renders upright mono — rare
    /// (`<code><i>`), and better than mixing a proportional italic into code.
    /// Eine Schrift der Seite aufnehmen. Liefert false, wenn die Bytes keine
    /// lesbare Schrift sind — der Rufer meldet das, statt es zu verschlucken.
    pub fn add_web(&mut self, family: u32, weight: u16, italic: bool, bytes: &[u8]) -> bool {
        let settings = FontSettings { load_substitutions: false, ..FontSettings::default() };
        match Font::from_bytes(bytes, settings) {
            Ok(font) => { self.web.push(WebFace { family, weight, italic, font }); true }
            Err(_) => false,
        }
    }

    pub fn web_count(&self) -> usize { self.web.len() }

    /// Hat die Seite diese Familie mitgebracht?
    pub fn has_web(&self, family: u32) -> bool {
        family != 0 && self.web.iter().any(|w| w.family == family)
    }

    /// Das beste Gesicht dieser Familie fuer Gewicht und Neigung.
    ///
    /// **Naechstliegend, nicht gleich.** Eine Seite laedt oft nur Regular und
    /// Bold und verlangt trotzdem 600 — dann ist Bold die Antwort, nicht die
    /// eingebaute Schrift.
    fn web_pick(&self, family: u32, bold: bool, italic: bool) -> Option<&Font> {
        if family == 0 { return None }
        let want = if bold { 700u16 } else { 400 };
        let mut best: Option<(&WebFace, i32)> = None;
        for w in self.web.iter().filter(|w| w.family == family) {
            // Die Neigung wiegt schwerer als das Gewicht: ein kursives
            // Gesicht durch ein aufrechtes zu ersetzen sieht falscher aus als
            // ein Strich zu duenn.
            let cost = (w.weight as i32 - want as i32).abs()
                     + if w.italic == italic { 0 } else { 1000 };
            if best.is_none_or(|(_, c)| cost < c) { best = Some((w, cost)); }
        }
        best.map(|(w, _)| &w.font)
    }

    pub fn pick(&self, bold: bool, italic: bool, mono: bool, family: u32) -> &Font {
        if let Some(f) = self.web_pick(family, bold, italic) { return f }
        match (mono, bold, italic) {
            (true, false, _) => &self.mono,
            (true, true, _) => &self.mono_bold,
            (false, false, false) => &self.regular,
            (false, true, false) => &self.bold,
            (false, false, true) => &self.italic,
            (false, true, true) => &self.bold_italic,
        }
    }

    /// The regular body face — for size-agnostic estimates (line-box height
    /// around floats, intrinsic auto-sizing) where a single reference face is
    /// fine and keeps behaviour identical to the old single-font path.
    pub fn regular(&self) -> &Font {
        &self.regular
    }

    /// A stable id per face — mixed into the raster's glyph-cache key so two
    /// faces never collide on the same `(char, size)`.
    /// Ein stabiler Schluessel je Gesicht — geht in den Glyphenspeicher, damit
    /// zwei Gesichter sich bei `(Zeichen, Groesse)` nicht ins Gehege kommen.
    /// Die Familie MUSS mit hinein: sonst malte die zweite Seitenschrift die
    /// Glyphen der ersten.
    pub fn face_key(bold: bool, italic: bool, mono: bool, family: u32) -> u32 {
        if family != 0 { return family | 0x8000_0000 }
        Self::face_id(bold, italic, mono)
    }

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
