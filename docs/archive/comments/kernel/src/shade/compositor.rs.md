# `kernel/src/shade/compositor.rs` @ 5e0102684

## L1-5 · `use alloc::vec::Vec;`

```
//! Compositor — manages windows, Z-order, tiling layout, and rendering.
//!
//! No per-window pixel buffers. Windows are metadata (position, size, state).
//! The compositor renders directly to the framebuffer shadow buffer.
//! Uses dwindle layout (Hyprland-style recursive binary split).
```

## L15 · `const DOCK_HOT_EDGE_PX: i32 = 2;`

```
/// Bottom hot-edge band (px) that arms the dock reveal.
```

## L17 · `const DOCK_REVEAL_DWELL_TICKS: u32 = 8;`

```
/// Ticks (100 Hz) the cursor must hold the hot-edge before revealing.
```

## L19 · `const DOCK_HIDE_DEBOUNCE_TICKS: u32 = 25;`

```
/// Ticks the cursor must be away from the dock before it hides again.
```

## L21 · `const DOCK_HIDE_MARGIN_PX: i32 = 6;`

```
/// Slack above the dock's top counted as "still over the dock".
```

## L23-25 · `const DOCK_HANDLE_H: u32 = 5;`

```
/// Height (1× px, scaled at draw time) of the collapsed-dock presence
/// bar — a thin, dock-width tray-coloured strip that hints "a dock lives
/// here" without reserving space or showing the icons.
```

## L27-28 · `const DOCK_BOTTOM_GAP: u32 = 12;`

```
/// Floating gap below the revealed dock (1× px, scaled), so it hovers
/// detached from the bottom edge the way the bar's pills do.
```

## L33-38 · `const GLOW_STEADY_ALPHA: u32 = 100;`

```
/// Focus halo. The focused tile bleeds a little accent into the gap around
/// it: a 1 px border in a wallpaper-derived colour is invisible on half the
/// wallpapers, a halo reads on all of them because it does not compete with
/// the colour underneath. It settles at `STEADY`; the moment focus lands it
/// starts at `FLASH` and fades down over `FLASH_TICKS` (100 Hz timer), which
/// is what makes a focus change catch the eye anywhere on screen.
```

## L42-43 · `const BORDER_ACTIVE_OPACITY: u32 = 235;`

```
/// Border blend of the focused / unfocused tile (0..256). The gap between the
/// two carries the steady-state distinction together with the halo.
```

## L47-49 · `#[derive(Clone, Copy)]`

```
/// What to paint around a window: `band` is the strip reserved for the halo
/// (restored from the wallpaper on every repaint, so a tile that LOST focus
/// erases its old halo), `alpha` is how strongly it is drawn right now.
```

## L61-64 · `const CLOSE_BTN_BOX: u32 = 26;`

```
/// Platform close button — the compositor draws a small "X" affordance in
/// the top-right corner of every real (non-panel) window so mouse users
/// can close it without remembering Mod+Q. Not per-app: provided here
/// once for all windows. Edge of the (square) hit box at 1× scale.
```

## L66-70 · `const CLOSE_BTN_BAND: u32 = 36;`

```
/// Assumed height of the app's top menu/toolbar band (loft/spell:
/// `Datei Bearbeiten …`) at 1× — the X is vertically centred in this
/// band so it lines up with the menu labels instead of clinging to the
/// very top edge. Windows without a menu bar (browser, terminal) just
/// get the X centred in their top ~band; close enough.
```

## L72-75 · `const CLOSE_BTN_GLYPH: u32 = 16;`

```
/// Close-X glyph size at 1×. MUST be an atlas-native size (16/24/32/…):
/// `icons::alpha_for` returns the nearest-not-smaller bitmap and the
/// blit below uses THAT size verbatim, so asking for 20 silently drew a
/// 24 px X — a quarter larger than intended.
```

## L78-84 · `fn close_btn_scale(_win: &Window, _scale: u32) -> u32 {`

```
/// The platform close-X is a FIXED pixel size — it never tracks the screen
/// HiDPI scale. Widget apps render their UI at fixed px (the widget rasterizer
/// runs at `RasterTarget.scale = 1`; apps size via density, not a HiDPI
/// factor), so a screen-scaled X balloons against the content on 4K. We tried
/// keeping the scale for terminals (whose text does grow 2×), but a 2× X on a
/// loop tile looked absurdly large next to the small widget X — Florian wants
/// every close-X the same small size. So: always 1×, on every window kind.
```

## L89-93 · `fn close_button_rect(win: &Window, border: u32, scale: u32)`

```
/// Screen rect `(x, y, w, h)` of a window's platform close button, or
/// `None` for panels (dock/bar are managed chrome — never closable here)
/// and windows too narrow to host the button. Shared by the renderer and
/// the click hit-test so the drawn disc and the clickable area always
/// coincide.
```

## L97-99 · `if win.is_dock || win.is_bar || win.is_overlay { return None; }`

```
// No X on managed chrome (dock/bar), on transient overlays (drun and
// other launchers — dismissed with Esc, not closed), or on Surface
// windows (the microvm browser owns its own close via its UI).
```

## L104-107 · `let band = CLOSE_BTN_BAND * scale;`

```
// The box is centred vertically in the app's menu-bar band so the X
// aligns with `Datei / Bearbeiten / …` rather than the top edge. Reuse
// that same vertical inset as the right-edge margin → equal gap on the
// top, bottom and right sides (Florian: "rundum gleicher Abstand").
```

## L116-125 · `struct ChromeCache { key: u64, w: u32, h: u32, px: Vec<u32> }`

```
/// Paint the platform close button — a bare X — into the shadow buffer,
/// vertically centred in the window's top menu-bar band. Drawn last in
// ── Terminal chrome cache ──────────────────────────────────────────────
// The translucent "glass" terminal background (bg_color blended over the
// wallpaper, per pixel) is the dominant compositor cost (~71ms for a
// maximised 4K terminal) and it's STATIC — only geometry / theme / focus /
// wallpaper change it, not the text drawn on top. Cache the rendered chrome
// region (wallpaper+border+glass) and memcpy it back each frame instead of
// re-blending. One entry (the common case is one focused terminal); a second
// terminal just thrashes it (still correct, recomputes on miss).
```

## L133-135 · `let wp_gen = crate::gui::background::wallpaper_generation();`

```
// The wallpaper generation is part of the key: the glass is composited over
// the backdrop pixels, so a same-theme wallpaper swap (which leaves every
// colour/geometry field unchanged) must still miss the cache and re-blend.
```

## L145-147 · `fn chrome_cache_blit(key: u64, x: u32, y: u32, w: u32, h: u32, shadow: *mut u8, info: &FbInfo) -> bool {`

```
/// On a cache hit, memcpy the cached chrome region into the back buffer and
/// return true. The text is drawn over it afterwards (every frame), so only
/// the static glass background is cached.
```

## L158 · `unsafe {`

```
// SAFETY: dst within fb (clamped), src within px (w*h, row<h).
```

## L170 · `fn chrome_cache_store(key: u64, x: u32, y: u32, w: u32, h: u32, shadow: *const u8, info: &FbInfo) {`

```
/// Capture the just-rendered chrome region from the back buffer into the cache.
```

## L179 · `unsafe {`

```
// SAFETY: src within fb (clamped), dst within px (w*h, row<h).
```

## L191-193 · `pub fn clear_chrome_cache() {`

```
/// Drop the cached terminal-glass blit. Called on every full redraw: the key
/// covers geometry, colours and the wallpaper generation, but not every glass
/// setting (`shade glass`), and a full redraw is when those change.
```

## L198-202 · `fn draw_close_button(shadow: *mut u8, info: &FbInfo, win: &Window,`

```
/// `render_window` so it sits over the window content. No disc / colour
/// highlight (Florian's call): just the glyph, sized and coloured like
/// the design's `close` — 16 px in a 26 px box, `OnSurfaceMuted`, so it
/// carries the same weight as the menu labels beside it and flips with
/// the theme (dark X on light, light X on dark).
```

## L207-209 · `let color = if matches!(win.kind, crate::shade::window::WindowKind::Terminal) {`

```
// Secondary text weight, like every other piece of window chrome —
// the close button shouldn't outshout the menu labels next to it.
// On loop's glass it is drawn like everything else on glass: dark theme.
```

## L234 · `#[derive(Clone, Copy)]`

```
/// Swap animation state — windows glide from old to new position.
```

## L239 · `pub a_from: (u32, u32, u32, u32), // x, y, w, h`

```
// x, y, w, h
```

## L244 · `pub duration: u64, // ticks (100Hz → 25 = 250ms)`

```
// ticks (100Hz → 25 = 250ms)
```

## L247 · `#[derive(Clone, Copy, PartialEq)]`

```
/// Drag mode: swap windows or resize split.
```

## L251 · `#[derive(Clone, Copy)]`

```
/// Window + geometry under the cursor for a scrollbar hit-test/drag.
```

## L257-258 · `pub text_rect: (i32, i32, u32, u32),`

```
/// Terminal text rect `(x, y, w, h)` in screen coords (content minus
/// the terminal padding) — the scrollbar track for terminals.
```

## L260 · `pub char_h: u32,`

```
/// Monospace cell height (px) — terminal rows = text_rect.h / char_h.
```

## L262 · `pub close_rect: Option<(u32, u32, u32, u32)>,`

```
/// The window's close-button rect, to exclude from the bar strip.
```

## L266 · `#[derive(Clone, Copy)]`

```
/// Drag state for Mod+LMB (swap) or Mod+RMB (resize).
```

## L271 · `pub last_target: Option<WindowId>,`

```
/// Last window we swapped with (prevent repeated swaps on same target).
```

## L273 · `pub start_mx: i32,`

```
/// Mouse position when drag started (for resize delta).
```

## L276 · `pub start_rw: i32,`

```
/// Resize delta when drag started.
```

## L279-281 · `pub w_target: Option<WindowId>,`

```
/// Which split line the drag moves per axis — resolved once at drag start
/// (see `resize_target`). The dragged window is rarely the one holding the
/// delta.
```

## L286-288 · `#[derive(Clone, Copy)]`

```
/// Auto-hide bottom dock state. The compositor owns the reveal/hide
/// slide; the dock app (`dock.wasm`) only declares itself via
/// `npk_window_set_dock` and renders its icon row.
```

## L292 · `pub thickness: u32,`

```
/// Dock (tray) height in px.
```

## L294-295 · `pub gap: u32,`

```
/// Floating gap below the tray when shown (px) — like the bar margin,
/// so the revealed dock hovers detached from the bottom edge.
```

## L297 · `pub target_shown: bool,`

```
/// Reveal target the hot-edge logic drives toward.
```

## L299-300 · `pub offset: u32,`

```
/// Current slide offset: 0 = fully shown (floating), `thickness + gap`
/// = fully hidden (below the screen edge).
```

## L302 · `pub dwell: u32,`

```
/// Ticks the cursor has held the bottom hot-edge (reveal dwell).
```

## L304 · `pub debounce: u32,`

```
/// Ticks the cursor has been away from the dock while shown (hide debounce).
```

## L308-312 · `#[derive(Clone, Copy)]`

```
/// Top strut panel registered by a bar app (`bar.wasm`) via
/// `npk_window_set_panel(Top, Strut)`. The app reports its height; the kernel
/// reserves a `margin + pill_h` band at the top and lays tiles below it. The
/// app owns ALL rendering — there is no native fallback, so an unregistered or
/// closed bar simply frees the band (tiles reclaim the full height).
```

## L316 · `pub pill_h: u32,`

```
/// Visible panel height in px (reported by the app via set_panel `h`).
```

## L318 · `pub margin: u32,`

```
/// Gap to the screen edge / tiles (px).
```

## L322 · `#[allow(dead_code)]`

```
/// Compositor manages all windows, the bar, and rendering state.
```

## L325 · `pub screen_w: u32,`

```
/// Screen dimensions.
```

## L328 · `pub scale: u32,`

```
/// Pixel scale (1x or 2x for 4K).
```

## L330 · `pub windows: Vec<Window>,`

```
/// All managed windows.
```

## L332 · `pub z_order: Vec<WindowId>,`

```
/// Z-order: front-to-back window IDs. First = topmost.
```

## L334 · `next_id: u32,`

```
/// Next window ID counter.
```

## L336 · `pub focused: Option<WindowId>,`

```
/// Currently focused window.
```

## L338 · `pub active_workspace: u8,`

```
/// Active workspace (0-based).
```

## L340 · `pub workspace_count: u8,`

```
/// Number of workspaces (was tracked by the old native bar).
```

## L342 · `pub gaps: u32,`

```
/// Gap between tiled windows (in pixels, scaled).
```

## L344 · `pub border: u32,`

```
/// Window border width (in pixels, scaled).
```

## L346-347 · `pub border_active: Option<u32>,`

```
/// Active window border color — `None` = follow the palette's
/// `AccentLine`, so a wallpaper/theme/accent switch moves it live.
```

## L349 · `pub border_inactive: Option<u32>,`

```
/// Inactive window border color — `None` = palette `Border`.
```

## L351 · `pub rounding: u32,`

```
/// Corner radius (in pixels, scaled).
```

## L353 · `pub needs_full_redraw: bool,`

```
/// Full redraw needed (including aurora background).
```

## L355 · `pub aurora_drawn: bool,`

```
/// Background has been drawn (skip on partial updates).
```

## L357 · `pub mouse: MouseState,`

```
/// Mouse cursor state.
```

## L359 · `pub drag: Option<DragState>,`

```
/// Drag state: which window is being dragged, and the grab offset.
```

## L361 · `pub animation: Option<SwapAnimation>,`

```
/// Active swap animation (windows gliding to new positions).
```

## L363-366 · `pub flash: Option<u64>,`

```
/// Screen flash: the tick it started at. A shutter blink confirming a
/// screenshot — the file itself lands seconds later, so without this
/// the button feels dead. Painted over the finished frame, so it can
/// never end up inside a capture.
```

## L368-370 · `pub focus_glow: Option<(WindowId, u64)>,`

```
/// Focus flash: (window, tick it gained focus). The halo starts bright
/// and settles into the steady one over `GLOW_FLASH_TICKS`, so a focus
/// change is visible even when you were not looking at that corner.
```

## L372-373 · `glow_last_tick: u64,`

```
/// Timer tick the halo was last repainted at — the callers can run far
/// more often than the animation has frames.
```

## L375 · `pub dock: Option<DockState>,`

```
/// Auto-hide bottom dock, if a dock app has registered one.
```

## L377-379 · `pub top_strut: Option<TopStrut>,`

```
/// Top strut bar, if a bar app (`bar.wasm`) has registered one. `None`
/// reserves no top band (tiles use the full height). The app renders
/// itself — there is no native fallback.
```

## L432-433 · `pub fn border_active(&self) -> u32 {`

```
/// Border of the focused tile. `AccentLine` (accent at 45 % over the
/// surface) unless `shade.border_active` pins a hex value.
```

## L440 · `pub fn border_inactive(&self) -> u32 {`

```
/// Border of unfocused tiles. Palette `Border` unless pinned.
```

## L447-448 · `fn active_border_color(&self) -> u32 {`

```
/// Border colour actually used for the focused / unfocused tile: a
/// wallpaper theme overrides the palette tokens.
```

## L465-470 · `fn glow_width(&self) -> u32 {`

```
/// Width of the focus halo. `shade.glow` in px (0 disables), capped at
/// the full gap: only ONE tile is ever focused, so two halos can never
/// meet, and the band stops one pixel short of the neighbour's rect
/// (tiles stand `gaps` apart, band columns are `[edge, edge + gaps)`).
/// The workspace area is inset by `gaps` on all four sides, so the band
/// stays on wallpaper at the screen edges and under the bar too.
```

## L478-479 · `pub fn glow_for(&self, win: &Window) -> Glow {`

```
/// Halo for this window right now. Unfocused tiles keep the `band` (they
/// have to repaint over their own former halo) but draw nothing.
```

## L497-498 · `pub fn tick_focus_glow(&mut self) -> bool {`

```
/// Advance the focus flash. True while it still has a frame to draw —
/// the last one settles the halo to its steady strength.
```

## L506-509 · `if now == self.glow_last_tick { return false }`

```
// At most one repaint per timer tick. The callers are not a frame
// clock: `poll_render` runs from a download's recv loop thousands of
// times a second, and repainting the band that often would burn the
// core for frames nobody can see — the alpha only moves once a tick.
```

## L518-521 · `pub fn render_focus_glow(&self, shadow: *mut u8, info: &FbInfo)`

```
/// Repaint ONLY the halo band of the focused tile. The flash animates
/// nothing inside the window, so redrawing the whole tile 20 times over
/// (millions of pixels at 4K, per frame) would be wasted — this touches
/// the four gap strips and hands them back as the damage to blit.
```

## L549-558 · `let r = self.rounding.min(w / 2).min(h / 2);`

```
// Und die vier ECKZWICKEL — innerhalb des umschliessenden Rechtecks,
// ausserhalb des Umrisses. Der Hof malt dort, die vier Streifen oben
// decken sie NICHT ab (sie enden an den Kanten des Kastens), und weil
// dieser Weg inkrementell ist, blieb der helle Blitzanstrich dort
// stehen, bis das Fenster irgendwann ganz neu gezeichnet wurde: ein
// kleiner Keil an jeder Ecke. Der volle Weg (`render_window`) stellt
// den ganzen Kasten her und hatte das Problem nie.
//
// Maskiert, weil in demselben Quadrat auch der Bogen des Fensters
// liegt — ein glattes Rechteck wuerde ihn wegwischen.
```

## L577 · `fn top_band(&self) -> u32 {`

```
/// Height reserved at the top for the bar strut (0 if no bar registered).
```

## L582 · `fn workspace_area(&self) -> (u32, u32, u32, u32) {`

```
/// Usable workspace area (excluding the top bar band + bottom dock).
```

## L594-599 · `fn dock_bottom_reserve(&self) -> u32 {`

```
/// Vertical band the tiling area gives up at the bottom for the
/// auto-hide dock. Zero when the dock is fully hidden (so tiles reach
/// their normal extent), ramping to the full dock band + a top gap
/// matching the dock's floating bottom gap when fully revealed.
/// Linear in the reveal fraction so a retile per `dock_tick` makes the
/// tiles glide up/down in lockstep with the sliding dock.
```

## L604-606 · `let full = (dock.thickness + 2 * dock.gap).saturating_sub(self.gaps);`

```
// Fully-shown reservation: the dock band plus a top gap equal to its
// floating bottom gap (→ equal air above and below), minus the tile
// gap the area already leaves above the baseline.
```

## L608 · `let risen = slide.saturating_sub(dock.offset.min(slide));`

```
// offset: 0 = shown, `slide` = hidden. risen = how far it has slid up.
```

## L613-614 · `fn dwindle_parent(&self) -> (Option<WindowId>, bool) {`

```
/// Create a new window and add it to the current workspace.
/// Returns None if no terminal slots available.
```

## L616-623 · `fn dwindle_parent(&self) -> (Option<WindowId>, bool) {`

```
/// Where a newly opened window hangs in the dwindle tree: it SPLITS the
/// focused window, which is what makes the leftmost tile splittable at all
/// — the old layout always subdivided the most recent window, so the first
/// tile could never be halved however long the session ran.
///
/// Direction comes from the focused tile's shape: a wide one splits
/// side-by-side, a tall one stacks. That is why the first split on a
/// landscape screen goes to the right.
```

## L625-626 · `if let Some(fid) = self.focused {`

```
// 1. Das fokussierte Fenster — aber nur, wenn es wirklich eine
//    Kachel ist.
```

## L633-647 · `for &id in &self.z_order {`

```
// 2. Der Fokus liegt auf einem OVERLAY — dem Launcher, einem Menue.
//
//    Hier stand frueher `return (None, true)`, und das machte das
//    neue Fenster zu einer WURZEL. Zwei Wurzeln liegen beide ueber
//    dem ganzen Schirm, ihre Mittelpunkte fallen also zusammen;
//    `swap_direction` sucht den naechsten Nachbarn geometrisch und
//    findet in KEINER Richtung einen. Mod+Shift+Pfeil tat damit
//    nichts — bei jeder App, die aus `drun` gestartet wurde. Aus
//    dem Dock ging es, weil ein Panel den Fokus gar nicht nimmt und
//    die Shell fokussiert bleibt.
//
//    Ein Overlay ist kein Ort, an den man kachelt. Es ist aber auch
//    kein Grund, den Baum zu vergessen: genommen wird die oberste
//    echte Kachel, also das Fenster, in dem der Nutzer gerade
//    gearbeitet hat, bevor er den Launcher aufzog.
```

## L656 · `fn is_tile(&self, w: &Window) -> bool {`

```
/// Kommt dieses Fenster als Elternteil einer neuen Kachel in Frage?
```

## L666 · `fn is_tiled(&self, id: WindowId) -> bool {`

```
/// Is this window part of the active workspace's tiling right now?
```

## L675-688 · `fn resize_target(&self, id: WindowId, beside: bool) -> Option<WindowId> {`

```
/// Which stored delta a resize gesture on `id` moves, for one axis
/// (`beside` = the vertical line between side-by-side tiles, else the
/// horizontal one). `None` = no such line bounds it.
///
/// The delta lives on the window that DREW the line, so a window owns the
/// line at only one of its edges; the opposite edge belongs to the last
/// child that split it. Adding the delta to the focused window
/// unconditionally was silently a no-op on every root window — the one
/// tile that has no line of its own.
///
/// No sign comes back any more. A delta always sits on the child that
/// drew the line and always pushes it towards the parent's origin
/// (left / up), so "move the line the way the arrow points" is a fixed
/// `- delta` for both callers — see `resize_focused`.
```

## L691 · `for _ in 0..self.windows.len() {`

```
// Bounded walk: a corrupt parent chain must not spin the compositor.
```

## L709-711 · `fn split_line_owner(&self, id: WindowId) -> Option<WindowId> {`

```
/// The window holding the split line the focused tile sits on, whatever
/// its orientation — its own if it has one, else the one its last child
/// drew. Used by `toggle_split`.
```

## L735 · `let pid = crate::process::spawn("loop", crate::process::KIND_SYSTEM, terminal_idx, 0);`

```
// Register window as a process in the process table
```

## L748-751 · `pub fn create_widget_window(&mut self, title: &str) -> WindowId {`

```
/// Create a widget-kind window for a Phase 10 GUI app. Doesn't
/// allocate a terminal buffer (widget apps aren't text-driven).
/// Focus stays on the current window so the spawning shell keeps
/// receiving the user's input.
```

## L759 · `win.terminal_idx = 255; // sentinel — no terminal buffer owned`

```
// sentinel — no terminal buffer owned
```

## L765-767 · `self.retile();`

```
// Deliberately NOT focus_window(id) — keep focus on the shell
// that spawned us, so the user's next keystroke lands there.
// But this insert(0) put us above the dock, so re-pin it on top.
```

## L775-778 · `pub fn create_surface_window(&mut self, title: &str) -> WindowId {`

```
/// Create a Surface-kind window for an external pixel source (a
/// microvm's virtio-gpu framebuffer). No terminal buffer, no
/// widget tree — content is a `GuestSurface` keyed by this id.
/// Like `create_widget_window`, focus stays on the spawning shell.
```

## L786 · `win.terminal_idx = 255; // sentinel — no terminal buffer owned`

```
// sentinel — no terminal buffer owned
```

## L799-811 · `pub fn promote_terminal_to_widget(&mut self, terminal_idx: u8) -> Option<WindowId> {`

```
/// Convert the Terminal-kind window backing `terminal_idx` into a
/// Widget-kind window in place. Keeps id, geometry, z-order, and
/// focus, but releases the terminal buffer (255 sentinel) so key
/// events flow through the widget event queue instead of the
/// terminal session.
///
/// Used by widget apps whose spawn path (`npk_spawn_module`) handed
/// them a terminal window they never meant to use — avoids the
/// "two windows for one app" seam.
///
/// Returns the WindowId on success, None if no terminal window
/// owns that terminal_idx. Does not touch the session; the worker's
/// exit path still cleans it up.
```

## L825-837 · `if pid != 0 { crate::process::exit(pid); }`

```
// Drop the "loop" process-table entry that create_window
// allocated — for a widget the app runs as its own KIND_WASM
// process (registered by the spawn path), so the loop PID is a
// misleading orphan that otherwise leaks on every drun/dock
// launch (close_window only frees a pid in its Terminal arm,
// which a promoted Widget window never reaches). Exiting a pid
// only touches the PROCS map. We deliberately do NOT free the
// session or terminal buffer here: the terminal's intent loop is
// still live and holds a long-lived `&mut IntentSession`, so
// freeing them mid-flight is a use-after-free (panicked in
// sync_session_to_terminal). Their lifecycle stays tied to the
// window via close_window. (Session/terminal-slot leak for
// promoted widgets is pre-existing — a separate follow-up.)
```

## L839-846 · `self.retile();`

```
// Re-tile: the promoted window kept whatever geometry it had as a
// terminal (often fullscreen, if it was the only window when its
// terminal was created). Without this, launching a second app from
// the dock left the first app fullscreen and the second stacked
// behind it instead of splitting — promote is the only window-
// producing path that wasn't re-tiling. Overlay/panel apps (drun,
// dock, bar) call set_overlay/set_panel right after, which un-tiles
// them again, so this is a no-op for them.
```

## L852-858 · `pub fn set_overlay(&mut self, id: WindowId, w: u32, h: u32) -> bool {`

```
/// Reconfigure an existing window as a centred overlay: floating
/// state, caller-chosen size, clamped to screen bounds. Used by the
/// `npk_window_set_overlay` host fn — the compositor stays ignorant
/// of which app (drun, future launchers, …) requested the change.
///
/// Does not touch focus. Caller decides whether to focus the window
/// afterwards.
```

## L881-882 · `self.retile();`

```
// Retile so tiled windows reclaim any space the window used
// to occupy (if it was previously tiled).
```

## L889 · `pub fn set_modal(&mut self, id: WindowId, modal: bool) -> bool {`

```
/// Toggle the `modal` flag on a window.
```

## L893-894 · `if modal { self.pin_overlays_to_front(); }`

```
// Raise it right away: from this moment it swallows clicks
// outside itself, so it had better be the thing on top.
```

## L902-905 · `pub fn set_overlay_at(&mut self, id: WindowId, x: i32, y: i32, w: u32, h: u32) -> bool {`

```
/// Position an overlay window's top-left at `(x, y)` with size `(w, h)`,
/// clamped to the screen. Like `set_overlay` but app-positioned instead
/// of centred — for dropdowns that anchor to a screen corner (e.g. the
/// volume slider just under the bar). Does not touch focus.
```

## L932 · `pub fn set_light_dismiss(&mut self, id: WindowId, on: bool) -> bool {`

```
/// Toggle light-dismiss (close-on-outside-click) for a window.
```

## L942-944 · `fn light_dismiss_outside(&self, x: i32, y: i32) -> Option<WindowId> {`

```
/// A visible light-dismiss window NOT containing `(x, y)`, if any — the
/// click-handler closes it so transient overlays vanish on an outside
/// click. (Clicks inside keep it open to interact with.)
```

## L953-954 · `fn dock_baseline(&self) -> u32 {`

```
/// Bottom edge of the dock's resting baseline. The bar is a top strut, so
/// the dock always rests at the screen bottom.
```

## L959-963 · `pub fn set_panel(&mut self, id: WindowId, edge: u8, behavior: u8, w: u32, h: u32) -> bool {`

```
/// `npk_window_set_panel` host fn — configure `id` as an edge panel.
/// `edge`: 0=Bottom, 1=Top. `behavior`: 0=AutoHide overlay (dock),
/// 1=Strut (bar). Bottom+AutoHide is the dock (slide + handle);
/// Top+Strut (the bar) is wired in the bar-render step. Returns false
/// for not-yet-implemented combos.
```

## L972-975 · `fn set_bar_panel(&mut self, id: WindowId, _w: u32, h: u32) -> bool {`

```
/// Top strut bar. The app reports its height via `h`; the compositor
/// reserves a `margin + h` band at the top and lays tiles below it.
/// Full width minus the margin, always visible, global across workspaces,
/// never focused, rendered with the panel blit. `w` is advisory.
```

## L981 · `let pill_h = h.max(22); // floor so a too-small report still fits content`

```
// floor so a too-small report still fits content
```

## L1004 · `if self.focused == Some(id) { self.focused = None; }`

```
// If the spawn path focused it, drop focus — panels never hold it.
```

## L1007 · `self.z_order.retain(|&wid| wid != id);`

```
// Pin the bar to the front of the z-order; keep the dock pinned too.
```

## L1016 · `pub fn set_dock(&mut self, id: WindowId, w: u32, h: u32) -> bool {`

```
/// Back-compat wrapper for the `npk_window_set_dock` host fn.
```

## L1021-1023 · `fn set_dock_panel(&mut self, id: WindowId, w: u32, h: u32) -> bool {`

```
/// Bottom auto-hide dock. Overlay (no strut, excluded from retile),
/// never modal, never focused on reveal, global across workspaces.
/// Starts fully hidden.
```

## L1030-1032 · `let dh = h.min(screen_h).max(40);`

```
// Cap at full screen height: a popover-bearing dock expands so the
// menu has room above the tray and click-outside lands inside the
// dock window. The visible tray still floats at the bottom.
```

## L1035-1037 · `let already_dock = self.dock.map(|d| d.id == id).unwrap_or(false);`

```
// Detect a resize on an already-set-up dock. Preserve the dock state
// (visible / offset / target_shown) so reopening the popover doesn't
// re-hide the tray; only the geometry is updated.
```

## L1065-1067 · `let (target_shown, offset, visible) = if already_dock {`

```
// On an in-place resize, keep the existing visibility so a
// popover-open expand doesn't flicker the dock through a hide
// cycle. Brand-new docks start hidden as before.
```

## L1078-1081 · `let slide = dh as i32 + gap as i32;`

```
// Position the window with the visible tray near the baseline
// and the popover space stretching upward inside the window.
// Mirrors `dock_tick`: y = baseline - (thickness + gap) + offset
// so the floating gap above the screen edge is preserved.
```

## L1098 · `if self.focused == Some(id) { self.focused = None; }`

```
// If the spawn path focused it, drop focus — panels never hold it.
```

## L1100-1101 · `self.retile();`

```
// Overlay → tiling grid is untouched, but retile reclaims any
// slot this window held if it was previously tiled.
```

## L1109-1110 · `pub fn bar_info(&self) -> (u8, u8, alloc::string::String) {`

```
/// Live state the bar app renders: (workspace_count, active_workspace,
/// focused window title). Fed to WASM via `npk_bar_state`.
```

## L1112-1115 · `let is_app = |w: &&Window| !w.is_overlay && !w.is_dock && !w.is_bar;`

```
// Title of the focused real app window — never a panel/overlay
// (the dock window is titled "dock"). If nothing real is focused
// (panels clear focus), fall back to the topmost real window on the
// active workspace so the bar still shows what's open.
```

## L1130-1134 · `pub fn shell_fingerprint(&self) -> u64 {`

```
/// A fingerprint of everything `bar_info` and `window_lines` report —
/// focus, workspaces, and each window's identity, place, visibility,
/// kind and title, in z-order. Compared after every full frame
/// (`render_frame_layered`); a change notifies `TOPIC_WINDOWS`. No
/// allocation: FNV-1a over the fields.
```

## L1155-1159 · `pub fn window_lines(&self) -> alloc::string::String {`

```
/// One line per real app window: `<flags>\t<workspace>\t<title>`,
/// flags being a decimal bitmask (1 = focused, 2 = on the active
/// workspace). Panels and transient overlays are excluded. Feeds the
/// dock's running/active indicators and the bar's occupied-workspace
/// hints; the kernel names no app.
```

## L1174-1180 · `pub fn dump_windows(&self) -> alloc::string::String {`

```
/// Jedes Fenster mit seinem ECHTEN Zustand — die Ansicht, die es
/// braucht, wenn eine App „optisch geschlossen" ist und trotzdem in
/// `top` und im Dock weiterlebt.
///
/// `window_lines` (was das Dock sieht) zeigt nur Titel und laesst
/// Overlays und Panels weg; genau dort verschwindet dann auch die
/// Frage, WELCHES Fenster ueberlebt hat und in welcher Art.
```

## L1212-1219 · `pub fn needs_tick(&self) -> bool {`

```
/// Drive the dock reveal/hide intent from the current cursor Y.
/// Called every frame (poll_render) so dwell/debounce advance even
/// while the cursor is parked. Suppressed during a drag/resize so the
/// dock never fights a tile being dragged toward the bottom.
/// Something moves on its own and needs the next frame (see
/// `shade::needs_tick`). The dock's dwell and debounce count CALLS, so
/// while either runs, or the slide has not reached its target, the loop
/// must keep calling at the frame rate.
```

## L1232-1235 · `let desktop_empty = !self.windows.iter().any(|w|`

```
// "Desktop free" = no real (non-overlay) window on this workspace.
// The dock then stays revealed as a launcher home surface; launching
// anything (terminal, widget, browser) hides it again — the bottom
// hot-edge still peeks it back on demand.
```

## L1253-1254 · `let dock_top = baseline - (dock.thickness + dock.gap) as i32;`

```
// Hide once the cursor leaves the dock band for long enough.
// The shown tray floats `gap` px above the baseline.
```

## L1267 · `if cursor_y >= baseline - DOCK_HOT_EDGE_PX {`

```
// Reveal once the cursor holds the bottom hot-edge.
```

## L1280-1282 · `pub fn dock_tick(&mut self) -> bool {`

```
/// Advance the dock slide one frame. Returns true while moving (caller
/// re-renders). Eases the offset toward 0 (shown, floating) /
/// `thickness + gap` (hidden, below the edge).
```

## L1294 · `let step = (delta.abs() / 4).max(2).min(delta.abs());`

```
// Ease-out: step a quarter of the remaining distance, min 2 px.
```

## L1303-1305 · `win.y = (baseline - slide + offset as i32).max(0) as u32;`

```
// Shown (offset 0): top = baseline - slide = baseline - thickness
// - gap → tray floats `gap` above the edge. Hidden (offset slide):
// top = baseline → fully below the usable area.
```

## L1310-1312 · `self.retile();`

```
// Reflow tiles to the dock-reserved area at the current offset so the
// windows glide up/down together with the sliding dock (the reserve
// is keyed off `dock.offset`, which we just stepped).
```

## L1318-1320 · `pub fn has_modal_window(&self) -> bool {`

```
/// True iff any visible window on the active workspace is modal.
/// Used by shade-action dispatch to lock out focus-shift shortcuts
/// while modal UI is on-screen.
```

## L1328-1330 · `pub fn is_panel(&self, id: WindowId) -> bool {`

```
/// Close a window by ID.
/// Is `id` a panel (dock or bar)? Panels are managed chrome — they
/// never hold focus and must not be closed by Mod+Q.
```

## L1335-1341 · `pub fn request_close_window(&mut self, id: WindowId) {`

```
/// A user gesture asked for `id` to close (Mod+Q, the title-bar X).
///
/// A window that opted into `npk_window_set_close_guard` gets an
/// `Event::CloseRequest` and stays open so it can prompt about unsaved
/// work. Asking a second time closes it for real — the guard buys one
/// round of politeness, not a veto. Everything else closes at once,
/// exactly as before.
```

## L1350 · `}`

```
// Second gesture — the app had its chance.
```

## L1355-1356 · `pub fn tick_close_requests(&mut self) -> bool {`

```
/// Close guarded windows that never answered their `CloseRequest`, so
/// a hung app can't leave an immortal window behind.
```

## L1369-1372 · `for picker in crate::shade::widgets::take_picks_for_requester(id.0) {`

```
// Any file dialog this window opened goes with it. Collected (and
// deregistered) first so closing them doesn't try to report a
// cancel back to the window we're about to remove. Recursion
// terminates: a picker owns no pickers of its own.
```

## L1377-1378 · `if self.dock.map(|d| d.id) == Some(id) {`

```
// If the dock app's window goes away, forget the dock so the
// reveal/tick machinery no-ops.
```

## L1383 · `if self.top_strut.map(|s| s.id) == Some(id) {`

```
// Bar window gone → free the top strut band; tiles reclaim the height.
```

## L1390 · `if let Some(win) = self.windows.iter().find(|w| w.id == id) {`

```
// Free session + terminal buffer + process before removing window
```

## L1399-1401 · `crate::shade::widgets::remove_scene(id.0);`

```
// No terminal buffer / session to free. Drop the
// per-window widget scene + event queue; their
// backing allocations free with the entries.
```

## L1404-1406 · `if win.pid != 0 { crate::process::exit(win.pid); }`

```
// A promoted-from-terminal widget clears its loop PID
// in promote_terminal_to_widget; this is belt-and-
// suspenders for any path that leaves a pid set.
```

## L1410-1412 · `crate::shade::surface::remove_surface(id.0);`

```
// Drop the bitmap surface; ask the bound microvm
// to power off (best-effort — it may already have
// exited, which is what closed this window).
```

## L1418-1421 · `self.reparent_children(id);`

```
// Dwindle: hand this window's children to its own parent, in its
// place. Without it they would each become a root and spread across
// the whole screen instead of taking over the space that just freed
// up — which is the one thing closing a window must do.
```

## L1434-1437 · `fn reparent_children(&mut self, id: WindowId) {`

```
/// Hand `id`'s children to its own parent, in its place — for every path
/// that takes a window out of a workspace's tree (close, or move away).
/// Without it they each become a root and spread over the whole screen
/// instead of taking over the region that just freed up.
```

## L1445-1449 · `if first {`

```
// The eldest child takes over the departing window's SLOT, so
// it inherits the direction AND the line's offset — the delta
// belongs to the line, not to the window that happens to hold
// it. Later children keep their own and chain after it,
// exactly as they did underneath.
```

## L1460-1468 · `fn refocus_active_workspace(&mut self) {`

```
/// Focus the topmost real window of the active workspace and point
/// ACTIVE_IDX + the cursor at it. Panels (dock/bar) are never focused —
/// otherwise closing the last app window would focus the dock and the
/// next Mod+Q would close it. No real window left → focus None (empty
/// desktop) and Mod+Q no-ops.
///
/// Every path that makes the focused window leave the screen must call
/// this: leaving ACTIVE_IDX behind means the loop keeps serving a terminal
/// nobody can see, and its input/output/cursor fall through to serial.
```

## L1482-1484 · `fn sync_active_terminal(&self, fid: WindowId) {`

```
/// Point ACTIVE_IDX + the cursor at `fid`'s terminal buffer. Widget and
/// Surface windows own none, so ACTIVE_IDX stays where it was and kprintln
/// keeps a valid sink while such a window is focused.
```

## L1494 · `pub fn focus_window(&mut self, id: WindowId) {`

```
/// Set focus to a window.
```

## L1496-1498 · `if self.windows.iter().any(|w| w.id == id && (w.is_dock || w.is_bar)) {`

```
// Panels (dock / bar) are never focusable — focusing them would
// route keyboard input into a window that ignores it (the shell
// appears to hang). Refuse, no matter which path asked.
```

## L1509-1511 · `if win.kind == crate::shade::window::WindowKind::Terminal {`

```
// Widget windows don't own a terminal buffer — leave ACTIVE_IDX
// pointing at the previously-active terminal so kprintln output
// keeps a valid sink while the widget app is focused.
```

## L1517-1518 · `self.pin_overlays_to_front();`

```
// The dock is never focused, so the insert(0) above would bury it
// behind the just-focused window. Re-pin it to the very top.
```

## L1520 · `}`

```
// Don't set needs_full_redraw — render_damaged handles 2 windows only
```

## L1523-1524 · `fn pin_overlays_to_front(&mut self) {`

```
/// Keep the dock window at the front of the z-order (topmost). Render
/// passes iterate `z_order.rev()`, drawing index 0 last → on top.
```

## L1530-1534 · `if let Some(id) = self.modal_window() {`

```
// A modal dialog must sit above everything, and this is not
// cosmetic: `modal_blocks` swallows every click outside it, so a
// buried modal would eat the pointer while invisible — the whole
// screen would go dead. Pinning it last puts it in front of the
// dock too.
```

## L1541 · `pub fn switch_workspace(&mut self, ws: u8) {`

```
/// Switch to workspace.
```

## L1544-1545 · `if ws >= self.workspace_count { return; }`

```
// The keys go up to 9; only the workspaces the bar actually shows
// exist, and switching past them would leave it with nothing to mark.
```

## L1549-1550 · `let baseline = self.dock_baseline();`

```
// The dock is global: follow the active workspace and snap shut so
// it re-reveals on demand rather than popping up mid-slide.
```

## L1566 · `if let Some(bw) = self.top_strut.map(|s| s.id) {`

```
// The bar is global too: follow the active workspace, stay visible.
```

## L1583-1585 · `self.sync_active_terminal(fid);`

```
// The new workspace serves a different terminal buffer — carry
// ACTIVE_IDX and the cursor over, or every keystroke after the
// switch lands in the window you just left behind.
```

## L1594-1596 · `pub fn move_to_workspace(&mut self, ws: u8) {`

```
/// Move the focused window to a different workspace: unhook it from this
/// workspace's tree, hang it into the target's, and go along with it
/// (Hyprland's `movetoworkspace`, not the `silent` variant).
```

## L1600 · `if self.is_panel(fid) { return }`

```
// Panels are global chrome — they belong to every workspace at once.
```

## L1603-1604 · `self.reparent_children(fid);`

```
// Unhook here first: the children stay behind and have to close the
// gap, exactly as if the window had been closed.
```

## L1607-1611 · `let host = self.z_order.iter().copied()`

```
// Hang it into the target's tree — splitting the topmost tile there,
// or becoming the root when that workspace is empty. Without this it
// arrives as a second root, is chained beside the existing one and
// never takes part in the layout properly. Its old delta stays with
// the line it drew here, so the arriving window starts centred.
```

## L1631-1638 · `self.switch_workspace(ws);`

```
// Go with it. Anything else means the window you just sent away keeps
// the focus on a workspace nobody is looking at — so if we stay, focus
// has to be handed back here (`refocus_active_workspace`, what
// close_window does). Following is the Hyprland default and keeps the
// window you are working on under your hands. switch_workspace does the
// retile, the panels and the terminal hand-over; the tree link above
// has to be set before it, or it would lay the window out as a stray
// second root.
```

## L1644-1646 · `pub fn retile(&mut self) {`

```
/// Dwindle tiling: every window splits the one that was focused when it
/// opened (`Window::split_from`), so the tree is stored on the windows
/// themselves and needs no separate structure.
```

## L1660-1663 · `let roots: Vec<WindowId> = tiled.iter().copied()`

```
// Roots: a tiled window whose parent is gone or lives on another
// workspace. Normally exactly one; a window moved between workspaces
// can leave a second, and chaining them keeps the screen usable
// instead of dropping tiles off it.
```

## L1680-1684 · `fn dwindle(&mut self, id: WindowId, tiled: &[WindowId],`

```
/// Lay a window out in `rect` together with everything that split it.
///
/// Children are walked in CREATION order, each taking half of what is left:
/// splitting A, then splitting A again, halves A twice — which is exactly
/// what happens interactively when the focus stays put.
```

## L1692 · `let (cx, cy, mut cw, mut ch) = (x, y, w, h);`

```
// Children only ever eat into the right/bottom, so the origin stays put.
```

## L1695-1697 · `let (beside, dw, dh) = self.windows.iter().find(|c| c.id == kid)`

```
// The split's position is the CHILD's resize delta: it is the one
// that arrived and drew the line, so Mod+arrow on it moves the
// line it created.
```

## L1729-1731 · `fn store_delta(&mut self, id: WindowId, beside: bool, effective: i32, current: i32) {`

```
/// Write back the offset a split line ACTUALLY took after clamping, so a
/// key held against the minimum tile size doesn't pile up an invisible
/// debt that has to be pressed off again before the line moves back.
```

## L1739-1740 · `#[allow(clippy::too_many_arguments)]`

```
/// Several roots share the area the way siblings would — only reachable
/// when a window changed workspace and left its parent behind.
```

## L1758-1764 · `fn sync_surface_tile_sizes(&self) {`

```
/// Push every Surface window's content rect into the surface
/// registry so virtio-gpu can advertise it via GET_DISPLAY_INFO
/// (D4 — guest renders to the tile size, no host scaling). Called
/// at the end of every retile; `set_tile_size` is idempotent and
/// only flags a config-change on a real size change. The `border`
/// must match render_window's content-rect inset exactly or the
/// guest would render a few px off.
```

## L1778 · `pub fn render(&mut self, shadow: *mut u8, info: &FbInfo) {`

```
/// Render the full compositor scene to the shadow buffer.
```

## L1780 · `if !self.aurora_drawn || self.needs_full_redraw {`

```
// Only redraw background when needed (expensive at 4K)
```

## L1786 · `let border = self.border;`

```
// Render windows (back to front)
```

## L1791 · `let mut wc = [0u32; 4]; // terminal, widget, surface, other(panel)`

```
// terminal, widget, surface, other(panel)
```

## L1822-1823 · `self.render_dock_handle(shadow, info);`

```
// The bar (bar.wasm) draws itself in the window loop above — no
// native bar render.
```

## L1825 · `self.render_dock_handle(shadow, info);`

```
// Presence handle for a fully-hidden dock.
```

## L1828 · `self.render_flash(shadow, info);`

```
// Last of all, over everything: the screenshot flash.
```

## L1837-1842 · `fn render_dock_handle(&self, shadow: *mut u8, info: &FbInfo) {`

```
/// Draw the collapsed-dock presence bar at the resting edge while the
/// dock is fully hidden, so the user knows a dock lives there. It
/// mirrors the open dock — same width + same translucent SurfaceElevated
/// tray colour — but only ~5 px tall: just the grey strip is enough to
/// signal "a dock lives here". Drawn over the wallpaper; cleared by the
/// full redraw `dock_tick` forces the moment it slides into view.
```

## L1845-1846 · `if dock.offset < dock.thickness + dock.gap { return; }`

```
// Only while fully hidden — once it slides up the window itself
// is the affordance.
```

## L1848 · `let Some(win) = self.windows.iter().find(|w| w.id == dock.id) else { return };`

```
// Mirror the dock window's geometry (centred, same width).
```

## L1858 · `let tray = crate::shade::widgets::palette::resolve_glass(`

```
// Tray-coloured bar: same token + translucency as the revealed dock.
```

## L1866 · `pub fn render_input_line(&self, shadow: *mut u8, info: &FbInfo) -> Option<(u32, u32, u32, u32)> {`

```
/// Fast render: only the current input line of the focused window.
```

## L1884 · `pub(crate) fn render_window(shadow: *mut u8, info: &FbInfo, win: &Window,`

```
/// Render a single window: background overwrite + border blend + content blend + text.
```

## L1888 · `static RW_BG: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
// [rw-phase] per-window timing: wallpaper restore | chrome | content.
```

## L1894-1899 · `if !win.is_overlay || win.is_bar {`

```
// Overlay windows skip the wallpaper restore — rounded-out
// corners keep showing whatever app is underneath instead of
// punching a wallpaper-shaped hole into it. The bar is the
// exception: it's a translucent overlay that repaints every clock
// tick, so it MUST restore its band from the wallpaper first or
// successive blends would stack into an opaque smear.
```

## L1901-1904 · `let g = glow.band;`

```
// Restore the halo band too, ALWAYS — also for an unfocused tile,
// which is exactly the one that has to paint over the halo it had
// while it was focused. The band lives inside the gap, so this
// never reaches a neighbour (see `glow_width`).
```

## L1917-1926 · `let (chrome_border, chrome_round) = if win.is_dock || win.is_bar {`

```
// 2+3. Single-pass chrome. Terminal windows get the full
// layered paint (border + bg_color content). Widget windows
// get border-only — the widget supplies its own content + AA
// at the inner edge, so the chrome must not bleed `bg_color`
// into the inner-fringe band.
// The dock is chrome-less: no hard bordered box around it. It
// supplies its own soft tray background in the widget tree, so
// we skip the chrome pass entirely and blit the widget content
// edge-to-edge with the full rounding. Everything else gets the
// normal border + bg chrome.
```

## L1945-1947 · `let content_bg = if paint_content {`

```
// Terminal (`loop`) windows track the active theme like the
// widget apps — Surface bg + OnSurface text — so light mode is
// consistent across every window instead of a lone dark tile.
```

## L1956-1958 · `background::draw_glass_backdrop(shadow, info,`

```
// Glass blends over the blurred wallpaper, not the sharp one
// (see `background::draw_glass_backdrop`). Inside the cached
// region, so a cache hit carries it.
```

## L1961 · `let key = chrome_key(win.x, win.y, win.width, win.height, win.focused,`

```
// Terminal glass bg is static + expensive — cache it.
```

## L1997-2001 · `let inner_r = chrome_round;`

```
// Inner shape is concentric with the outer at radius
// `rounding - border` — see `fill_rounded_chrome_aa`. The
// widget-blit AA at the inner edge is computed against this
// same inner curve so widget pixels and chrome border meet
// pixel-perfectly along the rounded inner curve.
```

## L2004 · `match win.kind {`

```
// 4. Content-kind specific draw.
```

## L2020-2022 · `if let Some((total, soff)) = terminal::scroll_metrics(win.terminal_idx as usize) {`

```
// Overlay scrollbar — only when the scrollback overflows.
// Mirrors the widget overlay bar; geometry must match
// `scroll_hit_at` so the drawn thumb and the drag area agree.
```

## L2032 · `let from_top = if max_scroll == 0 { 0 } else { travel * (max_scroll - soff) / max_scroll };`

```
// soff 0 = bottom → thumb at bottom; soff max = top.
```

## L2050-2053 · `crate::shade::widgets::with_scene(win.id.0, |scene| {`

```
// Widget pixels fill the inner rounded rect. Middle rows
// memcpy; rows that touch a corner curve fall through to
// a per-pixel SDF blend so widget content and chrome
// border meet with proper AA at the inner edge.
```

## L2059-2063 · `let usable_h = if scene.width > 0 {`

```
// Defensive: a scene caught mid-resize (e.g. the browser's
// Canvas re-sizing) can report a width/height larger than
// its actual pixel buffer. Clamp the blit to the rows the
// buffer really holds so a rounded-corner index can never
// run past `scene.pixels` — a kernel OOB there halts the OS.
```

## L2074-2080 · `if win.is_dock || win.is_bar {`

```
// Panels (dock + bar). Both root their tree in a Stack, so
// their scene carries real alpha (transparent where empty,
// chrome-opacity backgrounds, full-coverage glyphs — see
// rasterize_buffer_with_overlays). Composite the scene over
// the wallpaper by per-pixel alpha: translucent tray/pills,
// crisp glyphs, AA corners from the rasteriser, wallpaper in
// the gaps — no halo, no per-pill detection.
```

## L2089 · `if a == 0 { continue; }  // transparent → wallpaper`

```
// transparent → wallpaper
```

## L2090-2091 · `if a < 255 {`

```
// Translucent fill: blend over the blurred
// wallpaper where nothing else is beneath.
```

## L2112 · `let src_base = (local_y as usize) * (scene.width as usize);`

```
// Straight middle: fast memcpy of the full row.
```

## L2126-2127 · `let mid_lo = r.min(cw_local);`

```
// Corner row: r pixels on each side go through
// the SDF blend; the middle is still memcpy.
```

## L2166-2175 · `crate::shade::surface::with_front(win.id.0, |px, sw, sh| {`

```
// Raw guest framebuffer → tile, 1:1 (no scaling). D4:
// virtio-gpu GET_DISPLAY_INFO advertises this content
// rect, so the guest (wlroots/cage) reflows to the
// tile size natively — guest `sw×sh` == `cw×ch` in
// steady state; a brief mismatch during a resize
// round-trip just clips (never stretches). Same
// memcpy-middle + SDF-corner-blend shape as the Widget
// arm above so the browser tile gets the identical
// concentric rounded corners as every other window
// and sits flush in the dwindle layout.
```

## L2183-2185 · `let x1 = (cx + sw).min(cx + cw).min(fb_w);`

```
// Clip the 1:1 blit to the guest buffer, the
// content rect, and the framebuffer — never
// overdraw the border or a neighbour tile.
```

## L2198 · `let src_base = (local_y as usize) * (sw as usize);`

```
// Straight middle: fast memcpy of the row.
```

## L2212-2213 · `let mid_lo = r.min(cw_local);`

```
// Corner row: r pixels on each side go through
// the SDF blend; the middle is still memcpy.
```

## L2253-2254 · `draw_close_button(shadow, info, win, border, scale);`

```
// Platform close button (top-right). Panels return None and are
// skipped; everything else gets the mouse-friendly "X".
```

## L2260-2262 · `static T_PANEL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
// Panels return early from the widget arm but still flow here.
// Time them SEPARATELY — averaging them with the terminal made
// the phase numbers ambiguous (panels carry their own cost).
```

## L2285 · `pub fn render_damaged(&mut self, shadow: *mut u8, info: &FbInfo) -> Vec<(u32, u32, u32, u32)> {`

```
/// Render only changed regions. Returns list of (x, y, w, h) to blit.
```

## L2315-2317 · `let g = glow.band;`

```
// Damage covers the halo band: the tile that just lost
// focus repaints wallpaper there, and the one that gained
// it paints the halo — neither is inside its own rect.
```

## L2335 · `pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {`

```
/// Get a mutable reference to a window by ID.
```

## L2340 · `pub fn window(&self, id: WindowId) -> Option<&Window> {`

```
/// Get a reference to a window by ID.
```

## L2345 · `pub fn window_count(&self) -> usize {`

```
/// Count of windows on the active workspace.
```

## L2352 · `fn set_focused_flag(&mut self, focused_id: WindowId) {`

```
/// Update focused flag — only mark changed windows dirty.
```

## L2359 · `win.dirty = true; // Only re-render windows that changed`

```
// Only re-render windows that changed
```

## L2363-2364 · `if gained {`

```
// Flash only on a real change of focus — every path routes through
// here, and several of them re-assert the focus that is already set.
```

## L2370 · `pub fn focus_direction(&mut self, dx: i32, dy: i32) {`

```
/// Focus the nearest window in the given direction from the focused window.
```

## L2388 · `let in_direction = match (dx, dy) {`

```
// Check if this window is in the right direction
```

## L2390 · `(1, 0) => rel_x > 0,   // Right`

```
// Right
```

## L2391 · `(-1, 0) => rel_x < 0,  // Left`

```
// Left
```

## L2392 · `(0, 1) => rel_y > 0,   // Down`

```
// Down
```

## L2393 · `(0, -1) => rel_y < 0,  // Up`

```
// Up
```

## L2398 · `let dist = rel_x.abs() + rel_y.abs();`

```
// Distance (Manhattan for simplicity)
```

## L2410 · `pub fn focus_next(&mut self) {`

```
/// Focus next window on active workspace (cycle).
```

## L2426 · `pub fn focus_prev(&mut self) {`

```
/// Focus previous window on active workspace (cycle).
```

## L2442 · `pub fn handle_mouse(&mut self, evt: &crate::xhci::MouseEvent) -> bool {`

```
/// Process a mouse event (legacy path). Returns true if the scene needs re-rendering.
```

## L2448-2452 · `if self.drag.is_none()`

```
// A modal dialog owns the pointer. Swallow presses outside it so
// nothing behind takes focus and buries it. Motion still goes
// through (the cursor must keep moving) and drags in progress are
// let finish, so a resize started before the dialog opened doesn't
// stick.
```

## L2462 · `if let Some(mut drag) = self.drag {`

```
// Handle active drag (swap or resize)
```

## L2480 · `self.drag = Some(drag); // Keep drag alive`

```
// Keep drag alive
```

## L2487 · `self.drag = Some(drag); // Keep drag alive`

```
// Keep drag alive
```

## L2495 · `return drag.mode == DragMode::Resize; // resize needs final render`

```
// resize needs final render
```

## L2499 · `if mod_held && self.mouse.left_clicked() {`

```
// Mod+LMB: start swap-drag
```

## L2513 · `if mod_held && self.mouse.right_clicked() {`

```
// Mod+RMB: start resize-drag
```

## L2521-2522 · `if !mod_held && self.mouse.left_clicked() {`

```
// Plain LMB on a window's close button → close it (see the
// button-event path for the rationale).
```

## L2530 · `if self.mouse.left_clicked() {`

```
// Regular LMB click: focus window
```

## L2540 · `false`

```
// Cursor overlay handled by redraw_overlay — no scene redraw for movement
```

## L2545-2546 · `pub fn handle_mouse_buttons(&mut self) -> bool {`

```
/// Handle only button events (click, drag, release). Position already in self.mouse.
/// Called from lock-free input path — only when buttons change.
```

## L2558 · `if let Some(mut drag) = self.drag {`

```
// Active drag
```

## L2576 · `self.drag = Some(drag); // Keep drag alive`

```
// Keep drag alive
```

## L2583 · `self.drag = Some(drag); // Keep drag alive`

```
// Keep drag alive
```

## L2595-2597 · `if self.mouse.left_clicked() || self.mouse.right_clicked() {`

```
// Light-dismiss: a window that opted in (npk_window_set_light_dismiss)
// closes on a fresh click outside it — the volume slider overlay etc.
// The dismiss click is consumed (it just closes the overlay).
```

## L2605 · `if mod_held && self.mouse.left_clicked() {`

```
// Mod+LMB: start swap-drag
```

## L2619 · `if mod_held && self.mouse.right_clicked() {`

```
// Mod+RMB: start resize-drag
```

## L2627-2630 · `if !mod_held && self.mouse.left_clicked() {`

```
// Plain LMB on a window's close button → close it. Checked before
// focus/dispatch so the corner "X" always wins over whatever widget
// sits beneath it. Mod+LMB is the swap-drag above, so guard on
// `!mod_held` to keep the two gestures from overlapping.
```

## L2638 · `if self.mouse.left_clicked() {`

```
// Regular LMB click: focus window + dispatch widget event
```

## L2641-2644 · `let is_dock_win = self.windows.iter()`

```
// The dock must not steal keyboard focus from the tile you
// were working in — clicking an icon launches/focuses an
// app, the dock itself stays unfocused. Hit-test/events
// below still fire.
```

## L2647-2648 · `let focus_changed = !is_dock_win && self.focused != Some(wid);`

```
// Focus on first click into an unfocused window; later
// clicks on the same focused widget should dispatch.
```

## L2654-2656 · `let is_widget = self.windows.iter()`

```
// Widget-kind click: hit-test against the scene's
// layout tree, push Event::Action(id) or
// Event::MouseButton into the window's queue.
```

## L2663-2667 · `let pressed_dirty = crate::shade::widgets::press_at(wid.0, mx, my);`

```
// Move focus + start active state on the deepest
// focusable widget under the cursor. press_at must
// not touch the compositor (we're inside its lock)
// — it returns `true` when it re-rasterized so we
// can mark the window dirty here.
```

## L2677-2679 · `crate::shade::widgets::push_event(wid.0, Event::MouseButton {`

```
// Always queue the raw button too — apps that want
// position-sensitive behaviour (canvas, drag) use
// it directly.
```

## L2691-2695 · `if !mod_held && self.mouse.right_clicked() {`

```
// Plain RMB (no Mod): right-click for context menus. Hit-test the
// widget tree and push ContextAction(id) + raw MouseButton{Right}.
// Mod+RMB is the resize-drag above; this branch is only reached
// when `!mod_held`. The dock must not steal keyboard focus (same
// exclusion as LMB), so we skip the focus shift on dock/bar.
```

## L2717-2724 · `if !mod_held && self.mouse.middle_clicked() {`

```
// Middle click: raw button only, no hit-test.
//
// **Es gibt keine `Action` dafuer, und das ist Absicht.** Ein
// Mittelklick ist keine zweite Art, einen Knopf zu druecken — er ist
// eine Geste auf dem INHALT (im Browser: den Link unter dem Zeiger in
// einem neuen Tab oeffnen). Wer ihn auf den Treffertest legte, liesse
// jeden Knopf im System auf die mittlere Taste reagieren, ohne dass
// eine einzige App darum gebeten haette.
```

## L2743-2745 · `if self.mouse.left_released() {`

```
// Mouse release: clear active state on every widget window
// (we don't track which window held the press). Cheap — only
// does work when the window's `active_path` was actually set.
```

## L2768-2769 · `if !mod_held && self.mouse.middle_released() {`

```
// Und die mittlere Taste ebenso — eine App, die Druck und Loslassen
// paart, darf nicht auf ein Loslassen warten, das nie kommt.
```

## L2786 · `if !mod_held && self.mouse.right_released() {`

```
// Mirror RMB release to widget windows so apps see press+release pairs.
```

## L2806-2808 · `pub fn resize_focused(&mut self, dx: i32, dy: i32) {`

```
/// Resize the focused window by pushing the split line that bounds it.
/// A positive delta grows it, no matter which of its edges is the movable
/// one (`resize_target` picks the line and the sign).
```

## L2816-2822 · `if beside { win.resize_w -= delta } else { win.resize_h -= delta }`

```
// `- delta`, exactly like the mouse drag: the arrow moves the
// split line the way it points. It used to mean "make the
// focused window wider" (Hyprland's resizeactive), which is
// predictable in the abstract but looks wrong on screen —
// on a right-hand tile the LEFT edge moved, so pressing
// Right made the window grow leftwards. Keyboard and mouse
// now push the same line the same way.
```

## L2833-2836 · `pub fn toggle_split(&mut self) {`

```
/// Mod+J — flip the split the focused window sits on from side-by-side to
/// stacked and back (Hyprland's `togglesplit`). It flips the same line a
/// resize would push, so what you toggle is what you were just adjusting.
/// The offset is dropped: the delta is per-axis, and the axis just changed.
```

## L2849-2852 · `fn begin_resize_drag(&mut self, wid: WindowId, mx: i32, my: i32) {`

```
/// Resolve the split lines a Mod+RMB drag will move and remember where
/// they stood, so the drag stays absolute against its start position.
/// Only WHICH line comes from `resize_target` — the sign doesn't, see
/// `apply_resize_drag`.
```

## L2872-2879 · `fn apply_resize_drag(&mut self, drag: &DragState, dx: i32, dy: i32) {`

```
/// Apply a resize drag: absolute against the deltas captured at drag start.
///
/// Direct manipulation: the border follows the hand — mouse right = line
/// right, whichever side of it the focused tile is on. A delta always sits
/// on the child that drew the line and always moves it towards its parent's
/// origin (left / up), so following the cursor is a fixed `- delta` on both
/// axes. `resize_focused` (Mod+Ctrl+arrow) does the same thing, so a key
/// and a drag move the line identically.
```

## L2893 · `pub fn swap_direction(&mut self, dx: i32, dy: i32) {`

```
/// Swap focused window with the nearest window in the given direction.
```

## L2901 · `let mut best: Option<(WindowId, i32)> = None;`

```
// Find nearest window in direction (same logic as focus_direction)
```

## L2926 · `fn swap_window_order(&mut self, a: WindowId, b: WindowId) {`

```
/// Swap two windows with smooth animation.
```

## L2928 · `self.finish_animation();`

```
// Complete any active animation first
```

## L2931 · `let a_from = self.windows.iter().find(|w| w.id == a)`

```
// Save old positions
```

## L2937-2942 · `let slot = |ws: &Vec<crate::shade::window::Window>, id: WindowId| {`

```
// Swap their PLACES IN THE TREE, not their places in the list — the
// list order no longer decides geometry. Each takes the other's
// parent, and each other's children come along, so the two tiles
// exchange regions with everything nested inside them.
// The split offset travels with the slot too — it describes where a
// line stands, not how big a particular window wants to be.
```

## L2958 · `if a_slot.0 == Some(b) {`

```
// A child of the other is now its own parent — undo that one case.
```

## L2967 · `let a_to = self.windows.iter().find(|w| w.id == a)`

```
// Save new positions
```

## L2973 · `if a_from != a_to || b_from != b_to {`

```
// Start animation: put windows back at old positions, animate to new
```

## L2975 · `if let Some(w) = self.windows.iter_mut().find(|w| w.id == a) {`

```
// Set windows to starting position
```

## L2986 · `duration: 15, // 150ms at 100Hz`

```
// 150ms at 100Hz
```

## L2992 · `pub fn tick_animation(&mut self) -> bool {`

```
/// Advance swap animation. Returns true if a frame was updated.
```

## L3003 · `let t = (elapsed * 1000 / anim.duration) as i64; // 0..1000`

```
// Ease-out cubic: t' = 1 - (1-t)³  (fast start, smooth deceleration)
```

## L3004 · `let t = (elapsed * 1000 / anim.duration) as i64; // 0..1000`

```
// 0..1000
```

## L3032-3038 · `const FLASH_TICKS: u64 = 12;`

```
// ── Screen flash ──────────────────────────────────────────────────
//
// A shutter closing, not a floodlight: the screen dips DARK for a
// moment instead of being washed white. Same mechanism either way —
// a full-screen blend — but dimming reads as a soft blink while white
// is genuinely harsh on a dark desktop. Kept to a few frames because
// the overlay touches every pixel, the expensive operation at 4K.
```

## L3040 · `const FLASH_TICKS: u64 = 12;`

```
/// Ticks (100 Hz) the flash lasts.
```

## L3042-3043 · `const FLASH_ALPHA: u32 = 110;`

```
/// How far down the screen dips at the peak, 0..255.
/// `set shade.flash_opacity <0-255>` overrides it; 0 turns it off.
```

## L3053-3054 · `pub fn start_flash(&mut self) {`

```
/// Start (or restart) the shutter blink. Turned off entirely at
/// opacity 0, so the capture costs no extra frames at all.
```

## L3061-3063 · `pub fn flash_tick(&mut self) -> bool {`

```
/// Advance the flash. True while it still needs frames — including the
/// final one that clears it, otherwise the last white wash would stay
/// on screen until something else happened to repaint.
```

## L3074-3075 · `fn flash_alpha(&self) -> u32 {`

```
/// Current flash opacity, 0 when inactive. Ease-out so it snaps bright
/// and drains away.
```

## L3080 · `let remaining = Self::FLASH_TICKS - elapsed;      // 1..=FLASH_TICKS`

```
// 1..=FLASH_TICKS
```

## L3081-3082 · `let a = Self::flash_peak() as u64 * remaining * remaining`

```
// Quadratic falloff: brightest on the first frame, then a quick
// drain — a blink, not a fade.
```

## L3088-3089 · `fn render_flash(&self, shadow: *mut u8, info: &FbInfo) {`

```
/// Paint the flash over the finished frame. Black, not white — the
/// screen darkens and comes back.
```

## L3097 · `fn finish_animation(&mut self) {`

```
/// Instantly complete any active animation.
```

## L3114-3116 · `fn modal_window(&self) -> Option<WindowId> {`

```
/// Find the topmost window at screen coordinates (x, y).
/// The modal window on this workspace, if any — a dialog that owns the
/// pointer until it's answered.
```

## L3123-3130 · `fn modal_blocks(&self, x: i32, y: i32) -> bool {`

```
/// True if (x, y) lies outside an open modal dialog.
///
/// Clicking there used to focus and raise the window behind, burying
/// the dialog with no way back — a file picker vanished behind its own
/// app and the user was stuck. A modal dialog blocks the pointer
/// instead: nothing behind it reacts. Deliberately NOT light-dismiss,
/// which is right for a casual overlay (the volume slider) but would
/// throw away a half-typed filename on a stray click.
```

## L3146 · `for &wid in &self.z_order {`

```
// Z-order: front to back (first match = topmost)
```

## L3163-3165 · `fn close_button_at(&self, x: i32, y: i32) -> Option<WindowId> {`

```
/// Find the window whose platform close button contains `(x, y)`,
/// front-to-back so the topmost button wins. Returns `None` if the
/// point isn't over any close button.
```

## L3185-3189 · `pub fn scroll_hit_at(&self, x: i32, y: i32) -> Option<ScrollHit> {`

```
/// Geometry for a scrollbar drag at `(x, y)`: the window under the
/// cursor plus its terminal text rect (for terminals) and close-button
/// rect (to exclude from the bar). Widget windows fill the viewport from
/// their scene; here we just identify the window + kind. `None` for
/// panels or empty space.
```

## L3212 · `pub fn render_to_layers(&mut self, info: &FbInfo) {`

```
// ── Layer-based rendering ──────────────────────────────────────────
```

## L3214-3215 · `pub fn render_to_layers(&mut self, info: &FbInfo) {`

```
/// Render all windows to layer buffers (Chrome → Layer 1, Text → Layer 2).
/// Background (Layer 0) is rendered once in shade::init / force_redraw.
```

## L3229 · `for &wid in self.z_order.iter().rev() {`

```
// Render windows back to front
```

## L3240-3241 · `for win in &mut self.windows {`

```
// The bar (bar.wasm) renders itself as a window into the chrome
// layer — no native bar render.
```

## L3249 · `pub fn render_damaged_to_layers(&mut self, info: &FbInfo) {`

```
/// Render only dirty windows to layer buffers.
```

## L3281-3282 · `fn render_chrome_to_layer(info: &FbInfo, win: &Window,`

```
/// Render window chrome (border + content bg) to Layer 1.
/// Uses _alpha variants that write the alpha byte for layer compositing.
```

## L3291 · `let pitch = info.pitch as usize;`

```
// Clear window region first (transparent)
```

## L3298 · `unsafe { core::ptr::write_bytes(chrome_buf.add(off), 0, bytes); }`

```
// SAFETY: bounds checked above
```

## L3302 · `if crate::theme::is_active() && win.focused {`

```
// Border (gradient or solid) — alpha byte set for compositor
```

## L3314 · `let cx = win.content_x(border);`

```
// Content area (inner rect with bg color) — alpha byte set
```

## L3328 · `pub fn render_text_to_layer(&self, info: &FbInfo, win: &Window) {`

```
/// Render terminal text for a window to Layer 2.
```

## L3343 · `let pitch = info.pitch as usize;`

```
// Clear text region (transparent black)
```

## L3350 · `unsafe { core::ptr::write_bytes(text_buf.add(off), 0, bytes); }`

```
// SAFETY: bounds checked
```

## L3354 · `terminal::render_to_window(text_buf, info, cx, cy, cw, ch, scale, win.terminal_idx);`

```
// Render text characters into text layer
```

## L3360 · `pub fn render_input_line_to_layer(&self, info: &FbInfo) -> Option<(u32, u32, u32, u32)> {`

```
/// Render only the input line to Layer 2 (fast path for typing).
```

## L3378 · `terminal::render_input_line_to_layer(text_buf, info, cx, cy, cw, ch, win.terminal_idx)`

```
// Render input line directly to text layer (no cache hack needed!)
```

## L3382 · `fn layer_border_color(&self, win: &Window) -> u32 {`

```
/// Get border color for a window (theme-aware).
```

## L3400 · `fn parse_hex_color(s: &str) -> Option<u32> {`

```
/// Parse a hex color string ("RRGGBB") to u32.
```

