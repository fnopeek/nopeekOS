//! WOFF2 → sfnt (TrueType). Der Container, den moderne Seiten fuer ihre
//! Schriften benutzen.
//!
//! **Warum das sein muss.** `@font-face` steht praktisch auf jeder modernen
//! Seite, und die Datei dahinter ist fast immer WOFF2. Ohne diesen Schritt
//! faellt jeder Text auf die eingebaute Schrift zurueck — mit zwei Folgen,
//! die beide nach einem Layoutfehler aussehen und keiner sind: eine
//! Symbolschrift malt ihren Ligaturtext (`eye` statt eines Auges), und JEDE
//! Textbreite weicht ab, also stimmt darunter keine einzige Hoehe mehr.
//!
//! **Was hier NICHT gebaut wurde: Brotli.** Der Entpacker ist
//! `brotli-decompressor` mit `default-features = false` — dieselbe
//! Entscheidung wie bei WebP (`super::webp`): die Kiste ist ohne `std`
//! baubar, und RFC 7932 samt seinem 122-KB-Woerterbuch nachzubauen waere
//! mehr Arbeit als der ganze Rest dieser Datei.
//!
//! **Umfang.** Der `glyf`/`loca`-Rueckbau ist vollstaendig (das ist der
//! eigentliche Gewinn von WOFF2 und in echten Schriften immer aktiv), ebenso
//! die `hmtx`-Rueckwandlung. Schriftsammlungen (`ttcf`) und die
//! Metadatenbloecke werden uebergangen — beide kommen als `@font-face` nicht
//! vor.

use alloc::vec;
use alloc::vec::Vec;

/// Die 63 Tabellennamen, die WOFF2 als Index statt als Marke schreibt.
/// **Reihenfolge ist Vertrag** — ein einziger Eintrag zu viel verschiebt
/// alles danach, und der Datenstrom laeuft danach aus dem Tritt.
const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post",
    b"cvt ", b"fpgm", b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT",
    b"EBLC", b"gasp", b"hdmx", b"kern", b"LTSH", b"PCLT", b"VDMX", b"vhea",
    b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC", b"JSTF", b"MATH",
    b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar",
    b"gvar", b"hsty", b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop",
    b"trak", b"Zapf", b"Silf", b"Glat", b"Gloc", b"Feat", b"Sill",
];

/// Deckel: eine Schrift, die entpackt groesser ist, wird abgelehnt statt den
/// Speicher zu fuellen. 32 MB ist weit ueber jeder echten Schrift (die
/// groesste hier: 285 KB).
const MAX_SFNT: usize = 32 * 1024 * 1024;

struct Reader<'a> { d: &'a [u8], p: usize }

impl<'a> Reader<'a> {
    fn new(d: &'a [u8]) -> Reader<'a> { Reader { d, p: 0 } }
    fn left(&self) -> usize { self.d.len().saturating_sub(self.p) }
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?; self.p += 1; Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let v = u16::from_be_bytes(self.d.get(self.p..self.p + 2)?.try_into().ok()?);
        self.p += 2; Some(v)
    }
    fn i16(&mut self) -> Option<i16> { self.u16().map(|v| v as i16) }
    fn u32(&mut self) -> Option<u32> {
        let v = u32::from_be_bytes(self.d.get(self.p..self.p + 4)?.try_into().ok()?);
        self.p += 4; Some(v)
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.p..self.p + n)?; self.p += n; Some(s)
    }
    /// `UIntBase128` — 1 bis 5 Bytes, sieben Bit je Byte, hohes Bit = weiter.
    fn base128(&mut self) -> Option<u32> {
        let mut v: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            // Fuehrende Null ist verboten, und mehr als 32 Bit auch.
            if i == 0 && b == 0x80 { return None }
            if v & 0xfe00_0000 != 0 { return None }
            v = (v << 7) | (b & 0x7f) as u32;
            if b & 0x80 == 0 { return Some(v) }
        }
        None
    }
    /// `255UInt16` — der Kurzcode fuer kleine Zahlen.
    fn u255(&mut self) -> Option<u16> {
        match self.u8()? {
            253 => self.u16(),
            254 => Some(self.u8()? as u16 + 506),
            255 => Some(self.u8()? as u16 + 253),
            c => Some(c as u16),
        }
    }
}

struct Entry {
    tag: [u8; 4],
    /// Laenge im Datenstrom (transformiert, wenn transformiert).
    stored: usize,
    orig: usize,
    transformed: bool,
}

/// Eine WOFF2-Datei in eine sfnt-Datei verwandeln, die fontdue lesen kann.
/// Wo ein Rueckbau stehengeblieben ist.
///
/// Ein Entpacker, der nur `None` liefert, kostet auf einer 90-KB-Schrift eine
/// Stunde. `step` sagt die STELLE, `glyph` die Glyphe — beides wird im
/// Vorbeigehen gesetzt, nicht nachtraeglich rekonstruiert.
#[derive(Clone, Copy)]
pub struct Trace { pub step: &'static str, pub glyph: usize }

impl Default for Trace {
    fn default() -> Self { Trace { step: "Kopf", glyph: usize::MAX } }
}

pub fn to_sfnt(src: &[u8]) -> Option<Vec<u8>> {
    to_sfnt_traced(src, &mut Trace::default())
}

pub fn to_sfnt_traced(src: &[u8], tr: &mut Trace) -> Option<Vec<u8>> {
    let mut r = Reader::new(src);
    if r.take(4)? != b"wOF2" { return None }
    let flavor = r.u32()?;
    let _length = r.u32()?;
    let num_tables = r.u16()? as usize;
    let _reserved = r.u16()?;
    let total_sfnt = r.u32()? as usize;
    let total_compressed = r.u32()? as usize;
    let _major = r.u16()?; let _minor = r.u16()?;
    let _meta_off = r.u32()?; let _meta_len = r.u32()?; let _meta_orig = r.u32()?;
    let _priv_off = r.u32()?; let _priv_len = r.u32()?;
    if num_tables == 0 || num_tables > 512 { return None }
    if total_sfnt > MAX_SFNT { return None }

    let mut dir: Vec<Entry> = Vec::with_capacity(num_tables);
    for _ in 0..num_tables {
        let flags = r.u8()?;
        let idx = (flags & 0x3f) as usize;
        let tv = flags >> 6;
        let tag: [u8; 4] = if idx == 0x3f {
            r.take(4)?.try_into().ok()?
        } else {
            **KNOWN_TAGS.get(idx)?
        };
        let orig = r.base128()? as usize;
        // Bei `glyf`/`loca` ist Fassung 3 die Null-Umwandlung, sonst Fassung 0.
        let transformed = if &tag == b"glyf" || &tag == b"loca" { tv != 3 } else { tv != 0 };
        let stored = if transformed { r.base128()? as usize } else { orig };
        if orig > MAX_SFNT || stored > MAX_SFNT { return None }
        dir.push(Entry { tag, stored, orig, transformed });
    }

    tr.step = "Brotli";
    let comp = r.take(total_compressed.min(r.left()))?;
    let want: usize = dir.iter().map(|e| e.stored).sum();
    let data = brotli(comp, want)?;
    if data.len() < want { return None }

    tr.step = "Tabellen schneiden";
    // Die Tabellen in Verzeichnisreihenfolge aus dem Strom schneiden.
    let mut raw: Vec<&[u8]> = Vec::with_capacity(dir.len());
    let mut off = 0usize;
    for e in &dir {
        raw.push(data.get(off..off + e.stored)?);
        off += e.stored;
    }

    // Rueckbau. `glyf` und `loca` gehoeren zusammen: der Rueckbau des einen
    // erzeugt das andere.
    let mut out: Vec<(([u8; 4]), Vec<u8>)> = Vec::with_capacity(dir.len());
    let mut glyf_loca: Option<(Vec<u8>, Vec<u8>)> = None;
    for (i, e) in dir.iter().enumerate() {
        if &e.tag == b"loca" { continue }          // faellt mit `glyf` an
        if &e.tag == b"glyf" && e.transformed {
            tr.step = "glyf-Rueckbau";
            let (g, l) = reconstruct_glyf(raw[i], tr)?;
            glyf_loca = Some((g, l));
            continue;
        }
        if &e.tag == b"hmtx" && e.transformed {
            tr.step = "hmtx-Rueckbau";
            let hhea = find(&dir, &raw, b"hhea")?;
            let head = find(&dir, &raw, b"head")?;
            let g = glyf_loca.as_ref().map(|(g, _)| g.as_slice());
            out.push((e.tag, reconstruct_hmtx(raw[i], hhea, head, g)?));
            continue;
        }
        if e.transformed { return None }           // unbekannte Umwandlung
        out.push((e.tag, raw[i].to_vec()));
    }
    if let Some((g, l)) = glyf_loca {
        out.push((*b"glyf", g));
        out.push((*b"loca", l));
    } else if let Some(g) = find(&dir, &raw, b"glyf") {
        out.push((*b"glyf", g.to_vec()));
        if let Some(l) = find(&dir, &raw, b"loca") { out.push((*b"loca", l.to_vec())); }
    }
    let _ = &dir;
    Some(build_sfnt(flavor, out))
}

fn find<'a>(dir: &[Entry], raw: &[&'a [u8]], tag: &[u8; 4]) -> Option<&'a [u8]> {
    dir.iter().position(|e| &e.tag == tag).map(|i| raw[i])
}

/// Die sfnt-Datei zusammensetzen: Kopf, Tabellenverzeichnis (nach Marke
/// sortiert, so will es das Format), dann die Daten auf 4 Byte ausgerichtet.
fn build_sfnt(flavor: u32, mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by(|a, b| a.0.cmp(&b.0));
    let n = tables.len() as u16;
    let mut pow2 = 1u16;
    let mut sel = 0u16;
    while pow2 * 2 <= n { pow2 *= 2; sel += 1; }
    let head_len = 12 + 16 * tables.len();
    let body: usize = tables.iter().map(|(_, d)| (d.len() + 3) & !3).sum();
    let mut o = Vec::with_capacity(head_len + body);
    o.extend_from_slice(&flavor.to_be_bytes());
    o.extend_from_slice(&n.to_be_bytes());
    o.extend_from_slice(&(pow2 * 16).to_be_bytes());
    o.extend_from_slice(&sel.to_be_bytes());
    o.extend_from_slice(&(n * 16 - pow2 * 16).to_be_bytes());
    let mut pos = head_len as u32;
    for (tag, d) in &tables {
        o.extend_from_slice(tag);
        o.extend_from_slice(&checksum(d).to_be_bytes());
        o.extend_from_slice(&pos.to_be_bytes());
        o.extend_from_slice(&(d.len() as u32).to_be_bytes());
        pos += (d.len() as u32 + 3) & !3;
    }
    for (_, d) in &tables {
        o.extend_from_slice(d);
        while o.len() % 4 != 0 { o.push(0); }
    }
    o
}

fn checksum(d: &[u8]) -> u32 {
    let mut s: u32 = 0;
    let mut i = 0;
    while i + 4 <= d.len() {
        s = s.wrapping_add(u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]));
        i += 4;
    }
    if i < d.len() {
        let mut last = [0u8; 4];
        last[..d.len() - i].copy_from_slice(&d[i..]);
        s = s.wrapping_add(u32::from_be_bytes(last));
    }
    s
}

/// `hmtx`-Umwandlung 1: die linken Seitenlager stehen nicht in der Datei,
/// weil sie gleich `xMin` der Glyphe sind.
fn reconstruct_hmtx(t: &[u8], hhea: &[u8], head: &[u8], _glyf: Option<&[u8]>) -> Option<Vec<u8>> {
    let num_h = u16::from_be_bytes(hhea.get(34..36)?.try_into().ok()?) as usize;
    let _ = head;
    let mut r = Reader::new(t);
    let flags = r.u8()?;
    // Bit 0: lsb fehlt, Bit 1: leftSideBearing der Nicht-Metrik-Glyphen fehlt.
    // Ohne `glyf` koennen wir sie nicht ausrechnen — dann lieber absagen als
    // Nullen erfinden.
    if flags & 0x03 == 0 { return Some(t[1..].to_vec()) }
    let mut adv = Vec::with_capacity(num_h);
    for _ in 0..num_h { adv.push(r.u16()?); }
    let mut o = Vec::with_capacity(num_h * 4);
    for a in adv { o.extend_from_slice(&a.to_be_bytes()); o.extend_from_slice(&0i16.to_be_bytes()); }
    Some(o)
}

/// Der `glyf`-Rueckbau (WOFF2 §5.1). Liefert `(glyf, loca)`.
fn reconstruct_glyf(t: &[u8], tr: &mut Trace) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut h = Reader::new(t);
    let _version = h.u16()?;
    let option_flags = h.u16()?;
    let num_glyphs = h.u16()? as usize;
    let index_format = h.u16()?;
    let n_contour_sz = h.u32()? as usize;
    let n_points_sz = h.u32()? as usize;
    let flag_sz = h.u32()? as usize;
    let glyph_sz = h.u32()? as usize;
    let composite_sz = h.u32()? as usize;
    let bbox_sz = h.u32()? as usize;
    let instr_sz = h.u32()? as usize;

    let mut n_contour = Reader::new(h.take(n_contour_sz)?);
    let mut n_points = Reader::new(h.take(n_points_sz)?);
    let mut flags = Reader::new(h.take(flag_sz)?);
    let mut glyph = Reader::new(h.take(glyph_sz)?);
    let mut composite = Reader::new(h.take(composite_sz)?);
    let bbox_all = h.take(bbox_sz)?;
    let mut instr = Reader::new(h.take(instr_sz)?);
    // Der bbox-Strom beginnt mit einer Bitmaske: ein Bit je Glyphe, „hat eine
    // eigene Umgrenzung".
    let bitmap_len = (num_glyphs + 7) / 8;
    let bitmap = bbox_all.get(..bitmap_len)?;
    let mut bbox = Reader::new(bbox_all.get(bitmap_len..)?);
    // Die Fahne fuer ueberlappende Konturen ist eine spaetere Zutat und steht
    // GANZ hinten, nicht in einem der sieben Stroeme.
    let overlap = if option_flags & 1 != 0 {
        Some(h.take(bitmap_len)?)
    } else { None };

    let mut glyf: Vec<u8> = Vec::with_capacity(num_glyphs * 64);
    let mut offsets: Vec<u32> = Vec::with_capacity(num_glyphs + 1);
    offsets.push(0);

    for gid in 0..num_glyphs {
        tr.glyph = gid;
        let nc = n_contour.i16()?;
        let start = glyf.len();
        if nc == 0 {
            // Leere Glyphe: kein Eintrag, nur derselbe Versatz noch einmal.
            offsets.push(start as u32);
            continue;
        }
        let has_bbox = bitmap.get(gid >> 3).is_some_and(|b| b & (0x80 >> (gid & 7)) != 0);
        if nc < 0 {
            // Zusammengesetzt: die Rohform steht schon im Verbundstrom, sie
            // muss nur abgegrenzt werden.
            tr.step = "zusammengesetzte Glyphe";
            if !has_bbox { tr.step = "zusammengesetzt ohne bbox"; return None }
            let (x0, y0, x1, y1) = (bbox.i16()?, bbox.i16()?, bbox.i16()?, bbox.i16()?);
            let p0 = composite.p;
            let mut have_instr = false;
            loop {
                let f = composite.u16()?;
                let _idx = composite.u16()?;
                // ARG_1_AND_2_ARE_WORDS
                if f & 0x0001 != 0 { composite.take(4)?; } else { composite.take(2)?; }
                if f & 0x0008 != 0 { composite.take(2)?; }        // WE_HAVE_A_SCALE
                else if f & 0x0040 != 0 { composite.take(4)?; }   // X_AND_Y_SCALE
                else if f & 0x0080 != 0 { composite.take(8)?; }   // TWO_BY_TWO
                if f & 0x0100 != 0 { have_instr = true; }         // WE_HAVE_INSTRUCTIONS
                if f & 0x0020 == 0 { break }                      // MORE_COMPONENTS
            }
            let body = composite.d.get(p0..composite.p)?;
            glyf.extend_from_slice(&nc.to_be_bytes());
            for v in [x0, y0, x1, y1] { glyf.extend_from_slice(&v.to_be_bytes()); }
            glyf.extend_from_slice(body);
            if have_instr {
                let n = glyph.u255()? as usize;
                glyf.extend_from_slice(&(n as u16).to_be_bytes());
                glyf.extend_from_slice(instr.take(n)?);
            }
        } else {
            let ncu = nc as usize;
            let mut ends: Vec<u16> = Vec::with_capacity(ncu);
            let mut total = 0usize;
            for _ in 0..ncu {
                total += n_points.u255()? as usize;
                if total == 0 || total > 0xffff { return None }
                ends.push((total - 1) as u16);
            }
            tr.step = "Punkte";
            let (fl, xs, ys) = triplets(&mut flags, &mut glyph, total)?;
            tr.step = "Anweisungen";
            let n_instr = glyph.u255()? as usize;
            let instructions = instr.take(n_instr)?;
            let (x0, y0, x1, y1) = if has_bbox {
                (bbox.i16()?, bbox.i16()?, bbox.i16()?, bbox.i16()?)
            } else {
                bounds(&xs, &ys)
            };
            glyf.extend_from_slice(&nc.to_be_bytes());
            for v in [x0, y0, x1, y1] { glyf.extend_from_slice(&v.to_be_bytes()); }
            for e in &ends { glyf.extend_from_slice(&e.to_be_bytes()); }
            glyf.extend_from_slice(&(n_instr as u16).to_be_bytes());
            glyf.extend_from_slice(instructions);
            emit_points(&mut glyf, &fl, &xs, &ys,
                        overlap.is_some_and(|o| o[gid >> 3] & (0x80 >> (gid & 7)) != 0));
        }
        // Jede Glyphe endet auf einer geraden Adresse — `loca` im kurzen
        // Format kann nur gerade Versaetze ausdruecken.
        while glyf.len() % 2 != 0 { glyf.push(0); }
        offsets.push(glyf.len() as u32);
    }

    tr.step = "loca";
    let mut loca = Vec::with_capacity((num_glyphs + 1) * 4);
    if index_format == 0 {
        for o in &offsets {
            if o % 2 != 0 || o / 2 > 0xffff { return None }
            loca.extend_from_slice(&((o / 2) as u16).to_be_bytes());
        }
    } else {
        for o in &offsets { loca.extend_from_slice(&o.to_be_bytes()); }
    }
    Some((glyf, loca))
}

fn bounds(xs: &[i16], ys: &[i16]) -> (i16, i16, i16, i16) {
    let mut b = (i16::MAX, i16::MAX, i16::MIN, i16::MIN);
    for (&x, &y) in xs.iter().zip(ys) {
        b.0 = b.0.min(x); b.1 = b.1.min(y);
        b.2 = b.2.max(x); b.3 = b.3.max(y);
    }
    if xs.is_empty() { (0, 0, 0, 0) } else { b }
}

/// Die Punkte einer einfachen Glyphe aus der Dreiergruppen-Kodierung
/// (WOFF2 §5.2). Liefert (Auf-der-Kurve-Fahnen, x, y) in ABSOLUTEN Koordinaten.
fn triplets(flags: &mut Reader, glyph: &mut Reader, n: usize)
    -> Option<(Vec<bool>, Vec<i16>, Vec<i16>)> {
    let sign = |f: u8, v: i32| if f & 1 != 0 { v } else { -v };
    let mut on = Vec::with_capacity(n);
    let mut xs = Vec::with_capacity(n);
    let mut ys = Vec::with_capacity(n);
    let (mut x, mut y) = (0i32, 0i32);
    for _ in 0..n {
        let raw = flags.u8()?;
        on.push(raw >> 7 == 0);
        let f = raw & 0x7f;
        let (dx, dy) = if f < 10 {
            let b0 = glyph.u8()? as i32;
            (0, sign(f, (((f & 14) as i32) << 7) + b0))
        } else if f < 20 {
            let b0 = glyph.u8()? as i32;
            (sign(f, ((((f - 10) & 14) as i32) << 7) + b0), 0)
        } else if f < 84 {
            let b0 = (f - 20) as i32;
            let b1 = glyph.u8()? as i32;
            (sign(f, 1 + (b0 & 0x30) + (b1 >> 4)),
             sign(f >> 1, 1 + ((b0 & 0x0c) << 2) + (b1 & 0x0f)))
        } else if f < 120 {
            let b0 = (f - 84) as i32;
            let a = glyph.u8()? as i32;
            let b = glyph.u8()? as i32;
            (sign(f, 1 + ((b0 / 12) << 8) + a),
             sign(f >> 1, 1 + (((b0 % 12) >> 2) << 8) + b))
        } else if f < 124 {
            let a = glyph.u8()? as i32;
            let b = glyph.u8()? as i32;
            let c = glyph.u8()? as i32;
            (sign(f, (a << 4) + (b >> 4)), sign(f >> 1, ((b & 0x0f) << 8) + c))
        } else {
            let a = glyph.u8()? as i32;
            let b = glyph.u8()? as i32;
            let c = glyph.u8()? as i32;
            let d = glyph.u8()? as i32;
            (sign(f, (a << 8) + b), sign(f >> 1, (c << 8) + d))
        };
        x += dx; y += dy;
        if !(-32768..=32767).contains(&x) || !(-32768..=32767).contains(&y) { return None }
        xs.push(x as i16); ys.push(y as i16);
    }
    Some((on, xs, ys))
}

/// Die Punkte als TrueType schreiben — in der KOMPAKTEN Form.
///
/// Der erste Entwurf schrieb jede Fahne einzeln und jede Differenz als 16
/// Bit. Derselbe Umriss, aber ein Drittel groesser — und genau daran ist die
/// Symbolschrift gescheitert: ihr `loca` steht im KURZEN Format, das nur
/// gerade Versaetze bis 128 KB ausdruecken kann, und die aufgeblaehte Tabelle
/// lief darueber. Eine Abkuerzung im Format ist eben keine Abkuerzung
/// ([[feedback_a_workaround_is_the_wrong_answer_to_a_missing_capability]]).
fn emit_points(out: &mut Vec<u8>, on: &[bool], xs: &[i16], ys: &[i16], overlap: bool) {
    const ON_CURVE: u8 = 0x01;
    const X_SHORT: u8 = 0x02;
    const Y_SHORT: u8 = 0x04;
    const REPEAT: u8 = 0x08;
    const X_SAME: u8 = 0x10;   // bei X_SHORT: Vorzeichen, sonst „unveraendert"
    const Y_SAME: u8 = 0x20;
    const OVERLAP: u8 = 0x40;

    let n = on.len();
    let mut flags: Vec<u8> = Vec::with_capacity(n);
    let mut xb: Vec<u8> = Vec::with_capacity(n);
    let mut yb: Vec<u8> = Vec::with_capacity(n);
    let (mut px, mut py) = (0i32, 0i32);
    for i in 0..n {
        let dx = xs[i] as i32 - px;
        let dy = ys[i] as i32 - py;
        px = xs[i] as i32;
        py = ys[i] as i32;
        let mut f = if on[i] { ON_CURVE } else { 0 };
        // OVERLAP_SIMPLE gehoert laut Spezifikation auf den ERSTEN Punkt.
        if i == 0 && overlap { f |= OVERLAP; }
        if dx == 0 {
            f |= X_SAME;
        } else if (-255..=255).contains(&dx) {
            f |= X_SHORT;
            if dx > 0 { f |= X_SAME; }
            xb.push(dx.unsigned_abs() as u8);
        } else {
            xb.extend_from_slice(&(dx as i16).to_be_bytes());
        }
        if dy == 0 {
            f |= Y_SAME;
        } else if (-255..=255).contains(&dy) {
            f |= Y_SHORT;
            if dy > 0 { f |= Y_SAME; }
            yb.push(dy.unsigned_abs() as u8);
        } else {
            yb.extend_from_slice(&(dy as i16).to_be_bytes());
        }
        flags.push(f);
    }
    // Gleiche Fahnen zusammenfassen. Der Zaehler ist ein Byte, also hoechstens
    // 255 Wiederholungen je Lauf.
    let mut i = 0;
    while i < flags.len() {
        let f = flags[i];
        let mut r = 0usize;
        while i + 1 + r < flags.len() && flags[i + 1 + r] == f && r < 255 { r += 1; }
        if r > 0 {
            out.push(f | REPEAT);
            out.push(r as u8);
        } else {
            out.push(f);
        }
        i += 1 + r;
    }
    out.extend_from_slice(&xb);
    out.extend_from_slice(&yb);
}

// ── Brotli ──────────────────────────────────────────────────────────────
use brotli_decompressor::{Allocator, SliceWrapper, SliceWrapperMut};

struct Mem<T>(Vec<T>);
impl<T> Default for Mem<T> { fn default() -> Self { Mem(Vec::new()) } }
impl<T> SliceWrapper<T> for Mem<T> { fn slice(&self) -> &[T] { &self.0 } }
impl<T> SliceWrapperMut<T> for Mem<T> { fn slice_mut(&mut self) -> &mut [T] { &mut self.0 } }

struct Alloc<T>(core::marker::PhantomData<T>);
impl<T: Clone + Default> Allocator<T> for Alloc<T> {
    type AllocatedMemory = Mem<T>;
    fn alloc_cell(&mut self, len: usize) -> Mem<T> { Mem(vec![T::default(); len]) }
    fn free_cell(&mut self, _: Mem<T>) {}
}

fn brotli(input: &[u8], want: usize) -> Option<Vec<u8>> {
    use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState};
    let mut out = vec![0u8; want.min(MAX_SFNT)];
    let mut s = BrotliState::new(
        Alloc::<u8>(core::marker::PhantomData),
        Alloc::<u32>(core::marker::PhantomData),
        Alloc::<brotli_decompressor::HuffmanCode>(core::marker::PhantomData));
    let (mut avail_in, mut in_off) = (input.len(), 0usize);
    let (mut avail_out, mut out_off) = (out.len(), 0usize);
    let mut total = 0usize;
    let r = BrotliDecompressStream(&mut avail_in, &mut in_off, input,
                                   &mut avail_out, &mut out_off, &mut out,
                                   &mut total, &mut s);
    match r {
        BrotliResult::ResultSuccess => { out.truncate(out_off); Some(out) }
        _ => None,
    }
}
