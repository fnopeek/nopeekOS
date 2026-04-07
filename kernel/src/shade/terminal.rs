//! Shade Terminal — per-window text buffers for independent terminal sessions.
//!
//! Each window gets its own TerminalBuffer. kprintln output goes to the
//! active (focused) terminal. Windows are completely independent.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Maximum lines and columns in each terminal buffer.
const MAX_LINES: usize = 1000;
const MAX_COLS: usize = 256;
/// Maximum number of independent terminal sessions.
const MAX_TERMINALS: usize = 8;

/// Terminal text buffer (one per window).
pub struct TerminalBuffer {
    lines: [[u8; MAX_COLS]; MAX_LINES],
    lens: [usize; MAX_LINES],
    /// Total lines written (wraps in ring buffer).
    total: usize,
    /// Current cursor column.
    col: usize,
    /// View scroll offset (lines from bottom, 0 = latest).
    pub scroll_offset: usize,
    /// Whether this slot is in use.
    pub in_use: bool,
}

impl TerminalBuffer {
    pub const fn new() -> Self {
        TerminalBuffer {
            lines: [[0; MAX_COLS]; MAX_LINES],
            lens: [0; MAX_LINES],
            total: 0,
            col: 0,
            scroll_offset: 0,
            in_use: false,
        }
    }

    pub fn write_str(&mut self, s: &str) {
        for &byte in s.as_bytes() {
            self.write_byte(byte);
        }
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.total += 1;
                self.col = 0;
                let idx = self.total % MAX_LINES;
                self.lens[idx] = 0;
                self.lines[idx] = [0; MAX_COLS];
            }
            b'\r' => {
                self.col = 0;
            }
            0x08 => {
                // Backspace: move cursor left, shrink line length
                if self.col > 0 {
                    self.col -= 1;
                    let idx = self.total % MAX_LINES;
                    self.lines[idx][self.col] = b' ';
                    // Only shrink lens if we're at the end
                    if self.col < self.lens[idx] {
                        self.lens[idx] = self.col;
                    }
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

    pub fn clear(&mut self) {
        self.lines = [[0; MAX_COLS]; MAX_LINES];
        self.lens = [0; MAX_LINES];
        self.total = 0;
        self.col = 0;
        self.scroll_offset = 0;
    }

    /// Get visible lines for rendering (respects scroll_offset).
    pub fn visible_lines(&self, visible_rows: usize) -> impl Iterator<Item = (&[u8], usize)> {
        let max_end = self.total + 1;
        let end = max_end.saturating_sub(self.scroll_offset);
        let start = end.saturating_sub(visible_rows);
        let count = visible_rows.min(end.saturating_sub(start));

        (0..count).map(move |i| {
            let line_num = start + i;
            let idx = line_num % MAX_LINES;
            (&self.lines[idx][..], self.lens[idx])
        })
    }

    /// Get the current (bottom) line content for fast input rendering.
    pub fn current_line(&self) -> (&[u8], usize) {
        let idx = self.total % MAX_LINES;
        (&self.lines[idx][..], self.lens[idx])
    }
}

/// All terminal buffers.
static mut TERMINALS: [TerminalBuffer; MAX_TERMINALS] = {
    const INIT: TerminalBuffer = TerminalBuffer::new();
    [INIT; MAX_TERMINALS]
};

/// Per-terminal saved input state (for switching between windows).
const MAX_INPUT: usize = 512;
static mut SAVED_INPUT: [[u8; MAX_INPUT]; MAX_TERMINALS] = [[0; MAX_INPUT]; MAX_TERMINALS];
static mut SAVED_POS: [usize; MAX_TERMINALS] = [0; MAX_TERMINALS];

/// Currently active terminal index (receives kprintln output).
static ACTIVE_IDX: AtomicU8 = AtomicU8::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set when new content is written (cleared after render).
static DIRTY: AtomicBool = AtomicBool::new(false);

/// Enable/disable terminal capture.
pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Release);
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Allocate a new terminal buffer. Returns index (0-7) or None if full.
pub fn allocate() -> Option<u8> {
    let terms = unsafe { &mut *core::ptr::addr_of_mut!(TERMINALS) };
    for (i, t) in terms.iter_mut().enumerate() {
        if !t.in_use {
            t.in_use = true;
            t.clear();
            return Some(i as u8);
        }
    }
    None
}

/// Free a terminal buffer.
pub fn free(idx: u8) {
    if (idx as usize) < MAX_TERMINALS {
        let terms = unsafe { &mut *core::ptr::addr_of_mut!(TERMINALS) };
        terms[idx as usize].in_use = false;
    }
}

/// Set which terminal receives kprintln output.
pub fn set_active_terminal(idx: u8) {
    ACTIVE_IDX.store(idx, Ordering::Release);
}

/// Clear the active terminal buffer.
pub fn clear() {
    if !is_active() { return; }
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if idx < MAX_TERMINALS {
        let terms = unsafe { &mut *core::ptr::addr_of_mut!(TERMINALS) };
        terms[idx].clear();
    }
}

/// Write to the active terminal (called from serial::write_str).
pub fn write(s: &str) {
    if !is_active() { return; }
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if idx < MAX_TERMINALS {
        let terms = unsafe { &mut *core::ptr::addr_of_mut!(TERMINALS) };
        terms[idx].write_str(s);
        DIRTY.store(true, Ordering::Release);
    }
}

/// Check if terminal has new content since last render.
pub fn is_dirty() -> bool {
    DIRTY.load(Ordering::Acquire)
}

/// Clear dirty flag (called after render).
pub fn clear_dirty() {
    DIRTY.store(false, Ordering::Release);
}

/// Render a specific terminal's content into a window region.
pub fn render_to_window(
    shadow: *mut u8,
    info: &crate::framebuffer::FbInfo,
    x: u32, y: u32, w: u32, h: u32,
    _scale: u32,
    terminal_idx: u8,
) {
    if (terminal_idx as usize) >= MAX_TERMINALS { return; }

    let (char_w, char_h) = crate::gui::font::char_size(1);
    let cols = w / char_w;
    let rows = h / char_h;
    if cols == 0 || rows == 0 { return; }

    let visible_rows = rows as usize;
    let terms = unsafe { &*core::ptr::addr_of!(TERMINALS) };
    let term = &terms[terminal_idx as usize];

    let lines: alloc::vec::Vec<(alloc::vec::Vec<u8>, usize)> = term.visible_lines(visible_rows)
        .map(|(data, len)| {
            let mut v = alloc::vec![0u8; len];
            v.copy_from_slice(&data[..len]);
            (v, len)
        })
        .collect();

    let fg = 0x00E8E8E8u32;
    let prompt_color = crate::gui::background::accent_color();

    for (i, (line_data, len)) in lines.iter().enumerate() {
        let py = y + i as u32 * char_h;
        if py + char_h > y + h { break; }

        let len = *len;
        let visible_len = len.min(cols as usize);

        if visible_len == 0 { continue; }

        if let Ok(text) = core::str::from_utf8(&line_data[..visible_len]) {
            if text.starts_with("[npk]") {
                crate::gui::font::draw_str(shadow, info, "[npk]", x, py, prompt_color, None, 1);
                if visible_len > 5 {
                    if let Ok(rest) = core::str::from_utf8(&line_data[5..visible_len]) {
                        crate::gui::font::draw_str(shadow, info, rest, x + 5 * char_w, py, fg, None, 1);
                    }
                }
            } else if text.contains("@npk") {
                crate::gui::font::draw_str(shadow, info, text, x, py, prompt_color, None, 1);
            } else {
                crate::gui::font::draw_str(shadow, info, text, x, py, fg, None, 1);
            }
        }
    }
}

/// Fast render: only the current input line of the active terminal.
/// Returns the blit region (x, y, w, h) or None.
pub fn render_input_line(
    shadow: *mut u8,
    info: &crate::framebuffer::FbInfo,
    win_cx: u32, win_cy: u32, win_cw: u32, win_ch: u32,
    bg_color: u32, opacity: u32,
    terminal_idx: u8,
) -> Option<(u32, u32, u32, u32)> {
    if (terminal_idx as usize) >= MAX_TERMINALS { return None; }

    let (char_w, char_h) = crate::gui::font::char_size(1);
    let cols = win_cw / char_w;
    let rows = win_ch / char_h;
    if cols == 0 || rows == 0 { return None; }

    let terms = unsafe { &*core::ptr::addr_of!(TERMINALS) };
    let term = &terms[terminal_idx as usize];

    // Calculate Y position of the last visible line
    let visible_rows = rows as usize;
    let end = term.total + 1;
    let visible_count = visible_rows.min(end);
    let last_line_y = win_cy + (visible_count as u32).saturating_sub(1) * char_h;

    // Restore input line background (aurora + blend for just 1 line = fast, ~2ms)
    crate::gui::background::draw_aurora_region(shadow, info,
        win_cx, last_line_y, win_cw, char_h);
    crate::gui::render::fill_rounded_rect_blend(shadow, info,
        win_cx, last_line_y, win_cw, char_h,
        crate::gui::background::accent_color(), 0, 180);
    crate::gui::render::fill_rounded_rect_blend(shadow, info,
        win_cx, last_line_y, win_cw, char_h,
        bg_color, 0, opacity);

    let (line_data, len) = term.current_line();
    let visible_len = len.min(cols as usize);
    if visible_len > 0 {
        let prompt_color = crate::gui::background::accent_color();
        let fg = 0x00E8E8E8u32;
        if let Ok(text) = core::str::from_utf8(&line_data[..visible_len]) {
            if text.contains("@npk") {
                crate::gui::font::draw_str(shadow, info, text, win_cx, last_line_y, prompt_color, None, 1);
            } else {
                crate::gui::font::draw_str(shadow, info, text, win_cx, last_line_y, fg, None, 1);
            }
        }
    }

    Some((win_cx, last_line_y, win_cw, char_h))
}

/// Scroll the active terminal up (show older content).
pub fn scroll_up(lines: usize) {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if idx >= MAX_TERMINALS { return; }
    let terms = unsafe { &mut *core::ptr::addr_of_mut!(TERMINALS) };
    let term = &mut terms[idx];
    let max_scroll = term.total.saturating_sub(10); // Don't scroll past beginning
    term.scroll_offset = (term.scroll_offset + lines).min(max_scroll);
    DIRTY.store(true, Ordering::Release);
}

/// Scroll the active terminal down (show newer content).
pub fn scroll_down(lines: usize) {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if idx >= MAX_TERMINALS { return; }
    let terms = unsafe { &mut *core::ptr::addr_of_mut!(TERMINALS) };
    let term = &mut terms[idx];
    term.scroll_offset = term.scroll_offset.saturating_sub(lines);
    DIRTY.store(true, Ordering::Release);
}

/// Reset scroll to bottom (show latest content).
pub fn scroll_reset() {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if idx >= MAX_TERMINALS { return; }
    let terms = unsafe { &mut *core::ptr::addr_of_mut!(TERMINALS) };
    terms[idx].scroll_offset = 0;
}

/// Save the current input buffer to the active terminal's saved state.
pub fn save_input(buf: &[u8], pos: usize) {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if idx >= MAX_TERMINALS { return; }
    let saved = unsafe { &mut *core::ptr::addr_of_mut!(SAVED_INPUT) };
    let spos = unsafe { &mut *core::ptr::addr_of_mut!(SAVED_POS) };
    let len = pos.min(MAX_INPUT);
    saved[idx][..len].copy_from_slice(&buf[..len]);
    spos[idx] = len;
}

/// Restore the saved input buffer from the active terminal.
/// Returns the number of bytes restored (= pos).
pub fn restore_input(buf: &mut [u8]) -> usize {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if idx >= MAX_TERMINALS { return 0; }
    let saved = unsafe { &*core::ptr::addr_of!(SAVED_INPUT) };
    let spos = unsafe { &*core::ptr::addr_of!(SAVED_POS) };
    let len = spos[idx].min(buf.len());
    buf[..len].copy_from_slice(&saved[idx][..len]);
    len
}

/// Write the prompt string to the active terminal buffer.
pub fn write_prompt() {
    if !is_active() { return; }
    let user = crate::config::get("name");
    let cwd = crate::intent::get_cwd_for_shell();
    let user_str = user.as_deref().unwrap_or("npk");
    if cwd.is_empty() {
        write(&alloc::format!("{}@npk /> ", user_str));
    } else {
        write(&alloc::format!("{}@npk {}> ", user_str, cwd));
    }
}
