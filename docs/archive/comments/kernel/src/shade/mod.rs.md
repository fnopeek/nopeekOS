# `kernel/src/shade/mod.rs` @ 5e0102684

## L1-8 · `pub mod glass;`

```
//! Shade — nopeekOS Compositor
//!
//! Native Rust compositor layer. Manages windows, Z-order, damage tracking,
//! shadebar (status bar), terminal rendering, and keybindings.
//!
//! Architecture:
//!   Keyboard → Input (Super keybinds) → Compositor → Framebuffer
//!   kprintln → Terminal buffer → Window content rendering
```

## L23-25 · `pub const PERF_LOG: bool = false;`

```
/// Per-frame render/compositor performance logging ([render], [comp],
/// [chrome-cache], [gpu-blit], [rw-phase], [nvme-spin] …). Off for normal
/// use; flip to `true` to re-enable the breakdown for future perf work.
```

## L36 · `pub(crate) static COMPOSITOR: Mutex<Option<Compositor>> = Mutex::new(None);`

```
/// Global compositor instance.
```

## L39 · `static ACTIVE: AtomicBool = AtomicBool::new(false);`

```
/// Lock-free active flag (avoids COMPOSITOR lock from xHCI poll context).
```

## L42-43 · `#[allow(dead_code)]`

```
/// True while a render task is queued/running on a worker core.
/// Prevents flooding the scheduler with duplicate render tasks.
```

## L47 · `fn layers_usable() -> bool {`

```
/// Check if layer system is usable (initialized AND matches current framebuffer).
```

## L54 · `pub fn init() {`

```
/// Initialize shade compositor. Call after login + GPU setup.
```

## L61 · `crate::layers::init(screen_w, screen_h, pitch);`

```
// Initialize layer compositor (3 layers: Background, Chrome, Text)
```

## L64 · `if crate::layers::is_initialized() {`

```
// Render background into Layer 0
```

## L76 · `cursor::init_atomic(screen_w, screen_h);`

```
// Initialize lock-free cursor position (centered, screen bounds for clamping)
```

## L79 · `let fb_info = framebuffer::get_info();`

```
// Cache framebuffer MMIO address for IRQ-safe cursor draw
```

## L86 · `terminal::set_active(true);`

```
// Enable terminal capture + GUI mode (no window yet — Mod+Enter opens first loop)
```

## L91 · `pub fn with_compositor<F, R>(f: F) -> Option<R>`

```
/// Execute a closure with exclusive access to the compositor.
```

## L97-99 · `if r.is_some() && crate::smp::per_core::current_core_id() != 0 {`

```
// A change from another core (a widget app focusing, flashing, closing a
// window) is something Core 0 has to act on — and Core 0 no longer looks
// every 10 ms (stage 3e). Tell it. After the lock, not under it.
```

## L106 · `#[allow(dead_code)]`

```
/// Create a new window. Returns the window ID, or None if no terminal slots.
```

## L112 · `#[allow(dead_code)]`

```
/// Close and remove a window.
```

## L118-119 · `pub fn create_surface_window(title: &str) -> Option<WindowId> {`

```
/// Create a Surface-kind window (microvm framebuffer tile). Returns
/// its id, or None if the compositor isn't up yet.
```

## L124 · `#[allow(dead_code)]`

```
/// Set focus to a window.
```

## L130-131 · `const WIDGET_SCROLL_STEP: i32 = 48;`

```
/// Pixels a single mouse-wheel notch scrolls a widget app's
/// `Widget::Scroll` (≈3 text lines).
```

## L134 · `const SCROLLBAR_STRIP: i32 = 16;`

```
/// Width (px) of the clickable scrollbar strip at a window's right edge.
```

## L137 · `static SCROLL_DRAG: spin::Mutex<Option<ScrollDragState>> = spin::Mutex::new(None);`

```
/// Active scrollbar drag (set on press over the bar, cleared on release).
```

## L145-146 · `horizontal: bool,`

```
/// Sideways drag along a TextArea's bottom edge. The whole mapping runs
/// on the other axis then: track = `vw`, cursor = mx.
```

## L149 · `max_px: u32,            // widget: max scroll offset`

```
// widget: max scroll offset
```

## L150 · `total: usize, rows: usize, // terminal: logical lines / visible rows`

```
// terminal: logical lines / visible rows
```

## L159 · `#[inline]`

```
/// Bottom-edge counterpart of `in_scroll_strip`.
```

## L166-168 · `fn apply_scroll_drag(s: &ScrollDragState, my: i32) {`

```
/// Map a drag cursor position to a scroll offset and apply it. `my` is the
/// cursor along the bar's own axis — Y for the usual vertical bar, X for a
/// TextArea's bottom one.
```

## L194 · `let max_lines = (s.total as i64 - s.rows as i64).max(0);`

```
// offset 0 = bottom; thumb at top = max scrollback.
```

## L204-206 · `fn scrollbar_grab(mx: i32, my: i32) -> Option<ScrollDragState> {`

```
/// Try to grab a scrollbar under `(mx, my)`. Returns the drag state if the
/// cursor is on a scrollable window's right-edge strip (and not on the
/// close button).
```

## L221-222 · `if let Some((vp, max_px)) = widgets::scroll_viewport_x_of(hit.window) {`

```
// Bottom edge first: where the two strips overlap in the corner, the
// vertical bar is the one drawn there, so it must win the grab.
```

## L241-242 · `fn handle_scrollbar_drag(mx: i32, my: i32, lmb: bool, was: bool) -> bool {`

```
/// Drive a scrollbar drag from the current mouse state. Returns true if it
/// consumed the event (caller skips the normal focus/window-drag path).
```

## L260-262 · `pub fn focused_is_terminal() -> bool {`

```
/// True if the focused window is a terminal (loop) window — used to route
/// the mouse wheel to the terminal scrollback, and by the intent loop to
/// decide whether to drive a terminal session at all.
```

## L271-273 · `pub fn focused_widget_id() -> Option<u32> {`

```
/// If the currently focused window is a widget-kind window, return its id.
/// The intent loop uses this to route key events to the widget's event queue
/// instead of the terminal input path.
```

## L286-291 · `pub fn focused_surface_id() -> Option<u32> {`

```
/// If the focused window is a Surface-kind (microvm) window, return
/// its id. run_loop uses this to route input correctly: shade
/// keybinds (Mod+Q etc.) still work so the window is manageable;
/// other keys are swallowed for now (Phase B forwards them to the
/// guest's virtio-input). Without this a focused Surface window
/// wedges the terminal input path (terminal_idx 255, no session).
```

## L304-310 · `pub fn forward_pointer_to_guest(evt: &crate::xhci::MouseEvent) {`

```
/// Forward a host pointer event into the focused Surface window's
/// guest virtio-input eventq as an **absolute** pointer (qemu
/// usb-tablet model): cursor position relative to the tile content
/// rect, scaled to `0..ABS_MAX`, plus button transitions and wheel.
/// Resolution-independent — the guest scales `ABS_MAX` onto its own
/// display, so host-side tile scaling never desyncs the guest cursor.
/// Additive to `handle_mouse` (which still draws the host cursor).
```

## L326 · `let rect = with_compositor(|comp| {`

```
// Content rect of the focused Surface window.
```

## L336 · `_ => return, // no Surface focused → no-op (called on every move)`

```
// no Surface focused → no-op (called on every move)
```

## L347-349 · `let packed = ((ax as u64) << 32) | ay as u64;`

```
// Position — emit only on change. Absolute semantics mean a
// dropped intermediate sample only coarsens tracking; the final
// position is always exact (this also bounds INPUT_Q pressure).
```

## L357 · `let prev = LAST_BTN.swap(evt.buttons, Ordering::Relaxed);`

```
// Buttons — emit only on transition (bit 0=L 1=R 2=M).
```

## L369 · `if evt.scroll != 0 {`

```
// Wheel — Linux reads `value` as __s32; pass the signed delta.
```

## L389 · `static FORCE_REDRAW_PENDING: AtomicBool = AtomicBool::new(false);`

```
/// A full redraw asked for from another core, waiting for Core 0.
```

## L392-398 · `pub fn force_redraw() {`

```
/// Force a full redraw (e.g. after wallpaper change).
///
/// Only Core 0 composes. The wallpaper module runs on a worker core, and
/// composing from there raced Core 0 into the same back buffer: the
/// terminal-glass cache could capture a half-mixed frame under the NEW key
/// and keep showing it until the key changed again — which only a
/// light/dark switch did. From any other core this just leaves a note.
```

## L405 · `terminal::invalidate_input_cache();`

```
// Invalidate input line cache — will be rebuilt by render_window
```

## L408-410 · `compositor::clear_chrome_cache();`

```
// Drop the terminal-glass cache: its key doesn't track the wallpaper behind
// the translucent surface, so a same-theme wallpaper swap would keep the old
// backdrop until a theme change. A full redraw must re-composite the glass.
```

## L413 · `if crate::layers::is_initialized() {`

```
// Re-render background into Layer 0 if layers are active
```

## L429 · `pub fn render_frame() {`

```
/// Draw the entire compositor state to the framebuffer.
```

## L434-439 · `fn render_frame_cursor_only() {`

```
/// Move the cursor WITHOUT recompositing the scene. The front buffer
/// already holds the composed scene with the cursor baked in at its old
/// spot + the clean pixels under it saved. Restore those (erase old
/// cursor), save-under + bake at the new spot, and blit only the affected
/// bounding box. NO comp.render, NO bg memcpy — that per-move recomposite
/// was the bare-metal cost (+watts/stutter) whenever a window was open.
```

## L447 · `cursor::restore_under(front, &info);       // erase old cursor in RAM`

```
// erase old cursor in RAM
```

## L448 · `cursor::save_under_and_bake(front, &info);  // save clean + bake new`

```
// save clean + bake new
```

## L449 · `blit_cursor_bbox(fb, old, had_old);         // one atomic small blit`

```
// one atomic small blit
```

## L453 · `static SHELL_FP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
/// `Compositor::shell_fingerprint` of the last full frame.
```

## L456-459 · `fn note_shell_fingerprint(fp: Option<u64>) {`

```
/// What the panels show changed with this frame: tell them
/// (`notify::TOPIC_WINDOWS`), instead of letting them ask three times a
/// second. Called by both full-frame paths with the fingerprint taken under
/// the compositor lock.
```

## L478-481 · `fn blit_cursor_bbox(fb: &framebuffer::FbConsole, old: (i32, i32), had_old: bool) {`

```
/// Blit the bounding box of the old ∪ new cursor position from the front
/// buffer to MMIO — one shot, so the move is atomic (no flicker). The old
/// position in the front buffer is clean scene again (cursor restored +
/// re-baked at the new spot), so this both erases the old and draws the new.
```

## L498-501 · `fn render_frame_layered() {`

```
/// Full recomposite → blit. Scene is composed CLEAN into the back buffer
/// (no cursor), swapped to front, then the cursor is saved-under + baked
/// into the front and the whole frame blitted (cursor included → atomic,
/// no flicker; incl. the dock reveal animation + the microvm tile).
```

## L512 · `if let Some((bg_buf, _, _, _)) = crate::layers::buffer(crate::layers::LAYER_BG) {`

```
// Render scene to BACK buffer (front stays stable for cursor)
```

## L515 · `unsafe { core::ptr::copy_nonoverlapping(bg_buf, back, size); }`

```
// SAFETY: bg_buf and back are valid for size bytes
```

## L532-534 · `cursor::save_under_and_bake(fb.front_ptr(), &info);`

```
// Bake the cursor into the (now) front buffer, saving the clean
// pixels under it so a later pure move can erase it without a
// recompose. Carried by the full blit below → atomic, no flicker.
```

## L553-560 · `fn render_frame_surface() {`

```
/// Like `render_frame_layered` but for a Surface-only update (a guest/browser
/// FLUSH): recomposite the scene (cheap RAM work) yet blit ONLY the surface
/// tile rect(s) + the cursor to MMIO — not the whole screen. The full-screen
/// MMIO blit is the dominant bare-metal (GOP framebuffer) cost; a 60 Hz guest
/// doing it every frame saturated the framebuffer and starved cursor blits
/// → laggy mouse whenever an app was up. Only the surface pixels changed, so
/// the rest of the front buffer already matches what's on screen. See
/// project-baremetal-gfx-perf ("clip the MMIO blit = the main win").
```

## L573 · `unsafe { core::ptr::copy_nonoverlapping(bg_buf, back, size); }`

```
// SAFETY: bg_buf and back are valid for size bytes.
```

## L578-579 · `let mut surf_rects = [(0u32, 0u32, 0u32, 0u32); 4];`

```
// Recomposite the whole scene into back (correct w.r.t. overlays),
// collecting Surface tile rects while the lock is held.
```

## L588-595 · `let full = (win.x, win.y, win.width, win.height);`

```
// Blit only the guest's damage rect (content-clipped,
// translated to screen coords) instead of the whole
// tile — at 4K that's the difference between a tens-of-MB
// MMIO blit and just the changed region. comp.render
// already produced correct pixels in `back` everywhere,
// and nothing but this tile changed, so a partial blit is
// correct. Falls back to the full tile if no damage was
// recorded or the clamp degenerates.
```

## L619-620 · `let old = cursor::saved_pos();`

```
// Capture the old cursor rect (to erase a ghost if it moved this
// frame) BEFORE save_under_and_bake overwrites the saved position.
```

## L626-627 · `let gpu_ok = if crate::gpu::supports_blit() {`

```
// On a HW-blit host (native Xe / BCS) a full blit is cheap → keep it.
// Otherwise (GOP) clip the MMIO blit to the tiles + cursor bbox.
```

## L639 · `let (cw, ch) = cursor::cursor_size();`

```
// Cursor bbox (old ∪ new) so a move during this frame ghosts not.
```

## L652-655 · `static SURF_RENDER_COUNT: AtomicU64 = AtomicU64::new(0);`

```
// [surf-render] rolling cost of a full surface composite+blit — the real
// per-frame Core-0 work whose saturation (at the guest's 60 Hz, full-tile)
// stutters the cursor + spikes network rxlat. Tells us the per-blit µs so
// we can size the FPS cap above.
```

## L661-663 · `static MIX_LAYERED: AtomicU64 = AtomicU64::new(0);`

```
// [render-mix] which render path actually fires (a slow desktop with an app
// open may be re-rendering the whole scene on mouse-move instead of the cheap
// cursor-only path). Counts: layered, surface, cursor-only, legacy.
```

## L685 · `static GPU_BLIT_OK: AtomicU64 = AtomicU64::new(0);`

```
// [gpu-blit] does the real full-frame BCS blit succeed, and how long?
```

## L721 · `SURF_BG_SUM.fetch_add(t_bg.saturating_sub(t0), Ordering::Relaxed);`

```
// Sub-phase breakdown: bg memcpy | comp.render | swap+cursor | blit.
```

## L743 · `fn render_frame_legacy() {`

```
/// Legacy render with double-buffer (fallback when layers not initialized).
```

## L779-782 · `fn try_gpu_blit(fb: &framebuffer::FbConsole, pitch: u32, _w: u32, h: u32) -> bool {`

```
/// Try GPU BCS blit from front shadow → MMIO framebuffer.
/// Returns true if GPU blit succeeded, false = caller should CPU-blit.
/// Note: VSync via wait_vblank adds up to 16.7ms latency per render — not suitable
/// for on-demand compositing. True VSync needs PLANE_SURF double-buffer flip (TODO).
```

## L788-792 · `if src_ggtt == 0 { return false; }`

```
// dst_ggtt == 0 is VALID: when the firmware scanout sits at GGTT offset 0
// (Tiger Lake blit-only, fw PLANE_SURF=0), that IS the live scanout. Only
// a missing shadow mapping (src == 0) means "not set up". Treating dst 0 as
// invalid was silently routing the whole frame back to the 100ms CPU/UC
// blit even though BCS is up + readback-verified.
```

## L795-798 · `let t = crate::interrupts::rdtsc();`

```
// [gpu-blit] instrument: does the real (full 4K) blit succeed, and how
// long does submit+poll take? A small readback blit verified fine but a
// 33MB blit may time out submit_blit's completion poll → false → CPU
// fallback (the 100ms we still measure).
```

## L805 · `#[allow(dead_code)]`

```
/// Render only damaged regions (efficient partial update).
```

## L821-827 · `let old = cursor::saved_pos();`

```
// Erase → repaint → re-bake, never a bare `draw_cursor_on_shadow`:
// this is a PARTIAL repaint, so nothing here overwrites a cursor
// baked at the previous position. It went unnoticed while every
// mouse move was followed by `render_frame_cursor_only`, which
// cleaned up after it — but a scrollbar drag consumes the pointer
// event and leaves this pass as the only one running, so the
// cursor smeared a copy across the whole drag.
```

## L841-844 · `pub fn tick_focus_glow() {`

```
/// Fade the focus flash down to the steady halo, repainting only its band —
/// nothing inside the window changes, so a full frame per tick would be pure
/// waste. Every loop that idles while shade is up has to call this, or the
/// flash stands still at full strength until something else repaints.
```

## L853 · `fn render_focus_glow() {`

```
/// Repaint just the focus halo band (see `Compositor::render_focus_glow`).
```

## L860-866 · `let old = cursor::saved_pos();`

```
// The cursor is BAKED into the front buffer. Baking another one
// per tick — which is what a plain `draw_cursor_on_shadow` does —
// left a copy at every position the mouse passed through: this
// pass repaints only the halo band, so nothing overwrites them
// (they vanished later, when a full render came along). Erase,
// repaint, bake at the new spot: the same save/restore dance
// `render_frame_cursor_only` does for every partial repaint.
```

## L886 · `let old = cursor::saved_pos();`

```
// Same dance as the layered path — see the note there.
```

## L900 · `pub fn is_active() -> bool {`

```
/// Check if shade compositor is active (lock-free, safe from any context).
```

## L905 · `pub fn take_deferred_render() -> bool {`

```
/// Check and clear deferred render flag (for callers with their own mouse loop).
```

## L910-912 · `pub fn request_render() {`

```
/// Request a full shade render on the next Core-0 poll cycle. Safe to
/// call from any core (worker cores executing WASM etc.) — actual
/// MMIO work still runs on Core 0.
```

## L914 · `if !DEFERRED_RENDER.swap(true, Ordering::AcqRel) {`

```
// The shell loop on Core 0 renders; wake it on the edge (stage 3e).
```

## L920-925 · `pub fn needs_tick() -> bool {`

```
/// Does the shell loop have to come back within one frame even without an
/// event? True while something moves on its own — a swap animation, the
/// screenshot flash, the focus glow, the dock (its dwell and debounce count
/// calls, and it slides) — or a guest frame waits out its rate cap, a close
/// request waits for its answer, or a render is still pending. Otherwise the
/// shell sleeps until something wakes it (stage 3e).
```

## L935-936 · `fn with_compositor_local<F, R>(f: F) -> Option<R>`

```
/// `with_compositor` without the cross-core wake — for Core 0's own
/// questions, which must not wake itself.
```

## L944 · `pub fn handle_action(action: input::ShadeAction) {`

```
/// Process a shade action (called from intent loop).
```

## L948-953 · `let modal_open = with_compositor(|c| c.has_modal_window()).unwrap_or(false);`

```
// Modal gate: while ANY window is marked modal (set via
// `npk_window_set_modal` from the app), suppress every action that
// would steal focus or reshape the grid underneath. CloseWindow,
// SpawnLauncher, and Lock stay allowed so the user can still
// dismiss / relaunch the overlay. Kernel knows nothing about which
// app set itself modal — the flag is per-window.
```

## L972 · `crate::intent::reset_session_prompt(terminal::active_idx());`

```
// Terminal was freshly allocated (cleared) — reset session prompt
```

## L976 · `}`

```
// If not created: no free terminals, silently ignore
```

## L979-980 · `with_compositor(|comp| {`

```
// compositor::close_window handles widget-scene cleanup
// via remove_scene when the closed window's kind is Widget.
```

## L983-985 · `if !comp.is_panel(id) {`

```
// Never close a panel (dock/bar) via Mod+Q — they are
// managed chrome. Focus shouldn't land on one, but guard
// here too in case some path focuses a panel.
```

## L1086-1089 · `if let Some(wid) = focused_widget_id() {`

```
// Focused widget app with scrollable content → scroll its
// Widget::Scroll; otherwise the terminal scrollback. (scroll_by
// locks the compositor internally, so resolve the target first
// and release the lock before calling it — no re-entrancy.)
```

## L1118 · `}`

```
// Lock handled by intent loop
```

## L1123-1125 · `ShadeAction::Copy => {`

```
// Ctrl+Shift+C / V. In a terminal (where Ctrl+C stays SIGINT) copy
// the mouse selection / paste into the input line; in a widget app
// copy or paste the focused Input/TextArea selection.
```

## L1143-1151 · `fn spawn_launcher() {`

```
/// Spawn the configured launcher module as a widget app.
///
/// Module name is read from `sys/config/launcher` (single line, e.g.
/// `drun`). When the file is missing or unreadable we fall back to
/// `drun` so a fresh install still has a working Mod+D out of the box.
///
/// The kernel knows nothing else about the module — overlay size,
/// modal behaviour, and UI are entirely the module's responsibility
/// (see `npk_window_set_overlay` / `npk_window_set_modal`).
```

## L1155-1156 · `let name: alloc::string::String = match crate::npkfs::fetch("sys/config/launcher") {`

```
// Resolve the configured name. Limit to 64 bytes to prevent
// surprises from a giant config file.
```

## L1174-1175 · `let already_open = with_compositor(|comp| {`

```
// Already running? Re-focus instead of spawning a second instance.
// Window title matches the module name, set by the spawn path.
```

## L1196-1197 · `let rights = crate::capability::widget_rights_from_wasm(&bytes);`

```
// Grant exactly the rights the app declares in its `.npk.caps`
// section (per-app, no blanket WRITE); absent → safe default.
```

## L1204-1206 · `if !crate::wasm::spawn_widget_app(bytes.to_vec(), module_cap, &name, 0) {`

```
// Spawn with widget_wid = 0 — the module itself decides whether
// to be an overlay and modal. Window is created on its first
// scene_commit (or npk_window_set_overlay call).
```

## L1212-1222 · `pub fn start_autostart() {`

```
/// Launch an installed widget module by name (e.g. "loft") from kernel
/// code — the spawn half of `spawn_launcher`, exposed for cross-boundary
/// actions (the 9p "open in loft" trigger from the microvm browser).
/// MUST run on Core 0 (touches the compositor). Re-focuses an existing
/// window of that name instead of spawning a duplicate.
/// Launch every app named in the `autostart` config (comma/space-
/// separated) via the widget-spawn path — a fresh window, no terminal,
/// no `APP_RUNNING` capture (so background overlays like the dock don't
/// hijack keyboard focus the way `run <module>` would). Called once
/// after `shade::init()`. The kernel stays generic: the app names live
/// in config, not here. Set with e.g. `set autostart dock`.
```

## L1234-1239 · `if !first {`

```
// Stagger the launches. Every entry is fetched from npkFS, spawned as a
// fiber and starts working immediately; firing them together makes the
// first seconds after boot the busiest the machine ever is. A hardware
// driver brought up in that window is running handshakes against
// firmware timeouts it cannot extend — measured: the WiFi driver comes
// up reliably when started by hand and not at all from autostart.
```

## L1251 · `const AUTOSTART_STAGGER_TICKS: u64 = 40; // 400 ms`

```
/// Gap between autostart launches, in 100 Hz ticks.
```

## L1252 · `const AUTOSTART_STAGGER_TICKS: u64 = 40; // 400 ms`

```
// 400 ms
```

## L1258-1262 · `pub fn launch_app_with_ttl(name: &str, ttl_ticks: Option<u64>) {`

```
/// `ttl_ticks` = None for a resident service. Autostart entries are exactly
/// that: a WiFi driver, a supplicant, the dock. The default 600 000 ticks are
/// 100 minutes at 100 Hz — after which every gated host call of a still-running
/// module starts failing, which looks like the device breaking, not like a
/// capability expiring.
```

## L1274-1278 · `let job = alloc::boxed::Box::new(LaunchJob { name: alloc::string::String::from(name), ttl_ticks });`

```
// **Loading the module happens on a worker** (`docs/plan/CORES_AND_EVENTS.md`,
// stage 3). Reading it from npkFS, decrypting it and checking its hash
// took milliseconds to tens of milliseconds for a large module — on
// Core 0, between the click and the next frame. The check above stays
// here: it needs the compositor.
```

## L1294-1295 · `let job = unsafe { alloc::boxed::Box::from_raw(arg as *mut LaunchJob) };`

```
// SAFETY: `arg` is the `Box<LaunchJob>` leaked by `launch_app_with_ttl`,
// handed to exactly one task.
```

## L1303-1304 · `let rights = crate::capability::widget_rights_from_wasm(&bytes);`

```
// Per-app rights from the module's `.npk.caps` section (no blanket
// WRITE); absent → safe default.
```

## L1315-1316 · `pub fn render_input_line() {`

```
/// Fast re-render of just the current input line (for live typing feedback).
/// Uses INPUT_LINE_CACHE from render_window for pixel-perfect background match.
```

## L1333-1335 · `let old = cursor::saved_pos();`

```
// Partial repaint → erase, paint, re-bake (see
// `render_damaged_layered`). A bare bake would leave the
// pointer standing wherever it was when someone typed.
```

## L1346-1348 · `blit_cursor_bbox(fb, old, had_old);`

```
// Unconditional: the bake above already moved the cursor
// in the shadow, so skipping the blit when nothing was
// painted would strand the old one on screen for good.
```

## L1356-1358 · `pub fn poll_render() {`

```
/// Progressive render: if terminal has new output, re-render the focused window's text.
/// Fast path: only redraws text layer. Call from net::poll().
/// Only Core 0 may call this — compositor + framebuffer are not thread-safe.
```

## L1367 · `let animating = with_compositor(|comp| comp.tick_animation()).unwrap_or(false);`

```
// Tick swap animation (smooth window transition)
```

## L1373-1374 · `if with_compositor(|comp| comp.flash_tick()).unwrap_or(false) {`

```
// Screenshot flash — its own tick so it also runs while nothing else
// is moving (a capture usually happens on a still desktop).
```

## L1381 · `if with_compositor(|comp| comp.tick_close_requests()).unwrap_or(false) {`

```
// Reap close requests a guarded app never answered.
```

## L1386-1388 · `let (_, cy) = cursor::atomic_pos();`

```
// Drive the auto-hide dock. Feed it the current cursor Y so the reveal
// dwell / hide debounce advance even while the cursor is parked, then
// advance the slide. Re-render the frame while it's moving.
```

## L1398 · `while let Some(evt) = crate::xhci::poll_mouse() {`

```
// Process each mouse event with clean cursor restore + redraw
```

## L1404 · `if DEFERRED_RENDER.swap(false, Ordering::Relaxed) {`

```
// Deferred scene redraw (drag resize/swap sets this to avoid blocking event loop)
```

## L1406 · `SURFACE_DIRTY.store(false, Ordering::Relaxed); // a full render covers the tile too`

```
// a full render covers the tile too
```

## L1410-1420 · `let now = crate::interrupts::ticks();`

```
// Guest produced a new frame. On HW the guest reports 100% damage
// every frame, so this is always a full-tile blit — ~16 MB of MMIO
// at 4K. Run unthrottled at the guest's 60 Hz it saturates Core 0,
// which ALSO drains the NIC RX ring, so saturation spiked network
// rxlat into the 100 ms range and the host cursor stuttered across
// ALL windows. So FPS-cap the heavy blit: at most once per
// SURFACE_MIN_TICKS normally, and stretch the cap to
// SURFACE_MAX_STALE_TICKS while the mouse is actively moving (the
// browser briefly drops fps) so the cursor stays smooth — the gaps
// are filled with the cheap cursor-only render. Coalescing is free:
// SURFACE_DIRTY stays set and we render the latest frame when due.
```

## L1437 · `render_frame_cursor_only();`

```
// Pure mouse move, nothing else changed → save-under cursor move.
```

## L1445-1446 · `framebuffer::with_fb(|fb| {`

```
// Partial render: only re-render dirty or focused windows (not all).
// Focus change marks old+new windows dirty. Terminal output marks focused dirty.
```

## L1451-1452 · `let cur_old = cursor::saved_pos();`

```
// Erase the baked cursor before repainting windows; it's re-baked
// + blitted at the end so the partial update stays flicker-free.
```

## L1458 · `for win in &mut comp.windows {`

```
// Propagate per-terminal dirty flags to window dirty flags
```

## L1466-1468 · `let dirty_rects: alloc::vec::Vec<(u32, u32, u32, u32)> = comp.windows.iter()`

```
// Any overlay that sits on top of a dirty non-overlay must
// repaint too — otherwise the window below paints over it
// (classic top-refresh-punches-through-drun bug).
```

## L1487-1488 · `let mut others_repainted = false;`

```
// Wurde ausser dem fokussierten Fenster noch eines gemalt?
// Siehe die Begruendung hinter der Schleife.
```

## L1490-1491 · `let render_order: alloc::vec::Vec<crate::shade::window::WindowId> =`

```
// Render back-to-front so overlays paint last (otherwise a
// terminal behind drun would overwrite drun's pixels).
```

## L1494-1496 · `let active_border = comp.border_active();`

```
// One source of truth for the tile borders: the palette. It
// already folds in the wallpaper accent (`accent = auto`) and
// any `set accent` override, so there is no second theme path.
```

## L1500-1502 · `let glow = match comp.windows.iter().find(|w| w.id == wid) {`

```
// Skip checks and halo up front: the paint below needs a &mut
// to clear `dirty`, while the halo has to be resolved against
// the whole compositor (gaps, flash state).
```

## L1515-1518 · `if !win.is_overlay {`

```
// Restore window region from BG layer. Overlays skip
// this — we want their rounded-out corners to keep
// whatever scene (terminal, another app, wallpaper)
// sits below in the current shadow state.
```

## L1545-1558 · `if others_repainted {`

```
// Der Hof zum SCHLUSS, sobald ausser dem fokussierten Fenster
// noch eines gemalt wurde.
//
// `render_window` stellt `Kasten + Band` aus der Wand wieder
// her, und das Band IST die Luecke (`glow_width` klemmt auf
// `gaps`) — also genau das Stueck Schirm, auf dem der Hof des
// NACHBARN liegt. Kommt der Nachbar in dieser Schleife nach dem
// fokussierten Fenster, wischt er dessen Hof in der Luecke weg.
// Was im eigenen Kasten steht, die vier Eckzwickel, wird mit dem
// fokussierten Fenster geblittet und bleibt stehen: ein Keil an
// der Ecke mit nichts darunter. Sichtbar nur bei MEHREREN
// Fenstern, weil es sonst keinen Nachbarn gibt, der die Luecke
// anfasst, und nur auf diesem Teilweg — ein voller Neuaufbau
// malt ohnehin alles in einem Zug.
```

## L1566-1568 · `cursor::save_under_and_bake(shadow, info);`

```
// Re-bake the cursor on top of the repainted windows + blit its
// region (atomic). Covers a cursor over a just-redrawn window and
// erases it at the old spot.
```

## L1574 · `#[allow(unreachable_code)]`

```
// Legacy partial render path (kept for reference)
```

## L1584 · `framebuffer::with_fb(|fb| {`

```
// Focused window text changed — use BG layer to restore background, then re-render window
```

## L1592 · `if let Some((bg_buf, _, _, _)) = crate::layers::buffer(crate::layers::LAYER_BG) {`

```
// Restore window region from BG layer
```

## L1609 · `let border_color = if win.focused { comp.border_active() } else { comp.border_inactive() };`

```
// Re-render window chrome + text on clean background
```

## L1642-1643 · `static DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);`

```
/// True while a Mod+LMB/RMB drag is active (swap or resize).
/// When set, handle_mouse enters slow path on EVERY event (not just button changes).
```

## L1646-1647 · `static DEFERRED_RENDER: AtomicBool = AtomicBool::new(false);`

```
/// Set by handle_mouse when a full scene redraw is needed but deferred.
/// poll_render picks this up AFTER processing all mouse events.
```

## L1649-1650 · `static CURSOR_MOVED: AtomicBool = AtomicBool::new(false);`

```
/// A pure mouse move occurred (no scene change). poll_render moves the
/// cursor via save-under (no recomposite) for it — cheap, flicker-free.
```

## L1653-1657 · `static SURFACE_DIRTY: AtomicBool = AtomicBool::new(false);`

```
/// A guest Surface produced a new frame (browser/microvm FLUSH). Distinct
/// from DEFERRED_RENDER: only the surface tile pixels changed, so the final
/// MMIO blit can be clipped to the tile rect instead of the whole screen.
/// The full-screen blit per 60 Hz guest frame is the dominant bare-metal
/// (GOP framebuffer) cost and starved the cursor → lag when an app is up.
```

## L1660-1663 · `static LAST_SURFACE_TICK: AtomicU64 = AtomicU64::new(0);`

```
/// Last tick (100 Hz) at which a Surface (guest) frame was actually
/// composited+blit. poll_render throttles the expensive 4K-tile surface
/// render in favour of the cheap cursor-only path while the host mouse is
/// moving, so a busy guest can't starve the cursor across all windows.
```

## L1666-1668 · `const SURFACE_MIN_TICKS: u64 = 2;`

```
/// Min ticks (100 Hz) between full-tile surface blits when the mouse is
/// idle — FPS-caps the ~16 MB 4K MMIO blit at ~50 fps so a 60 Hz guest
/// can't peg Core 0 (which also drains the NIC RX ring).
```

## L1670-1672 · `const SURFACE_MAX_STALE_TICKS: u64 = 5;`

```
/// Min ticks between surface blits while the mouse is actively moving —
/// the browser drops to ~20 fps so the cheap cursor-only path can keep the
/// cursor smooth in the gaps.
```

## L1675-1678 · `pub const SURFACE_CLIP_BLIT: bool = true;`

```
/// Clip the blit on a Surface flush to the tile rect (vs full-screen). Flag-
/// gated because the cursor/blit path has a flicker history (see
/// project-baremetal-gfx-perf) — flip to `false` to fall back to a full
/// `request_render` per guest frame (the old behavior).
```

## L1681-1683 · `pub fn request_surface_render() {`

```
/// A guest Surface produced a new frame → clipped recomposite+blit. Set from
/// `surface::write_frame`; consumed by `poll_render`. No-op (caller uses the
/// full `request_render`) when `SURFACE_CLIP_BLIT` is off.
```

## L1690-1697 · `pub fn request_cursor_move() {`

```
/// Signal a bare pointer move from the input IRQ. Picks the cheap cursor-
/// only render as the default; `handle_mouse` (run by the Core-0 loop /
/// poll_render before this flag is consumed) upgrades to a full
/// DEFERRED_RENDER if the event turns out to be a drag/click/scroll. The
/// IRQ previously called the full `request_render`, which forced a whole-
/// scene recomposite on *every* pointer delta and defeated `handle_mouse`'s
/// own CURSOR_MOVED logic (DEFERRED_RENDER is never cleared by it) — the
/// dominant framebuffer cost while a window/tile was open.
```

## L1704-1705 · `pub fn handle_mouse(evt: &crate::xhci::MouseEvent) {`

```
/// Process a mouse event: handle buttons/drag, redraw cursor.
/// Position is already updated by timer IRQ (process_mouse_report).
```

## L1707-1715 · `forward_pointer_to_guest(evt);`

```
// Forward to a focused microvm guest HERE, not at the call sites:
// poll_mouse is a single consuming ring with several Core-0
// consumers (this via poll_render:612, and the run_loop Surface
// branch). Whichever drains first must still forward, else the
// guest only gets events when the timing happens to favour the
// forwarding path (was: "only with Mod held" — Mod+drag shifted
// the race). handle_mouse is on every consumer's path, so this is
// race-free. forward_pointer_to_guest no-ops unless a Surface
// window is focused, so this is free in normal desktop use.
```

## L1718-1723 · `if evt.hscroll != 0 {`

```
// Waagrechtes Rollen — beim Touchpad zwei Finger nach links/rechts,
// bei einer Maus das Kippen des Rades. EIGENER Zweig vor dem
// senkrechten, und ohne `return`: ein schraeger Wisch kann beide
// Achsen tragen, und wer hier aussteigt, verschluckt die andere.
//
// Kein Zweig fuer das Terminal: sein Rueckblick hat nur eine Achse.
```

## L1735-1740 · `if evt.scroll != 0 {`

```
// Mouse wheel over a focused widget app → scroll its Widget::Scroll.
// (Surface/microvm windows already consumed the wheel in
// forward_pointer_to_guest above; terminal scrollback uses PageUp/Down.)
// The raw USB wheel never went through the ShadeAction::Scroll* path —
// that only fires for keyboard/serial — so wire it here. scroll_by locks
// the compositor internally, so it must run outside any with_compositor.
```

## L1743-1746 · `if crate::keyboard::is_ctrl_held() {`

```
// Ctrl+wheel is a zoom request, not a scroll. Sent to the app
// instead of moving the viewport — an app that doesn't zoom
// just ignores it (and deliberately doesn't scroll either,
// which is what every editor does).
```

## L1755-1756 · `if crate::keyboard::is_shift_held() {`

```
// Shift+wheel scrolls sideways — the editor idiom, and the only
// way to reach a long line with a mouse that has one wheel.
```

## L1764-1765 · `widgets::push_event(wid, widgets::abi::Event::Wheel { dy: delta });`

```
// No Widget::Scroll consumed it → forward to the app so a
// Canvas/surface app (e.g. the browser) can scroll itself.
```

## L1769 · `if evt.scroll > 0 { terminal::scroll_up(3); } else { terminal::scroll_down(3); }`

```
// loop/terminal scrollback — wheel up = older lines.
```

## L1780-1782 · `{`

```
// Scrollbar drag (loop + widget windows). Handled before the
// compositor's focus/window-drag logic so dragging the bar never moves
// or refocuses windows; a consumed event short-circuits the rest.
```

## L1791-1792 · `{`

```
// Terminal text selection (drag to mark; Ctrl+Shift+C copies). After the
// scrollbar strip so a press on the bar scrolls instead of selecting.
```

## L1799-1800 · `let (hx, hy) = cursor::atomic_pos();`

```
// Hover routing — deduplicated per window. Runs on every move so the
// app can react even when no buttons changed.
```

## L1813-1822 · `{`

```
// Drag motion → the focused widget app. Only while its primary button
// is held, i.e. during an actual drag: hover is resolved above,
// compositor-side, precisely so apps never see a firehose of moves.
// A drag is the part only the app can interpret (panning a zoomed
// canvas, rubber-banding), and it is bounded by the button being down.
// Addressed to the FOCUSED window, not the one under the cursor, so a
// drag that leaves the window keeps arriving.
//
// This has to live here, on the real motion path: `handle_mouse` on
// the compositor looks like the right home but has no callers at all.
```

## L1857-1858 · `DEFERRED_RENDER.store(true, Ordering::Relaxed);`

```
// DEFER: don't render inside the event loop — blocks cursor.
// poll_render renders AFTER all events are drained.
```

## L1869-1871 · `{`

```
// Widget text selection — after the button block (press_at has focused
// the clicked Input/TextArea) and independent of it (so pure drag-moves,
// which raise no button change, still extend the selection).
```

## L1880-1883 · `if !is_button_change`

```
// If nothing above scheduled a real render (no button, no drag, no
// DEFERRED set by a hover/focus change), this was a pure positional
// move → flag a cheap cursor-only render (recompose + blit just the
// cursor rects). Otherwise the pending full render carries the cursor.
```

## L1894-1900 · `pub fn refresh_focused_terminal() {`

```
/// Terminal drag-selection state machine. A press inside a terminal's text
/// area begins a selection; holding + moving extends it; release ends it.
/// Geometry is captured at press so a drag can run past the window edge.
/// Doesn't consume the event — a plain LMB drag in a terminal never moves
/// the window, so the normal focus/click flow runs alongside.
/// Re-render the focused terminal so a selection change (mouse or keyboard)
/// shows. Marks the terminal + its window dirty, then repaints damage.
```

## L1928-1929 · `terminal::selection_begin(hit.term_idx as usize, rx, ry, rw, rh, mx, my);`

```
// Collapsed selection draws nothing; the click's focus
// redraw covers the (empty) initial state.
```

## L1937-1938 · `fn update_widget_slider(mx: i32, my: i32, lmb: bool, was: bool) -> bool {`

```
/// `Widget::Slider` drag. Returns true while the gesture belongs to a
/// slider, so a press on one does not also start a text selection.
```

## L1952-1955 · `fn update_widget_text_selection(mx: i32, my: i32, lmb: bool, was: bool) {`

```
/// Widget text drag-selection for the focused Input/TextArea. Mirrors
/// `update_terminal_selection`; the widget helpers re-render themselves.
/// Runs AFTER the compositor's button block so `press_at` has already moved
/// focus onto the clicked text widget.
```

## L1967-1970 · `match widgets::click_run_at(wid, mx, my) {`

```
// Der wievielte Klick dieser Reihe? Zwei waehlen das Wort, drei die
// ganze Zeile — und `click_run` zaehlt IMMER mit, auch wenn der
// Druck danach kein Textfeld trifft, sonst laeuft eine Reihe weiter,
// die woanders stattfindet.
```

## L1978 · `pub fn stop() {`

```
/// Stop shade compositor.
```

## L1987 · `pub fn default_config() -> &'static [(&'static str, &'static str, &'static str)] {`

```
/// Get shade config defaults.
```

