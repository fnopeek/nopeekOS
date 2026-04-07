//! Shade Terminal — text buffer for rendering intent loop output inside windows.
//!
//! When shade is active, kprintln output is captured here and rendered
//! as text content inside the focused terminal window.

use core::sync::atomic::{AtomicBool, Ordering};

/// Maximum lines and columns in the terminal buffer.
const MAX_LINES: usize = 200;
const MAX_COLS: usize = 256;

/// Terminal text buffer.
pub struct TerminalBuffer {
    lines: [[u8; MAX_COLS]; MAX_LINES],
    lens: [usize; MAX_LINES],
    /// Total lines written (wraps in ring buffer).
    total: usize,
    /// Current cursor column.
    col: usize,
    /// View scroll offset (lines from bottom).
    pub scroll_offset: usize,
}

impl TerminalBuffer {
    pub const fn new() -> Self {
        TerminalBuffer {
            lines: [[0; MAX_COLS]; MAX_LINES],
            lens: [0; MAX_LINES],
            total: 0,
            col: 0,
            scroll_offset: 0,
        }
    }

    /// Write a string to the terminal buffer.
    pub fn write_str(&mut self, s: &str) {
        for &byte in s.as_bytes() {
            self.write_byte(byte);
        }
    }

    /// Write a single byte.
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.total += 1;
                self.col = 0;
                let idx = self.total % MAX_LINES;
                self.lens[idx] = 0;
                // Clear next line
                self.lines[idx] = [0; MAX_COLS];
            }
            b'\r' => {
                self.col = 0;
            }
            0x08 => {
                // Backspace
                if self.col > 0 {
                    self.col -= 1;
                    let idx = self.total % MAX_LINES;
                    self.lines[idx][self.col] = b' ';
                    self.lens[idx] = self.col;
                }
            }
            byte if byte >= 0x20 && byte < 0x7F => {
                let idx = self.total % MAX_LINES;
                if self.col < MAX_COLS {
                    self.lines[idx][self.col] = byte;
                    self.col += 1;
                    if self.col > self.lens[idx] {
                        self.lens[idx] = self.col;
                    }
                }
            }
            _ => {}
        }
    }

    /// Get visible lines for rendering. Returns iterator of (line_content, length).
    /// `visible_rows` is how many lines fit on screen.
    pub fn visible_lines(&self, visible_rows: usize) -> impl Iterator<Item = (&[u8], usize)> {
        let end = self.total + 1; // Include current line
        let start = end.saturating_sub(visible_rows).saturating_sub(self.scroll_offset);
        let count = visible_rows.min(end.saturating_sub(start));

        (0..count).map(move |i| {
            let line_num = start + i;
            let idx = line_num % MAX_LINES;
            (&self.lines[idx][..], self.lens[idx])
        })
    }

    /// Total lines written.
    pub fn line_count(&self) -> usize {
        self.total + 1
    }
}

/// Global terminal buffer (protected by shade compositor lock).
static mut TERMINAL: TerminalBuffer = TerminalBuffer::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Enable/disable terminal capture.
pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Release);
}

/// Check if terminal capture is active.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Write a string to the shade terminal buffer (called from serial::write_str).
pub fn write(s: &str) {
    if !is_active() { return; }
    // SAFETY: single-core, no preemption. write_str is called under serial lock.
    let term = unsafe { &mut *core::ptr::addr_of_mut!(TERMINAL) };
    term.write_str(s);
}

/// Render terminal content into a window region on the shadow buffer.
pub fn render_to_window(
    shadow: *mut u8,
    info: &crate::framebuffer::FbInfo,
    x: u32, y: u32, w: u32, h: u32,
    _scale: u32,
) {
    let (char_w, char_h) = crate::gui::font::char_size(1); // Always use small font
    let cols = w / char_w;
    let rows = h / char_h;
    if cols == 0 || rows == 0 { return; }

    let visible_rows = rows as usize;

    // SAFETY: single-core, called under compositor lock
    let term = unsafe { &*core::ptr::addr_of!(TERMINAL) };
    let lines: alloc::vec::Vec<(alloc::vec::Vec<u8>, usize)> = term.visible_lines(visible_rows)
        .map(|(data, len)| {
            let mut v = alloc::vec![0u8; len];
            v.copy_from_slice(&data[..len]);
            (v, len)
        })
        .collect();

    let fg = 0x00E8E8E8u32; // Near-white text
    let prompt_color = crate::gui::background::accent_color();

    for (i, (line_data, len)) in lines.iter().enumerate() {
        let py = y + i as u32 * char_h;
        if py + char_h > y + h { break; }

        let len = *len;
        let visible_len = len.min(cols as usize);
        if visible_len == 0 { continue; }

        if let Ok(text) = core::str::from_utf8(&line_data[..visible_len]) {
            // Color [npk] prefix with accent color
            if text.starts_with("[npk]") {
                crate::gui::font::draw_str(shadow, info, "[npk]", x, py, prompt_color, None, 1);
                if visible_len > 5 {
                    if let Ok(rest) = core::str::from_utf8(&line_data[5..visible_len]) {
                        let rest_x = x + 5 * char_w;
                        crate::gui::font::draw_str(shadow, info, rest, rest_x, py, fg, None, 1);
                    }
                }
            } else if text.contains("@npk") {
                // Prompt line: color with accent
                crate::gui::font::draw_str(shadow, info, text, x, py, prompt_color, None, 1);
            } else {
                crate::gui::font::draw_str(shadow, info, text, x, py, fg, None, 1);
            }
        }
    }
}
