//! WOFF 1.0 → sfnt. Der Vorgaenger von WOFF2, und er ist NICHT ausgestorben.
//!
//! Hier stand ein `return false` mit der Begruendung, die Fassung sei
//! „praktisch ausgestorben". Gemessen: arcade.ch liefert alle vier Gesichter
//! seiner Hausschrift als WOFF1, und ohne sie faellt die ganze Seite auf die
//! eingebaute Schrift zurueck — also stimmt darunter keine einzige Breite und
//! keine einzige Hoehe mehr ([[feedback_a_comment_that_names_its_condition_expires]]).
//! Jeder Baukasten, der noch `.woff` neben `.woff2` ausliefert, trifft uns
//! genauso, sobald `@font-face` beide Formate anbietet und wir das zweite
//! nicht koennen.
//!
//! **Warum das kurz ist.** WOFF2 muss `glyf`/`loca` zurueckbauen und bringt
//! Brotli mit; WOFF1 tut nichts dergleichen. Es ist das sfnt selbst, Tabelle
//! fuer Tabelle mit zlib gepackt (RFC 1950) — und `miniz_oxide` liegt fuer PNG
//! ohnehin im Baum. Es bleibt: Kopf lesen, Verzeichnis lesen, entpacken,
//! sfnt wieder zusammensetzen.
//!
//! W3C WOFF File Format 1.0, §3 (header) und §4 (table directory).

use alloc::vec;
use alloc::vec::Vec;

/// Derselbe Deckel wie in `woff2` — eine Schrift, die entpackt groesser ist,
/// wird abgelehnt, statt den Speicher zu fuellen.
const MAX_SFNT: usize = 32 * 1024 * 1024;

/// Kopf (44 B) + je 20 B Verzeichniseintrag.
const HDR: usize = 44;
const DIR_ENTRY: usize = 20;

fn u16at(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(p..p + 2)?.try_into().ok()?))
}
fn u32at(d: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(p..p + 4)?.try_into().ok()?))
}

pub fn looks_like_woff(b: &[u8]) -> bool {
    b.starts_with(b"wOFF")
}

/// `wOFF` → sfnt, oder `None`, wenn der Container nicht haelt, was sein Kopf
/// sagt.
pub fn to_sfnt(d: &[u8]) -> Option<Vec<u8>> {
    if !looks_like_woff(d) {
        return None;
    }
    let flavor = u32at(d, 4)?;
    let num = u16at(d, 12)? as usize;
    let total = u32at(d, 16)? as usize;
    if num == 0 || total > MAX_SFNT {
        return None;
    }
    // `totalSfntSize` ist eine ANGABE der Datei, keine Messung. Sie wird
    // benutzt, um EINMAL zu reservieren, und danach nicht mehr geglaubt: die
    // Groesse, die zaehlt, ist die aufaddierte der Tabellen.
    let mut tables: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut sum = 0usize;
    for i in 0..num {
        let p = HDR + i * DIR_ENTRY;
        let tag = u32at(d, p)?;
        let off = u32at(d, p + 4)? as usize;
        let comp = u32at(d, p + 8)? as usize;
        let orig = u32at(d, p + 12)? as usize;
        let end = off.checked_add(comp)?;
        if end > d.len() || orig > MAX_SFNT {
            return None;
        }
        sum = sum.checked_add((orig + 3) & !3)?;
        if sum > MAX_SFNT {
            return None;
        }
        let src = &d[off..end];
        // §4: `compLength == origLength` heisst UNGEPACKT. Alles andere ist
        // zlib. Ein Entpacker auf rohe Tabellenbytes losgelassen scheitert
        // sonst an `head`, das fast immer unkomprimiert daliegt.
        let raw = if comp >= orig {
            src[..orig.min(src.len())].to_vec()
        } else {
            let out = miniz_oxide::inflate::decompress_to_vec_zlib(src).ok()?;
            if out.len() != orig {
                return None;
            }
            out
        };
        if raw.len() != orig {
            return None;
        }
        tables.push((tag, raw));
    }
    // Das sfnt-Verzeichnis ist nach Marke SORTIERT — die Spezifikation
    // verlangt das auch von der WOFF-Datei, aber eine Datei ist keine Zusage.
    tables.sort_by_key(|(t, _)| *t);

    let dir_len = 12 + num * 16;
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve(dir_len + sum).ok()?;
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(num as u16).to_be_bytes());
    // searchRange / entrySelector / rangeShift — die drei Suchhilfen aus dem
    // sfnt-Kopf. fontdue liest sie nicht, ein anderer Leser schon.
    let mut sel = 0u16;
    while (1usize << (sel + 1)) <= num {
        sel += 1;
    }
    let range = (1u16 << sel) * 16;
    out.extend_from_slice(&range.to_be_bytes());
    out.extend_from_slice(&sel.to_be_bytes());
    out.extend_from_slice(&((num as u16) * 16 - range).to_be_bytes());

    let mut off = dir_len;
    for (tag, raw) in &tables {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&checksum(raw).to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_be_bytes());
        off += (raw.len() + 3) & !3;
    }
    for (_, raw) in &tables {
        out.extend_from_slice(raw);
        out.extend_from_slice(&vec![0u8; ((raw.len() + 3) & !3) - raw.len()]);
    }
    Some(out)
}

/// sfnt-Pruefsumme: die u32 der Tabelle aufaddiert, mit Nullen aufgefuellt.
///
/// Neu gerechnet und nicht aus der WOFF-Datei uebernommen — dort steht die
/// des ORIGINALS, und wenn die Datei sich irrt, faellt der Fehler sonst
/// einem Leser vor die Fuesse, der die Summe prueft.
fn checksum(d: &[u8]) -> u32 {
    let mut sum = 0u32;
    let mut i = 0;
    while i < d.len() {
        let mut w = [0u8; 4];
        let n = (d.len() - i).min(4);
        w[..n].copy_from_slice(&d[i..i + n]);
        sum = sum.wrapping_add(u32::from_be_bytes(w));
        i += 4;
    }
    sum
}
