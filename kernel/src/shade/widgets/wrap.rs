//! Soft wrap for `Widget::TextArea`: the visual rows a document breaks
//! into. Layout, render, caret-follow, hit test and the arrow keys all ask
//! this one function, so the row a caret is drawn on is the row a click or
//! an arrow key resolves to.

use alloc::vec::Vec;

/// One visual row: `text[start..end]`, without the `\n`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub start: usize,
    pub end:   usize,
    /// Index of the `\n`-separated line the row belongs to.
    pub line:  usize,
    /// The row a line begins with; the others continue it.
    pub first: bool,
}

/// Columns narrower than this many glyphs do not wrap. The text comes
/// from a module, and one row per character of a large document would be
/// that many rows held in the kernel.
const MIN_WRAP_GLYPHS: f32 = 4.0;

/// The rows of `text` in the Mono face at `px`: one per line, or with
/// `width` set, each line broken so no row is wider than `width`.
pub fn rows(text: &str, width: Option<u32>, px: u16) -> Vec<Row> {
    let Some(w) = width else { return rows_with(text, None, &|_| 0.0) };
    crate::gui::text::with_advance(super::abi::TextStyle::Mono, px, |adv| {
        let wrap = (w as f32 >= MIN_WRAP_GLYPHS * adv('0')).then_some(w);
        // The text is rewrapped on every keystroke, several times; ASCII
        // advances come from a table instead of the font each time.
        let mut ascii = [0.0f32; 128];
        for (i, a) in ascii.iter_mut().enumerate() { *a = adv(i as u8 as char); }
        let fast = |c: char| if (c as u32) < 128 { ascii[c as usize] } else { adv(c) };
        rows_with(text, wrap, &fast)
    }).unwrap_or_else(|| rows_with(text, None, &|_| 0.0))
}

/// `rows` with the character advance passed in.
///
/// A line breaks after the last space or tab that still fits, and in the
/// middle of a word only when the word alone is wider than `width`. Spaces
/// at a break stay on the upper row and may hang past the edge. Every
/// row holds at least one character, so a column narrower than a glyph
/// still terminates.
pub fn rows_with(text: &str, width: Option<u32>, adv: &dyn Fn(char) -> f32) -> Vec<Row> {
    let mut out = Vec::new();
    let mut base = 0usize;
    for (li, line) in text.split('\n').enumerate() {
        match width {
            None => out.push(Row { start: base, end: base + line.len(), line: li, first: true }),
            Some(w) => wrap_line(line, base, li, w as f32, adv, &mut out),
        }
        base += line.len() + 1;
    }
    out
}

fn wrap_line(line: &str, base: usize, li: usize, w: f32, adv: &dyn Fn(char) -> f32,
             out: &mut Vec<Row>) {
    let mut row_start = 0usize;
    let mut x = 0.0f32;
    // Byte just past the last space or tab in the current row, and the
    // width up to it.
    let mut brk: Option<(usize, f32)> = None;
    let mut first = true;
    for (b, ch) in line.char_indices() {
        let a = adv(ch);
        // A space that overflows hangs past the edge and ends the row, so
        // the word before it stays where it fits.
        if (ch == ' ' || ch == '\t') && x + a > w && b > row_start {
            let cut = b + ch.len_utf8();
            out.push(Row { start: base + row_start, end: base + cut, line: li, first });
            first = false;
            row_start = cut;
            x = 0.0;
            brk = None;
            continue;
        }
        // Twice at most: a cut after a space can leave a row that still
        // does not take this character, and then it starts a row itself.
        while x + a > w && b > row_start {
            let (cut, cut_x) = match brk {
                Some((c, cx)) if c > row_start && c <= b => (c, cx),
                _ => (b, x),
            };
            out.push(Row { start: base + row_start, end: base + cut, line: li, first });
            first = false;
            row_start = cut;
            x -= cut_x;
            brk = None;
        }
        x += a;
        if ch == ' ' || ch == '\t' { brk = Some((b + ch.len_utf8(), x)); }
    }
    // An empty line is one row; a line that ended on a hanging space has
    // no empty row after it.
    if first || row_start < line.len() {
        out.push(Row { start: base + row_start, end: base + line.len(), line: li, first });
    }
}

/// Index of the row the caret at byte `cursor` is drawn on. A caret on a
/// break belongs to the row that starts there.
pub fn row_of(rows: &[Row], cursor: usize) -> usize {
    rows.partition_point(|r| r.start <= cursor).saturating_sub(1)
}

/// Whether row `i` is followed by a row continuing the same line. Its end
/// is then the next row's start, and a caret placed there would show on
/// the next row.
pub fn continues(rows: &[Row], i: usize) -> bool {
    rows.get(i + 1).is_some_and(|n| !n.first)
}
