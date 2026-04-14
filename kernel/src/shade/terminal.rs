//! Shade Terminal — per-window text buffers for independent terminal sessions.
//!
//! Each window gets its own TerminalBuffer. kprintln output goes to the
//! active (focused) terminal. Windows are completely independent.

use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};
use alloc::boxed::Box;

/// Maximum lines and columns in each terminal buffer.
const MAX_LINES: usize = 1000;
const MAX_COLS: usize = 256;
const MAX_INPUT: usize = 512;

/// Terminal slots — u8 index range, only pointers stored statically (~2KB).
/// Actual TerminalBuffers (~264KB each) are heap-allocated on demand.
const MAX_SLOTS: usize = 256;

/// Terminal text buffer (one per window, heap-allocated on demand).
pub struct TerminalBuffer {
    lines: [[u8; MAX_COLS]; MAX_LINES],
    lens: [usize; MAX_LINES],
    /// Total lines written (wraps in ring buffer).
    total: usize,
    /// Current cursor column.
    col: usize,
    /// View scroll offset (lines from bottom, 0 = latest).
    pub scroll_offset: usize,
    /// Saved input state (for window focus switching).
    saved_input: [u8; MAX_INPUT],
    saved_pos: usize,
    saved_cursor: usize,
}

impl TerminalBuffer {
    pub fn new() -> Self {
        TerminalBuffer {
            lines: [[0; MAX_COLS]; MAX_LINES],
            lens: [0; MAX_LINES],
            total: 0,
            col: 0,
            scroll_offset: 0,
            saved_input: [0; MAX_INPUT],
            saved_pos: 0,
            saved_cursor: 0,
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

/// Heap-allocated terminal buffers. Pointer is non-null when slot is in use.
static TERM_PTRS: [AtomicPtr<TerminalBuffer>; MAX_SLOTS] = {
    const NULL: AtomicPtr<TerminalBuffer> = AtomicPtr::new(core::ptr::null_mut());
    [NULL; MAX_SLOTS]
};

/// Get a shared reference to a terminal buffer (None if slot empty).
fn term_ref(idx: usize) -> Option<&'static TerminalBuffer> {
    let ptr = TERM_PTRS[idx].load(Ordering::Acquire);
    if ptr.is_null() { None } else { unsafe { Some(&*ptr) } }
}

/// Get a mutable reference to a terminal buffer (None if slot empty).
/// SAFETY: Only called from Core 0 or with exclusive access (output redirect).
fn term_mut(idx: usize) -> Option<&'static mut TerminalBuffer> {
    let ptr = TERM_PTRS[idx].load(Ordering::Acquire);
    if ptr.is_null() { None } else { unsafe { Some(&mut *ptr) } }
}

/// Currently active terminal index (receives kprintln output).
static ACTIVE_IDX: AtomicU8 = AtomicU8::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set when new content is written (cleared after render).
static DIRTY: AtomicBool = AtomicBool::new(false);

/// Input cursor position (for rendering blinking cursor on input line).
static mut INPUT_CURSOR_POS: usize = 0;

/// Cached background pixels for the input line (saved after full render).
/// Avoids re-blending on every keystroke — just restore + draw text.
// Heap-allocated input line cache (allocated on first use, avoids 983KB BSS bloat)
const INPUT_LINE_CACHE_MAX: usize = 3840 * 4 * 64; // max 4K width × 4 bytes × 64px font height
static mut INPUT_LINE_CACHE: *mut u8 = core::ptr::null_mut();
static mut INPUT_LINE_CACHE_X: u32 = 0;
static mut INPUT_LINE_CACHE_Y: u32 = 0;
static mut INPUT_LINE_CACHE_W: u32 = 0;
static mut INPUT_LINE_CACHE_H: u32 = 0;
static mut INPUT_LINE_CACHE_VALID: bool = false;

/// Set the input cursor position (called from intent loop on every key/move).
pub fn set_cursor_pos(pos: usize) {
    // SAFETY: single-core
    unsafe { INPUT_CURSOR_POS = pos; }
}

/// Rewrite the input portion of the current terminal line.
/// Keeps the prompt intact, overwrites from `prompt_len` onward with `input`,
/// and clears any trailing chars from the previous content.
pub fn rewrite_input(input: &[u8], input_len: usize) {
    if !is_active() { return; }
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    let term = match term_mut(idx) { Some(t) => t, None => return };
    let line_idx = term.total % MAX_LINES;

    // Find prompt length: everything already on the line before user input starts.
    // The prompt ends at the current col minus whatever the caller's pos is.
    // But we don't know the prompt length directly. Instead, we store it.
    let prompt_len = unsafe { PROMPT_LEN };

    // Rewrite from prompt_len onward
    let max = MAX_COLS.min(prompt_len + input_len);
    for i in prompt_len..max {
        term.lines[line_idx][i] = input[i - prompt_len];
    }
    // Clear any trailing chars (line got shorter)
    for i in max..term.lens[line_idx] {
        term.lines[line_idx][i] = b' ';
    }
    term.lens[line_idx] = max;
    term.col = max;
    DIRTY.store(true, Ordering::Release);
}

/// Stored prompt length for the active terminal.
static mut PROMPT_LEN: usize = 0;

/// Set the prompt length (called after write_prompt).
pub fn set_prompt_len(len: usize) {
    // SAFETY: single-core
    unsafe { PROMPT_LEN = len; }
}

/// Get the current line length in the active terminal (for cursor offset calculation).
pub fn current_line_len() -> usize {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    match term_ref(idx) { Some(t) => t.current_line().1, None => 0 }
}

/// Get the current (input) line data and length from the active terminal.
pub fn current_line_data() -> ([u8; 256], usize) {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    let term = match term_ref(idx) { Some(t) => t, None => return ([0; 256], 0) };
    let (data, len) = term.current_line();
    let mut buf = [0u8; 256];
    let copy_len = len.min(256);
    buf[..copy_len].copy_from_slice(&data[..copy_len]);
    (buf, len)
}

/// Get total line count in the active terminal (for input line Y calculation).
pub fn line_count() -> usize {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    match term_ref(idx) { Some(t) => t.total, None => 0 }
}

/// Get the input cursor position.
pub fn cursor_pos() -> usize {
    // SAFETY: single-core
    unsafe { INPUT_CURSOR_POS }
}

/// Enable/disable terminal capture.
pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Release);
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Allocate a new terminal buffer on the heap. Returns index or None if out of memory.
/// Uses alloc_zeroed to avoid 264KB stack frame (kernel stack is only 256KB).
pub fn allocate() -> Option<u8> {
    for i in 0..MAX_SLOTS {
        if TERM_PTRS[i].load(Ordering::Acquire).is_null() {
            // SAFETY: TerminalBuffer is ~264KB — too large for the 256KB kernel stack.
            // Allocate zeroed memory directly on the heap and cast to TerminalBuffer.
            // All fields are zero-initialized (arrays of 0, usize=0, bool=false).
            let layout = alloc::alloc::Layout::new::<TerminalBuffer>();
            let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) } as *mut TerminalBuffer;
            if ptr.is_null() { return None; }
            TERM_PTRS[i].store(ptr, Ordering::Release);
            return Some(i as u8);
        }
    }
    None
}

/// Free a terminal buffer (returns heap memory).
pub fn free(idx: u8) {
    let ptr = TERM_PTRS[idx as usize].swap(core::ptr::null_mut(), Ordering::AcqRel);
    if !ptr.is_null() {
        // SAFETY: ptr was created by alloc_zeroed in allocate()
        let layout = alloc::alloc::Layout::new::<TerminalBuffer>();
        unsafe { alloc::alloc::dealloc(ptr as *mut u8, layout); }
    }
}

/// Get the active terminal index.
pub fn active_idx() -> u8 {
    ACTIVE_IDX.load(Ordering::Acquire)
}

/// Set which terminal receives kprintln output.
pub fn set_active_terminal(idx: u8) {
    ACTIVE_IDX.store(idx, Ordering::Release);
}

/// Clear the active terminal buffer.
pub fn clear() {
    if !is_active() { return; }
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if let Some(t) = term_mut(idx) { t.clear(); }
}

/// Clear a specific terminal by index (for WASM apps on worker cores).
pub fn clear_idx(idx: usize) {
    if let Some(t) = term_mut(idx) {
        t.clear();
        DIRTY.store(true, Ordering::Release);
    }
}

/// Per-core output redirect (indexed by LAPIC ID, 255 = no redirect).
/// Workers set this before intent dispatch so kprintln goes to the right terminal.
static CORE_OUTPUT: [AtomicU8; 256] = {
    const NONE: AtomicU8 = AtomicU8::new(255);
    [NONE; 256]
};

/// Set output redirect for the current core (call before intent dispatch on worker).
pub fn set_output_redirect(terminal_idx: u8) {
    let apic_base = crate::interrupts::apic_base();
    if apic_base == 0 { return; }
    // SAFETY: APIC MMIO is identity-mapped, reading LAPIC ID register
    let apic_id = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) } >> 24;
    CORE_OUTPUT[apic_id as usize & 0xFF].store(terminal_idx, Ordering::Release);
}

/// Get the output redirect terminal for the current core (None if no redirect / Core 0).
pub fn output_redirect_terminal() -> Option<u8> {
    let apic_base = crate::interrupts::apic_base();
    if apic_base == 0 { return None; }
    // SAFETY: APIC MMIO is identity-mapped, reading LAPIC ID register
    let apic_id = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) } >> 24;
    let redirect = CORE_OUTPUT[apic_id as usize & 0xFF].load(Ordering::Acquire);
    if redirect != 255 && term_ref(redirect as usize).is_some() { Some(redirect) } else { None }
}

/// Clear output redirect for the current core.
pub fn clear_output_redirect() {
    let apic_base = crate::interrupts::apic_base();
    if apic_base == 0 { return; }
    let apic_id = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) } >> 24;
    CORE_OUTPUT[apic_id as usize & 0xFF].store(255, Ordering::Release);
}

/// Write to the active terminal (called from serial::write_str).
/// If the current core has an output redirect set, writes to that terminal instead.
pub fn write(s: &str) {
    if !is_active() { return; }

    // Check per-core output redirect (workers running intents)
    let apic_base = crate::interrupts::apic_base();
    if apic_base != 0 {
        // SAFETY: APIC MMIO is identity-mapped
        let apic_id = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) } >> 24;
        let redirect = CORE_OUTPUT[apic_id as usize & 0xFF].load(Ordering::Relaxed);
        if redirect != 255 {
            write_idx(redirect as usize, s);
            return;
        }
    }

    // Default: write to active terminal (Core 0 path)
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if let Some(t) = term_mut(idx) {
        t.write_str(s);
        DIRTY.store(true, Ordering::Release);
    }
}

/// Per-terminal dirty flags (set by worker-core WASM output, read by poll_render).
static TERM_DIRTY: [AtomicBool; MAX_SLOTS] = {
    const FALSE: AtomicBool = AtomicBool::new(false);
    [FALSE; MAX_SLOTS]
};

/// Write to a specific terminal by index (for WASM apps on worker cores).
pub fn write_idx(idx: usize, s: &str) {
    if let Some(t) = term_mut(idx) {
        t.write_str(s);
        TERM_DIRTY[idx].store(true, Ordering::Release);
        DIRTY.store(true, Ordering::Release);
    }
}

/// Check if a specific terminal has new content.
pub fn is_term_dirty(idx: usize) -> bool {
    if idx < MAX_SLOTS { TERM_DIRTY[idx].load(Ordering::Acquire) } else { false }
}

/// Clear per-terminal dirty flag.
pub fn clear_term_dirty(idx: usize) {
    if idx < MAX_SLOTS { TERM_DIRTY[idx].store(false, Ordering::Release); }
}

/// Check if terminal has new content since last render.
pub fn is_dirty() -> bool {
    DIRTY.load(Ordering::Acquire)
}

/// Mark terminal as dirty (triggers partial re-render on next poll_render).
pub fn mark_dirty() {
    DIRTY.store(true, Ordering::Release);
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
    let term = match term_ref(terminal_idx as usize) { Some(t) => t, None => return };

    let (char_w, char_h) = crate::gui::font::char_size(1);
    let cols = w / char_w;
    let rows = h / char_h;
    if cols == 0 || rows == 0 { return; }

    let visible_rows = rows as usize;

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
                // System message: [npk] in accent, rest in white
                crate::gui::font::draw_str(shadow, info, "[npk]", x, py, prompt_color, None, 1);
                if visible_len > 5 {
                    if let Ok(rest) = core::str::from_utf8(&line_data[5..visible_len]) {
                        crate::gui::font::draw_str(shadow, info, rest, x + 5 * char_w, py, fg, None, 1);
                    }
                }
            } else if let Some(pos) = text.find("> ") {
                // Prompt line: path> in accent, input in white
                let prompt_end = pos + 2; // include "> "
                crate::gui::font::draw_str(shadow, info, &text[..prompt_end], x, py, prompt_color, None, 1);
                if visible_len > prompt_end {
                    crate::gui::font::draw_str(shadow, info, &text[prompt_end..], x + prompt_end as u32 * char_w, py, fg, None, 1);
                }
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
    terminal_idx: u8,
) -> Option<(u32, u32, u32, u32)> {
    let term = match term_ref(terminal_idx as usize) { Some(t) => t, None => return None };

    let (char_w, char_h) = crate::gui::font::char_size(1);
    let cols = win_cw / char_w;
    let rows = win_ch / char_h;
    if cols == 0 || rows == 0 { return None; }

    // Calculate Y position of the last visible line
    let visible_rows = rows as usize;
    let end = term.total + 1;
    let visible_count = visible_rows.min(end);
    let last_line_y = win_cy + (visible_count as u32).saturating_sub(1) * char_h;

    // Restore cached background pixels (saved after full render_window).
    let cache_valid = unsafe { INPUT_LINE_CACHE_VALID && !INPUT_LINE_CACHE.is_null() };
    let pitch = info.pitch as usize;
    if cache_valid {
        let cx = unsafe { INPUT_LINE_CACHE_X } as usize;
        let cw = unsafe { INPUT_LINE_CACHE_W } as usize;
        let ch_cached = unsafe { INPUT_LINE_CACHE_H } as usize;
        let bytes_per_row = cw * 4;
        let rows_to_copy = (char_h as usize).min(ch_cached);
        for row in 0..rows_to_copy {
            let cache_off = row * bytes_per_row;
            let shadow_off = (last_line_y as usize + row) * pitch + cx * 4;
            if cache_off + bytes_per_row <= INPUT_LINE_CACHE_MAX
               && shadow_off + bytes_per_row <= (info.height as usize) * pitch
            {
                // SAFETY: single-core, bounds checked above
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        INPUT_LINE_CACHE.add(cache_off),
                        shadow.add(shadow_off),
                        bytes_per_row,
                    );
                }
            }
        }
    } else {
        // No cache — fallback: clear with bg_color
        crate::gui::render::fill_rect(shadow, info,
            win_cx, last_line_y, win_cw, char_h, 0x00101018);
    }

    let (line_data, len) = term.current_line();
    let visible_len = len.min(cols as usize);
    if visible_len > 0 {
        let prompt_color = crate::gui::background::accent_color();
        let fg = 0x00E8E8E8u32;
        if let Ok(text) = core::str::from_utf8(&line_data[..visible_len]) {
            if let Some(pos) = text.find("> ") {
                let prompt_end = pos + 2;
                crate::gui::font::draw_str(shadow, info, &text[..prompt_end], win_cx, last_line_y, prompt_color, None, 1);
                if visible_len > prompt_end {
                    crate::gui::font::draw_str(shadow, info, &text[prompt_end..], win_cx + prompt_end as u32 * char_w, last_line_y, fg, None, 1);
                }
            } else {
                crate::gui::font::draw_str(shadow, info, text, win_cx, last_line_y, fg, None, 1);
            }
        }
    }

    // Draw text cursor (solid bar at cursor position)
    let cur = cursor_pos();
    let cursor_x = win_cx + cur as u32 * char_w;
    if cursor_x + 2 <= win_cx + win_cw {
        let cursor_color = 0x00E8E8E8u32;
        crate::gui::render::fill_rect(shadow, info, cursor_x, last_line_y, 2, char_h, cursor_color);
    }

    Some((win_cx, last_line_y, win_cw, char_h))
}

/// Layer-based input line render: clear text region + draw text + cursor.
/// No background cache needed — text layer is transparent, composited on top.
pub fn render_input_line_to_layer(
    text_buf: *mut u8,
    info: &crate::framebuffer::FbInfo,
    win_cx: u32, win_cy: u32, win_cw: u32, win_ch: u32,
    terminal_idx: u8,
) -> Option<(u32, u32, u32, u32)> {
    let term = match term_ref(terminal_idx as usize) { Some(t) => t, None => return None };

    let (char_w, char_h) = crate::gui::font::char_size(1);
    let cols = win_cw / char_w;
    let rows = win_ch / char_h;
    if cols == 0 || rows == 0 { return None; }

    let visible_rows = rows as usize;
    let end = term.total + 1;
    let visible_count = visible_rows.min(end);
    let last_line_y = win_cy + (visible_count as u32).saturating_sub(1) * char_h;

    // Clear the input line region in text layer (transparent)
    let pitch = info.pitch as usize;
    let x1 = (win_cx + win_cw).min(info.width) as usize;
    let bytes = x1.saturating_sub(win_cx as usize) * 4;
    if bytes == 0 { return None; }
    for row in 0..char_h {
        if last_line_y + row < info.height {
            let off = (last_line_y + row) as usize * pitch + win_cx as usize * 4;
            // SAFETY: bounds checked
            unsafe { core::ptr::write_bytes(text_buf.add(off), 0, bytes); }
        }
    }

    // Draw text
    let (line_data, len) = term.current_line();
    let visible_len = len.min(cols as usize);
    if visible_len > 0 {
        let prompt_color = crate::gui::background::accent_color();
        let fg = 0x00E8E8E8u32;
        if let Ok(text) = core::str::from_utf8(&line_data[..visible_len]) {
            if let Some(pos) = text.find("> ") {
                let prompt_end = pos + 2;
                crate::gui::font::draw_str(text_buf, info, &text[..prompt_end], win_cx, last_line_y, prompt_color, None, 1);
                if visible_len > prompt_end {
                    crate::gui::font::draw_str(text_buf, info, &text[prompt_end..], win_cx + prompt_end as u32 * char_w, last_line_y, fg, None, 1);
                }
            } else {
                crate::gui::font::draw_str(text_buf, info, text, win_cx, last_line_y, fg, None, 1);
            }
        }
    }

    // Draw text cursor
    let cur = cursor_pos();
    let cursor_x = win_cx + cur as u32 * char_w;
    if cursor_x + 2 <= win_cx + win_cw {
        let cursor_color = 0x00E8E8E8u32;
        crate::gui::render::fill_rect(text_buf, info, cursor_x, last_line_y, 2, char_h, cursor_color);
    }

    Some((win_cx, last_line_y, win_cw, char_h))
}

/// Cache the input line background from the shadow buffer after a full render.
/// Called from render_window after drawing the focused window.
pub fn cache_input_line_bg(
    shadow: *mut u8,
    info: &crate::framebuffer::FbInfo,
    win_cx: u32, win_cy: u32, win_cw: u32, win_ch: u32,
    terminal_idx: u8,
) {
    let term = match term_ref(terminal_idx as usize) { Some(t) => t, None => return };
    let (_, char_h) = crate::gui::font::char_size(1);
    let rows = win_ch / char_h;
    if rows == 0 { return; }
    let visible_count = (rows as usize).min(term.total + 1);
    let last_line_y = win_cy + (visible_count as u32).saturating_sub(1) * char_h;

    let pitch = info.pitch as usize;
    let bytes_per_row = (win_cw as usize) * 4;
    let total_bytes = bytes_per_row * char_h as usize;
    if total_bytes > INPUT_LINE_CACHE_MAX { return; }

    // Allocate cache on first use (avoids 983KB BSS)
    unsafe {
        if INPUT_LINE_CACHE.is_null() {
            let layout = alloc::alloc::Layout::from_size_align(INPUT_LINE_CACHE_MAX, 16).unwrap();
            INPUT_LINE_CACHE = alloc::alloc::alloc_zeroed(layout);
            if INPUT_LINE_CACHE.is_null() { return; }
        }
    }

    // SAFETY: single-core, bounds checked, cache allocated above
    unsafe {
        for row in 0..char_h {
            let shadow_off = (last_line_y + row) as usize * pitch + win_cx as usize * 4;
            let cache_off = row as usize * bytes_per_row;
            core::ptr::copy_nonoverlapping(
                shadow.add(shadow_off),
                INPUT_LINE_CACHE.add(cache_off),
                bytes_per_row,
            );
        }
        INPUT_LINE_CACHE_X = win_cx;
        INPUT_LINE_CACHE_Y = last_line_y;
        INPUT_LINE_CACHE_W = win_cw;
        INPUT_LINE_CACHE_H = char_h;
        INPUT_LINE_CACHE_VALID = true;
    }
}

/// Invalidate the input line cache (call when window layout changes).
pub fn invalidate_input_cache() {
    unsafe { INPUT_LINE_CACHE_VALID = false; }
}

/// Scroll the active terminal up (show older content).
pub fn scroll_up(lines: usize) {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if let Some(term) = term_mut(idx) {
        let max_scroll = term.total.saturating_sub(10);
        term.scroll_offset = (term.scroll_offset + lines).min(max_scroll);
        DIRTY.store(true, Ordering::Release);
    }
}

/// Scroll the active terminal down (show newer content).
pub fn scroll_down(lines: usize) {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if let Some(term) = term_mut(idx) {
        term.scroll_offset = term.scroll_offset.saturating_sub(lines);
        DIRTY.store(true, Ordering::Release);
    }
}

/// Reset scroll to bottom (show latest content).
pub fn scroll_reset() {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    if let Some(term) = term_mut(idx) { term.scroll_offset = 0; }
}

/// Restore cursor position from per-terminal saved state.
pub fn restore_cursor() {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    let term = match term_ref(idx) { Some(t) => t, None => return };
    let pos = term.saved_pos;
    let cursor = term.saved_cursor.min(pos);
    let line_len = current_line_len();
    set_cursor_pos(line_len.saturating_sub(pos.saturating_sub(cursor)));
}

/// Save the current input buffer + cursor position to the active terminal's saved state.
pub fn save_input(buf: &[u8], pos: usize) {
    save_input_with_cursor(buf, pos, pos);
}

/// Save input buffer, pos, and cursor position.
pub fn save_input_with_cursor(buf: &[u8], pos: usize, cursor: usize) {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    let term = match term_mut(idx) { Some(t) => t, None => return };
    let len = pos.min(MAX_INPUT);
    term.saved_input[..len].copy_from_slice(&buf[..len]);
    term.saved_pos = len;
    term.saved_cursor = cursor.min(len);
}

/// Restore the saved input buffer from the active terminal. Returns (pos, cursor).
pub fn restore_input_with_cursor(buf: &mut [u8]) -> (usize, usize) {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    let term = match term_ref(idx) { Some(t) => t, None => return (0, 0) };
    let len = term.saved_pos.min(buf.len());
    buf[..len].copy_from_slice(&term.saved_input[..len]);
    (len, term.saved_cursor.min(len))
}

/// Restore the saved input buffer from the active terminal (legacy, cursor=pos).
pub fn restore_input(buf: &mut [u8]) -> usize {
    let (pos, _) = restore_input_with_cursor(buf);
    pos
}

/// Write the prompt string to the active terminal buffer.
pub fn write_prompt() {
    if !is_active() { return; }
    let cwd = crate::intent::get_cwd_for_shell();
    let path = if cwd.is_empty() { "/" } else { cwd.as_str() };
    let prompt = alloc::format!("{}> ", path);
    set_prompt_len(prompt.len());
    write(&prompt);
}
