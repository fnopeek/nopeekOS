# `tools/wasm/dock/src/lib.rs` @ 5e0102684

## L1-14 · `#![no_std]`

```
//! dock — bottom auto-hide app dock.
//!
//! A resident overlay launcher. Declares itself a dock via
//! `npk_window_set_dock`; the compositor owns the slide-in/out reveal
//! (cursor at the bottom edge reveals it, leaving hides it). Renders a
//! centred row of app icons plus a trailing launcher button that opens
//! `drun` for full search. Complementary to `drun`, not a replacement.
//!
//! Right-click on an icon opens a context Popover (Unpin / move left /
//! move right). Right-click on the trailing launcher opens an "Add to
//! dock" list of every catalog app not currently pinned. Mutations are
//! persisted to `sys/config/dock` and applied live.
//!
//! See docs/archive/DOCK.md for the architecture.
```

## L34-35 · `#[unsafe(link_section = ".npk.caps")]`

```
// Declared capabilities: read (catalog) + write (persist dock config) +
// exec (launch apps/intents) + render. The kernel grants exactly this.
```

## L40-42 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L59-61 · `struct Strings {`

```
// ── Strings ───────────────────────────────────────────────────────────
// English is the source language. A new language is one more `const`
// here — see `nopeek_widgets::i18n`.
```

## L88 · `fn fill(template: &str, value: &str) -> String {`

```
/// Substitute the single `{}` placeholder in a catalog string.
```

## L144-146 · `const HEAP_SIZE: usize = 512 * 1024;`

```
// Bump allocator — same pattern as drun. Bumped to 512 KB because we
// now hold the full catalog (every installed app) for the Add-to-dock
// submenu, and may re-render multiple times per session.
```

## L185-186 · `const CLICK_BASE:   u32 = 1;            // 1..HOVER_BASE : launch cell idx`

```
// ── ActionIds + NodeIds ───────────────────────────────────────────────
// Ranges are far apart so a future reshuffle can't collide silently.
```

## L187 · `const CLICK_BASE:   u32 = 1;            // 1..HOVER_BASE : launch cell idx`

```
// 1..HOVER_BASE : launch cell idx
```

## L188 · `const HOVER_BASE:   u32 = 50_000;       // HOVER_BASE+idx : OnHover for cell idx`

```
// HOVER_BASE+idx : OnHover for cell idx
```

## L189 · `const LAUNCHER:     u32 = 90_000;       // open drun (also: hover target N)`

```
// open drun (also: hover target N)
```

## L190 · `const NODE_CELL:    u32 = 100_000;      // NODE_CELL+idx : anchor for cell idx`

```
// NODE_CELL+idx : anchor for cell idx
```

## L191 · `const NODE_LAUNCHER:u32 = 199_000;      // anchor for trailing launcher`

```
// anchor for trailing launcher
```

## L194 · `const MENU_MOVE:    u32 = 200_001;      // enter drag-to-reorder mode`

```
// enter drag-to-reorder mode
```

## L196 · `const ADD_BASE:     u32 = 300_000;      // ADD_BASE+catalog_idx : add to dock`

```
// ADD_BASE+catalog_idx : add to dock
```

## L198-199 · `const ICON_SIZE: u16 = 24;`

```
// Visual sizing (px at 1× scale) — docs/spec/UI_REFRESH.md §3 "dock".
/// Glyph inside a cell tile.
```

## L201 · `const CELL_BOX: u16 = 34;`

```
/// Square tile the glyph sits in.
```

## L203 · `const CELL_RADIUS: u8 = 9;`

```
/// Corner radius of that tile.
```

## L205-206 · `const DASH_W_ACTIVE: u16 = 12;`

```
/// Running-indicator dash: full width when the app is focused, a stub
/// when it merely runs, invisible (but space-holding) when it doesn't.
```

## L210-211 · `const CELL_LIFT: u16 = 2;`

```
/// Ausgleichsmarke ueber der Kachel — siehe `icon_cell`. Unsichtbar,
/// belegt aber Platz, genau wie der Laufstrich einer ruhenden Anwendung.
```

## L213-215 · `const CELL_GAP: u16 = 1;`

```
/// Abstand zwischen den drei Teilen der Zelle. Er steht ZWEIMAL in der
/// Spalte (Marke|Kachel und Kachel|Strich), deshalb ist er 1 und nicht
/// `Spacing::Xs`.
```

## L217 · `const DOCK_HEIGHT: i32 = 50;`

```
/// Tile + gap + dash → the cell column's height.
```

## L219 · `const CELL_FOOTPRINT: i32 = 36; // tile + inter-cell gap`

```
// tile + inter-cell gap
```

## L220-222 · `const TRAY_PAD_X: u16 = 12;`

```
/// Waagrechte Polsterung IM Tray — Platz fuer den Eckbogen der Pille.
/// Gerechnet, nicht geschaetzt: die engste Stelle verlangt 4,8 px (siehe
/// `render`), 12 lassen 7,2 px Luft.
```

## L224-226 · `const TRAY_PAD_Y: u16 = 4;`

```
/// Senkrechte Polsterung. Sie setzt die Tray-Hoehe (40 + 2x4 = 48) und
/// damit den Pill-Radius (24). Nicht anfassen, ohne die Rechnung in
/// `render` neu zu machen.
```

## L228-229 · `const SIDE_PADDING: i32 = 24 + 2 * TRAY_PAD_X as i32;`

```
/// Fensterbreite = Zellen + dies. Traegt jetzt auch die zwei mal
/// `TRAY_PAD_X`, sonst nimmt der Bogen den Zellen ihren Platz weg.
```

## L231-233 · `const DOCK_GAP_RESERVE: i32 = 24;`

```
/// Approximate compositor `DOCK_BOTTOM_GAP * scale` (kernel default is 12,
/// HiDPI scale 2× → 24). Subtracted from the expanded window height so
/// the visible bottom gap is preserved when a menu is open.
```

## L237-240 · `const DOCK_CFG_MARKER: &str = "# nopeekos dock v1";`

```
/// Header line written by `persist`. Its presence (returned by
/// `read_pins`) tells the next boot the user has touched the config —
/// an otherwise-empty body then means "user wants an empty dock" and
/// the catalog fallback is suppressed.
```

## L243 · `#[derive(Clone, Copy)]`

```
// ── State ─────────────────────────────────────────────────────────────
```

## L247 · `IconCtx(usize),`

```
/// Context menu for the cell at `idx` (Unpin / move left / move right).
```

## L249 · `AddApp,`

```
/// "Add to dock" submenu showing every catalog app not yet pinned.
```

## L254 · `entries:    Vec<AppEntry>,`

```
/// Currently visible icon cells. Order = render order.
```

## L256-258 · `catalog:    Vec<AppEntry>,`

```
/// Every catalog entry (incl. pinned) — used to populate Add submenu.
/// Cached at load + after each mutation so we don't refetch + re-hydrate
/// the whole catalog on every right-click.
```

## L260 · `open:       Option<OpenMenu>,`

```
/// Right-click context menu state.
```

## L262-265 · `moving:     Option<String>,`

```
/// While `Some(launch_name)`, the dock is in drag-to-reorder mode:
/// hovering another cell live-shuffles the moving entry to that slot,
/// and the next click of any kind exits + persists. Tracked by name
/// (stable across remove+insert) instead of index.
```

## L267-270 · `suppress_next_press: bool,`

```
/// True for one MouseButton{Left, down} after entering drag-reorder
/// mode — the compositor pushes Action(MENU_MOVE) **and** the
/// raw left-down for the same physical click, so without this flag
/// the down would immediately re-exit the drag we just entered.
```

## L272-274 · `screen_h:   i32,`

```
/// Screen height, fetched once at startup. Used to expand the dock
/// window when a menu is open so the popover has room above the tray
/// and click-outside lands inside the (now-large) dock window.
```

## L280-281 · `let catalog = app_catalog::load(&["dock", "drun", "bar", "pick"]);`

```
// Exclude the dock itself and drun — the trailing launcher
// button already opens drun, so a separate drun tile is redundant.
```

## L284-285 · `Some(pins) => order_by_pins(&catalog, &pins),`

```
// File exists (incl. empty) → honour the user's choice,
// even if it means an empty dock.
```

## L287 · `None => catalog.clone(),`

```
// File never written → first boot, seed from the catalog.
```

## L290-292 · `let mut entries: Vec<AppEntry> = Vec::with_capacity(catalog.len() + 1);`

```
// Pre-allocate `entries` to the full catalog size so subsequent
// Add/Remove mutations never re-allocate behind the persistent
// bump mark (which would leak the Vec buffer on every alloc_reset).
```

## L304 · `fn width(&self) -> i32 {`

```
/// Total dock width to request from the compositor (it clamps).
```

## L306 · `let cells = self.entries.len() as i32 + 1; // + launcher button`

```
// + launcher button
```

## L312-313 · `cells.push(Widget::Spacer { flex: 1 });`

```
// Leading flex spacer centres the icon group on the main axis
// (the row fills the full tray width, icons don't left-pack).
```

## L325-328 · `cells.push(icon_cell(`

```
// Trailing launcher button → drun (full search). Right-click on
// it opens the Add-to-dock submenu. Hover with HOVER_BASE+N (where
// N == entries.len()) so a drag-reorder can move past the last
// pinned slot, dropping the moving entry at the end of the list.
```

## L338-344 · `let tray = Widget::Row {`

```
// The tray: a SurfaceElevated pill holding the icons.
// TRAY_PAD_Y (4 px) top and bottom bumps the Row's intrinsic height
// from (icon+OnHover-pad) = 40 to a full 48 px → matches DOCK_HEIGHT
// in the idle window AND keeps the visible tray the same size when
// the menu-expand wraps it in a bottom-anchored Column (whose Spacer
// would otherwise let the tray collapse to its intrinsic 40 px and
// make the pill look shorter on right-click).
```

## L351-352 · `Modifier::Rounded(Radius::Pill.as_u8()),`

```
// Pill = ganz rund: der Rasterer klemmt den Radius auf
// `min(w/2, h/2)`, bei 48 px Hoehe also echte Halbkreise.
```

## L354-372 · `Modifier::PaddingXY { x: TRAY_PAD_X, y: TRAY_PAD_Y },`

```
// Waagrecht MEHR als senkrecht, und das ist der Punkt.
//
// Der Eckbogen frisst waagrechten Platz, und am meisten
// nicht ganz oben, sondern dort, wo der Bogen der Pille
// und der Bogen der Kachel gegeneinander laufen.
// Ausgerechnet fuer 48 px Tray und eine 34er Kachel mit
// Radius 9: die engste Stelle liegt bei y = 6,4 px und
// verlangt **4,8 px**. Mit den 4 px von vorher schnitte
// die Pille 0,8 px in die erste und letzte Kachel — beim
// Hover-Highlight sichtbar.
//
// Heute stehen dort zufaellig ~13 px, weil die zwei
// flexiblen Abstandhalter die Icons zentrieren. Das ist
// ein NEBENPRODUKT: wer ein Icon dazupinnt, verbraucht
// den Schlupf, und dann schneidet es doch. Also
// ausdruecklich 12 px, unabhaengig von der Zentrierung.
//
// Senkrecht bleiben es 4: die Hoehe bestimmt den
// Pill-Radius, und 48 px sind auch DOCK_HEIGHT.
```

## L377-383 · `let expanded = self.open.is_some() || self.moving.is_some();`

```
// When the window is expanded (menu open or drag-reorder active),
// the tray must be pushed to the bottom — wrap it in a Column with
// a flex spacer above. In the normal small window, leave the tray
// as the direct Stack child so it fills the full DOCK_HEIGHT pill
// (a Column with a Spacer would otherwise leave the tray at its
// intrinsic ~40 px and add transparent slack on top, making the
// visible pill look shorter).
```

## L400-402 · `Widget::Stack {`

```
// Wrap in a Stack so the compositor renders the dock as a translucent
// panel scene (transparent clear + chrome-opacity background, crisp
// icons composited by alpha — no halo). Same path as the bar.
```

## L409-416 · `fn apply_window_size(&self) {`

```
/// Tell the compositor what size the dock window should be. Called
/// after every state change. When a menu is open we expand toward
/// the full screen height so the popover has room and click-outside
/// lands inside the dock window; auto-hide naturally pauses because
/// the reveal hot-zone becomes the entire screen. We undershoot by
/// `DOCK_GAP_RESERVE` so the visible bottom gap from the screen edge
/// is preserved when the window is `shown` (win.y = baseline - dh -
/// gap, the dock tray then floats above the bottom edge).
```

## L418-420 · `let h = if self.open.is_some() || self.moving.is_some() {`

```
// Stay expanded while the user is mid-drag too, so any click
// (incl. on empty area) lands inside the dock window and exits
// the move cleanly.
```

## L432-435 · `let name = self.entries.get(idx)`

```
// Include the app's display name in the unpin label so users
// don't have to recognise the icon to know which app they're
// about to remove. Falls back to the launch name if the
// catalog never hydrated a display name (rare).
```

## L450-451 · `if self.entries.len() > 1 {`

```
// Moving only makes sense when there's somewhere to move
// TO — at least two pinned entries.
```

## L459 · `let pinned: alloc::collections::BTreeSet<&str> = self.entries`

```
// Every catalog entry not currently pinned.
```

## L504-508 · `Event::MouseButton { button: MouseButton::Left, down: true, .. } => {`

```
// A press anywhere — even on the empty area of an expanded
// window — exits an active drag-reorder. The hit-tested
// Action(...) above already handles clicks that land on a
// cell; this catches the in-between case (clicks on the
// transparent expand-region with no hit-tested target).
```

## L511-514 · `self.suppress_next_press = false;`

```
// This is the down half of the click that just
// entered drag mode via Action(MENU_MOVE) — swallow
// it so we don't immediately exit. The NEXT press
// commits.
```

## L528-529 · `if self.moving.is_some() {`

```
// While dragging: a hover over another cell shuffles the moving
// entry into that slot. Any non-hover Action ends the drag.
```

## L536-537 · `return self.exit_move();`

```
// Any other Action (a click on a cell, MENU_DISMISS, …) means
// the user is done dragging.
```

## L541 · `match id {`

```
// Menu actions — only fire while a popover is open.
```

## L561-563 · `self.suppress_next_press = true;`

```
// The MouseButton{down:true} paired with this
// very click is still queued — tag it to be
// ignored so the drag we just armed survives.
```

## L573 · `let already = self.entries.iter()`

```
// Guard against double-add.
```

## L585-586 · `if id >= HOVER_BASE && id < LAUNCHER {`

```
// Hover events while no popover is open and no drag is active —
// ignore (the cells re-render their own hover modifier).
```

## L590 · `if self.open.is_some() {`

```
// Regular launch click — but close any open menu first.
```

## L604-605 · `if self.moving.is_some() {`

```
// Right-click during a drag commits at the current position too —
// no separate cancel, the user accepts wherever the entry sits now.
```

## L623-624 · `fn reorder_moving_to(&mut self, target: usize) {`

```
/// Shuffle the moving entry to `target` (0..=entries.len() — the
/// launcher slot maps to entries.len(), i.e. "end of list").
```

## L628 · `self.moving = None;`

```
// The moving entry was removed somehow — drop the mode.
```

## L639-640 · `fn exit_move(&mut self) -> bool {`

```
/// Exit drag mode and write the new order to disk. Returns true so
/// the main loop re-renders + shrinks the window.
```

## L650-653 · `fn persist(&self) {`

```
/// Write the current pin order to `sys/config/dock` — one launch_name
/// per line. A leading marker line is written even when entries is
/// empty so that "no apps pinned" is distinguishable from "file
/// never existed" on the next boot. Errors are logged but non-fatal.
```

## L668 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// How an app in the dock relates to the current window set.
```

## L671 · `Idle,`

```
/// No window open.
```

## L673 · `Running,`

```
/// Has a window somewhere.
```

## L675 · `Active,`

```
/// Has the focused window.
```

## L679-684 · `fn icon_cell(`

```
/// Single dock cell — a tile holding the glyph, with the running
/// indicator dash below it. Hover keeps the Mac-style glyph bump; the
/// tile background is reserved for the app that owns the focus, so the
/// two cues never mean the same thing. The `hover` ActionId also drives
/// the drag-reorder live shuffle, and the NodeId anchors the
/// right-click popover.
```

## L700-701 · `Modifier::Scale(320),`

```
// Q8.8: 320 = 1.25× — visible bump without overflowing the
// tray enough to trample the neighbours.
```

## L722-739 · `Widget::Column {`

```
// Die Kachel haengt ohne Ausgleich ZU HOCH, und der Grund ist der
// Laufstrich: unter der Kachel stehen Abstand und Strich, ueber ihr
// nichts. Gemessen in der 48 px hohen Ablage waren das 9 px ueber dem
// Symbol und 15 darunter — die Differenz ist genau Abstand + Strich.
// Bei einer Anwendung, die NICHT laeuft, ist der Strich unsichtbar
// (aber platzhaltend), und dann sieht man die 15 als leere Flaeche.
//
// Ausgeglichen wird mit einer ebenso unsichtbaren Marke oben. Die
// beiden Zahlen sind ausgerechnet und nicht gesetzt: die Spalte muss
// bei 40 bleiben (sonst waechst die Ablage und mit ihr der
// Pill-Radius), und der Abstand zaehlt ZWEIMAL, weil er zwischen
// Marke und Kachel ebenso steht wie zwischen Kachel und Strich —
//
//     M + 2*S + 34 + 2 = 40   und   Symbol mittig
//
// hat genau eine ganzzahlige Loesung: M = 2, S = 1. Ergebnis 12 px
// ueber dem Symbol und 12 darunter, die Hover-Kachel ebenfalls
// mittig (7/7), und der Strich bleibt auf demselben Pixel wie vorher.
```

## L748 · `fn separator() -> Widget {`

```
/// Vertical hairline between the pinned apps and the trailing launcher.
```

## L758-761 · `const TITLES_CAP: usize = 2048;`

```
// The compositor's window list, kept in a static buffer rather than on
// the heap: the render loop resets the bump allocator before every
// commit, so anything derived here that lived on the heap would be a
// use-after-free one frame later.
```

## L766-767 · `fn refresh_titles() -> bool {`

```
/// Re-read the window list. Returns true when it differs from the last
/// read — the dock re-renders only then, so polling stays cheap.
```

## L772 · `unsafe {`

```
// SAFETY: single-threaded WASM app; no concurrent access.
```

## L787 · `unsafe {`

```
// SAFETY: single-threaded; the buffer is only written by refresh_titles.
```

## L795-796 · `fn run_state_of(launch_name: &str) -> RunState {`

```
/// Window titles carry the module name, so a dock entry's `launch_name`
/// matches a window line directly.
```

## L811-812 · `fn read_pins() -> Option<Vec<String>> {`

```
/// Read `sys/config/dock` — one app name (module or intent) per line.
/// Missing / empty → None (caller falls back to the full catalog).
```

## L834 · `fn order_by_pins(catalog: &[AppEntry], pins: &[String]) -> Vec<AppEntry> {`

```
/// Keep only pinned entries, in pin order. Unmatched pins are skipped.
```

## L861-863 · `dock.apply_window_size();`

```
// Window may need to grow/shrink as the menu opens or
// closes. set_dock is idempotent at the same size, so
// unconditionally calling it here costs nothing extra.
```

## L865-868 · `alloc_reset(persistent_mark);`

```
// Per-frame Vecs from render() live past the mark —
// reset and rebuild so the heap doesn't grow on every
// mutation. entries/catalog were allocated before mark
// with capacity headroom, so they survive.
```

## L874-876 · `if refresh_titles() {`

```
// Focus and window opens/closes happen elsewhere; re-read
// the window list and re-render only when the running
// indicators changed.
```

## L881-888 · `const WAIT_INPUT: i32 = 1;`

```
// **Then wait to be told.** Until 0.7.0 the dock looked
// every 16 ms — 60 times a second — whether anything had
// changed. The kernel now wakes it on an event (hover,
// click) or a window change (`WAIT_STATE`, the compositor's
// fingerprint after each frame). No deadline: an indicator
// that goes stale means a change nobody reported, and that
// must show, not be papered over by a timer.
// docs/plan/CORES_AND_EVENTS.md, Stufe 2d.
```

