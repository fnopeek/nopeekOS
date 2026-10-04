# `kernel/src/shade/terminal.rs` @ 5e0102684

## L1-4 · `use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};`

```
//! Shade Terminal — per-window text buffers for independent terminal sessions.
//!
//! Each window gets its own TerminalBuffer. kprintln output goes to the
//! active (focused) terminal. Windows are completely independent.
```

## L11 · `const MAX_LINES: usize = 1000;`

```
/// Maximum lines and columns in each terminal buffer.
```

## L16-17 · `const MAX_SLOTS: usize = 256;`

```
/// Terminal slots — u8 index range, only pointers stored statically (~2KB).
/// Actual TerminalBuffers (~264KB each) are heap-allocated on demand.
```

## L20 · `pub struct TerminalBuffer {`

```
/// Terminal text buffer (one per window, heap-allocated on demand).
```

## L24 · `total: usize,`

```
/// Total lines written (wraps in ring buffer).
```

## L26 · `col: usize,`

```
/// Current cursor column.
```

## L28 · `pub scroll_offset: usize,`

```
/// View scroll offset (lines from bottom, 0 = latest).
```

## L30 · `saved_input: [u8; MAX_INPUT],`

```
/// Saved input state (for window focus switching).
```

## L70 · `if self.col > 0 {`

```
// Backspace: move cursor left, shrink line length
```

## L75 · `if self.col < self.lens[idx] {`

```
// Only shrink lens if we're at the end
```

## L103 · `pub fn visible_lines(&self, visible_rows: usize) -> impl Iterator<Item = (&[u8], usize)> {`

```
/// Get visible lines for rendering (respects scroll_offset).
```

## L117 · `pub fn current_line(&self) -> (&[u8], usize) {`

```
/// Get the current (bottom) line content for fast input rendering.
```

## L124 · `static TERM_PTRS: [AtomicPtr<TerminalBuffer>; MAX_SLOTS] = {`

```
/// Heap-allocated terminal buffers. Pointer is non-null when slot is in use.
```

## L130 · `fn term_ref(idx: usize) -> Option<&'static TerminalBuffer> {`

```
/// Get a shared reference to a terminal buffer (None if slot empty).
```

## L136-137 · `fn term_mut(idx: usize) -> Option<&'static mut TerminalBuffer> {`

```
/// Get a mutable reference to a terminal buffer (None if slot empty).
/// SAFETY: Only called from Core 0 or with exclusive access (output redirect).
```

## L143-144 · `static ACTIVE_IDX: AtomicU8 = AtomicU8::new(0);`

```
/// Currently active (focused) terminal index — drives rendering + is the
/// interactive I/O target.
```

## L146-148 · `static PRIMARY_IDX: AtomicU8 = AtomicU8::new(255);`

```
/// The "primary" terminal — the first loop opened. Spontaneous kernel debug
/// (kprintln with no per-core output redirect) lands here so it stays put
/// instead of chasing window focus across multiple loops. 255 = none open.
```

## L151 · `static DIRTY: AtomicBool = AtomicBool::new(false);`

```
/// Set when new content is written (cleared after render).
```

## L154-156 · `fn set_dirty() {`

```
/// Mark the terminal for repaint and, on the clean→dirty edge, wake the
/// shell on Core 0 — it no longer re-checks every 10 ms (stage 3e). Only
/// the edge, or every `kprint` from a worker would send an IPI.
```

## L163 · `static mut INPUT_CURSOR_POS: usize = 0;`

```
/// Input cursor position (for rendering blinking cursor on input line).
```

## L166-168 · `const INPUT_LINE_CACHE_MAX: usize = 3840 * 4 * 64; // max 4K width × 4 bytes × 64px font height`

```
/// Cached background pixels for the input line (saved after full render).
/// Avoids re-blending on every keystroke — just restore + draw text.
// Heap-allocated input line cache (allocated on first use, avoids 983KB BSS bloat)
```

## L169 · `const INPUT_LINE_CACHE_MAX: usize = 3840 * 4 * 64; // max 4K width × 4 bytes × 64px font height`

```
// max 4K width × 4 bytes × 64px font height
```

## L177 · `pub fn set_cursor_pos(pos: usize) {`

```
/// Set the input cursor position (called from intent loop on every key/move).
```

## L179 · `unsafe { INPUT_CURSOR_POS = pos; }`

```
// SAFETY: single-core
```

## L183-185 · `pub fn rewrite_input(input: &[u8], input_len: usize) {`

```
/// Rewrite the input portion of the current terminal line.
/// Keeps the prompt intact, overwrites from `prompt_len` onward with `input`,
/// and clears any trailing chars from the previous content.
```

## L192-194 · `let prompt_len = unsafe { PROMPT_LEN };`

```
// Find prompt length: everything already on the line before user input starts.
// The prompt ends at the current col minus whatever the caller's pos is.
// But we don't know the prompt length directly. Instead, we store it.
```

## L197 · `let max = MAX_COLS.min(prompt_len + input_len);`

```
// Rewrite from prompt_len onward
```

## L202 · `for i in max..term.lens[line_idx] {`

```
// Clear any trailing chars (line got shorter)
```

## L211 · `static mut PROMPT_LEN: usize = 0;`

```
/// Stored prompt length for the active terminal.
```

## L214 · `pub fn set_prompt_len(len: usize) {`

```
/// Set the prompt length (called after write_prompt).
```

## L216 · `unsafe { PROMPT_LEN = len; }`

```
// SAFETY: single-core
```

## L220 · `pub fn current_line_len() -> usize {`

```
/// Get the current line length in the active terminal (for cursor offset calculation).
```

## L226 · `#[allow(dead_code)]`

```
/// Get the current (input) line data and length from the active terminal.
```

## L238 · `#[allow(dead_code)]`

```
/// Get total line count in the active terminal (for input line Y calculation).
```

## L245 · `pub fn cursor_pos() -> usize {`

```
/// Get the input cursor position.
```

## L247 · `unsafe { INPUT_CURSOR_POS }`

```
// SAFETY: single-core
```

## L251 · `pub fn set_active(active: bool) {`

```
/// Enable/disable terminal capture.
```

## L260-261 · `pub fn allocate() -> Option<u8> {`

```
/// Allocate a new terminal buffer on the heap. Returns index or None if out of memory.
/// Uses alloc_zeroed to avoid 264KB stack frame (kernel stack is only 256KB).
```

## L265-267 · `let layout = alloc::alloc::Layout::new::<TerminalBuffer>();`

```
// SAFETY: TerminalBuffer is ~264KB — too large for the 256KB kernel stack.
// Allocate zeroed memory directly on the heap and cast to TerminalBuffer.
// All fields are zero-initialized (arrays of 0, usize=0, bool=false).
```

## L272 · `let _ = PRIMARY_IDX.compare_exchange(`

```
// First loop opened → it becomes the primary debug sink.
```

## L281 · `pub fn free(idx: u8) {`

```
/// Free a terminal buffer (returns heap memory).
```

## L285 · `let layout = alloc::alloc::Layout::new::<TerminalBuffer>();`

```
// SAFETY: ptr was created by alloc_zeroed in allocate()
```

## L289-290 · `if PRIMARY_IDX.load(Ordering::Acquire) == idx {`

```
// If the primary just closed, hand primary to the lowest surviving loop
// (the next-oldest by index), or 255 when none remain.
```

## L303 · `pub fn active_idx() -> u8 {`

```
/// Get the active terminal index.
```

## L308 · `pub fn set_active_terminal(idx: u8) {`

```
/// Set which terminal receives kprintln output.
```

## L313 · `pub fn clear() {`

```
/// Clear the active terminal buffer.
```

## L321 · `pub fn clear_idx(idx: usize) {`

```
/// Clear a specific terminal by index (for WASM apps on worker cores).
```

## L330-331 · `static CORE_OUTPUT: [AtomicU8; 256] = {`

```
/// Per-core output redirect (indexed by LAPIC ID, 255 = no redirect).
/// Workers set this before intent dispatch so kprintln goes to the right terminal.
```

## L337 · `pub fn set_output_redirect(terminal_idx: u8) {`

```
/// Set output redirect for the current core (call before intent dispatch on worker).
```

## L341 · `let apic_id = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) } >> 24;`

```
// SAFETY: APIC MMIO is identity-mapped, reading LAPIC ID register
```

## L346 · `pub fn output_redirect_terminal() -> Option<u8> {`

```
/// Get the output redirect terminal for the current core (None if no redirect / Core 0).
```

## L350 · `let apic_id = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) } >> 24;`

```
// SAFETY: APIC MMIO is identity-mapped, reading LAPIC ID register
```

## L356 · `pub fn clear_output_redirect() {`

```
/// Clear output redirect for the current core.
```

## L364-365 · `pub fn write(s: &str) {`

```
/// Write to the active terminal (called from serial::write_str).
/// If the current core has an output redirect set, writes to that terminal instead.
```

## L368-370 · `stream_push(usize::MAX, s);`

```
// The screen may be gone; the remote console is not. Anything printed
// before the terminal exists (or after it goes away) is exactly what a
// developer watching from outside needs most.
```

## L375 · `let apic_base = crate::interrupts::apic_base();`

```
// Check per-core output redirect (workers running intents)
```

## L378 · `let apic_id = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) } >> 24;`

```
// SAFETY: APIC MMIO is identity-mapped
```

## L387-392 · `let primary = PRIMARY_IDX.load(Ordering::Acquire);`

```
// Default (Core 0, no per-core redirect): the PRIMARY terminal — the
// first loop — so background/system debug stays put instead of following
// window focus. The interactive run loop brackets its prompt + command
// output with a Core-0 redirect to the focused terminal, so those still
// land where the user is typing. Falls back to the active terminal if no
// primary is set (shouldn't happen once a loop exists).
```

## L406 · `static TERM_DIRTY: [AtomicBool; MAX_SLOTS] = {`

```
/// Per-terminal dirty flags (set by worker-core WASM output, read by poll_render).
```

## L412 · `pub fn write_idx(idx: usize, s: &str) {`

```
/// Write to a specific terminal by index (for WASM apps on worker cores).
```

## L422 · `pub fn is_term_dirty(idx: usize) -> bool {`

```
/// Check if a specific terminal has new content.
```

## L427 · `pub fn clear_term_dirty(idx: usize) {`

```
/// Clear per-terminal dirty flag.
```

## L432 · `pub fn is_dirty() -> bool {`

```
/// Check if terminal has new content since last render.
```

## L437 · `pub fn mark_dirty() {`

```
/// Mark terminal as dirty (triggers partial re-render on next poll_render).
```

## L442 · `pub fn clear_dirty() {`

```
/// Clear dirty flag (called after render).
```

## L447-449 · `fn theme_fg() -> u32 {`

```
/// Terminal foreground (text + cursor) from the active theme's OnSurface
/// token — so `loop` windows turn dark-on-light in light mode, matching
/// the widget apps. Masked to opaque RGB (the shadow buffer is opaque).
```

## L455 · `fn theme_bg() -> u32 {`

```
/// Terminal background from the active theme's Surface token.
```

## L461-464 · `fn theme_prompt() -> u32 {`

```
/// Prompt / `[npk]` accent from the active theme. The widget Accent token
/// is contrast-adjusted against the surface (darkens on a light surface),
/// unlike the raw wallpaper accent — so the prompt stays readable in light
/// mode too.
```

## L470-471 · `fn theme_selection() -> u32 {`

```
/// Selection highlight colour from the active theme (muted accent behind
/// the selected glyphs).
```

## L481-496 · `const STATUS_MARKERS: &[u8] = b"+.!*";`

```
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
```

## L509-511 · `fn token_color(tok: &str, base: u32, accent: u32, faint: u32) -> u32 {`

```
/// Colour of one whitespace-delimited token on a status line.
/// `(…)` is the parenthetical the emitters use for sizes and asides, so it
/// steps back; a version number is the thing you actually came to read.
```

## L525-527 · `fn draw_status_row(shadow: *mut u8, info: &crate::framebuffer::FbInfo,`

```
/// Draw one row of terminal text, `[npk]` prefix already handled by the
/// caller. Returns false when the row is not a status line and the caller
/// should draw it plainly.
```

## L542-543 · `let mut i = first + 2;`

```
// Tokens keep their byte offset, so every glyph stays in the column it
// would have had — selection and wrapping still count plain bytes.
```

## L560-566 · `#[derive(Clone, Copy)]`

```
// ── Mouse text selection (drag to mark, Ctrl+Shift+C to copy) ─────────
//
// A cell is identified by (absolute logical line, byte column). Absolute
// line numbers survive scrolling; they alias back into the ring via
// `% MAX_LINES`, so a selection older than MAX_LINES lines is silently
// clamped (transient — you copy right after selecting). Only one terminal
// carries a selection at a time.
```

## L571 · `rx: i32, ry: i32, rw: u32, rh: u32,`

```
/// Text rect captured at press so a drag can clamp even off-window.
```

## L573 · `anchor: (usize, usize),  // (abs_line, col)`

```
// (abs_line, col)
```

## L575 · `active: bool,            // true while the mouse button is held`

```
// true while the mouse button is held
```

## L580 · `fn sel_bounds(s: &Selection) -> ((usize, usize), (usize, usize)) {`

```
/// Ordered (lo, hi) endpoints — tuples compare (line, then col).
```

## L585-588 · `pub fn cell_at(idx: usize, rx: i32, ry: i32, rw: u32, rh: u32, mx: i32, my: i32)`

```
/// Map a screen pixel to a terminal cell `(abs_line, col)`, clamped to the
/// grid. Mirrors the geometry in `render_to_window` exactly (monospace
/// `char_size(1)`, `visible_lines` snapshot, soft-wrap into `cols`-wide
/// segments) so the highlight lands on the glyph under the cursor.
```

## L605 · `let mut segs: alloc::vec::Vec<(usize, usize, usize)> = alloc::vec::Vec::new();`

```
// Soft-wrap each logical line into cols-wide segments (same as render).
```

## L620 · `let col = (s + col_in_seg).min(e); // clamp within this seg's byte range`

```
// clamp within this seg's byte range
```

## L624-625 · `pub fn selection_begin(idx: usize, rx: i32, ry: i32, rw: u32, rh: u32, mx: i32, my: i32) -> bool {`

```
/// Begin a selection at the click cell (collapsed). No-op if the point
/// resolves outside the grid.
```

## L638-640 · `pub fn selection_extend(mx: i32, my: i32) -> bool {`

```
/// Extend the active selection's moving end to `(mx, my)`. Returns true if
/// the selection changed (caller re-renders). Clamps to the press-time
/// rect, so dragging past the window edge keeps extending.
```

## L650-651 · `pub fn selection_end() -> bool {`

```
/// End the drag. A collapsed (single-click) selection is dropped so a plain
/// click doesn't leave a zero-width highlight. Returns true if state changed.
```

## L664 · `pub fn selection_dragging() -> bool {`

```
/// True while a drag is in progress (caller keeps routing moves to us).
```

## L669-670 · `pub fn copy_selection() {`

```
/// Copy the current selection to the kernel clipboard. Gathers each covered
/// logical line's byte range, trims trailing spaces, joins with '\n'.
```

## L683 · `while seg_end > c0 && term.lines[ridx][seg_end - 1] == b' ' { seg_end -= 1; }`

```
// Trim trailing spaces on the copied segment (avoids grid padding).
```

## L695-698 · `pub fn paste_clipboard() {`

```
/// Paste the clipboard into the focused terminal by injecting its bytes into
/// the keyboard stream — the interactive loop then consumes them exactly as
/// typed input. Newlines act as Enter (run the line), matching terminals.
/// Bounded to the keyboard ring so a huge paste can't overflow it.
```

## L711 · `if injected >= 256 { break; } // keyboard ring is 512; leave headroom`

```
// keyboard ring is 512; leave headroom
```

## L715-717 · `pub fn clear_selection(idx: usize) -> bool {`

```
/// Drop the selection if it belongs to terminal `idx` (its content just
/// changed under it, e.g. on clear or a plain key — the absolute lines no
/// longer map / the user moved on). Returns true if a selection was dropped.
```

## L723 · `#[derive(Clone, Copy)]`

```
/// Direction for keyboard (Shift+arrow) selection.
```

## L727-730 · `pub fn selection_key(idx: usize, dir: SelDir) -> bool {`

```
/// Extend a terminal selection one cell via Shift+arrow. Anchors at the
/// current input caret `(total, INPUT_CURSOR_POS)` when starting fresh; a
/// mouse selection already present is continued from its moving end. Moves
/// are clamped to the ring-valid range. Returns true if it changed.
```

## L734 · `let oldest = term.total.saturating_sub(MAX_LINES - 1); // first ring-valid line`

```
// first ring-valid line
```

## L744 · `sel.active = false; // keyboard selection is not a drag`

```
// keyboard selection is not a drag
```

## L770 · `pub fn render_to_window(`

```
/// Render a specific terminal's content into a window region.
```

## L788-789 · `let lines: alloc::vec::Vec<(alloc::vec::Vec<u8>, usize)> = term.visible_lines(visible_rows)`

```
// Snapshot the last `rows` logical lines. Each wraps to ≥1 screen row,
// so this is always enough to fill the viewport bottom-up.
```

## L798-800 · `let mut segs: alloc::vec::Vec<(usize, usize, usize)> = alloc::vec::Vec::new();`

```
// Soft-wrap each logical line into `cols`-wide screen-row segments
// (logical index, byte start, byte end) so long lines wrap instead of
// running off the right edge. Then show the bottom-most `rows` segments.
```

## L813-814 · `let start_line = (term.total + 1)`

```
// Absolute line number of the first snapshot line — lets each wrapped
// segment map back to (abs_line) for the selection test below.
```

## L818 · `let sel_bounds_opt = match SELECTION.lock().as_ref() {`

```
// Selection bounds for THIS terminal, if a selection covers it.
```

## L833-834 · `let first = s == 0;`

```
// Special colouring (system [npk] / `path> ` prompt) only on the
// FIRST wrapped row of a logical line; continuation rows are plain.
```

## L849 · `let prompt_end = pos + 2; // include "> "`

```
// include "> "
```

## L862-864 · `if let Some((lo, hi)) = sel_bounds_opt {`

```
// Selection highlight — overdraw the selected byte range of this
// wrapped segment with a muted-accent background so the glyphs read
// as selected. Per-line column range intersected with this seg [s,e).
```

## L884-885 · `pub fn render_input_line(`

```
/// Fast render: only the current input line of the active terminal.
/// Returns the blit region (x, y, w, h) or None.
```

## L899 · `let visible_rows = rows as usize;`

```
// Calculate Y position of the last visible line
```

## L905 · `let cache_valid = unsafe { INPUT_LINE_CACHE_VALID && !INPUT_LINE_CACHE.is_null() };`

```
// Restore cached background pixels (saved after full render_window).
```

## L920 · `unsafe {`

```
// SAFETY: single-core, bounds checked above
```

## L931 · `crate::gui::render::fill_rect(shadow, info,`

```
// No cache — fallback: clear with the theme surface color.
```

## L954 · `let cur = cursor_pos();`

```
// Draw text cursor (solid bar at cursor position)
```

## L965-966 · `pub fn render_input_line_to_layer(`

```
/// Layer-based input line render: clear text region + draw text + cursor.
/// No background cache needed — text layer is transparent, composited on top.
```

## L985 · `let pitch = info.pitch as usize;`

```
// Clear the input line region in text layer (transparent)
```

## L993 · `unsafe { core::ptr::write_bytes(text_buf.add(off), 0, bytes); }`

```
// SAFETY: bounds checked
```

## L998 · `let (line_data, len) = term.current_line();`

```
// Draw text
```

## L1017 · `let cur = cursor_pos();`

```
// Draw text cursor
```

## L1028-1029 · `pub fn cache_input_line_bg(`

```
/// Cache the input line background from the shadow buffer after a full render.
/// Called from render_window after drawing the focused window.
```

## L1048 · `unsafe {`

```
// Allocate cache on first use (avoids 983KB BSS)
```

## L1057 · `unsafe {`

```
// SAFETY: single-core, bounds checked, cache allocated above
```

## L1076 · `pub fn invalidate_input_cache() {`

```
/// Invalidate the input line cache (call when window layout changes).
```

## L1081 · `pub fn scroll_up(lines: usize) {`

```
/// Scroll the active terminal up (show older content).
```

## L1091 · `pub fn scroll_down(lines: usize) {`

```
/// Scroll the active terminal down (show newer content).
```

## L1100-1101 · `pub fn scroll_metrics(idx: usize) -> Option<(usize, usize)> {`

```
/// (total logical lines, current scroll_offset) for terminal `idx` —
/// used by the compositor to draw + drag the scrollbar.
```

## L1106-1107 · `pub fn set_scroll_offset(idx: usize, off: usize) {`

```
/// Set the absolute scroll_offset (logical lines from the bottom) for
/// terminal `idx`, clamped. Used by the scrollbar drag.
```

## L1116 · `pub fn scroll_reset() {`

```
/// Reset scroll to bottom (show latest content).
```

## L1122 · `pub fn restore_cursor() {`

```
/// Restore cursor position from per-terminal saved state.
```

## L1132 · `#[allow(dead_code)]`

```
/// Save the current input buffer + cursor position to the active terminal's saved state.
```

## L1138 · `pub fn save_input_with_cursor(buf: &[u8], pos: usize, cursor: usize) {`

```
/// Save input buffer, pos, and cursor position.
```

## L1148 · `#[allow(dead_code)]`

```
/// Restore the saved input buffer from the active terminal. Returns (pos, cursor).
```

## L1158 · `#[allow(dead_code)]`

```
/// Restore the saved input buffer from the active terminal (legacy, cursor=pos).
```

## L1165 · `#[allow(dead_code)]`

```
/// Write the prompt string to the active terminal buffer.
```

## L1176-1180 · `const STREAM_CAPACITY: usize = 65536;`

```
// ── Stream Sinks for Remote Mirroring ──────────────────────────
//
// Each terminal slot may have a byte ringbuffer that captures all output
// (from write() and write_idx()). Used by debug.wasm to mirror a terminal
// over TCP. Drop-oldest when full. Allocated on first open, freed on close.
```

## L1213-1220 · `static GLOBAL_SINK: AtomicPtr<StreamBuf> = AtomicPtr::new(core::ptr::null_mut());`

```
/// A sink that gets EVERY write, whichever terminal it was addressed to.
///
/// Per-slot mirroring is not what a remote console wants: output is routed by
/// the per-core redirect, so a background message goes to the primary loop, a
/// command's output to the loop it was typed in, and a failing path may print
/// from a core with no redirect at all. A mirror bound to one index then goes
/// quiet while the machine is still talking — observed as "it takes my commands
/// but sends nothing back".
```

## L1223 · `pub fn stream_open(idx: usize) -> bool {`

```
/// Open a stream sink for a terminal. Idempotent — calling twice is a no-op.
```

## L1233 · `unsafe { drop(Box::from_raw(ptr)); }`

```
// SAFETY: lost race, reclaim our unused allocation
```

## L1240 · `pub fn stream_read(idx: usize, dst: &mut [u8]) -> usize {`

```
/// Read buffered bytes into dst. Returns bytes read (0 if empty or not open).
```

## L1245-1247 · `unsafe { (*ptr).pop_into(dst) }`

```
// SAFETY: ptr remains valid until stream_close swaps it out.
// stream_close must not be called concurrently with stream_read on the
// same idx (enforced by single-reader convention: one debug.wasm module).
```

## L1251 · `pub fn stream_close(idx: usize) {`

```
/// Close a stream sink, freeing its buffer.
```

## L1256 · `unsafe { drop(Box::from_raw(ptr)); }`

```
// SAFETY: swap gave us exclusive ownership of ptr.
```

## L1261-1272 · `pub fn stream_push_global(s: &str) {`

```
/// Push to the everything-sink ONLY, touching no terminal slot.
///
/// `npk_log_serial` exists precisely to bypass the terminal write path -- a
/// widget-only app may run when no terminal has a backing buffer, and
/// `kprintln` can stall there. That bypass also made every such app invisible
/// to the remote mirror, which calls itself the everything-sink and was not
/// one: beak, loft, spell, iris, drun, dock, bar, snap and volume all log this
/// way, so on hardware their output existed only on the physical UART.
///
/// This closes the gap without giving the bypass back what it was avoiding:
/// one atomic load and a push into the sink's own buffer, no terminal slot,
/// no terminal lock.
```

## L1276 · `unsafe { (*g).push(s.as_bytes()); }`

```
// SAFETY: ptr valid until stream_close_global. push uses internal Mutex.
```

## L1281 · `fn stream_push(idx: usize, s: &str) {`

```
/// Internal: push bytes to sink if active. Called from write() and write_idx().
```

## L1285 · `unsafe { (*g).push(s.as_bytes()); }`

```
// SAFETY: ptr valid until stream_close_global. push uses internal Mutex.
```

## L1291 · `unsafe { (*ptr).push(s.as_bytes()); }`

```
// SAFETY: ptr valid until stream_close. push uses internal Mutex.
```

## L1296 · `pub fn stream_open_global() -> bool {`

```
/// Open/read/close the everything-sink. Index -1 on the ABI.
```

## L1311 · `unsafe { (*ptr).pop_into(dst) }`

```
// SAFETY: valid until stream_close_global; pop_into takes the inner Mutex.
```

## L1318 · `unsafe { drop(Box::from_raw(ptr)); }`

```
// SAFETY: swapped out, no new reader can obtain it.
```

