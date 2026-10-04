# `tools/wasm/pick/src/lib.rs` @ 5e0102684

## L1-15 · `#![no_std]`

```
//! pick 0.1 — the file dialog, as a portal.
//!
//! The kernel starts this module on `npk_pick` and hands it the request
//! as a launch argument (`"<open|save>\0<start-dir>\0<suggested-name>"`).
//! We browse npkFS, let the user choose, and report the path back with
//! `npk_pick_result`. The kernel turns that into an `Event::Picked` for
//! whoever asked.
//!
//! Two properties are the whole point of doing it this way:
//!
//!   - **We hold READ, the requester doesn't.** An app can offer Open
//!     and Save without the right to walk the filesystem itself.
//!   - **We never write.** Save mode returns a *path*; the requester
//!     does the writing. So this module needs no WRITE, and a bug here
//!     cannot damage a file.
```

## L35-36 · `#[unsafe(link_section = ".npk.caps")]`

```
// Read to list directories, render to draw. Deliberately no WRITE (we
// return paths, we don't create files) and no EXEC.
```

## L67 · `fn answer(path: &str) {`

```
/// Answer the requester and go away. An empty path means cancelled.
```

## L73 · `struct Strings {`

```
// ── Strings ───────────────────────────────────────────────────────────
```

## L149 · `const EVENT_BUF_SIZE: usize = 8 * 1024;`

```
// ── Buffers ───────────────────────────────────────────────────────────
```

## L154-155 · `const LIST_BUF_SIZE: usize = 256 * 1024;`

```
// Directory listings. npkFS names are long; a deep home dir with many
// files needs room. Oversized rather than truncating a listing silently.
```

## L159-160 · `const COUNT_BUF_SIZE: usize = 64 * 1024;`

```
// Separate scratch for the per-folder child count — the outer listing is
// still being read out of LIST_BUF while these run.
```

## L170-172 · `const PAYLOAD_CAP: usize = 1024;`

```
// InputChange hands us a heap String that `alloc_reset` frees before
// `handle` runs — copy it out first (the same use-after-free that bit
// spell and loft).
```

## L203 · `const HEAP_SIZE: usize = 2 * 1024 * 1024;`

```
// ── Bump allocator ────────────────────────────────────────────────────
```

## L233 · `const ACT_CONFIRM:   u32 = 1;`

```
// ── Action ids ────────────────────────────────────────────────────────
```

## L246 · `const ACT_ENTRY_BASE: u32 = 1_000;`

```
// i-th entry in the listing / i-th breadcrumb segment.
```

## L250 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
// ── State ─────────────────────────────────────────────────────────────
```

## L259 · `items:  Option<usize>,`

```
/// Folders only: how many entries they hold. None = not counted.
```

## L265 · `dir:      String,`

```
/// Directory currently shown (npkFS path, no trailing slash).
```

## L268 · `selected: Option<usize>,`

```
/// Index into `entries`, or None when nothing is picked yet.
```

## L270 · `name:     String,`

```
/// Save mode: the filename being typed, extension included.
```

## L272 · `history:  Vec<String>,`

```
/// Directories visited, for the Back arrow.
```

## L274 · `confirm_overwrite: bool,`

```
/// Save mode: showing the "replace existing file?" confirmation.
```

## L276-277 · `new_folder: bool,`

```
/// Naming a new folder. Its own buffer, not `name` — in save mode
/// that one already holds the filename and must survive the detour.
```

## L284 · `let arg = read_launch_arg();`

```
// Request wire: "<open|save>\0<start-dir>\0<suggested-name>".
```

## L301-302 · `name:     String::with_capacity(NAME_CAP),`

```
// Pre-allocate so typing doesn't reallocate past the
// persistent mark and get freed by the next alloc_reset.
```

## L314 · `fn full_name(&self) -> String {`

```
/// Name as it will land on disk — exactly what the field shows.
```

## L324 · `fn enter_dir(&mut self, path: String) {`

```
/// Navigate to `path`, remembering where we came from for Back.
```

## L342-344 · `fn go_parent(&mut self) {`

```
/// Go up one level. From a top-level directory ("home") that means the
/// npkFS root, which lists as the empty prefix — not a no-op, or the
/// user could never leave the branch they started in.
```

## L363-365 · `fn activate(&mut self) -> bool {`

```
/// Act on the current selection: descend into a folder, or return a
/// file. Files are only selectable in open mode (in save mode they are
/// shown dimmed, so you can see what you would overwrite).
```

## L382-383 · `fn confirm(&mut self) -> bool {`

```
/// The confirm button: open the selected file, or commit the typed
/// name. Returns true when the dialog is done.
```

## L397-398 · `if !self.confirm_overwrite && path_exists(&self.join(&name)) {`

```
// Warn before clobbering — the requester writes blind, so
// this is the only place the user can be asked.
```

## L409-411 · `fn create_folder(&mut self) -> bool {`

```
/// Create the folder the user just named and step into it — that is
/// almost always why they made it. Returns false so the dialog stays
/// open (a new folder is a step towards picking, not the answer).
```

## L428-430 · `fn selectable(&self, i: usize) -> bool {`

```
/// True if row `i` can hold the selection. In save mode files are on
/// display but not choosable, so the arrows skip over them rather
/// than parking on a row that does nothing.
```

## L445-446 · `while i >= 0 && i <= last {`

```
// Walk in the requested direction until a selectable row turns up;
// stop at the edge rather than wrapping.
```

## L457 · `fn read_launch_arg() -> String {`

```
// ── Filesystem ────────────────────────────────────────────────────────
```

## L478-479 · `fn list_dir(dir: &str) -> Vec<Entry> {`

```
/// Immediate children of `dir`, folders first then files, each group
/// sorted by name. Wire per line: `name\0size(8)\0is_dir(1)\0mtime(8)`.
```

## L494-496 · `let items = if e.is_dir { Some(count_children(&full)) } else { None };`

```
// Count a folder's children so the right-hand column carries real
// information. One extra listing per folder — fine for a dialog
// showing one directory, and it is the number the user wants.
```

## L513-514 · `fn count_children(dir: &str) -> usize {`

```
/// Number of entries directly inside `dir`. Uses its own buffer so it can
/// run while the outer listing is still being parsed out of `LIST_BUF`.
```

## L528-530 · `fn clamp_str(s: &str, max: usize) -> &str {`

```
/// Truncate to at most `max` BYTES without splitting a character. Slicing
/// a `&str` mid-codepoint panics, and a panicking widget app freezes the
/// machine — so every cap on a user-supplied name goes through here.
```

## L546-551 · `const ROW_H:   u16 = 34;`

```
// ── Render ────────────────────────────────────────────────────────────
//
// Follows loft's list language (docs/spec/UI_REFRESH.md §3/§5): 34 px rows, icon +
// name on the left, one mono column on the right carrying real
// information, and a selection that reads as an accent tint plus a 2 px
// leading edge — never a floating outline.
```

## L553 · `const ROW_H:   u16 = 34;`

```
/// Row metrics, matched to loft's list view so the two read as one system.
```

## L556 · `const COL_META_W: u16 = 96;`

```
/// Right-hand column: "4 items" / "12 KB" / "—".
```

## L570-574 · `let mut children: Vec<Widget> = Vec::with_capacity(9);`

```
// Flat in the panel's own Column, with Flex(1) on the Scroll ITSELF.
// `measure` reports only a 24 px floor for a scroll container on its
// axis (that's what lets a flex parent size it), so a Flex wrapper
// around an unflexed Scroll hands the list 24 px and squashes the
// rows. loft puts the Flex on the Scroll for the same reason.
```

## L593 · `fn render_title_bar(title: &str) -> Widget {`

```
/// Title row — icon + title, close button on the right.
```

## L602-603 · `Widget::Text {`

```
// Body, not Title: Title is 24 px bold and shouted over a
// dialog whose every other line is 14 px.
```

## L618 · `fn render_toolbar(p: &Pick) -> Widget {`

```
/// Back / up / breadcrumb, with "New folder" pinned right.
```

## L627-629 · `if p.mode == Mode::Save { new_folder_button() } else { Widget::Spacer { flex: 0 } },`

```
// Only when saving. Opening an existing file has no use for a
// new directory, and offering it there just invites a stray
// write on a dialog that is meant to be read-only.
```

## L666-667 · `fn nav_icon(icon: IconId, action: u32, enabled: bool) -> Widget {`

```
/// A toolbar arrow. Disabled ones stay in place (no layout shift) but
/// lose their click target and fade.
```

## L683-684 · `fn crumbs(dir: &str) -> Vec<(String, ActionId)> {`

```
/// Path split into clickable segments, so the user can jump back up
/// several levels at once instead of pressing "up" repeatedly.
```

## L714-715 · `modifiers: alloc::vec![Modifier::Flex(1)],`

```
// Flex on the Scroll: it swallows the leftover height, which is
// what pins the name field and buttons to the bottom.
```

## L720-725 · `fn entry_row(p: &Pick, e: &Entry, i: usize) -> Widget {`

```
/// One listing row: icon + name, then a mono column with the item count
/// (folders) or size (files). A chevron marks the selected row.
///
/// In save mode files are shown but NOT selectable — you can see what
/// you would overwrite without the list fighting the name field over
/// what "chosen" means.
```

## L774-775 · `let edge = prefab::mark(2, ROW_H - 2 * ROW_PAD,`

```
// The 2 px edge occupies its space on every row, so nothing shifts
// sideways when the selection moves (loft's rule).
```

## L792-793 · `fn meta_cell(e: &Entry) -> Widget {`

```
/// Right column — item count for folders, size for files. Mono, so the
/// numbers line up down the list.
```

## L826-829 · `fn render_name_field(p: &Pick) -> Widget {`

```
/// "Name" label + the field, separated from the list so it reads as the
/// thing you are creating rather than another list entry. A known
/// extension trails the caret dimmed, so it reads as a suffix, not as
/// part of the name you are typing.
```

## L831-837 · `let field: Vec<Widget> = alloc::vec![`

```
// One field, whole filename. The extension used to sit beside the
// Input as its own dimmed Text, but `measure` floors an Input at
// 120 px so empty fields don't collapse — so on a short name the
// suffix drifted off to the right of that floor ("test|      .py")
// and crept back as you typed. Spans on an Input would fix it
// properly; that needs ABI the widget doesn't have. A single field
// also lets the user change the extension, which "save as" wants.
```

## L876-878 · `fn render_footer(p: &Pick) -> Widget {`

```
/// The two buttons, right-aligned. No key-hint strip: the arrows, Enter
/// and Esc do what they do everywhere, and spelling that out under every
/// dialog is noise.
```

## L881-882 · `let ready = match p.mode {`

```
// A button that looks live but does nothing is worse than one that
// says it can't act yet: save needs a name, open needs a file picked.
```

## L905 · `fn can_commit_name(name: &str) -> bool {`

```
/// A name is committable when it's non-empty and names a file, not a path.
```

## L911-913 · `fn render_new_folder(p: &Pick) -> Widget {`

```
/// Name-the-new-folder sheet. The list and the save field are hidden
/// while this is up, so there is still exactly one editable widget and
/// `InputChange` stays unambiguous.
```

## L919-922 · `prefab::input(&p.folder_name, s().folder_hint, prefab::InputKind::Text,`

```
// Plain `input`, not the autofocus variant: the compositor
// only auto-focuses on a window's FIRST commit, and this
// sheet appears later. Claiming autofocus here would just
// be a lie in the tree — hence the hint below.
```

## L1002 · `enum Outcome { Idle, Rerender, Done }`

```
// ── Events ────────────────────────────────────────────────────────────
```

## L1009 · `if p.confirm_overwrite { p.confirm_overwrite = false; return Outcome::Rerender; }`

```
// Back out of a sheet first; only a bare Esc cancels the dialog.
```

## L1020 · `match p.selected_entry() {`

```
// A folder is always "descend"; anything else confirms.
```

## L1031-1033 · `let buf = if p.new_folder { &mut p.folder_name } else { &mut p.name };`

```
// Exactly one editable widget is on screen at a time: the
// folder sheet hides the save field, so there is no ambiguity
// about which buffer this belongs to.
```

## L1062 · `let path = p.join(&p.full_name());`

```
// Already asked — commit straight through.
```

## L1083-1085 · `if p.selected == Some(i) {`

```
// First click selects; clicking the selected row again
// activates it (descend / choose) — no double-click
// event exists, and this keeps one click reversible.
```

## L1099 · `fn commit_tree(p: &Pick) {`

```
// ── Main loop ─────────────────────────────────────────────────────────
```

## L1103-1104 · `match wire::encode(&tree) {`

```
// Always through `wire::encode` — a bare postcard payload is rejected
// by the compositor and the window stays blank.
```

## L1113-1114 · `unsafe { let _ = npk_window_set_modal(1); }`

```
// The kernel already made this window a centred overlay; modal keeps
// stray keystrokes out of the app behind us while a dialog is up.
```

## L1135 · `Outcome::Done => return,`

```
// `answer` already reported and closed the window.
```

## L1145 · `#[allow(dead_code)]`

```
// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
```

