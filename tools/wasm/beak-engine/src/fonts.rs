//! fonts.rs — the browser's font faces + per-run face selection.
//!
//! Six embedded faces: Inter Regular/Bold/Italic/BoldItalic for body text
//! (matching the compositor's Inter) and Noto Sans Mono Regular/Bold for
//! `<code>`/`<pre>`/`font-family:monospace`. Layout MEASURES and the raster
//! DRAWS through the same `pick(bold, italic, mono)`, so glyph advances agree
//! with the glyphs actually painted. This replaces the earlier single-face
//! approach that faked weight (a 1px horizontal smear) and slant (a shear) —
//! real faces render correctly and, for monospace, at the right advance width.

use fontdue::{Font, FontSettings};

/// The loaded set of faces. Built once (in `raster::Engine::new`) and shared
/// by reference into layout + paint.
pub struct Fonts {
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
    pub fn pick(&self, bold: bool, italic: bool, mono: bool) -> &Font {
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
