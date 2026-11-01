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

/// Columns narrower than this many glyphs do not wrap.
const MIN_WRAP_GLYPHS: f32 = 4.0;

/// Texts longer than this do not wrap. The text comes from a module, and
/// a wrapped text is a list of rows held in the kernel; this bounds it.
pub const MAX_WRAP_BYTES: usize = 256 * 1024;

/// The rows of a text. Unwrapped they are its lines, counted and walked in
/// the text itself; only a wrapped text, which is bounded, holds a list.
/// The text is passed to every call rather than borrowed, so a caller can
/// hold the rows and edit the text's owner.
pub enum Rows {
    Lines(usize),
    Wrapped(Vec<Row>),
}

impl Rows {
    pub fn len(&self) -> usize {
        match self {
            Rows::Lines(n) => *n,
            Rows::Wrapped(v) => v.len(),
        }
    }

    /// Row `i`, clamped to the last one.
    pub fn get(&self, text: &str, i: usize) -> Row {
        match self {
            Rows::Wrapped(v) => v[i.min(v.len() - 1)],
            Rows::Lines(n) => {
                let i = i.min(n.saturating_sub(1));
                let mut start = 0usize;
                for (li, line) in text.split('\n').enumerate() {
                    if li == i { return Row { start, end: start + line.len(), line: li, first: true }; }
                    start += line.len() + 1;
                }
                Row { start: text.len(), end: text.len(), line: i, first: true }
            }
        }
    }

    /// Index of the row the caret at byte `cursor` is drawn on. A caret on
    /// a break belongs to the row that starts there.
    pub fn row_of(&self, text: &str, cursor: usize) -> usize {
        match self {
            Rows::Lines(_) => text.as_bytes()[..cursor.min(text.len())].iter().filter(|&&b| b == b'\n').count(),
            Rows::Wrapped(v) => v.partition_point(|r| r.start <= cursor).saturating_sub(1),
        }
    }

    /// Whether row `i` is followed by a row continuing the same line. Its
    /// end is then the next row's start, and a caret placed there would
    /// show on the next row.
    pub fn continues(&self, i: usize) -> bool {
        match self {
            Rows::Lines(_) => false,
            Rows::Wrapped(v) => v.get(i + 1).is_some_and(|n| !n.first),
        }
    }

    /// Rows `skip..skip + take` with their indices: what a viewport shows.
    pub fn window(&self, text: &str, skip: usize, take: usize) -> Vec<(usize, Row)> {
        match self {
            Rows::Wrapped(v) => v.iter().copied().enumerate().skip(skip).take(take).collect(),
            Rows::Lines(_) => {
                let mut out = Vec::with_capacity(take.min(256));
                let mut start = 0usize;
                for (li, line) in text.split('\n').enumerate() {
                    if li >= skip + take { break; }
                    if li >= skip {
                        out.push((li, Row { start, end: start + line.len(), line: li, first: true }));
                    }
                    start += line.len() + 1;
                }
                out
            }
        }
    }
}

/// The rows of `text` in the Mono face at `px`: its lines, or with `width`
/// set, each line broken so no row is wider than `width`.
pub fn rows(text: &str, width: Option<u32>, px: u16) -> Rows {
    let lines = || Rows::Lines(text.split('\n').count());
    let Some(w) = width else { return lines() };
    if text.len() > MAX_WRAP_BYTES { return lines(); }
    crate::gui::text::with_advance(super::abi::TextStyle::Mono, px, |adv| {
        let wrap = (w as f32 >= MIN_WRAP_GLYPHS * adv('0')).then_some(w);
        // The text is rewrapped on every keystroke, several times; ASCII
        // advances come from a table instead of the font each time.
        let mut ascii = [0.0f32; 128];
        for (i, a) in ascii.iter_mut().enumerate() { *a = adv(i as u8 as char); }
        let fast = |c: char| if (c as u32) < 128 { ascii[c as usize] } else { adv(c) };
        match wrap {
            Some(_) => Rows::Wrapped(rows_with(text, wrap, &fast)),
            None => lines(),
        }
    }).unwrap_or_else(lines)
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
