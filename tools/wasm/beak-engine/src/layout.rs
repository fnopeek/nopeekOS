//! Layout — hand-rolled block flow. Turns parsed `Block`s into a positioned
//! **display list** (`DrawOp`s) plus link hit-rects and a total height.
//!
//! Slice-0.1 is block-only: each block stacks vertically with margins; text
//! wraps to the content width using the font's real advance widths. Inline
//! flow, floats, flex/grid, and CSS come next — this is the skeleton they
//! hang on. Scroll-independent: computed once, then `raster::paint` draws the
//! visible slice at any offset.

use alloc::string::String;
use alloc::vec::Vec;
use fontdue::Font;

use crate::Block;

/// 8-bit RGB. The rasteriser converts to the buffer's BGRA at blit time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Resolved page colours for the active theme. The shell fills these from the
/// compositor palette (npk_theme_token) so the page follows light/dark like
/// the rest of the UI; `DARK` is the fallback before the query.
#[derive(Clone, Copy)]
pub struct Theme {
    pub bg: Rgb,
    pub text: Rgb,
    pub heading: Rgb,
    pub link: Rgb,
    pub muted: Rgb,
    pub rule: Rgb,
}
impl Theme {
    pub const DARK: Theme = Theme {
        bg: Rgb(24, 24, 28),
        text: Rgb(212, 212, 216),
        heading: Rgb(245, 245, 248),
        link: Rgb(96, 165, 250),
        muted: Rgb(148, 148, 154),
        rule: Rgb(58, 58, 64),
    };
}

const PAD: i32 = 20;

/// One paint instruction, positioned in document space (pre-scroll).
pub enum DrawOp {
    /// A run of already-wrapped text; `y` is the line's top.
    Text { x: i32, y: i32, size: f32, color: Rgb, text: String },
    /// A filled rectangle (divider, list bullet).
    Rect { x: i32, y: i32, w: i32, h: i32, color: Rgb },
}

/// A clickable link's document-space rectangle.
pub struct LinkRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub href: String,
}

pub struct Layout {
    pub ops: Vec<DrawOp>,
    pub links: Vec<LinkRect>,
    /// Total document height (px). May exceed the viewport → scroll.
    pub height: u32,
}

impl Layout {
    /// Link href at a document-space point (caller adds scroll to screen y).
    pub fn hit_test(&self, x: i32, y: i32) -> Option<&str> {
        self.links
            .iter()
            .find(|l| x >= l.x && x < l.x + l.w && y >= l.y && y < l.y + l.h)
            .map(|l| l.href.as_str())
    }
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 30.0,
        2 => 24.0,
        3 => 20.0,
        _ => 18.0,
    }
}

fn measure(font: &Font, s: &str, size: f32) -> f32 {
    s.chars().map(|c| font.metrics(c, size).advance_width).sum()
}

/// `ceil` for a non-negative f32 — `no_std` has no `f32::ceil`.
fn ceil_i32(x: f32) -> i32 {
    let c = x as i32;
    if (c as f32) < x { c + 1 } else { c }
}

/// Greedy word-wrap `text` into the content box, emitting one `Text` op per
/// line. Returns the y just below the last line. When `href` is set, each
/// wrapped line also gets a `LinkRect` (so multi-line links are clickable).
#[allow(clippy::too_many_arguments)]
fn flow_text(
    font: &Font,
    text: &str,
    x: i32,
    y0: i32,
    w: i32,
    size: f32,
    color: Rgb,
    ops: &mut Vec<DrawOp>,
    links: &mut Vec<LinkRect>,
    href: Option<&str>,
) -> i32 {
    let line_h = ceil_i32(
        font.horizontal_line_metrics(size)
            .map(|m| m.new_line_size)
            .unwrap_or(size * 1.3),
    );
    let space_w = font.metrics(' ', size).advance_width;

    let mut y = y0;
    let mut line = String::new();
    let mut line_w = 0.0f32;

    let mut emit = |line: &mut String, line_w: &mut f32, y: &mut i32| {
        if line.is_empty() {
            return;
        }
        ops.push(DrawOp::Text {
            x,
            y: *y,
            size,
            color,
            text: core::mem::take(line),
        });
        if let Some(h) = href {
            links.push(LinkRect {
                x,
                y: *y,
                w: ceil_i32(*line_w),
                h: line_h,
                href: String::from(h),
            });
        }
        *line_w = 0.0;
        *y += line_h;
    };

    for word in text.split_whitespace() {
        let ww = measure(font, word, size);
        let projected = if line.is_empty() { ww } else { line_w + space_w + ww };
        if !line.is_empty() && projected > w as f32 {
            emit(&mut line, &mut line_w, &mut y);
        }
        if !line.is_empty() {
            line.push(' ');
            line_w += space_w;
        }
        line.push_str(word);
        line_w += ww;
    }
    emit(&mut line, &mut line_w, &mut y);
    y
}

/// Lay a document out into a scroll-independent display list.
pub fn layout(font: &Font, blocks: &[Block], width: u32, theme: &Theme) -> Layout {
    let mut ops: Vec<DrawOp> = Vec::new();
    let mut links: Vec<LinkRect> = Vec::new();

    let cx = PAD;
    let cw = (width as i32 - 2 * PAD).max(60);
    let mut y = PAD;

    for b in blocks {
        match b {
            Block::Heading { level, text } => {
                let size = heading_size(*level);
                y += (size * 0.5) as i32;
                y = flow_text(font, text, cx, y, cw, size, theme.heading, &mut ops, &mut links, None);
                y += (size * 0.28) as i32;
            }
            Block::Para(text) => {
                y = flow_text(font, text, cx, y, cw, 16.0, theme.text, &mut ops, &mut links, None);
                y += 9;
            }
            Block::ListItem(text) => {
                ops.push(DrawOp::Rect { x: cx + 5, y: y + 9, w: 4, h: 4, color: theme.muted });
                y = flow_text(font, text, cx + 22, y, cw - 22, 16.0, theme.text, &mut ops, &mut links, None);
                y += 5;
            }
            Block::Link { text, href } => {
                y = flow_text(font, text, cx, y, cw, 16.0, theme.link, &mut ops, &mut links, Some(href));
                y += 5;
            }
            Block::Rule => {
                y += 7;
                ops.push(DrawOp::Rect { x: cx, y, w: cw, h: 1, color: theme.rule });
                y += 11;
            }
        }
    }

    y += PAD;
    Layout {
        ops,
        links,
        height: y.max(1) as u32,
    }
}
