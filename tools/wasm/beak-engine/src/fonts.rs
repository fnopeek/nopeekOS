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

use crate::gsub::Ligatures;

/// A face as the text path needs it: the outlines AND the ligature table that
/// belongs to them.
///
/// `Copy` and `Deref<Target = Font>` on purpose — every existing
/// `font.metrics(…)` and `measure(font, …)` call site keeps working unchanged,
/// and the ligatures ride along to the two places that need them (measuring
/// and painting) instead of being plumbed through thirty signatures.
///
/// **Measuring and painting MUST take the same table.** A run shaped in the
/// painter but not the measurer lands in a box that was never reserved for it
/// ([[feedback_intrinsic_shared_path]]).
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

    /// The glyphs this text becomes, each with the BYTE RANGE of the source it
    /// covers. The byte ranges are what keeps line breaking working: it walks
    /// offsets into the original string, not glyph counts.
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
    /// Gelesen, wenn die Schrift ankommt: eine Symbolschrift bringt ihre
    /// Ligaturen mit, und genau die sind ihr Bild.
    pub lig: Ligatures,
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
    lig: core::cell::OnceCell<Ligatures>,
}

impl LazyFace {
    const fn new(bytes: &'static [u8]) -> LazyFace {
        LazyFace { bytes, font: core::cell::OnceCell::new(), lig: core::cell::OnceCell::new() }
    }

    fn get(&self) -> &Font {
        self.font.get_or_init(|| {
            // `load_substitutions` only feeds fontdue's glyph-INDEX API — it
            // makes the ligature glyphs rasterisable, it does not substitute
            // (fontdue has no shaper; `gsub.rs` does that part). The six
            // embedded faces are subsetted and carry no GSUB at all, so
            // leaving it on here would only outline dead glyphs.
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
        // **HIER ist `load_substitutions` noetig**, anders als bei den
        // eingebauten Gesichtern: eine Seitenschrift bringt ihr GSUB mit, und
        // ohne diese Zeile gaebe es die Ligaturglyphe zwar im Baum, aber
        // keinen Umriss zum Rastern.
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

    /// Hat die Seite diese Familie mitgebracht?
    pub fn has_web(&self, family: u32) -> bool {
        family != 0 && self.web.iter().any(|w| w.family == family)
    }

    /// Das beste Gesicht dieser Familie fuer Gewicht und Neigung.
    ///
    /// **Naechstliegend, nicht gleich.** Eine Seite laedt oft nur Regular und
    /// Bold und verlangt trotzdem 600 — dann ist Bold die Antwort, nicht die
    /// eingebaute Schrift.
    fn web_pick(&self, family: u32, bold: bool, italic: bool) -> Option<Face<'_>> {
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
        best.map(|(w, _)| Face { font: &w.font, lig: &w.lig })
    }

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

    /// The regular body face — for size-agnostic estimates (line-box height
    /// around floats, intrinsic auto-sizing) where a single reference face is
    /// fine and keeps behaviour identical to the old single-font path.
    pub fn regular(&self) -> Face<'_> {
        self.regular.face()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// **Die eingebauten Gesichter haben KEINE Ligaturen** — sie sind
    /// gekuerzt (`assets/subset.sh`). Das ist keine Nebenbemerkung: daran
    /// haengt, dass `measure` seinen alten, allokationsfreien Weg nimmt,
    /// und diese Behauptung soll gemessen sein statt geglaubt.
    #[test]
    fn the_embedded_faces_carry_no_ligatures() {
        let f = Fonts::new();
        for (b, i, m) in [(false, false, false), (true, false, false), (false, true, false),
                          (true, true, false), (false, false, true), (true, false, true)] {
            assert!(f.pick(b, i, m, 0).ligatures().is_none());
        }
    }

    /// Ohne Ligaturen ist `shape` die Identitaet: eine Glyphe je Zeichen, und
    /// die Byte-Spannen decken die Zeichenkette luecken- und ueberlappungsfrei
    /// ab. Der Zeilenumbruch laeuft auf diesen Spannen.
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
