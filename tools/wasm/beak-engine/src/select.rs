//! Text selection on the page: copy and find.
//!
//! The display list is a sequence of `DrawOp::Text`, each with position, size,
//! font choice and text, so "which character is under this point" is a
//! computation, and the same computation in reverse yields the highlight
//! rectangles. Measuring needs the faces held by `Engine`, so the public
//! interface lives there (`Engine::text_pos_at` and neighbours); this module
//! holds the arithmetic.
//!
//! Ordering follows the display list, which for text is document order. It is
//! paint order, not reading order (an absolutely positioned box is painted
//! later but may sit higher); for ordinary flowing text the two agree.

use alloc::string::String;
use alloc::vec::Vec;

use crate::fonts::Fonts;
use crate::layout::{DrawOp, Layout};

/// A position in the page text: index of the text draw op and byte offset in it.
///
/// `Ord` compares the op first, then the byte. A range is never stored
/// reversed; it is sorted on use, so dragging left selects the same as
/// dragging right.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct TextPos {
    pub op: u32,
    pub off: u32,
}

/// A text draw op, prepared: its box and what is needed to measure it.
struct Run<'a> {
    idx: u32,
    x: i32,
    y: i32,
    text: &'a str,
    size: f32,
    bold: bool,
    italic: bool,
    mono: bool,
    family: u32,
    sp: (f32, f32),
}

fn runs(lay: &Layout) -> Vec<Run<'_>> {
    let mut out = Vec::new();
    for (i, op) in lay.ops.iter().enumerate() {
        if let DrawOp::Text { x, y, size, bold, italic, mono, family, sp, text, .. } = op {
            if text.is_empty() { continue }
            out.push(Run {
                idx: i as u32, x: *x, y: *y, text, size: *size,
                bold: *bold, italic: *italic, mono: *mono, family: *family, sp: *sp,
            });
        }
    }
    out
}

/// Width of a substring in this run.
fn width(fonts: &Fonts, r: &Run, s: &str) -> f32 {
    let face = fonts.pick(r.bold, r.italic, r.mono, r.family);
    crate::layout::measure_sp_pub(face, s, r.size, r.sp)
}

fn height(fonts: &Fonts, r: &Run) -> i32 {
    let face = fonts.pick(r.bold, r.italic, r.mono, r.family);
    libm::ceilf(crate::layout::line_gap_pub(face, r.size)) as i32
}

/// The text position under `(x, y)` in document coordinates.
///
/// If the point hits no run, the nearest one is taken: first vertically
/// (which line), then horizontally (which end). Dragging depends on this:
/// past the right edge means "to end of line", and the gap between two
/// paragraphs means one of them.
pub fn text_pos_at(fonts: &Fonts, lay: &Layout, x: i32, y: i32) -> Option<TextPos> {
    let rs = runs(lay);
    if rs.is_empty() { return None }

    // Vertical distance of a line to the pointer; 0 means the point is inside.
    let vdist = |r: &Run| -> i32 {
        let h = height(fonts, r).max(1);
        if y < r.y { r.y - y } else if y >= r.y + h { y - (r.y + h) + 1 } else { 0 }
    };
    let best = rs.iter().map(vdist).min()?;
    // All runs of this line, in display order.
    let line: Vec<&Run> = rs.iter().filter(|r| vdist(r) == best).collect();

    // Horizontally: the run containing the point, else the last one starting
    // before it, else the first.
    let mut chosen = line[0];
    for r in &line {
        let w = libm::ceilf(width(fonts, r, r.text)) as i32;
        if x >= r.x && x < r.x + w { chosen = r; break }
        if r.x <= x { chosen = r }
    }
    Some(TextPos { op: chosen.idx, off: byte_at(fonts, chosen, x) })
}

/// The byte at which the point lies in this run.
///
/// Rounded to the nearest character boundary: pointing at the left half of a
/// glyph means before it, as in every editor; otherwise the first character
/// of a line could not be selected.
fn byte_at(fonts: &Fonts, r: &Run, x: i32) -> u32 {
    let rel = (x - r.x) as f32;
    if rel <= 0.0 { return 0 }
    let mut prev_w = 0.0f32;
    for (b, c) in r.text.char_indices() {
        let upto = b + c.len_utf8();
        let w = width(fonts, r, &r.text[..upto]);
        if rel < (prev_w + w) / 2.0 { return b as u32 }
        if rel < w { return upto as u32 }
        prev_w = w;
    }
    r.text.len() as u32
}

/// The rectangles highlighting the range, in document coordinates.
pub fn selection_rects(fonts: &Fonts, lay: &Layout, a: TextPos, b: TextPos)
    -> Vec<(i32, i32, i32, i32)>
{
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let mut out = Vec::new();
    for r in runs(lay) {
        if r.idx < a.op || r.idx > b.op { continue }
        let lo = if r.idx == a.op { a.off as usize } else { 0 };
        let hi = if r.idx == b.op { b.off as usize } else { r.text.len() };
        let (lo, hi) = (lo.min(r.text.len()), hi.min(r.text.len()));
        if hi <= lo { continue }
        if !r.text.is_char_boundary(lo) || !r.text.is_char_boundary(hi) { continue }
        let x0 = r.x + libm::roundf(width(fonts, &r, &r.text[..lo])) as i32;
        let w = (libm::roundf(width(fonts, &r, &r.text[..hi])) as i32
                 - libm::roundf(width(fonts, &r, &r.text[..lo])) as i32).max(1);
        out.push((x0, r.y, w, height(fonts, &r).max(1)));
    }
    out
}

/// The selected text.
///
/// Runs on different lines are joined with a newline, runs on the same line
/// with nothing: a paragraph consists of several runs (bold, italic, links)
/// and they carry their own spaces, so inserting a separator would be wrong.
pub fn selected_text(lay: &Layout, a: TextPos, b: TextPos) -> String {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let mut out = String::new();
    let mut last_y: Option<i32> = None;
    for r in runs(lay) {
        if r.idx < a.op || r.idx > b.op { continue }
        let lo = if r.idx == a.op { a.off as usize } else { 0 };
        let hi = if r.idx == b.op { b.off as usize } else { r.text.len() };
        let (lo, hi) = (lo.min(r.text.len()), hi.min(r.text.len()));
        if hi <= lo { continue }
        if !r.text.is_char_boundary(lo) || !r.text.is_char_boundary(hi) { continue }
        if last_y.is_some_and(|ly| ly != r.y) { out.push('\n'); }
        out.push_str(&r.text[lo..hi]);
        last_y = Some(r.y);
    }
    out
}

/// All case-insensitive occurrences of `needle` in the page text, as ranges
/// that `selection_rects` can highlight.
///
/// Limit: matches are found within a single run only; a word that wraps or
/// changes style mid-word is not found.
pub fn find_all(lay: &Layout, needle: &str) -> Vec<(TextPos, TextPos)> {
    let mut out = Vec::new();
    if needle.is_empty() { return out }
    let low_needle = needle.to_lowercase();
    for r in runs(lay) {
        // Lowercasing can change the byte length (İ → i̇), and then the match
        // offsets would be wrong. Search the lowercased text only if its length is
        // unchanged; otherwise compare character by character.
        let low = r.text.to_lowercase();
        if low.len() == r.text.len() {
            let mut from = 0usize;
            while let Some(i) = low[from..].find(&low_needle) {
                let s = from + i;
                let e = s + low_needle.len();
                if r.text.is_char_boundary(s) && r.text.is_char_boundary(e) {
                    out.push((TextPos { op: r.idx, off: s as u32 },
                              TextPos { op: r.idx, off: e as u32 }));
                }
                from = s + 1;
                if from >= low.len() { break }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    fn lay(html: &str) -> (Engine, Layout) {
        let eng = Engine::new();
        let l = eng.layout(html, 800);
        (eng, l)
    }

    /// The point hits the character it points at, and the round trip yields the
    /// same text.
    #[test]
    fn a_point_hits_the_character_under_it() {
        let (eng, l) = lay("<body><p>Hallo Welt</p></body>");
        // Far left in the paragraph: before the H.
        let runs = super::runs(&l);
        let r = &runs[0];
        assert_eq!(r.text, "Hallo Welt");
        let a = eng.text_pos_at(&l, r.x, r.y + 4).expect("Anfang");
        assert_eq!(a.off, 0, "links vom ersten Buchstaben ist Offset 0");
        // Far right: after the last character.
        let b = eng.text_pos_at(&l, r.x + 10_000, r.y + 4).expect("Ende");
        assert_eq!(b.off as usize, r.text.len(), "rechts vom letzten ist das Ende");
        assert_eq!(eng.selected_text(&l, a, b), "Hallo Welt");
    }

    /// A range dragged backwards is the same range.
    #[test]
    fn dragging_backwards_selects_the_same_thing() {
        let (eng, l) = lay("<body><p>Hallo Welt</p></body>");
        let r = &super::runs(&l)[0];
        let a = eng.text_pos_at(&l, r.x, r.y + 4).unwrap();
        let b = eng.text_pos_at(&l, r.x + 10_000, r.y + 4).unwrap();
        assert_eq!(eng.selected_text(&l, a, b), eng.selected_text(&l, b, a));
        assert_eq!(eng.selection_rects(&l, a, b).len(), eng.selection_rects(&l, b, a).len());
    }

    /// Across two paragraphs there is a newline in between, but not between
    /// two runs on the same line.
    #[test]
    fn a_line_break_only_between_lines() {
        let (eng, l) = lay("<body><p>eins</p><p>zwei</p></body>");
        let rs = super::runs(&l);
        assert!(rs.len() >= 2, "zwei Absaetze, zwei Laeufe");
        let a = TextPos { op: rs[0].idx, off: 0 };
        let b = TextPos { op: rs[1].idx, off: rs[1].text.len() as u32 };
        assert_eq!(eng.selected_text(&l, a, b), "eins\nzwei");

        // Bold in mid-sentence: one paragraph, several runs, no newline.
        let (eng2, l2) = lay("<body><p>a<b>b</b>c</p></body>");
        let rs2 = super::runs(&l2);
        let a2 = TextPos { op: rs2[0].idx, off: 0 };
        let last = rs2.last().unwrap();
        let b2 = TextPos { op: last.idx, off: last.text.len() as u32 };
        let got = eng2.selected_text(&l2, a2, b2);
        assert!(!got.contains('\n'), "eine Zeile, kein Umbruch: {got:?}");
    }

    /// The highlight lies on the text, not beside it.
    #[test]
    fn the_highlight_covers_the_text_it_names() {
        let (eng, l) = lay("<body><p>Hallo Welt</p></body>");
        let r = &super::runs(&l)[0];
        let a = TextPos { op: r.idx, off: 0 };
        let b = TextPos { op: r.idx, off: 5 };   // "Hallo"
        let rects = eng.selection_rects(&l, a, b);
        assert_eq!(rects.len(), 1);
        let (x, y, w, h) = rects[0];
        assert_eq!(x, r.x, "faengt am Lauf an");
        assert_eq!(y, r.y);
        assert!(h > 0 && w > 0, "{w}x{h}");
        // "Hallo" is shorter than "Hallo Welt".
        let all = eng.selection_rects(&l, a, TextPos { op: r.idx, off: r.text.len() as u32 });
        assert!(w < all[0].2, "der Teil ist schmaler als das Ganze: {w} vs {}", all[0].2);
    }

    /// Search is case-insensitive and finds every occurrence.
    #[test]
    fn find_is_case_blind_and_finds_every_hit() {
        let (eng, l) = lay("<body><p>Welt welt WELT</p></body>");
        let hits = eng.find_all(&l, "welt");
        assert_eq!(hits.len(), 3, "drei Schreibweisen, drei Treffer");
        for (a, b) in &hits {
            assert_eq!(eng.selected_text(&l, *a, *b).to_lowercase(), "welt");
        }
        assert!(eng.find_all(&l, "").is_empty(), "nichts zu suchen, nichts zu finden");
        assert!(eng.find_all(&l, "gibtesnicht").is_empty());
    }

    /// An umlaut is two bytes; a selection must never split it.
    #[test]
    fn a_selection_never_splits_a_character() {
        let (eng, l) = lay("<body><p>Grüezi wohl</p></body>");
        let r = &super::runs(&l)[0];
        // Probe every pixel of the run: each offset must be a character boundary,
        // otherwise `selected_text` cuts into a character.
        let w = super::width(&eng_fonts(&eng), r, r.text) as i32;
        for x in r.x..=(r.x + w + 4) {
            let p = eng.text_pos_at(&l, x, r.y + 4).unwrap();
            assert!(r.text.is_char_boundary(p.off as usize),
                    "Offset {} bei x={} ist keine Zeichengrenze in {:?}", p.off, x, r.text);
        }
    }

    fn eng_fonts(e: &Engine) -> core::cell::Ref<'_, crate::fonts::Fonts> { e.fonts_ref() }
}
