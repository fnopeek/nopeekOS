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

/// Ein eingebautes Gesicht, das erst beim ERSTEN Gebrauch geparst wird.
///
/// **fontdue hat keinen faulen Weg:** `Font::from_bytes` legt den Umriss
/// JEDER cmap-erreichbaren Glyphe an — das sagt `assets/subset.sh` selbst,
/// und deshalb wurde dort schon von 19 516 auf 14 524 Glyphen gekuerzt.
/// Gemessen kostet das **6,6 bis 7,0 MB Halde je Gesicht** (host-seitig,
/// `examples/heapcheck.rs`), bei sechs Gesichtern 40 MB — die beak beim START
/// bezahlte, obwohl eine gewoehnliche Seite zwei davon anfasst.
///
/// Am Geraet war das der groesste Posten ueberhaupt: `beakbench` misst die
/// Halde bei `instantiate` mit 11 MiB und nach dem Schriftrastern mit 89 MiB;
/// das Layout selbst legt nichts mehr drauf. Und weil talc Seiten nie
/// zurueckgibt, wurde diese Spitze zum Dauerbedarf.
///
/// `OnceCell` und kein `RefCell`: gelesen wird ueber `&self` (siehe `pick`),
/// und ein Gesicht wird genau einmal gebaut. Einen Faden gibt es nicht —
/// `Fonts` liegt einzeln in `Engine`.
struct LazyFace {
    bytes: &'static [u8],
    font: core::cell::OnceCell<Font>,
}

impl LazyFace {
    const fn new(bytes: &'static [u8]) -> LazyFace {
        LazyFace { bytes, font: core::cell::OnceCell::new() }
    }

    fn get(&self) -> &Font {
        self.font.get_or_init(|| {
            // `load_substitutions` only feeds fontdue's glyph-INDEX API; we
            // rasterise by char, and the subsetted faces carry no GSUB anyway
            // (assets/subset.sh). Leaving it on just outlines dead glyphs.
            let settings = FontSettings { load_substitutions: false, ..FontSettings::default() };
            Font::from_bytes(self.bytes, settings).expect("embedded font is valid TrueType")
        })
    }
}

pub struct Fonts {
    /// Die Gesichter der Seite. Werden EINGEHAENGT, nicht ersetzt: die
    /// eingebauten bleiben die Ersatzkette.
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

    /// Wieviele der sechs eingebauten Gesichter wirklich geparst wurden.
    /// Gehoert ins Log: „faul" ist sonst eine Behauptung.
    pub fn loaded_faces(&self) -> usize {
        [&self.regular, &self.bold, &self.italic,
         &self.bold_italic, &self.mono, &self.mono_bold]
            .iter().filter(|f| f.font.get().is_some()).count()
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
            (true, false, _) => self.mono.get(),
            (true, true, _) => self.mono_bold.get(),
            (false, false, false) => self.regular.get(),
            (false, true, false) => self.bold.get(),
            (false, false, true) => self.italic.get(),
            (false, true, true) => self.bold_italic.get(),
        }
    }

    /// The regular body face — for size-agnostic estimates (line-box height
    /// around floats, intrinsic auto-sizing) where a single reference face is
    /// fine and keeps behaviour identical to the old single-font path.
    pub fn regular(&self) -> &Font {
        self.regular.get()
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
