# `tools/wasm/loft/src/lib.rs` @ 5e0102684

## L1-14 · `#![no_std]`

```
//! loft 0.2 — file browser, fresh rewrite against the v3 mockup.
//!
//! Layout (top → bottom):
//!   menu_bar      — Datei / Bearbeiten / Ansicht / Gehe zu / Hilfe
//!   toolbar       — back / forward / up / refresh + breadcrumb + search
//!   body          — sidebar │ grid (with empty-state)
//!   footer        — nav hints   ·   counts + selection
//!
//! Auto-focused search filters the current directory live (substring,
//! ASCII case-insensitive). Up/Down navigate the filtered grid;
//! Enter opens the selected entry; Esc clears the search if non-empty,
//! otherwise closes the window. Menu-bar clicks are intentionally
//! no-ops in v0.2 — dropdown overlays land once `Widget::Popover`
//! ships (Phase 11).
```

## L34-36 · `#[unsafe(link_section = ".npk.caps")]`

```
// Declared capabilities: read + write (copy/move/rename/delete files) +
// exec (npk_open launches the handler app) + render. Without this section
// loft would get the default READ|EXEC|RENDER and could not mutate the FS.
```

## L41-43 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L62-64 · `struct Strings {`

```
// ── Strings ───────────────────────────────────────────────────────────
// English is the source language; a new one is one more `const` below.
// See `nopeek_widgets::i18n`.
```

## L169-172 · `const HEAP_SIZE: usize = 1024 * 1024;`

```
// ── Bump allocator (1 MB — bigger than drun/loft 0.1 because the grid
//    can cover hundreds of entries in a deep directory). State alloc'd
//    before `persistent_mark` survives `alloc_reset` between commits;
//    everything after the mark is rebuilt from scratch each frame. ──
```

## L201-208 · `const AUTO_REFRESH_TICKS: u32 = 90;`

```
// ── Action-id encoding ────────────────────────────────────────────────
//
// Each interaction surface gets its own base so the dispatcher can tell
// "which thing was clicked" by an integer comparison alone — no string
// keys, no payload. Bases are 1000 apart so each surface has plenty of
// room before colliding with the next. CLICK + HOVER share a surface
// but live in different bands so we can dedup hover events without
// confusing them with clicks.
```

## L210-212 · `const AUTO_REFRESH_TICKS: u32 = 90;`

```
// Idle iterations (~16 ms each) between auto-refresh dir re-scans.
// ~1.4 s — frequent enough that a new screenshot shows up promptly,
// rare enough to be negligible load.
```

## L215-217 · `const STATS_PUMP_BUDGET: usize = 3;`

```
// Folders scanned per idle tick (~16 ms) for recursive size/count. Small
// enough that each tick stays snappy, enough that a typical directory
// fills in within a few hundred ms.
```

## L229 · `const ACT_MENU_FILE:          u32 = 5_000;`

```
// Menu-bar label clicks toggle the corresponding dropdown.
```

## L235 · `const ACT_MENU_DISMISS:       u32 = 5_500;`

```
// Click-outside-popover dismiss action.
```

## L237 · `const ACT_FILE_QUIT:          u32 = 6_000;`

```
// Dropdown items.
```

## L244 · `const ACT_HEADER_NAME:        u32 = 7_000;`

```
// List-view column headers — click to sort / toggle direction.
```

## L251-252 · `const ACT_EDIT_COPY:          u32 = 8_000;`

```
// File operations — shared by the Edit menu and the right-click context
// menu; both act on the current selection (`grid_sel`).
```

## L257 · `const ACT_RENAME_SUBMIT:      u32 = 8_100;`

```
// Rename dialog buttons.
```

## L260 · `const ACT_CTX_DISMISS:        u32 = 8_200;`

```
// Click-outside dismiss for the right-click context menu.
```

## L263-264 · `const NODE_CTX_ANCHOR: u32 = 200;`

```
// NodeId the context-menu Popover anchors against — placed on the
// selected item while the menu is open.
```

## L267 · `const NODE_MENU_FILE: u32 = 100;`

```
// NodeIds for menu-bar labels — used as Popover anchors.
```

## L282 · `struct Place {`

```
// ── State ─────────────────────────────────────────────────────────────
```

## L292-295 · `name_lc: String,`

```
/// ASCII-lowercased mirror of `name`, computed once at parse
/// time so refilter() doesn't allocate a fresh lowercase string
/// on every keystroke. Critical for typing latency once the
/// directory is large.
```

## L297-300 · `size:    u64,`

```
/// For files: the file's own byte size. For folders: the recursive
/// sum of every descendant file's size, filled in by
/// `annotate_folder_stats` on refresh (0 until then). Lets the list
/// view show real folder sizes like Thunar/Finder.
```

## L303-305 · `files:   u64,`

```
/// Number of descendant files inside a folder (recursive). 0 for
/// files. Drives the "Files" column. Filled progressively off the
/// idle loop (see `pump_stats`).
```

## L307-310 · `stats_pending: bool,`

```
/// Folder whose recursive size/count hasn't been computed yet.
/// True from refresh until `pump_stats` reaches it; always false for
/// files. Rendered as "…" so the directory paints instantly and the
/// numbers fill in without ever blocking the event loop.
```

## L312-314 · `mtime:   u64,`

```
/// UTC seconds since the Unix epoch, captured at write time by
/// the kernel. Zero = unknown (RTC was unreadable when the entry
/// was created). Filled in from the v3 `npk_fs_list` ABI tail.
```

## L324-326 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// Which column the list view sorts by. Folders are always grouped
/// before files (Thunar/Files "folders first" idiom); the key only
/// orders entries *within* each group.
```

## L347 · `Copy,`

```
/// Ctrl+C — paste keeps the source (content-addressed alias, cheap).
```

## L349 · `Cut,`

```
/// Ctrl+X — paste moves the source (rename), then the clipboard clears.
```

## L353-355 · `struct Clip {`

```
/// A pending copy/cut, set by Ctrl+C/X and consumed by Ctrl+V (paste).
/// Holds the full source path + the bare name so paste can rebuild a
/// destination in the current directory.
```

## L367-368 · `entries:        Vec<Entry>,`

```
/// Direct children of `current`. Used when the search query is
/// empty (browse mode).
```

## L370-373 · `recursive:      Vec<Entry>,`

```
/// Recursive listing of `current` (every descendant). Loaded
/// lazily on first non-empty query, cached until we navigate to
/// a different directory. Search across the whole subtree —
/// matches a Nautilus / Spotlight / VS-Code Quick Open pattern.
```

## L375-378 · `recursive_dir:  Option<String>,`

```
/// `Some(path)` when `recursive` has been filled for that path
/// in the current session; `None` after navigate() invalidates
/// the cache. Lets refilter() decide "do I need to call
/// `list_dir_recursive` again?".
```

## L380-382 · `filtered:       Vec<usize>,`

```
/// Indices into the active source list (entries / recursive)
/// matching the current search query. Equal to 0..source.len()
/// when the query is empty.
```

## L386-389 · `query:          String,`

```
/// Pre-allocated (`String::with_capacity(QUERY_CAP + 1)`) so that
/// `clear` + `push_str` stays inside the same heap block — bump
/// allocator hands out the storage before `persistent_mark`, and
/// `alloc_reset` between frames must not invalidate it.
```

## L391-392 · `query_lc:       String,`

```
/// Pre-allocated mirror used to compute `query.to_ascii_lowercase()`
/// without an extra allocation per keystroke.
```

## L394-395 · `view_mode:      ViewMode,`

```
/// Grid (Pictures-style icons) vs List (table with name + size +
/// type + modified). Switched via the View menu dropdown.
```

## L397 · `open_menu:      Option<OpenMenu>,`

```
/// Which menu's dropdown is currently visible. None = no menu open.
```

## L399-400 · `assoc:          Vec<(String, String)>,`

```
/// File associations (extension → app module), loaded once from
/// `sys/config/associations`. Checked before the built-in defaults.
```

## L402-405 · `dir_sig:        u64,`

```
/// Cheap signature (count + name/size fold) of the current dir's
/// listing. The idle loop re-lists periodically and auto-refreshes
/// when this changes — so a new file (e.g. a fresh screenshot) shows
/// up without a manual refresh.
```

## L407-409 · `sort_key:       SortKey,`

```
/// Active list-view sort column + direction. Clicking a header sets
/// the key (or toggles direction if it's already active) and re-sorts
/// `entries` in place.
```

## L412-414 · `stats_queue:    Vec<usize>,`

```
/// Indices into `entries` of folders still awaiting a recursive
/// size/count scan. Drained a few at a time off the idle loop
/// (`pump_stats`) so opening a directory never blocks on a deep tree.
```

## L416-418 · `scratch:        String,`

```
/// Reusable path buffer for `pump_stats` — pre-allocated before the
/// persistent mark so building "current/sub" each tick allocates
/// nothing on the hot path.
```

## L420 · `clipboard:      Option<Clip>,`

```
/// Pending copy/cut awaiting a paste. None = clipboard empty.
```

## L422 · `ctx_open:       bool,`

```
/// True while the right-click context menu popover is showing.
```

## L424-426 · `rename_open:    bool,`

```
/// True while the rename dialog is showing. `rename_buf` holds the
/// edited name and `rename_old` the source's full path (captured when
/// the dialog opened, so a later selection change can't misdirect it).
```

## L429-430 · `rename_buf:     String,`

```
/// Pre-allocated (like `query`) so `clear` + `push_str` on every
/// InputChange stays inside the same heap block across `alloc_reset`.
```

## L469-484 · `fn associated_app_in(&self, dir: &str, name: &str) -> Option<String> {`

```
/// Resolve the handler app for a file name via its extension:
/// `sys/config/associations` overrides first, then built-in defaults.
/// Returns None for unknown types (loft does nothing on open).
/// Wie `associated_app`, aber mit dem VERZEICHNIS daneben.
///
/// **Die Konfigurationen des Systems haben keine Endung**, und ohne
/// sie gab die Zuordnung `None` zurueck — ein Doppelklick auf
/// `sys/config/wifi` tat schlicht nichts. Das war nicht nur
/// unbequem: der Weg ueber `npk_open` ist der EINZIGE, der dem
/// Editor ein Schreibrecht fuer genau diese Datei mitgibt (der
/// Kernel vergibt es, weil der Benutzer auf die Datei gezeigt hat).
/// Ohne ihn konnte niemand eine mehrzeilige Konfiguration schreiben:
/// `store` ersetzt das Objekt mit EINER Zeile, und `spell` aus der
/// Shell bekommt weder WRITE noch eine Erlaubnis fuer den Pfad.
///
/// Also: was unter `sys/config/` liegt, ist Text.
```

## L509-512 · `"mp3" | "wav" | "mp4" | "m4v" | "mov" | "m4a" | "aac" => Some("tune".to_string()),`

```
// Only what tune can actually decode today; a .flac would open
// a player that can say nothing but "unsupported format".
// Seit tune 0.2.0 auch Bewegtbild — aber nur H.264 in MP4, und
// ein fragmentiertes sagt es selbst, statt schwarz zu bleiben.
```

## L520-522 · `self.dir_sig = dir_signature(&self.entries);`

```
// Signature is captured from the raw (un-annotated) listing so it
// matches the idle probe's `dir_signature(&list_dir(..))` — folder
// sizes are filled in below and must not perturb change-detection.
```

## L524-528 · `init_folder_stats(&mut self.entries);`

```
// Mark folders "pending" and queue them for a recursive
// size/count scan — the actual scanning happens incrementally off
// the idle loop (`pump_stats`) so the directory paints instantly
// even when it holds many/large subtrees. Order by the active
// column first (pending folders all read size 0 → tie on name).
```

## L532-534 · `self.recursive.clear();`

```
// Navigation invalidates any cached recursive listing — the
// next non-empty query for this directory triggers a fresh
// `list_dir_recursive` call.
```

## L541-545 · `fn set_sort(&mut self, key: SortKey) {`

```
/// Re-sort the browse listing under a (possibly new) column. Clicking
/// the already-active column flips direction; a different column
/// switches to it with a sensible default direction (ascending for
/// text, descending for the numeric/date columns where "biggest /
/// newest first" is the usual intent).
```

## L557-563 · `fn pump_stats(&mut self, budget: usize) -> bool {`

```
/// Compute the recursive size + file count for up to `budget` queued
/// folders, one host scan per folder. Runs off the idle loop so a
/// directory with many or deep subfolders fills in progressively
/// instead of freezing the app on open. Returns true if anything
/// changed (→ caller re-renders). Caller must wrap this between
/// `alloc_reset(persistent_mark)` / re-capture like the other state
/// mutations so `filtered`/`scratch` growth lands in the kept region.
```

## L570-571 · `self.scratch.clear();`

```
// Build "current/<name>" into the reusable scratch buffer
// (disjoint field borrows — no per-tick allocation).
```

## L584-585 · `if self.stats_queue.is_empty()`

```
// Settle the order once every folder is in, but only when the
// active column actually depends on the numbers we just filled.
```

## L595-598 · `fn ensure_search_source(&mut self) -> bool {`

```
/// Pick the active source list for filtering — direct children
/// when the query is empty (browse mode), recursive descendants
/// otherwise (search mode). Lazy-loads the recursive listing on
/// first non-empty query for the current directory.
```

## L616 · `self.query_lc.clear();`

```
// Reuse the pre-mark buffer for the lowercased query.
```

## L631-633 · `fn source(&self) -> &Vec<Entry> {`

```
/// Source list paired with `filtered` — entries when browsing,
/// recursive when searching. Renderer + open_selected use this
/// instead of always going through `entries`.
```

## L654-655 · `self.query.clear();`

```
// Navigation clears the search filter — entering a fresh
// directory should show its full contents, not an empty view.
```

## L686-688 · `let (is_dir, name) = match self.source().get(entry_idx) {`

```
// In search mode `source()` returns the recursive list, so
// `entry.name` is a relative path like "wallpapers/aurora"
// — the same join below gives the correct absolute target.
```

## L701 · `let full = if self.current.is_empty() {`

```
// Open the file with its associated app (file association).
```

## L716-718 · `fn select_delta_y(&mut self, dy: isize) {`

```
/// One row down. In the grid a row is `GRID_COLS` entries; in the list
/// — which is the default view — a row is ONE entry. Multiplying
/// unconditionally made Down skip three files at a time.
```

## L727-729 · `fn select_delta_x(&mut self, dx: isize) {`

```
/// Sideways only means something in the grid. In a one-per-row list it
/// would just duplicate Up/Down, which reads as the selection jumping
/// for no reason.
```

## L745 · `fn selected(&self) -> Option<(String, bool)> {`

```
// ── File operations ───────────────────────────────────────────────
```

## L747-748 · `fn selected(&self) -> Option<(String, bool)> {`

```
/// The selected entry's name (in search mode a relative sub-path) and
/// dir flag, or None if nothing is selected.
```

## L756 · `fn can_paste(&self) -> bool { self.clipboard.is_some() }`

```
/// True if a paste is possible (clipboard holds something).
```

## L759 · `fn do_copy(&mut self) {`

```
/// Ctrl+C — remember the selection for a keep-source paste.
```

## L770 · `fn do_cut(&mut self) {`

```
/// Ctrl+X — remember the selection for a move paste.
```

## L781-782 · `fn do_paste(&mut self) {`

```
/// Ctrl+V — copy or move the clipboard source into the current dir,
/// picking a collision-free name. A Cut clears the clipboard on success.
```

## L807 · `fn open_rename(&mut self) {`

```
/// Open the rename dialog pre-filled with the selection's name.
```

## L813-814 · `let max = self.rename_buf.capacity().min(base.len());`

```
// Stay within the pre-allocated capacity so the InputChange
// mirror never reallocates (bump-heap discipline).
```

## L823 · `fn commit_rename(&mut self) {`

```
/// Commit the rename dialog: move `rename_old` → current/<new name>.
```

## L826 · `if new.is_empty() || new.contains('/') {`

```
// Reject empty / path-bearing names — rename stays in-place.
```

## L842 · `fn unique_dest(&self, name: &str) -> String {`

```
/// A collision-free destination for `name` in the current directory.
```

## L848 · `fn render(lf: &Loft) -> Widget {`

```
// ── Render ────────────────────────────────────────────────────────────
```

## L853-854 · `let body = if lf.rename_open { render_rename_dialog(lf) } else { render_body(lf) };`

```
// The rename dialog replaces the file area while it's up (same idiom
// as spell's "save as" dialog) so its Input is the only editable field.
```

## L857-862 · `let mut children: Vec<Widget> = alloc::vec![`

```
// Custom outer column instead of `prefab::panel`: panel's
// Padding-Xs + Spacing-Md kept the menu-bar bg from reaching
// the window edges + put a 12 px gap between menu and divider.
// Loft wants the menu strip + sidebar fill to be flush —
// file-manager idiom (Thunar / Files / Finder all do this).
// Footer removed (noise) — the body fills to the bottom edge.
```

## L868 · `body,                           // Modifier::Flex(1) — fills`

```
// Modifier::Flex(1) — fills
```

## L871-875 · `if let Some(kind) = lf.open_menu {`

```
// Append the open menu's dropdown as a Popover. The compositor
// resolves `anchor` against the matching menu-label NodeId
// (recorded during layout) and floats the dropdown directly
// below it. Click outside fires `on_dismiss = ACT_MENU_DISMISS`
// which we route to clearing `open_menu`.
```

## L885-886 · `children.push(Widget::Popover {`

```
// Right-click context menu, floated at the selected item (which
// carries NODE_CTX_ANCHOR while the menu is open).
```

## L903-905 · `fn file_op_items(lf: &Loft) -> Vec<(String, ActionId)> {`

```
/// Copy / Cut / Paste / Rename items, shared by the Edit menu and the
/// right-click context menu. Entries appear only when meaningful — Copy /
/// Cut / Rename need a selection, Paste needs a filled clipboard.
```

## L920 · `items.push((s().no_action.to_string(), ActionId(ACT_MENU_DISMISS)));`

```
// Keep the surface non-empty so the click still reads as handled.
```

## L926-928 · `fn ctx_anchor_wrap(child: Widget) -> Widget {`

```
/// Wrap a grid/list item so the context-menu Popover has an anchor rect.
/// Applied only to the selected item while the menu is open, so normal
/// rendering is untouched.
```

## L938-940 · `fn render_rename_dialog(lf: &Loft) -> Widget {`

```
/// Modal rename dialog — mirrors spell's name dialog. Focus doesn't
/// auto-jump on a re-commit, so the field must be clicked before typing
/// (the footer hint says so); Enter commits, Esc cancels.
```

## L997-999 · `fn render_dropdown(lf: &Loft, kind: OpenMenu) -> (u32, Widget) {`

```
/// Build the dropdown for the currently-open menu. Returns
/// `(anchor_node_id, content_widget)` so the caller can wrap the
/// content in a `Widget::Popover` against the matching menu label.
```

## L1053-1054 · `modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],`

```
// Own padding now that the outer Column is flush — keeps
// back/forward/breadcrumbs + search bar off the chrome.
```

## L1059-1064 · `fn search_input(query: &str) -> Widget {`

```
/// Hand-rolled search input with always-visible chrome — `prefab::input`
/// blends with the panel by design (drun's launcher look), but loft's
/// toolbar wants the search bar to read as a discrete, framed widget
/// matching the v3 mockup. Same magnifier prefix + Heading text +
/// focus-accent border, plus a baseline `SurfaceMuted` fill and a
/// `Border` stroke that's visible without focus too.
```

## L1084-1087 · `Modifier::PaddingXY { x: 8, y: 0 },`

```
// Asymmetric on purpose: uniform Padding tied the side air to
// the row height, so shrinking the field to 30 px squeezed the
// magnifier against the border. (The Input adds ~4 px of its
// own chrome, so x=8 reads as the design's 12.)
```

## L1093-1094 · `Modifier::Focus(alloc::vec![`

```
// 1 px accent border plus a 3 px ring, per the design's
// `text_field` focus state (docs/spec/UI_REFRESH.md §3).
```

## L1103-1105 · `fn storage_meter() -> Option<Widget> {`

```
/// Disk fill level at the foot of the sidebar: a slim accent-filled
/// track plus the percentage. Hidden when the kernel reports no
/// mounted filesystem.
```

## L1114-1115 · `const TRACK_W: u16 = 108;`

```
// The track is a fixed-width bar; the fill is the same bar clipped
// to `pct` of that width, laid over it in a Stack.
```

## L1149-1152 · `let mut places_rows: Vec<Widget> = Vec::new();`

```
// Sidebar — PLACES (Home/Documents/Downloads/Pictures/Projects)
// + DEVICES (Filesystem/Trash) per the mockup. `nav_row`
// selected-state lights up when the current dir matches a
// sidebar path verbatim.
```

## L1165-1169 · `let sections = Widget::Column {`

```
// Sections scroll; the capacity meter is a fixed footer BELOW that
// scroll, so it stays glued to the bottom edge at any window height.
// Putting it inside the scrolled column (or leaning on
// `prefab::sidebar_pane`, which appends its own trailing Spacer)
// left it floating in the middle of the leftover space.
```

## L1183-1184 · `modifiers: alloc::vec![Modifier::Flex(1)],`

```
// Flex(1) is what pins the footer down: the scroll swallows
// all leftover height instead of the meter drifting up.
```

## L1203-1204 · `let content: Widget = if lf.filtered.is_empty() {`

```
// Content — filtered grid OR list, plus two empty states
// (genuinely empty directory vs. nothing matched the search).
```

## L1219-1222 · `let content = Widget::Scroll {`

```
// Wrap the file area in a vertical Scroll so a long listing scrolls
// (mouse wheel) and is clipped to the body instead of overflowing and
// pushing the footer off-screen in a small (¼-screen) window. The
// overlay scrollbar only shows when the content actually overflows.
```

## L1233-1237 · `modifiers: alloc::vec![Modifier::Flex(1)],`

```
// Flex(1) makes the body absorb all leftover vertical space in
// the parent Column. Sidebar inherits via Stretch align so its
// SurfaceMuted bg now reaches the footer divider regardless of
// grid content height. Without this the body is intrinsic-sized
// and the bg ends where its tallest child does.
```

## L1243-1248 · `let source = lf.source();`

```
// `source()` gives us either direct children (browse) or
// recursive descendants (search) — `filtered` indexes into
// whichever is active. Recursive entries already carry their
// sub-path in `name` so the grid label reads "wallpapers/aurora"
// for a search hit, which is the desired "show me where the
// match lives" UX.
```

## L1265-1269 · `fn render_list(lf: &Loft) -> Widget {`

```
/// Detail-list view: one row per entry, columns Name | Size | Files |
/// Type | Modified, spanning the full window width (Name flexes to fill
/// the slack). Headers are clickable — a click sorts by that column,
/// clicking the active column flips direction (▲/▼ marker). English
/// headers (Florian's request — international FS UX).
```

## L1272-1274 · `let browsing = lf.query.is_empty();`

```
// Folder size/count is only computed for the browse listing; in
// search mode (recursive source) folders show "—" rather than a
// misleading zero.
```

## L1315-1318 · `fn header_cell(lf: &Loft, label: &str, key: SortKey, action: u32,`

```
/// One clickable column header. Mono + faint, so the header band reads
/// as structure rather than as another row of data (docs/spec/UI_REFRESH.md §5).
/// Appends a ↑/↓ marker on the active sort column; `flex` lets the Name
/// header grow to fill the row.
```

## L1342-1343 · `let name_cell = Widget::Row {`

```
// Name cell with icon + label. Flex(1) so the column absorbs the
// row's slack and the fixed columns sit flush against the right edge.
```

## L1363-1365 · `let size_str = if e.is_dir {`

```
// Folders show their recursive byte sum + file count once scanned
// ("…" while pending, "—" in search mode where we don't compute it);
// files show their own size and "—" in the Files column.
```

## L1386-1389 · `let mut row_mods: Vec<Modifier> = alloc::vec![`

```
// Selection reads as a tint plus a 2 px accent edge on the leading
// side — never a boxed-in row (docs/spec/UI_REFRESH.md §3 `list_row`). The
// edge occupies its space on every row so nothing shifts sideways
// when the selection moves.
```

## L1420-1421 · `fn list_cell_text(text: &str, min_w: u16) -> Widget {`

```
/// A metadata column. Mono + faint so the eye runs down the file names
/// and only lands on the numbers when it goes looking for them.
```

## L1433 · `const SIDEBAR_W:    u16 = 176;`

```
/// Sidebar width from the design (docs/spec/UI_REFRESH.md §5).
```

## L1438-1439 · `const ROW_PAD:      u16 = 4;`

```
/// Vertical inset a list row adds around its content. Subtracted from the
/// selection edge so edge + padding lands exactly on DATA_ROW_H.
```

## L1442 · `const COL_NAME_W:  u16 = 240;   // min — flexes to fill the row`

```
// min — flexes to fill the row
```

## L1459-1461 · `segs.push((part.to_string(),`

```
// Each segment fires the same action base + segment count
// so the dispatcher can rebuild the prefix from the path.
// Simpler than embedding the path bytes in the ActionId.
```

## L1469 · `enum Outcome { Idle, Rerender, Exit }`

```
// ── Event dispatch ────────────────────────────────────────────────────
```

## L1474-1476 · `if lf.rename_open {`

```
// The rename dialog is modal: while it's up it owns Enter/Esc + its
// buttons, and InputChange feeds the name buffer. Every other event is
// swallowed so grid navigation doesn't run underneath the dialog.
```

## L1494-1496 · `if lf.ctx_open {`

```
// Cancel the most-specific overlay first, then clear a search,
// then quit — the common cancel-then-quit ladder (Finder /
// Spotlight / editors).
```

## L1514-1517 · `lf.select_delta_x(-1); Outcome::Rerender`

```
// Compositor consumes Left/Right when the search Input
// is focused; if we get this event it means search is
// empty AND focus is somewhere non-editing — fall back
// to grid horizontal nav.
```

## L1522 · `Event::Key(KeyCode::F(2))      => { lf.open_rename(); Outcome::Rerender }`

```
// F2 renames the selection — the familiar file-manager shortcut.
```

## L1525-1528 · `lf.go_up(); Outcome::Rerender`

```
// Same fall-through reasoning as Left/Right above —
// Backspace inside a non-empty search is consumed by the
// editor; reaching us means search was empty, treat it
// as "go up" (Finder convention).
```

## L1531-1532 · `Event::Clipboard(ClipKind::Copy)  => { lf.do_copy();  Outcome::Rerender }`

```
// Ctrl+C / X / V, delivered by the compositor because loft's grid
// isn't a text widget. Copy/cut arm the clipboard; paste applies it.
```

## L1537-1540 · `lf.query.clear();`

```
// Mirror the new buffer into our pre-mark `query` slot
// (clear + push_str within capacity) so it survives the
// upcoming `alloc_reset`. Past QUERY_CAP we hard-cap;
// the compositor reconciles on the next round-trip.
```

## L1547 · `Event::ContextAction(ActionId(id)) => handle_context(lf, id),`

```
// Right-click a file/folder → select it and open the context menu.
```

## L1554-1555 · `fn handle_context(lf: &mut Loft, id: u32) -> Outcome {`

```
/// Right-click dispatch: select the clicked grid/list item and raise the
/// context menu popover anchored to it.
```

## L1575-1578 · `ACT_MENU_FILE => { lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::File); Outcome::Rerender }`

```
// Menu-bar labels: toggle the matching dropdown. Clicking the
// already-open menu's label re-fires this and closes it
// (matches macOS / Files behavior). Clicking a different menu
// switches dropdowns directly.
```

## L1584 · `ACT_MENU_DISMISS => {`

```
// Click-outside-popover dismiss — close the open menu.
```

## L1593 · `ACT_FILE_QUIT => Outcome::Exit,`

```
// Dropdown items.
```

## L1621-1623 · `ACT_EDIT_COPY  => { lf.do_copy();  lf.open_menu = None; lf.ctx_open = false; Outcome::Rerender }`

```
// File operations — from the Edit menu or the right-click context
// menu. Both close whichever menu raised them and act on the
// current selection / clipboard.
```

## L1633 · `ACT_HEADER_NAME  => { lf.set_sort(SortKey::Name);     Outcome::Rerender }`

```
// Column-header clicks → sort / toggle direction.
```

## L1684 · `fn default_sidebar(home: &str) -> Vec<Place> {`

```
// ── Sidebar helpers ───────────────────────────────────────────────────
```

## L1700-1701 · `fn toggle_menu(current: Option<OpenMenu>, target: OpenMenu) -> Option<OpenMenu> {`

```
/// Click on a menu-bar label: open it if no menu was open or a
/// different one was, close it if the same one was already open.
```

## L1709 · `fn load_associations() -> Vec<(String, String)> {`

```
// ── Kernel-side calls ─────────────────────────────────────────────────
```

## L1711-1713 · `fn load_associations() -> Vec<(String, String)> {`

```
/// Parse `sys/config/associations` (optional) into (ext, app) pairs.
/// One mapping per line: `ext=app` (`#` comments + blanks skipped).
/// Absent file → empty (loft falls back to built-in defaults).
```

## L1738-1740 · `let buf_ptr = core::ptr::addr_of_mut!(NAME_BUF) as *mut u8;`

```
// The username lives in the single encrypted `.system/config` blob,
// not a fetchable `sys/config/name` object — ask the kernel for the
// resolved home dir directly.
```

## L1755-1757 · `fn dir_signature(entries: &[Entry]) -> u64 {`

```
/// Cheap order-sensitive signature of a directory listing — folds count,
/// names, sizes and is_dir of every entry (FNV-1a). Changes when a file
/// is added, removed, renamed or resized. Used for auto-refresh.
```

## L1771-1775 · `fn list_dir_recursive(prefix: &str) -> Vec<Entry> {`

```
/// Recursive listing — `recursive=1` to the host fn — for search
/// mode. Each entry's `name` is the full sub-path under `prefix`
/// (e.g. "wallpapers/aurora" when listing under
/// "home/florian/pictures"), so a search hit visually points at the
/// match's location. Skips synthetic `.dir` markers.
```

## L1811 · `fn init_folder_stats(entries: &mut [Entry]) {`

```
// ── Folder size/count + sorting ───────────────────────────────────────
```

## L1813-1815 · `fn init_folder_stats(entries: &mut [Entry]) {`

```
/// Reset every folder row to "pending" so `pump_stats` will scan it.
/// Order-independent (just flips flags), so it's safe to call before the
/// sort. Files are left untouched (they already carry their own size).
```

## L1826 · `fn pending_folder_indices(entries: &[Entry]) -> Vec<usize> {`

```
/// Indices of folders still flagged pending, in current (sorted) order.
```

## L1834-1837 · `fn scan_folder_stats(path: &str) -> (u64, u64) {`

```
/// Recursive (size, file_count) of a single folder. One
/// `npk_fs_list(recursive=1)` scan, summed inline without allocating an
/// `Entry` per descendant — cheap even for large subtrees, and called one
/// folder at a time off the idle loop so no single scan stalls the app.
```

## L1851 · `if e.is_dir { continue; }                // directory → count files only`

```
// directory → count files only
```

## L1858-1860 · `fn sort_entries(v: &mut [Entry], key: SortKey, asc: bool) {`

```
/// Sort the browse listing: folders always grouped before files
/// (Thunar/Files idiom), then ordered within each group by `key`,
/// reversed for descending.
```

## L1884-1887 · `fn filter_sidebar_to_existing(places: Vec<Place>) -> Vec<Place> {`

```
/// Drop sidebar entries whose path is not currently backed by a
/// `.dir` marker. Keeps "Filesystem" (empty path = npkFS root) — it
/// always exists by definition. Honest UI: if you can see it, you
/// can navigate into it without hitting an empty phantom.
```

## L1890 · `if p.path.is_empty() { return true; } // Filesystem root`

```
// Filesystem root
```

## L1896-1900 · `let mut out = [0u8; 17];`

```
// npk_fs_stat returns 17 bytes since kernel v0.146 (size + is_dir
// + mtime). Kept buffer-sized to the wider shape; the is_dir byte
// sits at offset 8 in both v2 and v3 ABI so the check stays
// forward-compat against future appends. `n > 0` distinguishes a
// valid stat from "not found" (0) or "error" (-1).
```

## L1911 · `fn join(dir: &str, name: &str) -> String {`

```
// ── Path helpers for file operations ──────────────────────────────────
```

## L1913-1915 · `fn join(dir: &str, name: &str) -> String {`

```
/// Join a directory path with a child name. npkFS uses slash paths and
/// the filesystem root is the empty string, so a join off root omits the
/// leading slash.
```

## L1920 · `fn basename(name: &str) -> &str {`

```
/// Final path component of a (possibly relative, search-mode) name.
```

## L1928 · `fn path_exists(path: &str) -> bool {`

```
/// True if any object (file or directory) exists at `path`.
```

## L1937-1939 · `fn split_ext(name: &str) -> (&str, &str) {`

```
/// Split a file name into (stem, extension-with-dot): "a.txt" → ("a",
/// ".txt"); "README" → ("README", ""). A leading dot (dotfile) stays in
/// the stem so the copy suffix lands before any real extension.
```

## L1947-1949 · `fn unique_in(dir: &str, name: &str) -> String {`

```
/// A collision-free full path for `name` inside `dir`. Appends " copy",
/// then " copy 2", " copy 3"… before the extension until the path is free
/// (Finder/Files idiom), capped so a pathological directory can't spin.
```

## L1967-1971 · `fn parent_path(path: &str) -> String {`

```
// Wire: name\0size_le_u64(8)\0is_dir_u8(1)\0mtime_le_u64(8) on
// kernel ≥ v0.146; older kernels stop after is_dir (10 trailing
// bytes). Parse defensively — accept either shape so the loft
// .wasm boots on a stale-kernel disk during dev cycles.
// ── Path helpers ──────────────────────────────────────────────────────
```

## L1993 · `fn type_for(e: &Entry) -> String {`

```
// ── Icon + type label ─────────────────────────────────────────────────
```

## L1995-1996 · `fn type_for(e: &Entry) -> String {`

```
/// Human-readable type column for the list view. Mirrors the
/// `icon_for` taxonomy so the icon and the label always agree.
```

## L2009-2014 · `fn format_mtime(secs: u64) -> String {`

```
/// Render a Unix-second timestamp as "YYYY-MM-DD HH:MM" UTC. Zero
/// → "—" (mtime unknown — RTC was unreadable when the entry was
/// created, or the entry was written by a pre-v3 kernel that
/// didn't have the field). No std::time, no chrono — pure integer
/// math against the proleptic Gregorian calendar, matching what
/// `kernel/src/drivers/rtc.rs::datetime_to_unix` reverses.
```

## L2027-2029 · `fn unix_to_civil(secs: u64) -> (i32, u32, u32, u32, u32, u32) {`

```
/// `Howard Hinnant`-style civil_from_days. Converts Unix seconds to
/// (year, month [1..=12], day [1..=31], hour, minute, second) in UTC
/// without leap-second awareness (good enough for "modified" UI).
```

## L2037 · `let z = days + 719_468;`

```
// Shift epoch to 0000-03-01 to make leap math simple.
```

## L2061-2062 · `fn format_size(n: u64) -> String {`

```
/// Wrapper around the in-place `push_size` helper used by the
/// footer — returns an owned String for the list view's Size cell.
```

## L2076 · `"mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aac" => IconId::FileAudio,`

```
// The icon says what the file IS, not what we can play yet.
```

## L2078-2079 · `"mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" => IconId::Image,`

```
// Kein eigenes Filmsymbol im Atlas — `Image` ist das naechste, das
// stimmt: ein Film ist eine Folge davon.
```

## L2085 · `fn push_usize(s: &mut String, mut n: usize) {`

```
// ── Number formatters (no_std friendly) ───────────────────────────────
```

## L2096-2098 · `const K: u64 = 1024;`

```
// Powers of 1024 — KB / MB / GB. Two decimals once we leave bytes,
// mockup-aligned ("2.4 GB" rather than "2456 MB"). Pure integer
// math (no f64 in no_std without messing with the linker).
```

## L2126 · `fn commit_tree(lf: &Loft) {`

```
// ── Entry point ───────────────────────────────────────────────────────
```

## L2138-2155 · `let mut loft = Loft::new();`

```
// No `npk_window_set_overlay` — loft is a regular tiled app, the
// first commit creates its window via shade::create_widget_window.
//
// Bump-allocator lifecycle:
//   * `persistent_mark` is the heap top *after* the last state
//     mutation. Anything below it is live `Loft` state (entries,
//     history, sidebar Strings, …) that next frame still needs.
//     Anything above it is the previous frame's Widget tree —
//     transient, safe to wipe.
//   * Reset goes *before* `handle()`, not before render.
//     Otherwise `navigate()`'s freshly-loaded entries land above
//     the old mark and get clobbered by the very Widget allocs
//     that follow — the Vec metadata in `loft.entries` survives
//     but its String contents are overwritten mid-render →
//     UTF-8 / bounds panic on the next navigate.
//   * `persistent_mark` is re-captured after `handle()` so the
//     new state allocs (if any) become part of the persistent
//     region for next frame.
```

## L2161-2162 · `unsafe { let _ = npk_window_set_clipboard_sink(); }`

```
// The widget window exists after the first commit — opt into receiving
// Ctrl+C/X/V as Event::Clipboard so the shortcuts drive file operations.
```

## L2181-2184 · `if loft.query.is_empty() && !loft.stats_queue.is_empty() {`

```
// Progressively fill folder sizes/counts a few per tick.
// Wrapped in the alloc_reset/recapture discipline so the
// scratch/filtered growth lands in the persistent region.
// Skipped while searching (folder stats aren't shown then).
```

## L2195-2197 · `if loft.open_menu.is_none() && loft.query.is_empty() {`

```
// Auto-refresh the browse view when the folder changed
// on disk (new screenshot, download, …). Skip while a
// search or menu is active so we don't disturb the user.
```

## L2199-2202 · `alloc_reset(persistent_mark);`

```
// Probe via a THROWAWAY listing + reset so an
// unchanged folder leaks nothing in the bump heap
// (this runs ~every 1.4 s). Only a real change does
// the persistent re-list + re-render.
```

## L2219-2220 · `#[allow(dead_code)]`

```
// Silence unused warning on app_meta::IconRef — referenced through
// the build.rs-generated AppMeta blob, not directly.
```

