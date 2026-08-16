//! Shade Terminal — per-window text buffers for independent terminal sessions.
//!
//! Each window gets its own TerminalBuffer. kprintln output goes to the
//! active (focused) terminal. Windows are completely independent.

use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use spin::Mutex;

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
    #[allow(dead_code)]
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

/// Currently active (focused) terminal index — drives rendering + is the
/// interactive I/O target.
static ACTIVE_IDX: AtomicU8 = AtomicU8::new(0);
/// The "primary" terminal — the first loop opened. Spontaneous kernel debug
/// (kprintln with no per-core output redirect) lands here so it stays put
/// instead of chasing window focus across multiple loops. 255 = none open.
static PRIMARY_IDX: AtomicU8 = AtomicU8::new(255);
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
            // First loop opened → it becomes the primary debug sink.
            let _ = PRIMARY_IDX.compare_exchange(
                255, i as u8, Ordering::AcqRel, Ordering::Relaxed);
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
    // If the primary just closed, hand primary to the lowest surviving loop
    // (the next-oldest by index), or 255 when none remain.
    if PRIMARY_IDX.load(Ordering::Acquire) == idx {
        let mut new_primary = 255u8;
        for i in 0..MAX_SLOTS {
            if !TERM_PTRS[i].load(Ordering::Acquire).is_null() {
                new_primary = i as u8;
                break;
            }
        }
        PRIMARY_IDX.store(new_primary, Ordering::Release);
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
    clear_selection(idx);
}

/// Clear a specific terminal by index (for WASM apps on worker cores).
pub fn clear_idx(idx: usize) {
    if let Some(t) = term_mut(idx) {
        t.clear();
        DIRTY.store(true, Ordering::Release);
    }
    clear_selection(idx);
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
    if !is_active() {
        // The screen may be gone; the remote console is not. Anything printed
        // before the terminal exists (or after it goes away) is exactly what a
        // developer watching from outside needs most.
        stream_push(usize::MAX, s);
        return;
    }

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

    // Default (Core 0, no per-core redirect): the PRIMARY terminal — the
    // first loop — so background/system debug stays put instead of following
    // window focus. The interactive run loop brackets its prompt + command
    // output with a Core-0 redirect to the focused terminal, so those still
    // land where the user is typing. Falls back to the active terminal if no
    // primary is set (shouldn't happen once a loop exists).
    let primary = PRIMARY_IDX.load(Ordering::Acquire);
    let idx = if primary != 255 {
        primary as usize
    } else {
        ACTIVE_IDX.load(Ordering::Acquire) as usize
    };
    if let Some(t) = term_mut(idx) {
        t.write_str(s);
        DIRTY.store(true, Ordering::Release);
    }
    stream_push(idx, s);
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
    stream_push(idx, s);
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

/// Terminal foreground (text + cursor) from the active theme's OnSurface
/// token — so `loop` windows turn dark-on-light in light mode, matching
/// the widget apps. Masked to opaque RGB (the shadow buffer is opaque).
fn theme_fg() -> u32 {
    crate::shade::widgets::palette::resolve(
        crate::shade::widgets::abi::Token::OnSurface) & 0x00FF_FFFF
}

/// Terminal background from the active theme's Surface token.
fn theme_bg() -> u32 {
    crate::shade::widgets::palette::resolve(
        crate::shade::widgets::abi::Token::Surface) & 0x00FF_FFFF
}

/// Prompt / `[npk]` accent from the active theme. The widget Accent token
/// is contrast-adjusted against the surface (darkens on a light surface),
/// unlike the raw wallpaper accent — so the prompt stays readable in light
/// mode too.
fn theme_prompt() -> u32 {
    crate::shade::widgets::palette::resolve(
        crate::shade::widgets::abi::Token::Accent) & 0x00FF_FFFF
}

/// Selection highlight colour from the active theme (muted accent behind
/// the selected glyphs).
fn theme_selection() -> u32 {
    crate::shade::widgets::palette::resolve(
        crate::shade::widgets::abi::Token::AccentMuted) & 0x00FF_FFFF
}

fn theme_token(t: crate::shade::widgets::abi::Token) -> u32 {
    crate::shade::widgets::palette::resolve(t) & 0x00FF_FFFF
}

// ── Status lines ──────────────────────────────────────────────────────
//
// The terminal has no escape codes and the font is ASCII-only, so colour
// cannot travel inside the text. It comes from the SHAPE of a line, the way
// the `[npk]` prefix and the `path> ` prompt already work: a line whose first
// non-blank character is one of these markers (followed by a space) is a
// status line, and its tokens are coloured by what they look like.
//
//   +  something will change / did change   (accent)
//   .  already current                      (faint, whole line)
//   !  failed                               (danger, whole line)
//   *  finished                             (success)
//
// Emitters: `intent::update`. Anything else printing these markers gets the
// same treatment, which is the point — it is a shell-wide convention, not an
// update-specific hack.
const STATUS_MARKERS: &[u8] = b"+.!*";

fn status_colors(marker: u8) -> (u32, u32) {
    use crate::shade::widgets::abi::Token;
    match marker {
        b'.' => (theme_token(Token::OnSurfaceFaint), theme_token(Token::OnSurfaceFaint)),
        b'!' => (theme_token(Token::Danger), theme_token(Token::Danger)),
        b'*' => (theme_token(Token::Success), theme_fg()),
        _ => (theme_token(Token::Accent), theme_fg()),
    }
}

/// Colour of one whitespace-delimited token on a status line.
/// `(…)` is the parenthetical the emitters use for sizes and asides, so it
/// steps back; a version number is the thing you actually came to read.
fn token_color(tok: &str, base: u32, accent: u32, faint: u32) -> u32 {
    let b = tok.as_bytes();
    if b.first() == Some(&b'(') || b.last() == Some(&b')') { return faint }
    if tok == "->" { return faint }
    let looks_like_version = match b.first() {
        Some(&b'v') => b.get(1).is_some_and(|c| c.is_ascii_digit()),
        Some(c) => c.is_ascii_digit(),
        None => false,
    };
    if looks_like_version && tok.contains('.') { return accent }
    base
}

/// Draw one row of terminal text, `[npk]` prefix already handled by the
/// caller. Returns false when the row is not a status line and the caller
/// should draw it plainly.
fn draw_status_row(shadow: *mut u8, info: &crate::framebuffer::FbInfo,
                   text: &str, x: u32, py: u32, char_w: u32) -> bool {
    let bytes = text.as_bytes();
    let first = match bytes.iter().position(|c| *c != b' ') { Some(i) => i, None => return false };
    let marker = bytes[first];
    if !STATUS_MARKERS.contains(&marker) { return false }
    if bytes.get(first + 1) != Some(&b' ') { return false }

    let (marker_color, base) = status_colors(marker);
    let accent = theme_token(crate::shade::widgets::abi::Token::Accent);
    let faint = theme_token(crate::shade::widgets::abi::Token::OnSurfaceFaint);
    crate::gui::font::draw_str(shadow, info, &text[first..first + 1],
        x + first as u32 * char_w, py, marker_color, None, 1);

    // Tokens keep their byte offset, so every glyph stays in the column it
    // would have had — selection and wrapping still count plain bytes.
    let mut i = first + 2;
    while i < bytes.len() {
        if bytes[i] == b' ' { i += 1; continue }
        let end = bytes[i..].iter().position(|c| *c == b' ').map(|p| i + p).unwrap_or(bytes.len());
        let tok = &text[i..end];
        let color = if marker == b'+' || marker == b'*' {
            token_color(tok, base, accent, faint)
        } else {
            base
        };
        crate::gui::font::draw_str(shadow, info, tok, x + i as u32 * char_w, py, color, None, 1);
        i = end;
    }
    true
}

// ── Mouse text selection (drag to mark, Ctrl+Shift+C to copy) ─────────
//
// A cell is identified by (absolute logical line, byte column). Absolute
// line numbers survive scrolling; they alias back into the ring via
// `% MAX_LINES`, so a selection older than MAX_LINES lines is silently
// clamped (transient — you copy right after selecting). Only one terminal
// carries a selection at a time.

#[derive(Clone, Copy)]
struct Selection {
    term_idx: usize,
    /// Text rect captured at press so a drag can clamp even off-window.
    rx: i32, ry: i32, rw: u32, rh: u32,
    anchor: (usize, usize),  // (abs_line, col)
    head:   (usize, usize),
    active: bool,            // true while the mouse button is held
}

static SELECTION: Mutex<Option<Selection>> = Mutex::new(None);

/// Ordered (lo, hi) endpoints — tuples compare (line, then col).
fn sel_bounds(s: &Selection) -> ((usize, usize), (usize, usize)) {
    if s.anchor <= s.head { (s.anchor, s.head) } else { (s.head, s.anchor) }
}

/// Map a screen pixel to a terminal cell `(abs_line, col)`, clamped to the
/// grid. Mirrors the geometry in `render_to_window` exactly (monospace
/// `char_size(1)`, `visible_lines` snapshot, soft-wrap into `cols`-wide
/// segments) so the highlight lands on the glyph under the cursor.
pub fn cell_at(idx: usize, rx: i32, ry: i32, rw: u32, rh: u32, mx: i32, my: i32)
    -> Option<(usize, usize)>
{
    let term = term_ref(idx)?;
    let (char_w, char_h) = crate::gui::font::char_size(1);
    if char_w == 0 || char_h == 0 { return None; }
    let cols = (rw / char_w) as usize;
    let visible_rows = (rh / char_h) as usize;
    if cols == 0 || visible_rows == 0 { return None; }

    let lens: alloc::vec::Vec<usize> =
        term.visible_lines(visible_rows).map(|(_, len)| len).collect();
    let start_line = (term.total + 1)
        .saturating_sub(term.scroll_offset)
        .saturating_sub(lens.len());

    // Soft-wrap each logical line into cols-wide segments (same as render).
    let mut segs: alloc::vec::Vec<(usize, usize, usize)> = alloc::vec::Vec::new();
    for (li, &len) in lens.iter().enumerate() {
        if len == 0 { segs.push((li, 0, 0)); continue; }
        let mut s = 0usize;
        while s < len { let e = (s + cols).min(len); segs.push((li, s, e)); s = e; }
    }
    let first_seg = segs.len().saturating_sub(visible_rows);
    let disp = segs.len() - first_seg;
    if disp == 0 { return None; }

    let row = ((((my - ry).max(0)) as u32) / char_h) as usize;
    let row = row.min(disp - 1);
    let (li, s, e) = segs[first_seg + row];
    let col_in_seg = ((((mx - rx).max(0)) as u32) / char_w) as usize;
    let col = (s + col_in_seg).min(e); // clamp within this seg's byte range
    Some((start_line + li, col))
}

/// Begin a selection at the click cell (collapsed). No-op if the point
/// resolves outside the grid.
pub fn selection_begin(idx: usize, rx: i32, ry: i32, rw: u32, rh: u32, mx: i32, my: i32) -> bool {
    match cell_at(idx, rx, ry, rw, rh, mx, my) {
        Some(cell) => {
            *SELECTION.lock() = Some(Selection {
                term_idx: idx, rx, ry, rw, rh, anchor: cell, head: cell, active: true,
            });
            true
        }
        None => false,
    }
}

/// Extend the active selection's moving end to `(mx, my)`. Returns true if
/// the selection changed (caller re-renders). Clamps to the press-time
/// rect, so dragging past the window edge keeps extending.
pub fn selection_extend(mx: i32, my: i32) -> bool {
    let mut g = SELECTION.lock();
    let sel = match g.as_mut() { Some(s) if s.active => s, _ => return false };
    match cell_at(sel.term_idx, sel.rx, sel.ry, sel.rw, sel.rh, mx, my) {
        Some(cell) if cell != sel.head => { sel.head = cell; true }
        _ => false,
    }
}

/// End the drag. A collapsed (single-click) selection is dropped so a plain
/// click doesn't leave a zero-width highlight. Returns true if state changed.
pub fn selection_end() -> bool {
    let mut g = SELECTION.lock();
    match g.as_mut() {
        Some(sel) => {
            sel.active = false;
            if sel.anchor == sel.head { *g = None; }
            true
        }
        None => false,
    }
}

/// True while a drag is in progress (caller keeps routing moves to us).
pub fn selection_dragging() -> bool {
    SELECTION.lock().as_ref().map_or(false, |s| s.active)
}

/// Copy the current selection to the kernel clipboard. Gathers each covered
/// logical line's byte range, trims trailing spaces, joins with '\n'.
pub fn copy_selection() {
    let sel = match *SELECTION.lock() { Some(s) => s, None => return };
    let (lo, hi) = sel_bounds(&sel);
    let term = match term_ref(sel.term_idx) { Some(t) => t, None => return };

    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    for line in lo.0..=hi.0 {
        let ridx = line % MAX_LINES;
        let len = term.lens[ridx];
        let c0 = (if line == lo.0 { lo.1 } else { 0 }).min(len);
        let c1 = (if line == hi.0 { hi.1 } else { len }).min(len);
        let mut seg_end = c1;
        // Trim trailing spaces on the copied segment (avoids grid padding).
        while seg_end > c0 && term.lines[ridx][seg_end - 1] == b' ' { seg_end -= 1; }
        if seg_end > c0 {
            out.extend_from_slice(&term.lines[ridx][c0..seg_end]);
        }
        if line != hi.0 { out.push(b'\n'); }
    }
    if !out.is_empty() {
        crate::shade::clipboard::set_text(&out);
    }
}

/// Paste the clipboard into the focused terminal by injecting its bytes into
/// the keyboard stream — the interactive loop then consumes them exactly as
/// typed input. Newlines act as Enter (run the line), matching terminals.
/// Bounded to the keyboard ring so a huge paste can't overflow it.
pub fn paste_clipboard() {
    let bytes = match crate::shade::clipboard::get_text() { Some(b) => b, None => return };
    let mut injected = 0usize;
    for &b in bytes.iter() {
        let c = match b {
            b'\n' | b'\r' => b'\n',
            b'\t'         => b' ',
            0x20..=0x7E   => b,
            _             => continue,
        };
        crate::keyboard::inject_byte(c);
        injected += 1;
        if injected >= 256 { break; } // keyboard ring is 512; leave headroom
    }
}

/// Drop the selection if it belongs to terminal `idx` (its content just
/// changed under it, e.g. on clear or a plain key — the absolute lines no
/// longer map / the user moved on). Returns true if a selection was dropped.
pub fn clear_selection(idx: usize) -> bool {
    let mut g = SELECTION.lock();
    if g.as_ref().map_or(false, |s| s.term_idx == idx) { *g = None; true } else { false }
}

/// Direction for keyboard (Shift+arrow) selection.
#[derive(Clone, Copy)]
pub enum SelDir { Left, Right, Up, Down }

/// Extend a terminal selection one cell via Shift+arrow. Anchors at the
/// current input caret `(total, INPUT_CURSOR_POS)` when starting fresh; a
/// mouse selection already present is continued from its moving end. Moves
/// are clamped to the ring-valid range. Returns true if it changed.
pub fn selection_key(idx: usize, dir: SelDir) -> bool {
    let term = match term_ref(idx) { Some(t) => t, None => return false };
    let line_len = |ln: usize| -> usize { term.lens[ln % MAX_LINES] };
    let oldest = term.total.saturating_sub(MAX_LINES - 1); // first ring-valid line
    let caret_col = unsafe { INPUT_CURSOR_POS }.min(line_len(term.total));
    let caret = (term.total, caret_col);

    let mut g = SELECTION.lock();
    let mut sel = match g.take() {
        Some(s) if s.term_idx == idx => s,
        _ => Selection { term_idx: idx, rx: 0, ry: 0, rw: 0, rh: 0,
                         anchor: caret, head: caret, active: false },
    };
    sel.active = false; // keyboard selection is not a drag

    let (mut hl, mut hc) = sel.head;
    match dir {
        SelDir::Left => {
            if hc > 0 { hc -= 1; }
            else if hl > oldest { hl -= 1; hc = line_len(hl); }
        }
        SelDir::Right => {
            if hc < line_len(hl) { hc += 1; }
            else if hl < term.total { hl += 1; hc = 0; }
        }
        SelDir::Up => {
            if hl > oldest { hl -= 1; hc = hc.min(line_len(hl)); }
        }
        SelDir::Down => {
            if hl < term.total { hl += 1; hc = hc.min(line_len(hl)); }
        }
    }
    let new_head = (hl, hc);
    let changed = new_head != sel.head;
    sel.head = new_head;
    if sel.anchor == sel.head { *g = None; } else { *g = Some(sel); }
    changed
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
    let cols = cols as usize;

    // Snapshot the last `rows` logical lines. Each wraps to ≥1 screen row,
    // so this is always enough to fill the viewport bottom-up.
    let lines: alloc::vec::Vec<(alloc::vec::Vec<u8>, usize)> = term.visible_lines(visible_rows)
        .map(|(data, len)| {
            let mut v = alloc::vec![0u8; len];
            v.copy_from_slice(&data[..len]);
            (v, len)
        })
        .collect();

    // Soft-wrap each logical line into `cols`-wide screen-row segments
    // (logical index, byte start, byte end) so long lines wrap instead of
    // running off the right edge. Then show the bottom-most `rows` segments.
    let mut segs: alloc::vec::Vec<(usize, usize, usize)> = alloc::vec::Vec::new();
    for (li, (_, len)) in lines.iter().enumerate() {
        if *len == 0 { segs.push((li, 0, 0)); continue; }
        let mut s = 0usize;
        while s < *len {
            let e = (s + cols).min(*len);
            segs.push((li, s, e));
            s = e;
        }
    }
    let first_seg = segs.len().saturating_sub(visible_rows);

    // Absolute line number of the first snapshot line — lets each wrapped
    // segment map back to (abs_line) for the selection test below.
    let start_line = (term.total + 1)
        .saturating_sub(term.scroll_offset)
        .saturating_sub(lines.len());
    // Selection bounds for THIS terminal, if a selection covers it.
    let sel_bounds_opt = match SELECTION.lock().as_ref() {
        Some(s) if s.term_idx == terminal_idx as usize => Some(sel_bounds(s)),
        _ => None,
    };
    let sel_color = theme_selection();

    let fg = theme_fg();
    let prompt_color = theme_prompt();

    for (row, &(li, s, e)) in segs[first_seg..].iter().enumerate() {
        let py = y + row as u32 * char_h;
        if py + char_h > y + h { break; }
        if e == s { continue; }
        let line_data = &lines[li].0;
        // Special colouring (system [npk] / `path> ` prompt) only on the
        // FIRST wrapped row of a logical line; continuation rows are plain.
        let first = s == 0;
        if let Ok(text) = core::str::from_utf8(&line_data[s..e]) {
            if first && text.starts_with("[npk]") {
                crate::gui::font::draw_str(shadow, info, "[npk]", x, py, prompt_color, None, 1);
                if e > 5 {
                    if let Ok(rest) = core::str::from_utf8(&line_data[5..e]) {
                        let rx = x + 5 * char_w;
                        if !draw_status_row(shadow, info, rest, rx, py, char_w) {
                            crate::gui::font::draw_str(shadow, info, rest, rx, py, fg, None, 1);
                        }
                    }
                }
            } else if first {
                if let Some(pos) = text.find("> ") {
                    let prompt_end = pos + 2; // include "> "
                    crate::gui::font::draw_str(shadow, info, &text[..prompt_end], x, py, prompt_color, None, 1);
                    if text.len() > prompt_end {
                        crate::gui::font::draw_str(shadow, info, &text[prompt_end..], x + prompt_end as u32 * char_w, py, fg, None, 1);
                    }
                } else {
                    crate::gui::font::draw_str(shadow, info, text, x, py, fg, None, 1);
                }
            } else {
                crate::gui::font::draw_str(shadow, info, text, x, py, fg, None, 1);
            }
        }

        // Selection highlight — overdraw the selected byte range of this
        // wrapped segment with a muted-accent background so the glyphs read
        // as selected. Per-line column range intersected with this seg [s,e).
        if let Some((lo, hi)) = sel_bounds_opt {
            let abs_line = start_line + li;
            if abs_line >= lo.0 && abs_line <= hi.0 {
                let line_len = lines[li].1;
                let csel0 = if abs_line == lo.0 { lo.1 } else { 0 };
                let csel1 = if abs_line == hi.0 { hi.1 } else { line_len };
                let a = csel0.max(s);
                let b = csel1.min(e);
                if b > a {
                    if let Ok(seltext) = core::str::from_utf8(&line_data[a..b]) {
                        crate::gui::font::draw_str(shadow, info, seltext,
                            x + (a - s) as u32 * char_w, py, fg, Some(sel_color), 1);
                    }
                }
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
        // No cache — fallback: clear with the theme surface color.
        crate::gui::render::fill_rect(shadow, info,
            win_cx, last_line_y, win_cw, char_h, theme_bg());
    }

    let (line_data, len) = term.current_line();
    let visible_len = len.min(cols as usize);
    if visible_len > 0 {
        let prompt_color = theme_prompt();
        let fg = theme_fg();
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
        let cursor_color = theme_fg();
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
        let prompt_color = theme_prompt();
        let fg = theme_fg();
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
        let cursor_color = theme_fg();
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

/// (total logical lines, current scroll_offset) for terminal `idx` —
/// used by the compositor to draw + drag the scrollbar.
pub fn scroll_metrics(idx: usize) -> Option<(usize, usize)> {
    term_ref(idx).map(|t| (t.total, t.scroll_offset))
}

/// Set the absolute scroll_offset (logical lines from the bottom) for
/// terminal `idx`, clamped. Used by the scrollbar drag.
pub fn set_scroll_offset(idx: usize, off: usize) {
    if let Some(term) = term_mut(idx) {
        let max_scroll = term.total.saturating_sub(1);
        term.scroll_offset = off.min(max_scroll);
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub fn restore_input_with_cursor(buf: &mut [u8]) -> (usize, usize) {
    let idx = ACTIVE_IDX.load(Ordering::Acquire) as usize;
    let term = match term_ref(idx) { Some(t) => t, None => return (0, 0) };
    let len = term.saved_pos.min(buf.len());
    buf[..len].copy_from_slice(&term.saved_input[..len]);
    (len, term.saved_cursor.min(len))
}

/// Restore the saved input buffer from the active terminal (legacy, cursor=pos).
#[allow(dead_code)]
pub fn restore_input(buf: &mut [u8]) -> usize {
    let (pos, _) = restore_input_with_cursor(buf);
    pos
}

/// Write the prompt string to the active terminal buffer.
#[allow(dead_code)]
pub fn write_prompt() {
    if !is_active() { return; }
    let cwd = crate::intent::get_cwd_for_shell();
    let path = if cwd.is_empty() { "/" } else { cwd.as_str() };
    let prompt = alloc::format!("{}> ", path);
    set_prompt_len(prompt.len());
    write(&prompt);
}

// ── Stream Sinks for Remote Mirroring ──────────────────────────
//
// Each terminal slot may have a byte ringbuffer that captures all output
// (from write() and write_idx()). Used by debug.wasm to mirror a terminal
// over TCP. Drop-oldest when full. Allocated on first open, freed on close.

const STREAM_CAPACITY: usize = 65536;

struct StreamBuf {
    data: Mutex<VecDeque<u8>>,
}

impl StreamBuf {
    fn new() -> Self {
        Self { data: Mutex::new(VecDeque::with_capacity(STREAM_CAPACITY)) }
    }

    fn push(&self, bytes: &[u8]) {
        let mut q = self.data.lock();
        let overflow = (q.len() + bytes.len()).saturating_sub(STREAM_CAPACITY);
        for _ in 0..overflow { q.pop_front(); }
        q.extend(bytes.iter().copied());
    }

    fn pop_into(&self, dst: &mut [u8]) -> usize {
        let mut q = self.data.lock();
        let n = q.len().min(dst.len());
        for i in 0..n { dst[i] = q.pop_front().unwrap(); }
        n
    }
}

static STREAM_SINKS: [AtomicPtr<StreamBuf>; MAX_SLOTS] = {
    const NULL: AtomicPtr<StreamBuf> = AtomicPtr::new(core::ptr::null_mut());
    [NULL; MAX_SLOTS]
};

/// A sink that gets EVERY write, whichever terminal it was addressed to.
///
/// Per-slot mirroring is not what a remote console wants: output is routed by
/// the per-core redirect, so a background message goes to the primary loop, a
/// command's output to the loop it was typed in, and a failing path may print
/// from a core with no redirect at all. A mirror bound to one index then goes
/// quiet while the machine is still talking — observed as "it takes my commands
/// but sends nothing back".
static GLOBAL_SINK: AtomicPtr<StreamBuf> = AtomicPtr::new(core::ptr::null_mut());

/// Open a stream sink for a terminal. Idempotent — calling twice is a no-op.
pub fn stream_open(idx: usize) -> bool {
    if idx >= MAX_SLOTS { return false; }
    if !STREAM_SINKS[idx].load(Ordering::Acquire).is_null() { return true; }
    let ptr = Box::into_raw(Box::new(StreamBuf::new()));
    match STREAM_SINKS[idx].compare_exchange(
        core::ptr::null_mut(), ptr, Ordering::AcqRel, Ordering::Acquire
    ) {
        Ok(_) => true,
        Err(_) => {
            // SAFETY: lost race, reclaim our unused allocation
            unsafe { drop(Box::from_raw(ptr)); }
            true
        }
    }
}

/// Read buffered bytes into dst. Returns bytes read (0 if empty or not open).
pub fn stream_read(idx: usize, dst: &mut [u8]) -> usize {
    if idx >= MAX_SLOTS { return 0; }
    let ptr = STREAM_SINKS[idx].load(Ordering::Acquire);
    if ptr.is_null() { return 0; }
    // SAFETY: ptr remains valid until stream_close swaps it out.
    // stream_close must not be called concurrently with stream_read on the
    // same idx (enforced by single-reader convention: one debug.wasm module).
    unsafe { (*ptr).pop_into(dst) }
}

/// Close a stream sink, freeing its buffer.
pub fn stream_close(idx: usize) {
    if idx >= MAX_SLOTS { return; }
    let ptr = STREAM_SINKS[idx].swap(core::ptr::null_mut(), Ordering::AcqRel);
    if !ptr.is_null() {
        // SAFETY: swap gave us exclusive ownership of ptr.
        unsafe { drop(Box::from_raw(ptr)); }
    }
}

/// Internal: push bytes to sink if active. Called from write() and write_idx().
fn stream_push(idx: usize, s: &str) {
    let g = GLOBAL_SINK.load(Ordering::Acquire);
    if !g.is_null() {
        // SAFETY: ptr valid until stream_close_global. push uses internal Mutex.
        unsafe { (*g).push(s.as_bytes()); }
    }
    if idx >= MAX_SLOTS { return; }
    let ptr = STREAM_SINKS[idx].load(Ordering::Acquire);
    if !ptr.is_null() {
        // SAFETY: ptr valid until stream_close. push uses internal Mutex.
        unsafe { (*ptr).push(s.as_bytes()); }
    }
}

/// Open/read/close the everything-sink. Index -1 on the ABI.
pub fn stream_open_global() -> bool {
    if !GLOBAL_SINK.load(Ordering::Acquire).is_null() { return true; }
    let ptr = Box::into_raw(Box::new(StreamBuf::new()));
    match GLOBAL_SINK.compare_exchange(
        core::ptr::null_mut(), ptr, Ordering::AcqRel, Ordering::Acquire
    ) {
        Ok(_) => true,
        Err(_) => { unsafe { drop(Box::from_raw(ptr)); } true }
    }
}

pub fn stream_read_global(dst: &mut [u8]) -> usize {
    let ptr = GLOBAL_SINK.load(Ordering::Acquire);
    if ptr.is_null() { return 0; }
    // SAFETY: valid until stream_close_global; pop_into takes the inner Mutex.
    unsafe { (*ptr).pop_into(dst) }
}

pub fn stream_close_global() {
    let ptr = GLOBAL_SINK.swap(core::ptr::null_mut(), Ordering::AcqRel);
    if !ptr.is_null() {
        // SAFETY: swapped out, no new reader can obtain it.
        unsafe { drop(Box::from_raw(ptr)); }
    }
}
