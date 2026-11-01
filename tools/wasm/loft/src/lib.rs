//! loft — file browser.
//!
//! Layout (top → bottom):
//!   menu_bar      — File / Edit / View / Go / Help
//!   toolbar       — back / forward / up / refresh / select mode + breadcrumb + search
//!   body          — sidebar │ grid or list (with empty-state)
//!   status bar    — item count, selection size, transient results
//!
//! The keyboard cursor (`cursor`) and the marked set (`marks`) are separate:
//! file operations act on the marks when there are any, else on the cursor
//! item. Hover is drawn by the compositor (`Modifier::Hover`) and changes no
//! state. A single click selects, a second click on the same item within
//! `DOUBLE_CLICK_MS` (or Enter) opens it.
//!
//! Esc closes a dialog or menu, then clears the marks, then the search, then
//! closes the window.

#![no_std]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_meta::IconRef;
use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Declared capabilities: read + write (copy/move/rename/delete files) +
// exec (npk_open launches the handler app) + render + canvas (thumbnails).
// Without this section loft would get the default READ|EXEC|RENDER and
// could not mutate the FS.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::WRITE | caps::EXEC | caps::RENDER | caps::CANVAS];

use nopeek_widgets::host;

// The calls without a shared wrapper; the rest are in `npk_sys` and
// `nopeek_widgets::host`.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_open(app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32;
    fn npk_fs_usage() -> i64;
    fn npk_window_set_clipboard_sink() -> i32;
    fn npk_clipboard_set(ptr: i32, len: i32) -> i32;
}

/// Copy `old` to `new` (directories as a whole). True on success.
fn fs_copy(old: &str, new: &str) -> bool {
    npk_sys::fs_copy(old.as_bytes(), new.as_bytes()) == 0
}

/// Move `old` to `new` (directories as a whole). True on success.
fn fs_rename(old: &str, new: &str) -> bool {
    npk_sys::fs_rename(old.as_bytes(), new.as_bytes()) == 0
}

/// Remove a file or an empty directory. True on success.
fn fs_delete(path: &str) -> bool {
    npk_sys::fs_delete(path.as_bytes()) == 0
}

/// Create directory `path`; the parent must exist. True on success.
fn fs_mkdir(path: &str) -> bool {
    npk_sys::fs_mkdir(path.as_bytes()) == 0
}

/// Open `arg` with module `app`.
fn open(app: &str, arg: &str) {
    // SAFETY: FFI; the kernel validates both ranges.
    unsafe { npk_open(app.as_ptr() as i32, app.len() as i32, arg.as_ptr() as i32, arg.len() as i32) };
}

/// Packed `(used << 32) | total`, negative without a mounted filesystem.
fn fs_usage() -> i64 {
    // SAFETY: FFI without pointers.
    unsafe { npk_fs_usage() }
}

fn window_set_clipboard_sink() {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_clipboard_sink() };
}

/// Put `text` on the system clipboard. False if refused (window not focused).
fn clipboard_set(text: &str) -> bool {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_clipboard_set(text.as_ptr() as i32, text.len() as i32) >= 0 }
}

// ── Strings ───────────────────────────────────────────────────────────
// English only for now; a translation is one more `const` like `EN`,
// picked in `s()` via `nopeek_widgets::i18n`.

struct Strings {
    menu_file:      &'static str,
    menu_edit:      &'static str,
    menu_view:      &'static str,
    menu_go:        &'static str,
    menu_help:      &'static str,
    quit:           &'static str,
    view_grid:      &'static str,
    view_list:      &'static str,
    go_home:        &'static str,
    go_filesystem:  &'static str,
    about:          &'static str,
    open:           &'static str,
    copy:           &'static str,
    cut:            &'static str,
    paste:          &'static str,
    rename:         &'static str,
    select_all:     &'static str,
    new_folder:     &'static str,
    refresh:        &'static str,
    move_to_trash:  &'static str,
    delete_perm:    &'static str,
    restore:        &'static str,
    empty_trash:    &'static str,
    copy_path:      &'static str,
    properties:     &'static str,
    no_action:      &'static str,
    rename_title:   &'static str,
    rename_label:   &'static str,
    cancel:         &'static str,
    confirm_rename: &'static str,
    folder_title:   &'static str,
    folder_label:   &'static str,
    folder_default: &'static str,
    create:         &'static str,
    delete_title:   &'static str,
    /// `{}` = the item's name.
    delete_one:     &'static str,
    /// `{}` = the number of items.
    delete_many:    &'static str,
    delete_hint:    &'static str,
    delete:         &'static str,
    props_title:    &'static str,
    props_name:     &'static str,
    props_type:     &'static str,
    props_size:     &'static str,
    props_files:    &'static str,
    props_modified: &'static str,
    props_location: &'static str,
    props_folder:   &'static str,
    close:          &'static str,
    search:         &'static str,
    empty_dir:      &'static str,
    no_matches:     &'static str,
    col_name:       &'static str,
    col_size:       &'static str,
    col_files:      &'static str,
    col_type:       &'static str,
    col_modified:   &'static str,
    // Status bar. `{}` placeholders are filled in order by `fill`.
    items_one:      &'static str,
    items_many:     &'static str,
    selected:       &'static str,
    trashed:        &'static str,
    trash_failed:   &'static str,
    deleted:        &'static str,
    delete_failed:  &'static str,
    restored:       &'static str,
    restored_home:  &'static str,
    restore_failed: &'static str,
    restore_nested: &'static str,
    pasted:         &'static str,
    paste_failed:   &'static str,
    copied:         &'static str,
    cut_done:       &'static str,
    path_copied:    &'static str,
    path_failed:    &'static str,
    folder_failed:  &'static str,
    name_exists:    &'static str,
    name_invalid:   &'static str,
    rename_failed:  &'static str,
    // Sidebar.
    place_home:      &'static str,
    place_documents: &'static str,
    place_downloads: &'static str,
    place_music:     &'static str,
    place_pictures:  &'static str,
    place_videos:    &'static str,
    place_projects:  &'static str,
    place_trash:     &'static str,
    sec_places:      &'static str,
    sec_devices:     &'static str,
    // View menu.
    sort_name:      &'static str,
    sort_size:      &'static str,
    sort_files:     &'static str,
    sort_type:      &'static str,
    sort_modified:  &'static str,
    sort_asc:       &'static str,
    sort_desc:      &'static str,
    show_hidden:    &'static str,
    zoom_in:        &'static str,
    zoom_out:       &'static str,
    // Location bar.
    go_location:    &'static str,
    location_hint:  &'static str,
    /// `{}` = the typed path.
    location_missing: &'static str,
    // Open with.
    open_with:      &'static str,
    ow_title:       &'static str,
    /// `{}` = the extension without the dot.
    ow_always:      &'static str,
    ow_none:        &'static str,
    assoc_failed:   &'static str,
    // New file, duplicate.
    new_file:       &'static str,
    file_title:     &'static str,
    file_label:     &'static str,
    file_default:   &'static str,
    file_failed:    &'static str,
    duplicate:      &'static str,
    duplicated:     &'static str,
    duplicate_failed: &'static str,
    // Undo.
    undo:           &'static str,
    undone:         &'static str,
    undo_failed:    &'static str,
    nothing_to_undo: &'static str,
    // Name conflict on paste.
    cf_title:       &'static str,
    /// `{}` = the item's name.
    cf_text:        &'static str,
    cf_replace:     &'static str,
    cf_keep:        &'static str,
    cf_skip:        &'static str,
    cf_all:         &'static str,
    cf_hint:        &'static str,
    // Background delete.
    /// `{}` = items still to go.
    deleting:       &'static str,
    /// `{}` = items deleted before the stop.
    delete_stopped: &'static str,
    busy:           &'static str,
    // Tabs and bookmarks.
    new_tab:        &'static str,
    open_tab:       &'static str,
    close_tab:      &'static str,
    sec_bookmarks:  &'static str,
    bm_here:        &'static str,
    bm_item:        &'static str,
    bm_remove:      &'static str,
    bm_up:          &'static str,
    /// `{}` = the folder's name.
    bm_added:       &'static str,
    preview_pane:   &'static str,
    preview_none:   &'static str,
    dragging:       &'static str,
    // Renaming several items.
    /// `{}` = the number of items.
    br_title:       &'static str,
    br_find:        &'static str,
    br_replace:     &'static str,
    /// `{}` = how many more are not shown.
    br_more:        &'static str,
    br_clash:       &'static str,
    renamed:        &'static str,
}

const EN: Strings = Strings {
    menu_file: "File", menu_edit: "Edit", menu_view: "View",
    menu_go: "Go", menu_help: "Help",
    quit: "Quit",
    view_grid: "Grid", view_list: "List",
    go_home: "Home", go_filesystem: "Filesystem",
    about: "About loft",
    open: "Open",
    copy: "Copy", cut: "Cut", paste: "Paste", rename: "Rename…",
    select_all: "Select all", new_folder: "New folder…", refresh: "Refresh",
    move_to_trash: "Move to trash", delete_perm: "Delete permanently",
    restore: "Restore", empty_trash: "Empty trash",
    copy_path: "Copy path", properties: "Properties",
    no_action: "(no action)",
    rename_title: "Rename", rename_label: "New name:",
    cancel: "Cancel", confirm_rename: "Rename",
    folder_title: "New folder", folder_label: "Folder name:",
    folder_default: "New folder", create: "Create",
    delete_title: "Delete permanently",
    delete_one: "Delete “{}” permanently?",
    delete_many: "Delete {} items permanently?",
    delete_hint: "This cannot be undone.",
    delete: "Delete",
    props_title: "Properties",
    props_name: "Name", props_type: "Type", props_size: "Size",
    props_files: "Files", props_modified: "Modified", props_location: "Location",
    props_folder: "folder",
    close: "Close",
    search: "search",
    empty_dir: "Empty directory", no_matches: "No matches",
    col_name: "NAME", col_size: "SIZE", col_files: "FILES",
    col_type: "TYPE", col_modified: "MODIFIED",
    items_one: "{} item", items_many: "{} items",
    selected: "{} of {} selected, {}",
    trashed: "Moved to trash: {}",
    trash_failed: "Could not move to trash: {}",
    deleted: "Deleted: {}",
    delete_failed: "Could not delete: {}",
    restored: "Restored: {}",
    restored_home: "Restored to home, the original folder is gone: {}",
    restore_failed: "Could not restore: {}",
    restore_nested: "Only top-level trash items can be restored",
    pasted: "Pasted: {}",
    paste_failed: "Could not paste: {}",
    copied: "Copied: {}",
    cut_done: "Cut: {}",
    path_copied: "Path copied",
    path_failed: "Could not copy the path",
    folder_failed: "Could not create folder",
    name_exists: "An item with that name already exists",
    name_invalid: "Invalid name",
    rename_failed: "Could not rename",
    place_home: "Home", place_documents: "Documents", place_downloads: "Downloads",
    place_music: "Music", place_pictures: "Pictures", place_videos: "Videos", place_projects: "Projects", place_trash: "Trash",
    sec_places: "PLACES", sec_devices: "DEVICES",
    sort_name: "Sort by name", sort_size: "Sort by size", sort_files: "Sort by file count",
    sort_type: "Sort by type", sort_modified: "Sort by date",
    sort_asc: "Ascending", sort_desc: "Descending",
    show_hidden: "Show hidden files",
    zoom_in: "Bigger icons", zoom_out: "Smaller icons",
    go_location: "Go to location…",
    location_hint: "path, e.g. home/documents",
    location_missing: "Not found: {}",
    open_with: "Open with…", ow_title: "Open with",
    ow_always: "Always use for .{} files",
    ow_none: "No app is installed that can open files",
   
    assoc_failed: "Could not save the association",
    new_file: "New file…", file_title: "New file", file_label: "File name:",
    file_default: "Untitled.txt", file_failed: "Could not create file",
    duplicate: "Duplicate", duplicated: "Duplicated: {}", duplicate_failed: "Could not duplicate: {}",
    undo: "Undo", undone: "Undone", undo_failed: "Could not undo, the items changed since",
    nothing_to_undo: "Nothing to undo",
    cf_title: "Item exists",
    cf_text: "“{}” already exists here.",
    cf_replace: "Replace", cf_keep: "Keep both", cf_skip: "Skip",
    cf_all: "Same for all",
    cf_hint: "Replace moves the existing item to the trash.",
    deleting: "Deleting… {} left · Esc stops",
    delete_stopped: "Stopped after {} deleted",
    busy: "Still deleting, wait or press Esc",
    new_tab: "New tab", open_tab: "Open in new tab", close_tab: "Close tab",
    sec_bookmarks: "BOOKMARKS",
    bm_here: "Bookmark this folder", bm_item: "Add bookmark",
    bm_remove: "Remove bookmark", bm_up: "Move up",
    bm_added: "Bookmarked: {}",
    preview_pane: "Preview pane", preview_none: "Nothing selected", dragging: "Dragging {} · drop on a folder, Ctrl copies, Esc cancels",
    br_title: "Rename {} items", br_find: "Find:", br_replace: "Replace with:",
   
    br_more: "… and {} more",
    br_clash: "Two items would get the same name, or a name is taken or invalid",
    renamed: "Renamed: {}",
};


fn s() -> &'static Strings { &EN }

/// Replace each `{}` in `template` with the next of `args`.
fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    let mut args = args.iter();
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        out.push_str(args.next().copied().unwrap_or(""));
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

fn num(n: usize) -> String {
    let mut s = String::with_capacity(8);
    push_usize(&mut s, n);
    s
}

fn log(msg: &str) { host::log_serial(msg); }

const EVENT_BUF_SIZE: usize = 256;

enum PollResult { Event(Event), Empty, WindowGone }

fn poll_event(buf: &mut [u8]) -> PollResult {
    match nopeek_widgets::events::poll(buf) {
        nopeek_widgets::events::Poll::Event(ev) => PollResult::Event(ev),
        nopeek_widgets::events::Poll::Empty => PollResult::Empty,
        nopeek_widgets::events::Poll::Gone => PollResult::WindowGone,
    }
}

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { log("[loft] panic!"); core::arch::wasm32::unreachable() }

// ── Action-id encoding ────────────────────────────────────────────────
//
// Each interaction surface gets its own base so the dispatcher can tell
// "which thing was clicked" by an integer comparison alone — no string
// keys, no payload.

// Idle iterations (~16 ms each) between auto-refresh dir re-scans.
// ~1.4 s — frequent enough that a new screenshot shows up promptly,
// rare enough to be negligible load.
const AUTO_REFRESH_TICKS: u32 = 90;

// Folders scanned per idle tick (~16 ms) for recursive size/count. Small
// enough that each tick stays snappy, enough that a typical directory
// fills in within a few hundred ms.
const STATS_PUMP_BUDGET: usize = 3;

/// A second click on the same item within this window opens it.
const DOUBLE_CLICK_MS: u64 = 400;
/// How long a status-bar message stays up.
const STATUS_MS: u64 = 4_000;
/// PageUp/PageDown step in rows; the app is not told its visible height.
const PAGE_ROWS: isize = 10;
/// Trash folder under home, and its index of original locations.
const TRASH_DIR: &str = ".trash";
const TRASH_INDEX: &str = ".index";
/// Longest side of a thumbnail, the largest grid icon.
const THUMB_PX: u32 = 128;
/// The preview pane: its canvas (thumbnails count from 1), the decoded
/// picture size (the canvas scales it to the pane), its default and
/// narrowest width, and how much of a text file it reads and shows.
const PREVIEW_CANVAS: u32 = 0;
const PREVIEW_PX: u32 = 768;
const PREVIEW_W: u16 = 280;
const PREVIEW_W_MIN: u16 = 160;
const PREVIEW_BYTES: usize = 64 * 1024;
const PREVIEW_LINES: usize = 1000;
/// Width of the bar between file area and preview pane that resizes it.
const GRIP_W: u16 = 5;

/// Upper bound on list-and-delete passes for one directory tree; each pass
/// removes at least what one `npk_fs_list` buffer holds.
const DELETE_ROUNDS: usize = 64;
/// `npk_fs_delete` calls per idle tick (~16 ms) of a background delete.
const DELETE_BUDGET: usize = 48;
/// loft's own settings, and the extension → app table it shares.
const CONFIG_PATH: &str = "sys/config/loft";
const ASSOC_PATH: &str = "sys/config/associations";
/// The file clipboard, shared by every loft window. In loft's private
/// area, which no other module can write, so no app can plant paths
/// for a paste to copy or move.
const CLIP_PATH: &str = "priv/loft/clip";
/// Grid zoom steps as (columns, icon size in px), smallest icons first.
const ZOOM_STEPS: [(usize, u16); 5] = [(8, 32), (6, 48), (4, 64), (3, 96), (2, 128)];
const ZOOM_DEFAULT: usize = 2;
/// Modules that are panels or system parts, never an "Open with" target.
const NOT_HANDLERS: &[&str] = &["loft", "dock", "bar", "drun", "deck", "pick", "volume", "link"];

// Grid/list items: one id per visible row, so the band is wide enough for
// any directory without running into the next surface.
const ACT_GRID_CLICK_BASE:    u32 = 1_000_000;
const ACT_GRID_CLICK_END:     u32 = 2_000_000;
// Sidebar rows and tabs grow with the user's bookmarks and tabs, so they
// get bands as wide as the item band.
const ACT_SIDEBAR_CLICK_BASE: u32 = 2_000_000;
const ACT_SIDEBAR_CLICK_END:  u32 = 3_000_000;
// Drop targets: sidebar rows, then folder items (one per visible row).
const ACT_DROP_SIDEBAR_BASE:  u32 = 3_000_000;
const ACT_DROP_SIDEBAR_END:   u32 = 4_000_000;
const ACT_TAB_BASE:           u32 = 4_000_000;
const ACT_TAB_CLOSE_BASE:     u32 = 5_000_000;
const ACT_TAB_CLOSE_END:      u32 = 6_000_000;
const ACT_DROP_ITEM_BASE:     u32 = 6_000_000;
const ACT_DROP_ITEM_END:      u32 = 7_000_000;
const ACT_BREADCRUMB_BASE:    u32 = 3_000;
const ACT_TOOLBAR_BACK:       u32 = 4_000;
const ACT_TOOLBAR_FORWARD:    u32 = 4_001;
const ACT_TOOLBAR_UP:         u32 = 4_002;
const ACT_TOOLBAR_REFRESH:    u32 = 4_003;
const ACT_TOOLBAR_SELECT:     u32 = 4_004;
// Menu-bar label clicks toggle the corresponding dropdown.
const ACT_MENU_FILE:          u32 = 5_000;
const ACT_MENU_EDIT:          u32 = 5_001;
const ACT_MENU_VIEW:          u32 = 5_002;
const ACT_MENU_GO:            u32 = 5_003;
const ACT_MENU_HELP:          u32 = 5_004;
// Click-outside-popover dismiss action.
const ACT_MENU_DISMISS:       u32 = 5_500;
// Dropdown items.
const ACT_FILE_QUIT:          u32 = 6_000;
const ACT_VIEW_GRID:          u32 = 6_100;
const ACT_VIEW_LIST:          u32 = 6_101;
const ACT_SORT_NAME:          u32 = 6_102;
const ACT_SORT_SIZE:          u32 = 6_103;
const ACT_SORT_FILES:         u32 = 6_104;
const ACT_SORT_TYPE:          u32 = 6_105;
const ACT_SORT_MTIME:         u32 = 6_106;
const ACT_SORT_ASC:           u32 = 6_107;
const ACT_SORT_DESC:          u32 = 6_108;
const ACT_VIEW_HIDDEN:        u32 = 6_109;
const ACT_ZOOM_IN:            u32 = 6_110;
const ACT_ZOOM_OUT:           u32 = 6_111;
const ACT_VIEW_PREVIEW:       u32 = 6_112;
const ACT_PREVIEW_GRIP:       u32 = 6_113;
/// The empty part of the file area: a drop into the current folder.
const ACT_DROP_HERE:          u32 = 6_114;
/// Physical px the pointer travels with the button held before a press on
/// an item becomes a drag.
const DRAG_SLOP: i32 = 6;
const ACT_GO_HOME:            u32 = 6_200;
const ACT_GO_FILESYSTEM:      u32 = 6_201;
const ACT_GO_LOCATION:        u32 = 6_202;
const ACT_LOCATION_SUBMIT:    u32 = 6_203;
const ACT_HELP_ABOUT:         u32 = 6_300;
// List-view column headers — click to sort / toggle direction.
const ACT_HEADER_NAME:        u32 = 7_000;
const ACT_HEADER_SIZE:        u32 = 7_001;
const ACT_HEADER_FILES:       u32 = 7_002;
const ACT_HEADER_TYPE:        u32 = 7_003;
const ACT_HEADER_MTIME:       u32 = 7_004;

// File operations — shared by the menus and the context menu; they act on
// `Loft::targets`.
const ACT_EDIT_COPY:          u32 = 8_000;
const ACT_EDIT_CUT:           u32 = 8_001;
const ACT_EDIT_PASTE:         u32 = 8_002;
const ACT_EDIT_RENAME:        u32 = 8_003;
const ACT_EDIT_TRASH:         u32 = 8_004;
const ACT_EDIT_DELETE:        u32 = 8_005;
const ACT_EDIT_RESTORE:       u32 = 8_006;
const ACT_EDIT_EMPTY_TRASH:   u32 = 8_007;
const ACT_EDIT_NEW_FOLDER:    u32 = 8_008;
const ACT_EDIT_SELECT_ALL:    u32 = 8_009;
const ACT_EDIT_COPY_PATH:     u32 = 8_010;
const ACT_EDIT_PROPERTIES:    u32 = 8_011;
const ACT_EDIT_OPEN:          u32 = 8_012;
const ACT_EDIT_REFRESH:       u32 = 8_013;
const ACT_EDIT_OPEN_WITH:     u32 = 8_014;
const ACT_EDIT_NEW_FILE:      u32 = 8_015;
const ACT_EDIT_DUPLICATE:     u32 = 8_016;
const ACT_EDIT_UNDO:          u32 = 8_017;
const ACT_EDIT_OPEN_TAB:      u32 = 8_018;
const ACT_EDIT_BOOKMARK:      u32 = 8_019;
// Dialog buttons (name, confirm, properties).
const ACT_DIALOG_SUBMIT:      u32 = 8_100;
const ACT_DIALOG_CANCEL:      u32 = 8_101;
const ACT_OW_ALWAYS:          u32 = 8_102;
const ACT_CF_REPLACE:         u32 = 8_103;
const ACT_CF_KEEP:            u32 = 8_104;
const ACT_CF_SKIP:            u32 = 8_105;
const ACT_CF_ALL:             u32 = 8_106;
// Click-outside dismiss for the right-click context menu.
const ACT_CTX_DISMISS:        u32 = 8_200;
// The file area behind the items: right-click opens the background menu.
const ACT_BACKGROUND:         u32 = 8_300;
// "Open with" app rows, one id per app.
const ACT_OW_BASE:            u32 = 8_400;
const ACT_OW_END:             u32 = 9_000;
// The tab strip's + button.
const ACT_TAB_NEW:            u32 = 9_800;
// Sidebar context menu.
const ACT_SB_OPEN_TAB:        u32 = 9_900;
const ACT_SB_REMOVE:          u32 = 9_901;
const ACT_SB_UP:              u32 = 9_902;
const ACT_GO_BOOKMARK:        u32 = 6_204;
const TAB_W: u16 = 180;
const TAB_H: u16 = 28;

// NodeIds for menu-bar labels — used as Popover anchors.
const NODE_MENU_FILE: u32 = 100;
const NODE_MENU_EDIT: u32 = 101;
const NODE_MENU_VIEW: u32 = 102;
const NODE_MENU_GO:   u32 = 103;
const NODE_MENU_HELP: u32 = 104;
// Text fields, so `InputChangeAt` says which one changed.
const NODE_SEARCH:    u32 = 110;
const NODE_LOCATION:  u32 = 111;
const NODE_NAME:      u32 = 112;
const NODE_FIND:      u32 = 113;
const NODE_REPL:      u32 = 114;
/// Rows of new names shown in the batch-rename preview.
const PREVIEW_ROWS: usize = 8;

const QUERY_CAP: usize = 127;

// ── State ─────────────────────────────────────────────────────────────

struct Place {
    label:  String,
    icon:   IconId,
    path:   String,
    section: Section,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section { Places, Bookmarks, Devices }

/// Where one tab is. The active tab lives in `Loft`'s own fields; this
/// holds the others.
struct Tab {
    current:  String,
    history:  Vec<String>,
    forward:  Vec<String>,
    query:    String,
    cursor:   Option<String>,
    marks:    BTreeSet<String>,
}

struct Entry {
    name:    String,
    /// ASCII-lowercased mirror of `name`, computed once at parse
    /// time so refilter() doesn't allocate a fresh lowercase string
    /// on every keystroke; matters for typing latency in large
    /// directories.
    name_lc: String,
    /// For files: the file's own byte size. For folders: the recursive
    /// sum of every descendant file's size, filled in by
    /// `pump_stats` (0 until then).
    size:    u64,
    is_dir:  bool,
    /// Number of descendant files inside a folder (recursive). 0 for
    /// files. Drives the "Files" column. Filled progressively off the
    /// idle loop (see `pump_stats`).
    files:   u64,
    /// Folder whose recursive size/count hasn't been computed yet.
    /// True from refresh until `pump_stats` reaches it; always false for
    /// files. Rendered as "…" so the directory paints instantly and the
    /// numbers fill in without ever blocking the event loop.
    stats_pending: bool,
    /// UTC seconds since the Unix epoch, captured at write time by
    /// the kernel. Zero = unknown (RTC was unreadable when the entry
    /// was created). Filled in from the `npk_fs_list` record tail.
    mtime:   u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Grid,
    List,
}

/// Which column the list view sorts by. Folders are always grouped
/// before files (Thunar/Files "folders first" idiom); the key only
/// orders entries *within* each group.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Name,
    Size,
    Files,
    Type,
    Modified,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenMenu {
    File,
    Edit,
    View,
    Go,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipMode {
    /// Ctrl+C — paste keeps the source (content-addressed alias, cheap).
    Copy,
    /// Ctrl+X — paste moves the source (rename), then the clipboard clears.
    Cut,
}

/// A pending copy/cut, set by Ctrl+C/X and consumed by Ctrl+V (paste).
struct Clip {
    /// Full source paths.
    items: Vec<String>,
    mode:  ClipMode,
}

/// What the right-click menu was opened on. Fixed at the click, so neither
/// hover nor a cursor move can re-target or move it.
#[derive(Clone, PartialEq, Eq)]
enum CtxMenu {
    /// An item, by its name in the active source list.
    Item(String),
    /// The empty part of the file area.
    Background,
    /// A sidebar row, by its index in `Loft::sidebar`.
    Sidebar(usize),
}

/// One item a file operation acts on.
struct Target {
    /// Name in the active source list (in search mode a relative sub-path).
    name:   String,
    full:   String,
    is_dir: bool,
}

enum NameKind {
    /// `old` is the source's full path, captured when the dialog opened.
    Rename { old: String },
    NewFolder,
    NewFile,
}

/// One reversible step of a file operation.
enum Step {
    /// `from` was moved (or renamed) to `to`.
    Move { from: String, to: String },
    /// `path` was created (new file or folder, copy, duplicate).
    Create { path: String, is_dir: bool },
}

/// How a name clash on paste is resolved.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clash { Replace, Keep, Skip }

/// A paste in progress; it pauses on a name clash until the user answers.
struct PasteJob {
    mode:      ClipMode,
    /// The folder the items go into.
    dest:      String,
    /// Started by a drop, not Ctrl+V: the clipboard stays as it is.
    dropped:   bool,
    /// All source paths, for the clipboard afterwards.
    all:       Vec<String>,
    /// Sources still to go, next one last.
    todo:      Vec<String>,
    /// The answer to reuse once "same for all" is ticked.
    policy:    Option<Clash>,
    apply_all: bool,
    /// The source the clash dialog asked about; an answer applies to it only.
    asked:     Option<String>,
    steps:     Vec<Step>,
    ok:        usize,
    failed:    usize,
    last:      Option<String>,
}

/// A thumbnail of one picture, kept as a canvas in the kernel.
struct Thumb {
    /// Canvas id, or `None` when the file could not be decoded.
    canvas: Option<u32>,
    /// Size and mtime it was made from; a change makes it stale.
    stamp:  (u64, u64),
}

/// A picture being shrunk off the idle loop, a slice of the file per tick.
struct PngJob {
    path:  String,
    stamp: (u64, u64),
    file:  host::File,
    off:   u64,
    dec:   nopeek_widgets::png::Shrink,
}

/// Source pixels a picture job works through per idle tick, and the file
/// piece it reads at a time.
const PNG_TICK_PIXELS: usize = 1 << 20;
const PNG_PIECE: usize = 64 * 1024;
/// File pieces a job reads per tick, so a file of skipped chunks or empty
/// deflate blocks cannot hold one tick for the whole file either.
const PNG_TICK_PIECES: usize = 16;

impl PngJob {
    fn open(path: String, stamp: (u64, u64), max: u32) -> Option<PngJob> {
        let file = host::File::open(&path)?;
        Some(PngJob { path, stamp, file, off: 0, dec: nopeek_widgets::png::Shrink::new(max) })
    }

    /// One tick of work: `None` while pending, then the picture, or
    /// `Some(None)` if the file is no PNG loft reads.
    fn step(&mut self, buf: &mut [u8]) -> Option<Option<nopeek_widgets::png::Image>> {
        let mut budget = PNG_TICK_PIXELS;
        for _ in 0..PNG_TICK_PIECES {
            if self.dec.done() { return Some(self.dec.take()); }
            if budget == 0 { return None; }
            let n = match self.file.read_at(self.off, buf) {
                Some(n) if n > 0 => n,
                _ => return Some(None),
            };
            let before = budget;
            let Ok(used) = self.dec.feed(&buf[..n], &mut budget) else { return Some(None) };
            self.off += used as u64;
            if used == 0 && budget == before && !self.dec.done() { return Some(None); }
        }
        if self.dec.done() { Some(self.dec.take()) } else { None }
    }
}

/// What the preview pane shows for one item.
struct Preview {
    path:  String,
    stamp: (u64, u64),
    /// The picture is on `PREVIEW_CANVAS`.
    image: bool,
    /// The first lines, for a file that reads as text.
    text:  Option<Vec<String>>,
}

/// One path of a background delete.
struct DelItem {
    path:   String,
    is_dir: bool,
    /// A selected item (counted in the report), not something inside one.
    top:    bool,
    /// How often this folder was expanded, and how many entries the last
    /// listing had: a folder that does not shrink between two tries holds
    /// something that cannot go, and is given up.
    tries:  usize,
    last:   usize,
}

/// A permanent delete running off the idle loop.
struct DeleteJob {
    /// Paths still to delete, next one last.
    todo:   Vec<DelItem>,
    ok:     usize,
    failed: usize,
}

struct Props {
    name:     String,
    kind:     String,
    size:     String,
    files:    Option<String>,
    modified: String,
    location: String,
}

/// The modal dialog that replaces the file area while it is up.
enum Dialog {
    /// Name entry; the edited text is `Loft::name_buf`.
    Name(NameKind),
    ConfirmDelete(Vec<Target>),
    Properties(Props),
    /// Pick an app for `target`; `apps` holds (module, display name).
    OpenWith { target: Target, apps: Vec<(String, String)>, always: bool },
    /// A paste hit an existing name; the job waits in `Loft::paste`.
    Clash { name: String },
    /// Find and replace in the names of `targets` (`Loft::find_buf`,
    /// `Loft::repl_buf`).
    BatchRename(Vec<Target>),
}

struct Status {
    text:  String,
    error: bool,
    /// `host::ticks_ms` at which the message goes away.
    until: u64,
}

/// One line of the trash index: `<trash name>\t<original path>\t<unix time>`.
struct TrashRecord {
    name:   String,
    origin: String,
    time:   u64,
}

struct Loft {
    home:           String,
    current:        String,
    history:        Vec<String>,
    forward:        Vec<String>,
    sidebar:        Vec<Place>,
    /// Direct children of `current`. Used when the search query is
    /// empty (browse mode).
    entries:        Vec<Entry>,
    /// Recursive listing of `current` (every descendant). Loaded
    /// lazily on first non-empty query, cached until we navigate to
    /// a different directory.
    recursive:      Vec<Entry>,
    /// `Some(path)` when `recursive` has been filled for that path;
    /// `None` after a refresh invalidates the cache.
    recursive_dir:  Option<String>,
    /// Indices into the active source list (entries / recursive)
    /// matching the current search query. Equal to 0..source.len()
    /// when the query is empty.
    filtered:       Vec<usize>,
    /// Keyboard cursor, an index into `filtered`.
    cursor:         Option<usize>,
    /// Marked items by name in the active source list. Names rather than
    /// indices so a refilter or refresh keeps the marks on the same items.
    marks:          BTreeSet<String>,
    /// In select mode a click toggles a mark instead of selecting.
    select_mode:    bool,
    /// Last left click: item index and `host::ticks_ms`, for double click.
    last_click:     Option<(usize, u64)>,
    sidebar_sel:    Option<usize>,
    /// The search query, at most `QUERY_CAP` bytes.
    query:          String,
    /// Pre-allocated mirror used to compute `query.to_ascii_lowercase()`
    /// without an extra allocation per keystroke.
    query_lc:       String,
    view_mode:      ViewMode,
    /// Which menu's dropdown is currently visible. None = no menu open.
    open_menu:      Option<OpenMenu>,
    /// File associations (extension → app module), loaded once from
    /// `sys/config/associations`. Checked before the built-in defaults.
    assoc:          Vec<(String, String)>,
    /// Cheap signature (count + name/size fold) of the current dir's
    /// listing. The idle loop re-lists periodically and auto-refreshes
    /// when this changes.
    dir_sig:        u64,
    /// Active list-view sort column + direction.
    sort_key:       SortKey,
    sort_asc:       bool,
    /// Indices into `entries` of folders still awaiting a recursive
    /// size/count scan, drained a few at a time off the idle loop.
    stats_queue:    Vec<usize>,
    /// Reusable path buffer for `pump_stats`.
    scratch:        String,
    /// Pending copy/cut awaiting a paste. None = clipboard empty.
    clipboard:      Option<Clip>,
    ctx:            Option<CtxMenu>,
    dialog:         Option<Dialog>,
    /// The name dialog's text, at most its pre-allocated capacity.
    name_buf:       String,
    status:         Option<Status>,
    show_hidden:    bool,
    /// Index into `ZOOM_STEPS`.
    zoom:           usize,
    /// The typed location while the location bar replaces the breadcrumb.
    location:       Option<String>,
    /// Raised to move the keyboard into the location bar.
    focus_seq:      u32,
    /// Finished operations, most recent last. Each entry is one user action.
    undo:           Vec<Vec<Step>>,
    paste:          Option<PasteJob>,
    deleting:       Option<DeleteJob>,
    /// Saved state of every tab; the entry at `active` is stale while the
    /// tab is shown.
    tabs:           Vec<Tab>,
    active:         usize,
    /// Bookmarked folders, in sidebar order.
    bookmarks:      Vec<String>,
    find_buf:       String,
    repl_buf:       String,
    /// (shift, ctrl) from the `Event::Mods` just before the current event.
    mods:           (bool, bool),
    /// Item a Shift range starts from.
    anchor:         Option<usize>,
    /// `Event::Open`s that arrived while a dialog was up.
    pending_open:   Vec<String>,
    /// Thumbnails by full path, for the pictures the grid shows now.
    thumbs:         BTreeMap<String, Thumb>,
    /// Pictures still to decode, next one last.
    thumb_queue:    Vec<(String, (u64, u64))>,
    /// The thumbnail being made, and the preview pane's picture.
    thumb_job:      Option<PngJob>,
    preview_job:    Option<PngJob>,
    png_buf:        Vec<u8>,
    /// Canvas ids given back, for the next thumbnail.
    free_canvas:    Vec<u32>,
    next_canvas:    u32,
    preview_on:     bool,
    preview:        Option<Preview>,
    /// Width of the preview pane in logical px.
    preview_w:      u16,
    grip:           Grip,
    /// A press on an item, its screen position, until the button comes up:
    /// moved far enough, it becomes a drag of the targets.
    press:          Option<(i32, i32)>,
    press_armed:    bool,
    /// The press landed on one of several marked items: the marks stay for
    /// a drag and go on a plain click (the release).
    press_keep:     bool,
    dragging:       bool,
    /// The folder behind each item drop target of the committed scene, by
    /// row: a drop is hit-tested against that scene, and the listing may
    /// have changed since.
    drop_dirs:      Vec<Option<String>>,
}

/// Resizing the preview pane. A press on the grip arrives as its `Action`
/// first, then as the raw button with the position the drag starts from.
#[derive(Clone, Copy, PartialEq)]
enum Grip {
    Idle,
    Armed,
    /// Pointer x (physical px), pane width and pixel scale at the press.
    Drag { x0: i32, w0: u16, scale: i32 },
}

impl Loft {
    fn new() -> Self {
        let home = read_home_dir();
        let mut lf = Loft {
            current:       home.clone(),
            home,
            history:       Vec::new(),
            forward:       Vec::new(),
            sidebar:       Vec::new(),
            entries:       Vec::new(),
            recursive:     Vec::new(),
            recursive_dir: None,
            filtered:      Vec::with_capacity(64),
            cursor:        None,
            marks:         BTreeSet::new(),
            select_mode:   false,
            last_click:    None,
            sidebar_sel:   Some(0),
            query:         String::with_capacity(QUERY_CAP + 1),
            query_lc:      String::with_capacity(QUERY_CAP + 1),
            view_mode:     ViewMode::List,
            open_menu:     None,
            assoc:         load_associations(),
            dir_sig:       0,
            sort_key:      SortKey::Name,
            sort_asc:      true,
            stats_queue:   Vec::new(),
            scratch:       String::with_capacity(512),
            clipboard:     None,
            ctx:           None,
            dialog:        None,
            name_buf:      String::with_capacity(256),
            status:        None,
            show_hidden:   false,
            zoom:          ZOOM_DEFAULT,
            location:      None,
            focus_seq:     0,
            undo:          Vec::new(),
            paste:         None,
            deleting:      None,
            tabs:          Vec::new(),
            active:        0,
            bookmarks:     Vec::new(),
            find_buf:      String::new(),
            repl_buf:      String::new(),
            mods:          (false, false),
            anchor:        None,
            pending_open:  Vec::new(),
            thumbs:        BTreeMap::new(),
            thumb_queue:   Vec::new(),
            thumb_job:     None,
            preview_job:   None,
            png_buf:       alloc::vec![0; PNG_PIECE],
            free_canvas:   Vec::new(),
            next_canvas:   1,
            preview_on:    false,
            preview:       None,
            preview_w:     PREVIEW_W,
            grip:          Grip::Idle,
            press:         None,
            press_armed:   false,
            press_keep:    false,
            dragging:      false,
            drop_dirs:     Vec::new(),
        };
        lf.load_settings();
        lf.load_clip();
        lf.rebuild_sidebar();
        if let Some(arg) = host::launch_arg().filter(|a| !a.trim().is_empty()) {
            lf.current = arg.trim().trim_matches('/').to_string();
            if !dir_exists(&lf.current) && !lf.current.is_empty() {
                lf.current = parent_path(&lf.current);
            }
        }
        lf.tabs.push(lf.snapshot());
        lf.refresh();
        lf
    }

    /// Resolve the handler app for a file name via its extension:
    /// `sys/config/associations` overrides first, then built-in defaults.
    /// Returns None for unknown types (loft does nothing on open).
    /// Like `associated_app`, but with the directory alongside.
    ///
    /// System configs have no extension, yet opening them must work: the
    /// `npk_open` path is the only one that gives the editor write access to
    /// exactly this file (the kernel grants it because the user pointed at
    /// the file). So everything under `sys/config/` is text.
    fn associated_app_in(&self, dir: &str, name: &str) -> Option<String> {
        if dir == "sys/config" || dir.starts_with("sys/config/") {
            if let Some((_, app)) = self.assoc.iter().find(|(k, _)| *k == "conf") {
                return Some(app.clone());
            }
            return Some("spell".to_string());
        }
        self.associated_app(name)
    }

    fn associated_app(&self, name: &str) -> Option<String> {
        let ext = match name.rsplit_once('.') {
            Some((_, e)) if !e.is_empty() => e.to_ascii_lowercase(),
            _ => return None,
        };
        if let Some((_, app)) = self.assoc.iter().find(|(k, _)| *k == ext) {
            return Some(app.clone());
        }
        match ext.as_str() {
            "md" | "markdown" | "txt" | "text" | "rs" | "toml" | "json"
            | "log" | "conf" | "ini" | "cfg" | "sh" | "csv" | "yaml" | "yml"
            | "xml" | "html" | "htm" | "c" | "h" | "py" | "js" | "ts"
                => Some("spell".to_string()),
            "png" => Some("iris".to_string()),
            // Only what tune can actually decode; a .flac would open a
            // player that can say nothing but "unsupported format". Video
            // is H.264 in MP4 only.
            "mp3" | "wav" | "mp4" | "m4v" | "mov" | "m4a" | "aac" => Some("tune".to_string()),
            _ => None,
        }
    }

    // ── Trash locations ───────────────────────────────────────────────

    fn trash_dir(&self) -> String { join(&self.home, TRASH_DIR) }

    fn trash_index(&self) -> String { join(&self.trash_dir(), TRASH_INDEX) }

    /// Showing the top level of the trash (where Restore applies).
    fn in_trash(&self) -> bool { self.current == self.trash_dir() }

    /// Showing the trash or a folder inside it: deleting here is permanent.
    fn in_trash_tree(&self) -> bool {
        let trash = self.trash_dir();
        self.current == trash || is_below(&self.current, &trash)
    }

    /// A listing of `path` without the trash index.
    fn list(&self, path: &str, recursive: bool) -> Vec<Entry> {
        let hidden = self.trash_index();
        let mut v = list_dir_internal(path, recursive);
        v.retain(|e| join(path, &e.name) != hidden);
        v
    }

    // ── Listing ───────────────────────────────────────────────────────

    fn refresh(&mut self) {
        self.entries = self.list(&self.current, false);
        // Signature is captured from the raw (un-annotated) listing so it
        // matches the idle probe — folder sizes are filled in below and
        // must not perturb change-detection.
        self.dir_sig = dir_signature(&self.entries);
        // Folders are scanned for size/count incrementally off the idle
        // loop (`pump_stats`), so the directory paints instantly.
        init_folder_stats(&mut self.entries);
        sort_entries(&mut self.entries, self.sort_key, self.sort_asc);
        self.stats_queue = pending_folder_indices(&self.entries);
        self.recursive.clear();
        self.recursive_dir = None;
        self.refilter();
        self.sync_sidebar_from_current();
    }

    /// Re-sort the browse listing under a (possibly new) column. Clicking
    /// the already-active column flips direction; a different column
    /// switches to it with a sensible default direction (ascending for
    /// text, descending for the numeric/date columns where "biggest /
    /// newest first" is the usual intent).
    fn set_sort(&mut self, key: SortKey) {
        if self.sort_key == key {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_key = key;
            self.sort_asc = matches!(key, SortKey::Name | SortKey::Type);
        }
        sort_entries(&mut self.entries, self.sort_key, self.sort_asc);
        self.stats_queue = pending_folder_indices(&self.entries);
        self.refilter();
        self.save_settings();
    }

    /// Compute the recursive size + file count for up to `budget` queued
    /// folders, one host scan per folder. Returns true if anything
    /// changed (→ caller re-renders).
    fn pump_stats(&mut self, budget: usize) -> bool {
        if self.stats_queue.is_empty() { return false; }
        let mut changed = false;
        for _ in 0..budget {
            let Some(idx) = self.stats_queue.pop() else { break };
            if self.entries.get(idx).map(|e| e.is_dir) != Some(true) { continue; }
            // Build "current/<name>" into the reusable scratch buffer
            // (disjoint field borrows — no per-tick allocation).
            self.scratch.clear();
            self.scratch.push_str(&self.current);
            if !self.current.is_empty() { self.scratch.push('/'); }
            self.scratch.push_str(&self.entries[idx].name);
            let (bytes, files) = scan_folder_stats(&self.scratch);
            if let Some(e) = self.entries.get_mut(idx) {
                e.size = bytes;
                e.files = files;
                e.stats_pending = false;
            }
            changed = true;
        }
        // Settle the order once every folder is in, but only when the
        // active column actually depends on the numbers we just filled.
        if self.stats_queue.is_empty()
            && matches!(self.sort_key, SortKey::Size | SortKey::Files)
        {
            sort_entries(&mut self.entries, self.sort_key, self.sort_asc);
            self.refilter();
        }
        changed
    }

    /// Pick the active source list for filtering — direct children
    /// when the query is empty (browse mode), recursive descendants
    /// otherwise (search mode). Lazy-loads the recursive listing on
    /// first non-empty query for the current directory.
    fn ensure_search_source(&mut self) -> bool {
        if self.query.is_empty() { return false; }
        if self.recursive_dir.as_deref() == Some(self.current.as_str()) {
            return true;
        }
        log("[loft] loading recursive listing");
        self.recursive = self.list(&self.current, true);
        self.recursive_dir = Some(self.current.clone());
        true
    }

    /// Recompute `filtered`; the cursor stays on the same item when it is
    /// still visible, and marks on items that no longer exist are dropped.
    fn refilter(&mut self) {
        let keep = self.cursor_name();
        let recursive_mode = self.ensure_search_source();
        self.filtered.clear();
        if self.query.is_empty() {
            for (i, e) in self.entries.iter().enumerate() {
                if self.show_hidden || !is_hidden(&e.name) { self.filtered.push(i); }
            }
        } else {
            // Reuse the buffer for the lowercased query.
            self.query_lc.clear();
            for ch in self.query.chars() {
                self.query_lc.push(ch.to_ascii_lowercase());
            }
            let source: &Vec<Entry> = if recursive_mode { &self.recursive } else { &self.entries };
            for (i, e) in source.iter().enumerate() {
                if e.name_lc.contains(self.query_lc.as_str())
                    && (self.show_hidden || !is_hidden(&e.name))
                {
                    self.filtered.push(i);
                }
            }
        }
        self.prune_marks();
        // Rows move under a new filter or order; a range starts afresh.
        self.anchor = None;
        self.cursor = keep.and_then(|n| self.index_of(&n))
            .or(if self.filtered.is_empty() { None } else { Some(0) });
        self.sync_thumbs();
    }

    // ── Thumbnails ────────────────────────────────────────────────────

    /// Keep thumbnails for exactly the pictures the grid shows: drop the
    /// others (their canvases go back to the kernel), queue the missing
    /// and the stale ones.
    fn sync_thumbs(&mut self) {
        let mut want: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        if self.view_mode == ViewMode::Grid {
            for ui in 0..self.filtered.len() {
                let Some(e) = self.entry_at(ui) else { continue };
                if e.is_dir || extension(&e.name).as_deref() != Some("png") { continue; }
                want.insert(join(&self.current, &e.name), (e.size, e.mtime));
            }
        }
        let gone: Vec<String> = self.thumbs.iter()
            .filter(|(p, t)| want.get(*p) != Some(&t.stamp))
            .map(|(p, _)| p.clone())
            .collect();
        for p in gone {
            if let Some(id) = self.thumbs.remove(&p).and_then(|t| t.canvas) {
                host::canvas_drop(id);
                self.free_canvas.push(id);
            }
        }
        if self.thumb_job.as_ref().is_some_and(|j| want.get(&j.path) != Some(&j.stamp)) {
            self.thumb_job = None;
        }
        let busy = self.thumb_job.as_ref().map(|j| j.path.clone());
        self.thumb_queue = want.into_iter()
            .filter(|(p, _)| !self.thumbs.contains_key(p) && busy.as_ref() != Some(p))
            .collect();
        // The queue pops from the end: decode in display order.
        self.thumb_queue.reverse();
    }

    /// Work on the queued pictures for one tick. True if a thumbnail
    /// appeared.
    fn pump_thumbs(&mut self) -> bool {
        if self.thumb_job.is_none() {
            while let Some((path, stamp)) = self.thumb_queue.pop() {
                match PngJob::open(path.clone(), stamp, THUMB_PX) {
                    Some(job) => { self.thumb_job = Some(job); break; }
                    None => { self.thumbs.insert(path, Thumb { canvas: None, stamp }); }
                }
            }
        }
        let Some(job) = &mut self.thumb_job else { return false };
        let Some(img) = job.step(&mut self.png_buf) else { return false };
        let Some(job) = self.thumb_job.take() else { return false };
        let canvas = img.and_then(|img| {
            let id = self.free_canvas.pop().unwrap_or_else(|| {
                self.next_canvas += 1;
                self.next_canvas - 1
            });
            if host::canvas_commit(id, &img.bgra, img.width, img.height) {
                Some(id)
            } else {
                self.free_canvas.push(id);
                None
            }
        });
        let shown = canvas.is_some();
        self.thumbs.insert(job.path, Thumb { canvas, stamp: job.stamp });
        shown
    }

    /// Load the preview for the cursor item if it changed. True if the pane
    /// has something new to show.
    fn pump_preview(&mut self) -> bool {
        let want = if self.preview_on {
            self.cursor.and_then(|c| self.entry_at(c))
                .map(|e| (join(&self.current, &e.name), (e.size, e.mtime), e.is_dir))
        } else {
            None
        };
        let same = match (&self.preview, &want) {
            (Some(p), Some((path, stamp, _))) => p.path == *path && p.stamp == *stamp,
            (None, None) => true,
            _ => false,
        };
        if same { return self.pump_preview_picture(); }
        if self.preview.take().is_some_and(|p| p.image) { host::canvas_drop(PREVIEW_CANVAS); }
        self.preview_job = None;
        let Some((path, stamp, is_dir)) = want else { return true };
        let mut text = None;
        if !is_dir {
            if extension(&path).as_deref() == Some("png") {
                self.preview_job = PngJob::open(path.clone(), stamp, PREVIEW_PX);
            } else {
                text = text_head(&path);
            }
        }
        self.preview = Some(Preview { path, stamp, image: false, text });
        true
    }

    /// One tick on the preview pane's picture. True when it is ready.
    fn pump_preview_picture(&mut self) -> bool {
        let Some(job) = &mut self.preview_job else { return false };
        let Some(img) = job.step(&mut self.png_buf) else { return false };
        self.preview_job = None;
        let shown = img.is_some_and(|img| host::canvas_commit(PREVIEW_CANVAS, &img.bgra, img.width, img.height));
        if let Some(p) = &mut self.preview { p.image = shown; }
        shown
    }

    /// The pointer moved with the button held after a press on an item:
    /// past the slop, hand the targets to the compositor as a drag.
    fn drag_item(&mut self, x: i32, y: i32) -> bool {
        let Some((x0, y0)) = self.press else { return false };
        if self.dragging || (x - x0).abs() + (y - y0).abs() <= DRAG_SLOP { return false; }
        let paths: Vec<String> = self.targets().into_iter().map(|t| t.full).collect();
        self.press = None;
        if paths.is_empty() || !host::drag_start(&paths) { return false; }
        self.dragging = true;
        self.press_keep = false;
        let what = match paths.as_slice() {
            [one] => basename(one).to_string(),
            _ => num(paths.len()),
        };
        self.say(fill(s().dragging, &[&what]), false);
        true
    }

    /// Follow the pointer while the grip is dragged. The pane is on the
    /// right, so moving left widens it.
    fn drag_grip(&mut self, x: i32) -> bool {
        let Grip::Drag { x0, w0, scale } = self.grip else { return false };
        let dx = (x0 - x) * 1000 / scale;
        let (screen_w, _) = host::screen_size();
        let max = (screen_w * 2 / 3).clamp(PREVIEW_W_MIN as u32, u16::MAX as u32) as i32;
        let w = (w0 as i32 + dx).clamp(PREVIEW_W_MIN as i32, max) as u16;
        if w == self.preview_w { return false; }
        self.preview_w = w;
        true
    }

    fn toggle_preview(&mut self) {
        self.preview_on = !self.preview_on;
        self.save_settings();
    }

    /// The canvas of `e`'s thumbnail, if it is ready.
    fn thumb_of(&self, e: &Entry) -> Option<u32> {
        self.thumbs.get(&join(&self.current, &e.name)).and_then(|t| t.canvas)
    }

    fn prune_marks(&mut self) {
        if self.marks.is_empty() { return; }
        let names: BTreeSet<&str> = self.source().iter().map(|e| e.name.as_str()).collect();
        let kept: BTreeSet<String> = self.marks.iter()
            .filter(|m| names.contains(m.as_str()))
            .cloned()
            .collect();
        self.marks = kept;
    }

    /// Source list paired with `filtered` — entries when browsing,
    /// recursive when searching.
    fn source(&self) -> &Vec<Entry> {
        if self.query.is_empty() || self.recursive_dir.is_none() {
            &self.entries
        } else {
            &self.recursive
        }
    }

    /// The entry shown at row `ui` of the filtered view.
    fn entry_at(&self, ui: usize) -> Option<&Entry> {
        let &i = self.filtered.get(ui)?;
        self.source().get(i)
    }

    fn cursor_name(&self) -> Option<String> {
        self.cursor.and_then(|c| self.entry_at(c)).map(|e| e.name.clone())
    }

    /// Row of the item called `name` in the filtered view.
    fn index_of(&self, name: &str) -> Option<usize> {
        let src = self.source();
        self.filtered.iter().position(|&i| src.get(i).is_some_and(|e| e.name == name))
    }

    fn select_name(&mut self, name: &str) {
        if let Some(i) = self.index_of(name) { self.cursor = Some(i); }
    }

    fn is_marked(&self, ui: usize) -> bool {
        !self.marks.is_empty() && self.entry_at(ui).is_some_and(|e| self.marks.contains(&e.name))
    }

    /// Marked items still visible under the current filter.
    fn visible_marks(&self) -> usize {
        if self.marks.is_empty() { return 0; }
        (0..self.filtered.len()).filter(|&ui| self.is_marked(ui)).count()
    }

    /// Light the sidebar place the current folder lies in: the deepest
    /// one that contains it, so `Music/Album` still lights Music.
    fn sync_sidebar_from_current(&mut self) {
        self.sidebar_sel = self.sidebar.iter().enumerate()
            .filter(|(_, p)| p.path == self.current || is_below(&self.current, &p.path))
            .max_by_key(|(_, p)| p.path.len())
            .map(|(i, _)| i);
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Show `path`; search, marks and cursor start fresh.
    fn enter(&mut self, path: String) {
        self.current = path;
        self.query.clear();
        self.marks.clear();
        self.cursor = None;
        self.last_click = None;
        self.ctx = None;
        self.location = None;
        self.anchor = None;
        self.refresh();
    }

    fn navigate(&mut self, new_path: String) {
        if new_path == self.current { return; }
        self.history.push(self.current.clone());
        self.forward.clear();
        self.enter(new_path);
    }

    fn go_back(&mut self) {
        if let Some(p) = self.history.pop() {
            self.forward.push(self.current.clone());
            self.enter(p);
        }
    }

    fn go_forward(&mut self) {
        if let Some(p) = self.forward.pop() {
            self.history.push(self.current.clone());
            self.enter(p);
        }
    }

    fn go_up(&mut self) {
        let parent = parent_path(&self.current);
        if parent != self.current { self.navigate(parent); }
    }

    /// Open the cursor item: enter a folder, hand a file to its app.
    fn open_cursor(&mut self) {
        // In search mode the name is a relative path like
        // "wallpapers/aurora"; the join below gives the absolute target.
        let Some((is_dir, name)) = self.cursor.and_then(|c| self.entry_at(c))
            .map(|e| (e.is_dir, e.name.clone())) else { return };
        let full = join(&self.current, &name);
        if is_dir {
            self.navigate(full);
        } else if let Some(app) = self.associated_app_in(&self.current, &name) {
            open(&app, &full);
        } else {
            self.open_with();
        }
    }

    // ── Cursor and marks ──────────────────────────────────────────────

    /// One row down. In the grid a row is `GRID_COLS` entries; in the list
    /// — the default view — a row is one entry.
    fn select_delta_y(&mut self, dy: isize) {
        self.anchor = None;
        self.move_cursor(dy * self.row_stride());
    }

    /// Sideways only means something in the grid. In a one-per-row list it
    /// would just duplicate Up/Down, which reads as the selection jumping
    /// for no reason.
    fn select_delta_x(&mut self, dx: isize) {
        if matches!(self.view_mode, ViewMode::List) { return; }
        self.anchor = None;
        self.move_cursor(dx);
    }

    fn row_stride(&self) -> isize {
        match self.view_mode {
            ViewMode::Grid => self.grid_cols() as isize,
            ViewMode::List => 1,
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.filtered.is_empty() { self.cursor = None; return; }
        let max = self.filtered.len() as isize - 1;
        let next = (self.cursor.unwrap_or(0) as isize + delta).clamp(0, max);
        self.cursor = Some(next as usize);
    }

    fn cursor_to(&mut self, ui: usize) {
        self.cursor = if self.filtered.is_empty() { None } else { Some(ui.min(self.filtered.len() - 1)) };
    }

    /// Jump to the next item (after the cursor, wrapping) whose name starts
    /// with `ch`, ASCII case-insensitive.
    fn type_ahead(&mut self, ch: u8) {
        let want = ch.to_ascii_lowercase();
        let n = self.filtered.len();
        let start = self.cursor.map_or(0, |c| c + 1);
        let hit = (0..n).map(|k| (start + k) % n).find(|&ui| {
            self.entry_at(ui).is_some_and(|e| basename(&e.name_lc).as_bytes().first() == Some(&want))
        });
        if let Some(ui) = hit { self.cursor = Some(ui); }
    }

    /// Shift/Ctrl + left click. Ctrl toggles the item; Shift marks the
    /// range from the anchor (the last plain click) to it.
    fn modified_click(&mut self, ui: usize, shift: bool, ctrl: bool) {
        self.close_menus();
        if ui >= self.filtered.len() { return; }
        self.last_click = None;
        if shift {
            let from = self.anchor.or(self.cursor).unwrap_or(ui);
            if !ctrl { self.marks.clear(); }
            self.mark_range(from, ui);
        } else {
            // The first Ctrl+click keeps the item selected so far.
            if self.marks.is_empty() {
                if let Some(c) = self.cursor.filter(|&c| c != ui) { self.toggle_mark(c); }
            }
            self.toggle_mark(ui);
            self.anchor = Some(ui);
        }
        self.cursor = Some(ui);
    }

    /// Shift+arrow: move the cursor and mark from the anchor to it.
    fn extend_marks(&mut self, delta: isize) {
        if self.filtered.is_empty() { return; }
        let from = self.anchor.or(self.cursor).unwrap_or(0);
        self.anchor = Some(from);
        self.move_cursor(delta);
        let to = self.cursor.unwrap_or(from);
        self.marks.clear();
        self.mark_range(from, to);
    }

    fn mark_range(&mut self, a: usize, b: usize) {
        let (lo, hi) = (a.min(b), a.max(b).min(self.filtered.len().saturating_sub(1)));
        for ui in lo..=hi {
            if let Some(name) = self.entry_at(ui).map(|e| e.name.clone()) { self.marks.insert(name); }
        }
    }

    fn toggle_mark(&mut self, ui: usize) {
        let Some(name) = self.entry_at(ui).map(|e| e.name.clone()) else { return };
        if !self.marks.remove(&name) { self.marks.insert(name); }
    }

    fn select_all(&mut self) {
        let names: Vec<String> = (0..self.filtered.len())
            .filter_map(|ui| self.entry_at(ui).map(|e| e.name.clone()))
            .collect();
        self.marks = names.into_iter().collect();
    }

    /// Left click on an item. Select mode toggles its mark; otherwise the
    /// first click selects it and a second one on the same item opens it.
    fn click_item(&mut self, ui: usize) {
        self.close_menus();
        if ui >= self.filtered.len() { return; }
        self.cursor = Some(ui);
        if self.select_mode {
            self.toggle_mark(ui);
            self.last_click = None;
            return;
        }
        self.anchor = Some(ui);
        self.press_armed = true;
        let now = host::ticks_ms();
        let double = matches!(self.last_click,
            Some((i, t)) if i == ui && now.saturating_sub(t) <= DOUBLE_CLICK_MS);
        self.press_keep = !double && self.is_marked(ui) && self.visible_marks() > 1;
        if !self.press_keep { self.marks.clear(); }
        if double {
            self.last_click = None;
            self.open_cursor();
        } else {
            self.last_click = Some((ui, now));
        }
    }

    /// Right click on an item. A marked item keeps the marks, so the menu
    /// acts on all of them; any other item becomes the only target.
    fn open_item_menu(&mut self, ui: usize) {
        let Some(name) = self.entry_at(ui).map(|e| e.name.clone()) else { return };
        if !self.marks.contains(&name) { self.marks.clear(); }
        self.cursor = Some(ui);
        self.open_menu = None;
        self.ctx = Some(CtxMenu::Item(name));
    }

    fn close_menus(&mut self) {
        self.open_menu = None;
        self.ctx = None;
    }

    // ── Status bar ────────────────────────────────────────────────────

    fn say(&mut self, text: String, error: bool) {
        self.status = Some(Status { text, error, until: host::ticks_ms() + STATUS_MS });
    }

    /// Drop an expired message. True if the status bar changed.
    fn expire_status(&mut self) -> bool {
        if self.status.as_ref().is_some_and(|st| host::ticks_ms() >= st.until) {
            self.status = None;
            return true;
        }
        false
    }

    /// One message for a batch: failures win over successes.
    fn report(&mut self, ok: usize, failed: usize, ok_t: &str, fail_t: &str) {
        if failed > 0 {
            self.say(fill(fail_t, &[&num(failed)]), true);
        } else if ok > 0 {
            self.say(fill(ok_t, &[&num(ok)]), false);
        }
    }

    // ── File operations ───────────────────────────────────────────────

    fn target_of(&self, e: &Entry) -> Target {
        Target { name: e.name.clone(), full: join(&self.current, &e.name), is_dir: e.is_dir }
    }

    /// What an operation acts on: the visible marked items, or else the
    /// cursor item.
    fn targets(&self) -> Vec<Target> {
        let mut out: Vec<Target> = (0..self.filtered.len())
            .filter(|&ui| self.is_marked(ui))
            .filter_map(|ui| self.entry_at(ui).map(|e| self.target_of(e)))
            .collect();
        if out.is_empty() {
            out.extend(self.cursor_target());
        }
        out
    }

    fn cursor_target(&self) -> Option<Target> {
        self.cursor.and_then(|c| self.entry_at(c)).map(|e| self.target_of(e))
    }

    /// True if a paste is possible (clipboard holds something).
    fn can_paste(&self) -> bool { self.clipboard.is_some() }

    /// Write the clipboard where other loft windows read it: the mode,
    /// then the paths, NUL-separated (a name may hold a newline).
    fn store_clip(&self) {
        let Some(clip) = &self.clipboard else { fs_delete(CLIP_PATH); return };
        let mut out = String::from(if clip.mode == ClipMode::Copy { "copy" } else { "cut" });
        for p in &clip.items {
            out.push('\0');
            out.push_str(p);
        }
        host::store(CLIP_PATH, out.as_bytes());
    }

    /// Take the clipboard another loft window may have set.
    fn load_clip(&mut self) {
        let Some(buf) = host::fetch(CLIP_PATH) else { self.clipboard = None; return };
        let text = String::from_utf8_lossy(&buf);
        let mut parts = text.split('\0');
        let mode = match parts.next() {
            Some("copy") => ClipMode::Copy,
            Some("cut") => ClipMode::Cut,
            _ => { self.clipboard = None; return }
        };
        let items: Vec<String> = parts.filter(|p| !p.is_empty()).map(String::from).collect();
        self.clipboard = (!items.is_empty()).then_some(Clip { items, mode });
    }

    /// Ctrl+C / Ctrl+X — remember the targets for a paste.
    fn arm_clipboard(&mut self, mode: ClipMode) {
        let items: Vec<String> = self.targets().into_iter().map(|t| t.full).collect();
        if items.is_empty() { return; }
        let n = num(items.len());
        self.clipboard = Some(Clip { items, mode });
        self.store_clip();
        let t = if mode == ClipMode::Copy { s().copied } else { s().cut_done };
        self.say(fill(t, &[&n]), false);
    }

    /// Ctrl+V: copy or move the clipboard items into the current folder.
    /// A name that exists already asks first (see `Dialog::Clash`).
    fn do_paste(&mut self) {
        if self.paste.is_some() { return; }
        self.load_clip();
        let Some(clip) = self.clipboard.take() else { return };
        let mut todo = clip.items.clone();
        todo.reverse();
        self.paste = Some(PasteJob {
            mode: clip.mode, dest: self.current.clone(), dropped: false,
            all: clip.items, todo, policy: None, apply_all: false, asked: None,
            steps: Vec::new(), ok: 0, failed: 0, last: None,
        });
        self.run_paste(None);
    }

    /// The folder a drop target stands for.
    fn drop_dest(&self, id: u32) -> Option<String> {
        match id {
            ACT_DROP_HERE => Some(self.current.clone()),
            ACT_DROP_SIDEBAR_BASE..ACT_DROP_SIDEBAR_END =>
                self.sidebar.get((id - ACT_DROP_SIDEBAR_BASE) as usize).map(|p| p.path.clone()),
            ACT_DROP_ITEM_BASE..ACT_DROP_ITEM_END =>
                self.drop_dirs.get((id - ACT_DROP_ITEM_BASE) as usize).cloned().flatten(),
            _ => None,
        }
    }

    /// Items dropped on folder `dest`: moved, or copied with Ctrl. A drop
    /// on the trash moves them there as Delete does.
    fn drop_into(&mut self, dest: String, paths: Vec<String>, copy: bool) {
        if self.paste.is_some() || !self.idle_fs() || paths.is_empty() { return; }
        if dest == self.trash_dir() {
            if copy { return; }
            let targets: Vec<Target> = paths.iter().map(|p| Target {
                name: basename(p).to_string(), full: p.clone(), is_dir: dir_exists(p),
            }).collect();
            let (steps, failed) = self.trash_items(&targets);
            let ok = steps.len();
            self.record(steps);
            self.marks.clear();
            self.refresh();
            self.report(ok, failed, s().trashed, s().trash_failed);
            return;
        }
        let mut todo = paths.clone();
        todo.reverse();
        self.paste = Some(PasteJob {
            mode: if copy { ClipMode::Copy } else { ClipMode::Cut }, dest, dropped: true,
            all: paths, todo, policy: None, apply_all: false, asked: None,
            steps: Vec::new(), ok: 0, failed: 0, last: None,
        });
        self.run_paste(None);
    }

    /// Work through the paste job. `answer` resolves the clash the job is
    /// waiting on; the next clash without a standing policy pauses it again.
    fn run_paste(&mut self, mut answer: Option<Clash>) {
        let Some(mut job) = self.paste.take() else { return };
        while let Some(src) = job.todo.last().cloned() {
            // Moving an item into the folder it is already in changes nothing.
            if job.mode == ClipMode::Cut && parent_path(&src) == job.dest {
                job.todo.pop();
                continue;
            }
            // Dropped on itself: nothing to do. A folder cannot go below
            // itself.
            if job.dest == src || is_below(&job.dest, &src) {
                job.todo.pop();
                if job.dest != src { job.failed += 1; }
                continue;
            }
            if job.asked.as_deref().is_some_and(|a| a != src) { answer = None; }
            job.asked = None;
            let name = basename(&src).to_string();
            let is_dir = dir_exists(&src);
            let plain = join(&job.dest, &name);
            // A copy into its own folder is a duplicate, not a clash.
            let clash = plain != src && path_exists(&plain);
            let choice = if !clash {
                Clash::Keep
            } else if let Some(c) = answer.take().or(job.policy) {
                if job.apply_all { job.policy = Some(c); }
                c
            } else {
                self.open_dialog(Dialog::Clash { name });
                job.asked = Some(src);
                self.paste = Some(job);
                return;
            };
            job.todo.pop();
            if choice == Clash::Skip { continue; }
            // Replacing a folder that holds the source would trash the source.
            if choice == Clash::Replace && is_below(&src, &plain) { job.failed += 1; continue; }
            if choice == Clash::Replace {
                let old = Target { name: name.clone(), full: plain.clone(), is_dir: dir_exists(&plain) };
                match self.trash_items(core::slice::from_ref(&old)).0.pop() {
                    Some(step) => job.steps.push(step),
                    None => { job.failed += 1; continue; }
                }
            }
            let dest = if choice == Clash::Replace { plain } else { unique_in(&job.dest, &name, is_dir) };
            let done = match job.mode {
                ClipMode::Copy => fs_copy(&src, &dest),
                ClipMode::Cut  => fs_rename(&src, &dest),
            };
            if done {
                job.ok += 1;
                job.last = Some(basename(&dest).to_string());
                job.steps.push(match job.mode {
                    ClipMode::Copy => Step::Create { path: dest, is_dir },
                    ClipMode::Cut  => Step::Move { from: src, to: dest },
                });
            } else {
                job.failed += 1;
            }
        }
        self.finish_paste(job);
    }

    fn finish_paste(&mut self, job: PasteJob) {
        // A copy can be pasted again; a cut keeps only what did not move.
        if !job.dropped {
            let mut items = job.all;
            if job.mode == ClipMode::Cut { items.retain(|p| path_exists(p)); }
            self.clipboard = (!items.is_empty()).then_some(Clip { items, mode: job.mode });
            self.store_clip();
        }
        self.record(job.steps);
        self.marks.clear();
        self.refresh();
        if job.dest == self.current {
            if let Some(name) = job.last { self.select_name(&name); }
        }
        self.report(job.ok, job.failed, s().pasted, s().paste_failed);
    }

    /// Esc on the clash dialog: keep what is done, drop the rest.
    fn stop_paste(&mut self) {
        if let Some(mut job) = self.paste.take() {
            job.todo.clear();
            self.finish_paste(job);
        }
    }

    /// Delete key / "Move to trash": to the trash, or with a confirmation
    /// for good when the items are in the trash already.
    fn delete_targets(&mut self) {
        let targets = self.targets();
        if targets.is_empty() { return; }
        self.close_menus();
        if self.in_trash_tree() {
            self.open_dialog(Dialog::ConfirmDelete(targets));
        } else {
            self.move_to_trash(targets);
        }
    }

    fn move_to_trash(&mut self, targets: Vec<Target>) {
        let (steps, failed) = self.trash_items(&targets);
        let ok = steps.len();
        self.record(steps);
        self.marks.clear();
        self.refresh();
        self.report(ok, failed, s().trashed, s().trash_failed);
    }

    /// Move `targets` into the trash and note where they came from. Returns
    /// the moves done and the number that failed; refreshes nothing.
    fn trash_items(&mut self, targets: &[Target]) -> (Vec<Step>, usize) {
        if !self.ensure_trash() { return (Vec::new(), targets.len()); }
        let trash = self.trash_dir();
        let mut records = self.read_trash_index();
        let now = host::unix_time();
        let mut steps = Vec::new();
        let mut failed = 0usize;
        for t in targets {
            // The trash cannot move into itself, nor can a folder holding it.
            if t.full == trash || is_below(&trash, &t.full) { failed += 1; continue; }
            let dest = unique_in(&trash, basename(&t.name), t.is_dir);
            if fs_rename(&t.full, &dest) {
                records.push(TrashRecord { name: basename(&dest).to_string(), origin: t.full.clone(), time: now });
                steps.push(Step::Move { from: t.full.clone(), to: dest });
            } else {
                failed += 1;
            }
        }
        if !steps.is_empty() { self.write_trash_index(&records); }
        (steps, failed)
    }

    /// Create the trash folder if it is missing. False if that failed.
    fn ensure_trash(&mut self) -> bool {
        let trash = self.trash_dir();
        if dir_exists(&trash) { return true; }
        if !fs_mkdir(&trash) { return false; }
        // The sidebar shows only existing places; Trash exists now.
        self.rebuild_sidebar();
        true
    }

    /// Move trash items back to where they came from. An item whose folder
    /// is gone, or that has no index line, goes to home instead.
    fn restore_targets(&mut self) {
        self.close_menus();
        if !self.in_trash() { return; }
        let targets = self.targets();
        let mut records = self.read_trash_index();
        let (mut ok, mut homed, mut nested, mut failed) = (0usize, 0usize, 0usize, 0usize);
        let mut steps = Vec::new();
        for t in &targets {
            // A search hit below the top level has no index line of its own.
            if t.name.contains('/') { nested += 1; continue; }
            let pos = records.iter().position(|r| r.name == t.name);
            let origin = pos.map(|p| records[p].origin.clone());
            let (dir, name) = match &origin {
                Some(o) => (parent_path(o), basename(o).to_string()),
                None => (self.home.clone(), t.name.clone()),
            };
            let back_home = origin.is_none() || !(dir.is_empty() || dir_exists(&dir));
            let dir = if back_home { self.home.clone() } else { dir };
            let dest = unique_in(&dir, &name, t.is_dir);
            if fs_rename(&t.full, &dest) {
                if let Some(p) = pos { records.remove(p); }
                steps.push(Step::Move { from: t.full.clone(), to: dest });
                if back_home { homed += 1 } else { ok += 1 }
            } else {
                failed += 1;
            }
        }
        self.write_trash_index(&records);
        self.record(steps);
        self.marks.clear();
        self.refresh();
        if failed > 0 {
            self.say(fill(s().restore_failed, &[&num(failed)]), true);
        } else if homed > 0 {
            self.say(fill(s().restored_home, &[&num(homed)]), false);
        } else if nested > 0 {
            self.say(s().restore_nested.to_string(), true);
        } else if ok > 0 {
            self.say(fill(s().restored, &[&num(ok)]), false);
        }
    }

    /// "Delete permanently" from a menu: always asks first.
    fn request_delete_permanently(&mut self) {
        let targets = self.targets();
        if targets.is_empty() { return; }
        self.close_menus();
        self.open_dialog(Dialog::ConfirmDelete(targets));
    }

    fn request_empty_trash(&mut self) {
        self.close_menus();
        let trash = self.trash_dir();
        let targets: Vec<Target> = self.list(&trash, false).into_iter()
            .map(|e| Target { full: join(&trash, &e.name), name: e.name, is_dir: e.is_dir })
            .collect();
        if !targets.is_empty() { self.dialog = Some(Dialog::ConfirmDelete(targets)); }
    }

    /// Start deleting `targets` for good; `pump_delete` does the work a
    /// slice per idle tick so a big tree never freezes the window.
    fn delete_permanently(&mut self, targets: Vec<Target>) {
        if self.deleting.is_some() { return; }
        let mut todo: Vec<DelItem> = targets.into_iter()
            .map(|t| DelItem { path: t.full, is_dir: t.is_dir, top: true, tries: 0, last: 0 })
            .collect();
        todo.reverse();
        self.marks.clear();
        self.deleting = Some(DeleteJob { todo, ok: 0, failed: 0 });
    }

    /// Delete up to `budget` paths. A folder is expanded into its contents
    /// (files first, deepest folders next) and removed once empty; a listing
    /// cut short by its buffer takes another round. True if anything changed.
    fn pump_delete(&mut self, budget: usize) -> bool {
        let Some(job) = self.deleting.as_mut() else { return false };
        for _ in 0..budget {
            let Some(item) = job.todo.pop() else { break };
            if fs_delete(&item.path) {
                if item.top { job.ok += 1; }
                continue;
            }
            let children = if item.is_dir { list_dir_internal(&item.path, true) } else { Vec::new() };
            let stuck = item.tries > 0 && children.len() >= item.last;
            if children.is_empty() || stuck || item.tries >= DELETE_ROUNDS {
                if item.top { job.failed += 1; }
                continue;
            }
            let path = item.path.clone();
            job.todo.push(DelItem { tries: item.tries + 1, last: children.len(), ..item });
            let mut dirs: Vec<&Entry> = children.iter().filter(|e| e.is_dir).collect();
            dirs.sort_by_key(|e| e.name.matches('/').count());
            let child = |name: &str, is_dir| DelItem { path: join(&path, name), is_dir, top: false, tries: 0, last: 0 };
            for d in dirs { job.todo.push(child(&d.name, true)); }
            for f in children.iter().filter(|e| !e.is_dir) { job.todo.push(child(&f.name, false)); }
        }
        if job.todo.is_empty() { self.finish_delete(); }
        true
    }

    fn finish_delete(&mut self) {
        let Some(job) = self.deleting.take() else { return };
        self.prune_trash_index();
        self.refresh();
        if job.todo.is_empty() {
            self.report(job.ok, job.failed, s().deleted, s().delete_failed);
        } else {
            self.say(fill(s().delete_stopped, &[&num(job.ok)]), false);
        }
    }

    /// Esc while deleting: stop after the current slice.
    fn stop_delete(&mut self) { self.finish_delete(); }

    // ── Trash index ───────────────────────────────────────────────────

    fn read_trash_index(&self) -> Vec<TrashRecord> {
        let path = self.trash_index();
        let Some((_, false, _)) = host::fs_stat(&path) else { return Vec::new() };
        parse_trash_index(&host::fetch(&path).unwrap_or_default())
    }

    /// Replace the index with `records`; an empty index is removed.
    fn write_trash_index(&self, records: &[TrashRecord]) {
        let path = self.trash_index();
        let mut text = String::new();
        // A tab or newline in a name would break the line format; such an
        // item restores to home like one without a line.
        let plain = |v: &str| !v.contains(['\t', '\n']);
        for r in records.iter().filter(|r| plain(&r.name) && plain(&r.origin)) {
            text.push_str(&r.name);
            text.push('\t');
            text.push_str(&r.origin);
            text.push('\t');
            push_usize(&mut text, r.time as usize);
            text.push('\n');
        }
        let done = if text.is_empty() {
            !path_exists(&path) || fs_delete(&path)
        } else {
            host::store(&path, text.as_bytes())
        };
        if !done { log("[loft] could not write the trash index"); }
    }

    /// Drop index lines whose trash item no longer exists.
    fn prune_trash_index(&self) {
        let trash = self.trash_dir();
        let records = self.read_trash_index();
        let before = records.len();
        let kept: Vec<TrashRecord> = records.into_iter()
            .filter(|r| path_exists(&join(&trash, &r.name)))
            .collect();
        if kept.len() != before { self.write_trash_index(&kept); }
    }

    // ── Dialogs ───────────────────────────────────────────────────────

    fn set_name_buf(&mut self, value: &str) {
        self.name_buf.clear();
        // At a character boundary: typed text is UTF-8.
        let cap = self.name_buf.capacity();
        self.name_buf.push_str(nopeek_widgets::rt::str_clamp(value, cap));
    }

    /// Open the rename dialog for the cursor item, or the find-and-replace
    /// dialog when several items are marked.
    fn open_rename(&mut self) {
        let targets = self.targets();
        if targets.len() > 1 {
            self.close_menus();
            self.find_buf.clear();
            self.repl_buf.clear();
            self.open_dialog(Dialog::BatchRename(targets));
            return;
        }
        let Some(t) = self.cursor_target() else { return };
        self.close_menus();
        self.set_name_buf(basename(&t.name));
        self.open_dialog(Dialog::Name(NameKind::Rename { old: t.full }));
    }

    fn open_new_folder(&mut self) {
        self.close_menus();
        let dest = unique_in(&self.current, s().folder_default, true);
        let name = basename(&dest).to_string();
        self.set_name_buf(&name);
        self.open_dialog(Dialog::Name(NameKind::NewFolder));
    }

    fn open_new_file(&mut self) {
        self.close_menus();
        let dest = unique_in(&self.current, s().file_default, false);
        let name = basename(&dest).to_string();
        self.set_name_buf(&name);
        self.open_dialog(Dialog::Name(NameKind::NewFile));
    }

    fn open_properties(&mut self) {
        let Some(e) = self.cursor.and_then(|c| self.entry_at(c)) else { return };
        // Folder totals exist only for the browse listing, once scanned.
        let known = e.is_dir && self.query.is_empty() && !e.stats_pending;
        let size = if !e.is_dir || known { format_size(e.size) } else { s().props_folder.to_string() };
        let files = known.then(|| num(e.files as usize));
        let full = join(&self.current, &e.name);
        let parent = parent_path(&full);
        let props = Props {
            name:     basename(&e.name).to_string(),
            kind:     type_for(e),
            size,
            files,
            modified: format_mtime(e.mtime),
            location: if parent.is_empty() { "/".to_string() } else { parent },
        };
        self.close_menus();
        self.open_dialog(Dialog::Properties(props));
    }

    /// Show `d`; its first field takes the keyboard focus.
    fn open_dialog(&mut self, d: Dialog) {
        self.dialog = Some(d);
        self.focus_seq = self.focus_seq.wrapping_add(1).max(1);
    }

    /// Enter / the dialog's primary button.
    fn submit_dialog(&mut self) {
        match self.dialog.take() {
            Some(Dialog::Name(kind)) => self.commit_name(kind),
            Some(Dialog::ConfirmDelete(targets)) => self.delete_permanently(targets),
            // Enter on a clash keeps both, the choice that loses nothing.
            Some(Dialog::Clash { .. }) => self.run_paste(Some(Clash::Keep)),
            Some(Dialog::BatchRename(targets)) => self.commit_batch_rename(targets),
            Some(Dialog::OpenWith { target, apps, always }) => {
                self.dialog = Some(Dialog::OpenWith { target, apps, always });
            }
            Some(Dialog::Properties(_)) | None => {}
        }
    }

    /// Apply the name dialog. On a bad or taken name the dialog stays up.
    fn commit_name(&mut self, kind: NameKind) {
        let new = self.name_buf.trim().to_string();
        if !name_ok(&new) {
            self.say(s().name_invalid.to_string(), true);
            self.open_dialog(Dialog::Name(kind));
            return;
        }
        let dir = match &kind {
            NameKind::Rename { old } => parent_path(old),
            NameKind::NewFolder | NameKind::NewFile => self.current.clone(),
        };
        let dest = join(&dir, &new);
        if let NameKind::Rename { old } = &kind {
            if *old == dest { return; }
        }
        if path_exists(&dest) {
            self.say(s().name_exists.to_string(), true);
            self.open_dialog(Dialog::Name(kind));
            return;
        }
        let done = match &kind {
            NameKind::Rename { old } => fs_rename(old, &dest),
            NameKind::NewFolder => fs_mkdir(&dest),
            NameKind::NewFile => host::store(&dest, &[]),
        };
        if !done {
            let msg = match kind {
                NameKind::Rename { .. } => s().rename_failed,
                NameKind::NewFolder => s().folder_failed,
                NameKind::NewFile => s().file_failed,
            };
            self.say(msg.to_string(), true);
            return;
        }
        let step = match kind {
            NameKind::Rename { old } => Step::Move { from: old, to: dest },
            NameKind::NewFolder => Step::Create { path: dest, is_dir: true },
            NameKind::NewFile => Step::Create { path: dest, is_dir: false },
        };
        self.record(alloc::vec![step]);
        self.marks.clear();
        self.refresh();
        if dir == self.current { self.select_name(&new); }
    }

    /// Put the targets' full paths on the system clipboard, one per line.
    fn copy_path(&mut self) {
        self.close_menus();
        let paths: Vec<String> = self.targets().into_iter().map(|t| t.full).collect();
        if paths.is_empty() { return; }
        if clipboard_set(&paths.join("\n")) {
            self.say(s().path_copied.to_string(), false);
        } else {
            self.say(s().path_failed.to_string(), true);
        }
    }

    /// New names for a batch rename, in target order. `None` for a name
    /// that does not change.
    fn batch_names(&self, targets: &[Target]) -> Vec<Option<String>> {
        targets.iter().map(|t| {
            let old = basename(&t.name);
            if self.find_buf.is_empty() { return None; }
            let new = old.replace(self.find_buf.as_str(), &self.repl_buf);
            (new != old).then_some(new)
        }).collect()
    }

    /// Apply a batch rename. Nothing moves unless every new name is valid,
    /// free, and unique within the batch.
    fn commit_batch_rename(&mut self, targets: Vec<Target>) {
        let names = self.batch_names(&targets);
        let mut dests: Vec<(usize, String)> = Vec::new();
        let mut ok = true;
        for (i, n) in names.iter().enumerate() {
            let Some(n) = n else { continue };
            let bad = n.trim().is_empty() || n.contains('/') || n == "." || n == "..";
            let dest = join(&parent_path(&targets[i].full), n);
            if bad || path_exists(&dest) || dests.iter().any(|(_, d)| *d == dest) { ok = false; break; }
            dests.push((i, dest));
        }
        if !ok {
            self.say(s().br_clash.to_string(), true);
            self.open_dialog(Dialog::BatchRename(targets));
            return;
        }
        let (mut done, mut failed) = (0usize, 0usize);
        let mut steps = Vec::new();
        for (i, dest) in dests {
            if fs_rename(&targets[i].full, &dest) {
                done += 1;
                steps.push(Step::Move { from: targets[i].full.clone(), to: dest });
            } else {
                failed += 1;
            }
        }
        self.record(steps);
        self.marks.clear();
        self.refresh();
        self.report(done, failed, s().renamed, s().rename_failed);
    }

    // ── Duplicate ─────────────────────────────────────────────────────

    /// Ctrl+D: a copy of each target next to it, under a free name.
    fn duplicate(&mut self) {
        self.close_menus();
        let targets = self.targets();
        if targets.is_empty() || self.in_trash_tree() { return; }
        let (mut ok, mut failed) = (0usize, 0usize);
        let mut steps = Vec::new();
        let mut last = None;
        for t in &targets {
            let dest = unique_in(&parent_path(&t.full), basename(&t.name), t.is_dir);
            if fs_copy(&t.full, &dest) {
                ok += 1;
                last = Some(basename(&dest).to_string());
                steps.push(Step::Create { path: dest, is_dir: t.is_dir });
            } else {
                failed += 1;
            }
        }
        self.record(steps);
        self.marks.clear();
        self.refresh();
        if let Some(name) = last { self.select_name(&name); }
        self.report(ok, failed, s().duplicated, s().duplicate_failed);
    }

    // ── Undo ──────────────────────────────────────────────────────────

    /// False, with a note in the status bar, while a delete is running.
    fn idle_fs(&mut self) -> bool {
        if self.deleting.is_none() { return true; }
        self.say(s().busy.to_string(), true);
        false
    }

    fn record(&mut self, steps: Vec<Step>) {
        if !steps.is_empty() { self.undo.push(steps); }
    }

    /// Ctrl+Z: reverse the last operation. A move goes back, something
    /// created goes to the trash, so an undo never destroys data. Steps
    /// whose items changed since are left alone and reported.
    fn undo_last(&mut self) {
        self.close_menus();
        let Some(steps) = self.undo.pop() else {
            self.say(s().nothing_to_undo.to_string(), false);
            return;
        };
        let mut failed = 0usize;
        let trash = self.trash_dir();
        // In reverse and one by one: a replace trashed an item and then put
        // the new one at its name, so the new one must leave first.
        for step in steps.into_iter().rev() {
            match step {
                Step::Move { from, to } => {
                    let back = path_exists(&to) && !path_exists(&from)
                        && (parent_path(&from).is_empty() || dir_exists(&parent_path(&from)));
                    if !(back && fs_rename(&to, &from)) { failed += 1; continue; }
                    // Undoing a restore puts the item back into the trash;
                    // it gets its index line back too.
                    if parent_path(&from) == trash {
                        let mut records = self.read_trash_index();
                        records.push(TrashRecord { name: basename(&from).to_string(), origin: to, time: host::unix_time() });
                        self.write_trash_index(&records);
                    }
                }
                Step::Create { path, is_dir } => {
                    let gone = path_exists(&path).then(|| {
                        let t = Target { name: basename(&path).to_string(), full: path, is_dir };
                        self.trash_items(core::slice::from_ref(&t)).1 == 0
                    });
                    if gone != Some(true) { failed += 1; }
                }
            }
        }
        self.prune_trash_index();
        self.marks.clear();
        self.refresh();
        if failed > 0 {
            self.say(s().undo_failed.to_string(), true);
        } else {
            self.say(s().undone.to_string(), false);
        }
    }

    // ── Open with ─────────────────────────────────────────────────────

    /// Offer every installed app for the cursor file.
    fn open_with(&mut self) {
        let Some(target) = self.cursor_target() else { return };
        if target.is_dir { return; }
        self.close_menus();
        let apps: Vec<(String, String)> = app_catalog::load(NOT_HANDLERS).into_iter()
            .filter(|a| a.kind == app_catalog::EntryKind::Module)
            .map(|a| (a.launch_name, a.display_name))
            .collect();
        self.open_dialog(Dialog::OpenWith { target, apps, always: false });
    }

    /// The user picked app `i` in the "Open with" dialog.
    fn open_with_pick(&mut self, i: usize) {
        let Some(Dialog::OpenWith { target, apps, always }) = self.dialog.take() else { return };
        let Some((app, _)) = apps.get(i) else { return };
        if always {
            if let Some(ext) = extension(&target.name) {
                if !self.set_association(&ext, app) {
                    self.say(s().assoc_failed.to_string(), true);
                }
            }
        }
        open(app, &target.full);
    }

    /// Point `ext` at `app` in `sys/config/associations`, keeping every
    /// other line (comments included) as it is.
    fn set_association(&mut self, ext: &str, app: &str) -> bool {
        if app.is_empty() || app.contains(['=', '\n', '\r', '#']) { return false; }
        let old = host::fetch(ASSOC_PATH).unwrap_or_default();
        let text = String::from_utf8_lossy(&old);
        let mut out = String::with_capacity(text.len() + 32);
        let mut placed = false;
        for line in text.lines() {
            let key = line.split_once('=').map(|(k, _)| k.trim().to_ascii_lowercase());
            if !line.trim_start().starts_with('#') && key.as_deref() == Some(ext) {
                if !placed { out.push_str(&alloc::format!("{}={}\n", ext, app)); placed = true; }
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        if !placed { out.push_str(&alloc::format!("{}={}\n", ext, app)); }
        if !host::store(ASSOC_PATH, out.as_bytes()) { return false; }
        self.assoc = load_associations();
        true
    }

    // ── Location bar ──────────────────────────────────────────────────

    /// Ctrl+L: replace the breadcrumb with a field holding the path.
    fn open_location(&mut self) {
        self.close_menus();
        self.location = Some(self.current.clone());
        self.focus_seq = self.focus_seq.wrapping_add(1).max(1);
    }

    /// Enter in the location bar: a folder is entered, a file selected in
    /// its folder. `~` stands for home, leading and trailing slashes are
    /// ignored.
    fn submit_location(&mut self) {
        let Some(typed) = self.location.clone() else { return };
        let t = typed.trim().trim_matches('/');
        let path = if t == "~" {
            self.home.clone()
        } else if let Some(rest) = t.strip_prefix("~/") {
            join(&self.home, rest.trim_matches('/'))
        } else {
            t.to_string()
        };
        if path.is_empty() || dir_exists(&path) {
            self.location = None;
            self.navigate(path);
        } else if path_exists(&path) {
            self.location = None;
            let name = basename(&path).to_string();
            self.navigate(parent_path(&path));
            self.select_name(&name);
        } else {
            self.say(fill(s().location_missing, &[&path]), true);
        }
    }

    // ── Tabs ──────────────────────────────────────────────────────────

    fn snapshot(&self) -> Tab {
        Tab {
            current: self.current.clone(),
            history: self.history.clone(),
            forward: self.forward.clone(),
            query:   self.query.clone(),
            cursor:  self.cursor_name(),
            marks:   self.marks.clone(),
        }
    }

    /// Show tab `i`, saving the one shown so far.
    fn switch_tab(&mut self, i: usize) {
        if i >= self.tabs.len() || i == self.active { return; }
        self.tabs[self.active] = self.snapshot();
        self.active = i;
        self.show_active_tab();
    }

    fn show_active_tab(&mut self) {
        let t = &self.tabs[self.active];
        let (cur, hist, fwd, query, cursor, marks) = (t.current.clone(), t.history.clone(),
            t.forward.clone(), t.query.clone(), t.cursor.clone(), t.marks.clone());
        self.enter(cur);
        self.history = hist;
        self.forward = fwd;
        self.query.push_str(&query);
        self.marks = marks;
        self.refilter();
        if let Some(name) = cursor { self.select_name(&name); }
    }

    /// A new tab on `path` (a folder), shown at once.
    fn new_tab(&mut self, path: String) {
        self.close_menus();
        self.tabs[self.active] = self.snapshot();
        self.tabs.push(Tab {
            current: path, history: Vec::new(), forward: Vec::new(),
            query: String::new(), cursor: None, marks: BTreeSet::new(),
        });
        self.active = self.tabs.len() - 1;
        self.show_active_tab();
    }

    /// Close tab `i`. False if it was the last one (the window goes then).
    fn close_tab(&mut self, i: usize) -> bool {
        if self.tabs.len() <= 1 { return false; }
        if i >= self.tabs.len() { return true; }
        self.tabs[self.active] = self.snapshot();
        self.tabs.remove(i);
        if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.show_active_tab();
        true
    }

    /// Open the cursor folder in a new tab.
    fn open_cursor_in_tab(&mut self) {
        if let Some(t) = self.cursor_target().filter(|t| t.is_dir) { self.new_tab(t.full); }
    }

    /// `Event::Open` and the launch argument: a folder opens in a new tab,
    /// a file in a new tab on its folder with the file selected.
    fn open_path(&mut self, arg: &str) {
        let path = arg.trim().trim_matches('/').to_string();
        if path.is_empty() || dir_exists(&path) {
            self.new_tab(path);
        } else if path_exists(&path) {
            let name = basename(&path).to_string();
            self.new_tab(parent_path(&path));
            self.select_name(&name);
        }
    }

    // ── Sidebar and bookmarks ─────────────────────────────────────────

    /// Places that exist, then the bookmarks, then the devices.
    fn rebuild_sidebar(&mut self) {
        let mut places = filter_sidebar_to_existing(default_sidebar(&self.home));
        let devices: Vec<Place> = places.iter().filter(|p| p.section == Section::Devices)
            .map(|p| Place { label: p.label.clone(), icon: p.icon, path: p.path.clone(), section: p.section })
            .collect();
        places.retain(|p| p.section == Section::Places);
        for b in &self.bookmarks {
            let label = if b.is_empty() { "/".to_string() } else { basename(b).to_string() };
            places.push(Place { label, icon: IconId::Folder, path: b.clone(), section: Section::Bookmarks });
        }
        places.extend(devices);
        self.sidebar = places;
        self.sync_sidebar_from_current();
    }

    /// Bookmark `path`, or say it is one already.
    fn add_bookmark(&mut self, path: String) {
        self.close_menus();
        let name = if path.is_empty() { "/".to_string() } else { basename(&path).to_string() };
        if !self.bookmarks.contains(&path) {
            self.bookmarks.push(path);
            self.save_settings();
            self.rebuild_sidebar();
        }
        self.say(fill(s().bm_added, &[&name]), false);
    }

    /// Ctrl+B / Go menu: toggle a bookmark on the current folder.
    fn toggle_bookmark_here(&mut self) {
        let here = self.current.clone();
        if let Some(i) = self.bookmarks.iter().position(|b| *b == here) {
            self.close_menus();
            self.bookmarks.remove(i);
            self.save_settings();
            self.rebuild_sidebar();
        } else {
            self.add_bookmark(here);
        }
    }

    /// Context menu on sidebar row `i`.
    fn sidebar_op(&mut self, i: usize, id: u32) {
        self.close_menus();
        let Some(path) = self.sidebar.get(i).map(|p| p.path.clone()) else { return };
        let bm = self.bookmarks.iter().position(|b| *b == path);
        match (id, bm) {
            (ACT_SB_OPEN_TAB, _) => self.new_tab(path),
            (ACT_SB_REMOVE, Some(k)) => { self.bookmarks.remove(k); }
            (ACT_SB_UP, Some(k)) if k > 0 => self.bookmarks.swap(k, k - 1),
            _ => return,
        }
        if id != ACT_SB_OPEN_TAB {
            self.save_settings();
            self.rebuild_sidebar();
        }
    }

    // ── View settings ─────────────────────────────────────────────────

    fn grid_cols(&self) -> usize { ZOOM_STEPS[self.zoom].0 }

    fn set_zoom(&mut self, delta: i32) {
        let z = (self.zoom as i32 + delta.signum()).clamp(0, ZOOM_STEPS.len() as i32 - 1) as usize;
        if z != self.zoom {
            self.zoom = z;
            self.save_settings();
        }
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.refilter();
        self.save_settings();
    }

    /// Sort from the View menu: a new column keeps its default direction,
    /// the active one stays as it is.
    fn sort_from_menu(&mut self, key: SortKey) {
        if self.sort_key != key { self.set_sort(key); }
    }

    fn set_direction(&mut self, asc: bool) {
        self.sort_asc = asc;
        sort_entries(&mut self.entries, self.sort_key, self.sort_asc);
        self.stats_queue = pending_folder_indices(&self.entries);
        self.refilter();
        self.save_settings();
    }

    /// Read `sys/config/loft`: `key: value` lines. Anything missing or
    /// unreadable keeps the default.
    fn load_settings(&mut self) {
        let Some(buf) = host::fetch(CONFIG_PATH) else { return };
        let text = String::from_utf8_lossy(&buf);
        for line in text.lines() {
            let Some((k, v)) = line.split_once(':') else { continue };
            let v = v.trim();
            match k.trim() {
                "view" => self.view_mode = if v == "grid" { ViewMode::Grid } else { ViewMode::List },
                "sort" => if let Some(key) = sort_key_from(v) { self.sort_key = key },
                "order" => self.sort_asc = v != "desc",
                "hidden" => self.show_hidden = v == "show",
                "zoom" => if let Ok(z) = v.parse::<usize>() { self.zoom = z.min(ZOOM_STEPS.len() - 1) },
                "bookmark" => if !v.is_empty() { self.bookmarks.push(v.to_string()) },
                "preview" => self.preview_on = v == "on",
                "preview_width" => if let Ok(w) = v.parse::<u16>() { self.preview_w = w.max(PREVIEW_W_MIN) },
                _ => {}
            }
        }
    }

    fn save_settings(&self) {
        let mut out = String::with_capacity(128);
        out.push_str("# loft view settings\n");
        out.push_str(if self.view_mode == ViewMode::Grid { "view: grid\n" } else { "view: list\n" });
        out.push_str("sort: ");
        out.push_str(sort_key_name(self.sort_key));
        out.push('\n');
        out.push_str(if self.sort_asc { "order: asc\n" } else { "order: desc\n" });
        out.push_str(if self.show_hidden { "hidden: show\n" } else { "hidden: hide\n" });
        out.push_str(if self.preview_on { "preview: on\n" } else { "preview: off\n" });
        out.push_str("preview_width: ");
        push_usize(&mut out, self.preview_w as usize);
        out.push('\n');
        out.push_str("zoom: ");
        push_usize(&mut out, self.zoom);
        out.push('\n');
        for b in &self.bookmarks {
            out.push_str("bookmark: ");
            out.push_str(b);
            out.push('\n');
        }
        if !host::store(CONFIG_PATH, out.as_bytes()) { log("[loft] could not save settings"); }
    }
}

// ── Render ────────────────────────────────────────────────────────────

fn render(lf: &Loft) -> Widget {
    let menu = render_menu_bar();
    let toolbar = render_toolbar(lf);
    // A dialog replaces the file area while it's up (same idiom as spell's
    // "save as" dialog), so its Input is the only editable field.
    let body = match &lf.dialog {
        Some(d) => render_dialog(lf, d),
        None => render_body(lf),
    };

    // Custom outer column instead of `prefab::panel`: panel's
    // Padding-Xs + Spacing-Md would keep the menu-bar bg from reaching
    // the window edges and put a 12 px gap between menu and divider.
    let mut children: Vec<Widget> = alloc::vec![menu, Widget::Divider];
    if lf.tabs.len() > 1 { children.push(render_tabbar(lf)); }
    children.extend([
        toolbar,
        Widget::Divider,
        body,                           // Modifier::Flex(1) — fills
        Widget::Divider,
        render_status_bar(lf),
    ]);

    // The open menu's dropdown floats below its label (NodeId anchor);
    // a click outside fires `on_dismiss`.
    if let Some(kind) = lf.open_menu {
        let (anchor_id, content) = render_dropdown(lf, kind);
        children.push(Widget::Popover {
            anchor:     NodeId(anchor_id),
            child:      alloc::boxed::Box::new(content),
            on_dismiss: ActionId(ACT_MENU_DISMISS),
            modifiers:  alloc::vec![],
        });
    } else if let Some(ctx) = &lf.ctx {
        // Opens where the right click landed.
        let mut m = MenuBuf::default();
        ctx_menu_items(lf, ctx, &mut m);
        children.push(Widget::Popover {
            anchor:     NodeId::POINTER,
            child:      alloc::boxed::Box::new(m.widget()),
            on_dismiss: ActionId(ACT_CTX_DISMISS),
            modifiers:  alloc::vec![],
        });
    }

    Widget::Column {
        children,
        spacing:   Spacing::None.as_u16(),
        align:     Align::Stretch,
        modifiers: alloc::vec![],
    }
}

/// One tab per open location, shown once there are two. The active tab
/// carries the × and the page colour; the + opens the current folder again.
fn render_tabbar(lf: &Loft) -> Widget {
    let mut tabs: Vec<Widget> = Vec::with_capacity(lf.tabs.len() + 1);
    for (i, t) in lf.tabs.iter().enumerate() {
        let active = i == lf.active;
        let path = if active { &lf.current } else { &t.current };
        let title = if path.is_empty() { "/".to_string() } else { basename(path).to_string() };
        let mut mods: Vec<Modifier> = alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::MinWidth(TAB_W),
            Modifier::MaxWidth(TAB_W),
            Modifier::MinHeight(TAB_H),
            Modifier::OnClick(ActionId(ACT_TAB_BASE + i as u32)),
            Modifier::Rounded(Radius::Md.as_u8()),
        ];
        if active {
            mods.push(Modifier::Background(Token::Page));
        } else {
            mods.push(Modifier::Hover(alloc::vec![
                Modifier::Background(Token::SurfaceMuted),
                Modifier::Rounded(Radius::Md.as_u8()),
            ]));
        }
        tabs.push(Widget::Row {
            children: alloc::vec![
                Widget::Icon { id: IconId::Folder, size: 16, modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceFaint)] },
                Widget::Text {
                    content:   title,
                    style:     TextStyle::Body,
                    modifiers: if active { alloc::vec![] } else { alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)] },
                },
                Widget::Spacer { flex: 1 },
                Widget::Icon {
                    id:   IconId::X,
                    size: 16,
                    modifiers: alloc::vec![
                        Modifier::OnClick(ActionId(ACT_TAB_CLOSE_BASE + i as u32)),
                        Modifier::Tint(Token::OnSurfaceMuted),
                    ],
                },
            ],
            spacing:   Spacing::Sm.as_u16(),
            align:     Align::Center,
            modifiers: mods,
        });
    }
    tabs.push(Widget::Text {
        content:   "+".to_string(),
        style:     TextStyle::Body,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::MinHeight(TAB_H),
            Modifier::Tint(Token::OnSurfaceFaint),
            Modifier::OnClick(ActionId(ACT_TAB_NEW)),
            Modifier::Rounded(Radius::Md.as_u8()),
            Modifier::Hover(alloc::vec![
                Modifier::Background(Token::SurfaceMuted),
                Modifier::Rounded(Radius::Md.as_u8()),
            ]),
        ],
    });
    Widget::Scroll {
        child: alloc::boxed::Box::new(Widget::Row {
            children:  tabs,
            spacing:   Spacing::Xxs.as_u16(),
            align:     Align::End,
            modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
        }),
        axis:      Axis::Horizontal,
        modifiers: alloc::vec![Modifier::Background(Token::SurfaceElevated)],
    }
}

/// Menu rows with their shortcut hints, built up conditionally.
#[derive(Default)]
struct MenuBuf {
    items: Vec<(String, ActionId)>,
    hints: Vec<&'static str>,
}

impl MenuBuf {
    fn add(&mut self, label: &str, action: u32, hint: &'static str) {
        self.items.push((label.to_string(), ActionId(action)));
        self.hints.push(hint);
    }

    fn widget(mut self) -> Widget {
        if self.items.is_empty() {
            // Keep the surface non-empty so the click still reads as handled.
            self.add(s().no_action, ACT_MENU_DISMISS, "");
        }
        prefab::popover_menu_shortcuts(&self.items, &self.hints, None)
    }
}

/// The operations on the current targets, for the Edit menu and the item
/// context menu. Only what applies here is listed.
fn target_items(lf: &Loft, m: &mut MenuBuf, with_open: bool) {
    let n = lf.targets().len();
    if n == 0 { return; }
    if lf.in_trash() {
        m.add(s().restore, ACT_EDIT_RESTORE, "");
        m.add(s().delete_perm, ACT_EDIT_DELETE, "Del");
    } else {
        if with_open { m.add(s().open, ACT_EDIT_OPEN, "Enter"); }
        let one = if n == 1 { lf.cursor_target() } else { None };
        match &one {
            Some(t) if t.is_dir => {
                m.add(s().open_tab, ACT_EDIT_OPEN_TAB, "");
                if !lf.bookmarks.contains(&t.full) { m.add(s().bm_item, ACT_EDIT_BOOKMARK, ""); }
            }
            Some(_) => m.add(s().open_with, ACT_EDIT_OPEN_WITH, ""),
            None => {}
        }
        m.add(s().copy, ACT_EDIT_COPY, "Ctrl+C");
        if !lf.in_trash_tree() { m.add(s().cut, ACT_EDIT_CUT, "Ctrl+X"); }
        if with_open && lf.can_paste() && !lf.in_trash_tree() {
            m.add(s().paste, ACT_EDIT_PASTE, "Ctrl+V");
        }
        if !lf.in_trash_tree() { m.add(s().duplicate, ACT_EDIT_DUPLICATE, "Ctrl+D"); }
        if !lf.in_trash_tree() { m.add(s().rename, ACT_EDIT_RENAME, "F2"); }
        if lf.in_trash_tree() {
            m.add(s().delete_perm, ACT_EDIT_DELETE, "Del");
        } else {
            m.add(s().move_to_trash, ACT_EDIT_TRASH, "Del");
        }
    }
    m.add(s().copy_path, ACT_EDIT_COPY_PATH, "");
    if n == 1 { m.add(s().properties, ACT_EDIT_PROPERTIES, ""); }
}

fn ctx_menu_items(lf: &Loft, ctx: &CtxMenu, m: &mut MenuBuf) {
    match ctx {
        CtxMenu::Sidebar(i) => {
            m.add(s().open_tab, ACT_SB_OPEN_TAB, "");
            if lf.sidebar.get(*i).is_some_and(|p| p.section == Section::Bookmarks) {
                m.add(s().bm_up, ACT_SB_UP, "");
                m.add(s().bm_remove, ACT_SB_REMOVE, "");
            }
        }
        CtxMenu::Item(_) => {
            target_items(lf, m, true);
            if lf.in_trash() { m.add(s().empty_trash, ACT_EDIT_EMPTY_TRASH, ""); }
        }
        CtxMenu::Background => {
            if !lf.in_trash_tree() {
                m.add(s().new_folder, ACT_EDIT_NEW_FOLDER, "Ctrl+Shift+N");
                m.add(s().new_file, ACT_EDIT_NEW_FILE, "");
                if lf.can_paste() { m.add(s().paste, ACT_EDIT_PASTE, "Ctrl+V"); }
            }
            m.add(s().select_all, ACT_EDIT_SELECT_ALL, "");
            m.add(s().refresh, ACT_EDIT_REFRESH, "F5");
            if lf.in_trash() { m.add(s().empty_trash, ACT_EDIT_EMPTY_TRASH, ""); }
        }
    }
}

fn edit_menu_items(lf: &Loft, m: &mut MenuBuf) {
    if !lf.undo.is_empty() { m.add(s().undo, ACT_EDIT_UNDO, "Ctrl+Z"); }
    m.add(s().select_all, ACT_EDIT_SELECT_ALL, "Ctrl+A");
    if !lf.in_trash_tree() {
        m.add(s().new_folder, ACT_EDIT_NEW_FOLDER, "Ctrl+Shift+N");
        if lf.can_paste() { m.add(s().paste, ACT_EDIT_PASTE, "Ctrl+V"); }
    }
    target_items(lf, m, false);
}

fn file_menu_items(lf: &Loft, m: &mut MenuBuf) {
    if !lf.in_trash_tree() {
        m.add(s().new_folder, ACT_EDIT_NEW_FOLDER, "Ctrl+Shift+N");
        m.add(s().new_file, ACT_EDIT_NEW_FILE, "");
    }
    m.add(s().new_tab, ACT_TAB_NEW, "Ctrl+T");
    m.add(s().refresh, ACT_EDIT_REFRESH, "F5");
    if lf.in_trash() { m.add(s().empty_trash, ACT_EDIT_EMPTY_TRASH, ""); }
    if lf.tabs.len() > 1 { m.add(s().close_tab, ACT_TAB_CLOSE_BASE + lf.active as u32, "Ctrl+W"); }
    m.add(s().quit, ACT_FILE_QUIT, "");
}

/// View menu: the active choices carry a dot in the hint column.
fn view_menu_items(lf: &Loft, m: &mut MenuBuf) {
    let on = |b: bool| if b { "•" } else { "" };
    m.add(s().view_grid, ACT_VIEW_GRID, on(lf.view_mode == ViewMode::Grid));
    m.add(s().view_list, ACT_VIEW_LIST, on(lf.view_mode == ViewMode::List));
    for (label, key, id) in [
        (s().sort_name, SortKey::Name, ACT_SORT_NAME),
        (s().sort_size, SortKey::Size, ACT_SORT_SIZE),
        (s().sort_files, SortKey::Files, ACT_SORT_FILES),
        (s().sort_type, SortKey::Type, ACT_SORT_TYPE),
        (s().sort_modified, SortKey::Modified, ACT_SORT_MTIME),
    ] {
        m.add(label, id, on(lf.sort_key == key));
    }
    m.add(s().sort_asc, ACT_SORT_ASC, on(lf.sort_asc));
    m.add(s().sort_desc, ACT_SORT_DESC, on(!lf.sort_asc));
    m.add(s().show_hidden, ACT_VIEW_HIDDEN, if lf.show_hidden { "• Ctrl+H" } else { "Ctrl+H" });
    m.add(s().preview_pane, ACT_VIEW_PREVIEW, if lf.preview_on { "• F9" } else { "F9" });
    if lf.view_mode == ViewMode::Grid {
        if lf.zoom + 1 < ZOOM_STEPS.len() { m.add(s().zoom_in, ACT_ZOOM_IN, "Ctrl+Wheel"); }
        if lf.zoom > 0 { m.add(s().zoom_out, ACT_ZOOM_OUT, "Ctrl+Wheel"); }
    }
}

/// Append `m` to a Row's or Column's modifiers; other widgets are left as is.
fn add_modifier(w: &mut Widget, m: Modifier) {
    if let Widget::Row { modifiers, .. } | Widget::Column { modifiers, .. } = w {
        modifiers.push(m);
    }
}

/// Count line, or the selection and its size, or a transient message.
fn render_status_bar(lf: &Loft) -> Widget {
    let (text, tint) = match (&lf.deleting, &lf.status) {
        (Some(job), _) => (fill(s().deleting, &[&num(job.todo.len())]), Token::OnSurface),
        (None, Some(st)) => (st.text.clone(), if st.error { Token::Danger } else { Token::OnSurface }),
        (None, None) => (selection_summary(lf), Token::OnSurfaceMuted),
    };
    Widget::Row {
        children: alloc::vec![
            Widget::Text { content: text, style: TextStyle::Caption, modifiers: alloc::vec![Modifier::Tint(tint)] },
            Widget::Spacer { flex: 1 },
        ],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::PaddingXY { x: 12, y: 4 },
            Modifier::MinHeight(STATUS_H),
            Modifier::Background(Token::SurfaceElevated),
        ],
    }
}

fn selection_summary(lf: &Loft) -> String {
    let n = lf.filtered.len();
    let k = lf.visible_marks();
    if k == 0 {
        return fill(if n == 1 { s().items_one } else { s().items_many }, &[&num(n)]);
    }
    let size: u64 = (0..n)
        .filter(|&ui| lf.is_marked(ui))
        .filter_map(|ui| lf.entry_at(ui))
        .map(|e| e.size)
        .fold(0, u64::saturating_add);
    fill(s().selected, &[&num(k), &num(n), &format_size(size)])
}

fn render_dialog(lf: &Loft, d: &Dialog) -> Widget {
    let card = match d {
        Dialog::Name(kind) => name_dialog(lf, kind),
        Dialog::ConfirmDelete(targets) => confirm_dialog(targets),
        Dialog::Properties(p) => properties_dialog(p),
        Dialog::OpenWith { target, apps, always } => open_with_dialog(target, apps, *always),
        Dialog::Clash { name } => clash_dialog(lf, name),
        Dialog::BatchRename(targets) => batch_rename_dialog(lf, targets),
    };
    Widget::Column {
        children:  alloc::vec![Widget::Spacer { flex: 1 }, card, Widget::Spacer { flex: 1 }],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Flex(1), Modifier::Padding(Padding::Lg.as_u16())],
    }
}

/// A name a file or folder can take: not empty, no path.
fn name_ok(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.contains('/') && n != "." && n != ".."
}

/// Cancel plus the main button, right-aligned. The main button is always
/// filled; until it can act (`ready`) it is disabled, not restyled. With
/// no main button the cancel one is the only way out and is filled itself.
fn dialog_buttons(primary: Option<(&str, prefab::ButtonStyle, bool)>, cancel: &str) -> Widget {
    let mut children = alloc::vec![Widget::Spacer { flex: 1 }];
    match primary {
        Some((label, style, ready)) => {
            children.push(prefab::button(cancel, prefab::ButtonStyle::Ghost, ActionId(ACT_DIALOG_CANCEL)));
            let mut main = prefab::button(label, style, ActionId(ACT_DIALOG_SUBMIT));
            if !ready {
                if let Widget::Button { modifiers, .. } = &mut main {
                    modifiers.push(Modifier::Disabled(alloc::vec![Modifier::Opacity(110)]));
                }
            }
            children.push(main);
        }
        None => children.push(prefab::button(cancel, prefab::ButtonStyle::Primary, ActionId(ACT_DIALOG_CANCEL))),
    }
    Widget::Row {
        children,
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![],
    }
}

fn dialog_body(children: Vec<Widget>) -> Widget {
    Widget::Column {
        children,
        spacing:   Spacing::Md.as_u16(),
        align:     Align::Stretch,
        modifiers: alloc::vec![],
    }
}

/// A framed field like the toolbar's (docs/spec/UI_REFRESH.md §3
/// `text_field`). `focus` raises a focus request with that number; 0 asks
/// for nothing.
fn dialog_field(value: &str, node: u32, focus: u32) -> Widget {
    let mut mods = alloc::vec![Modifier::NodeId(NodeId(node)), Modifier::Flex(1)];
    if focus != 0 { mods.push(Modifier::FocusRequest(focus)); }
    Widget::Row {
        children: alloc::vec![Widget::Input {
            value:       value.to_string(),
            placeholder: String::new(),
            on_submit:   ActionId(ACT_DIALOG_SUBMIT),
            modifiers:   mods,
        }],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::PaddingXY { x: 8, y: 0 },
            Modifier::MinHeight(FIELD_H),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
            Modifier::Focus(alloc::vec![
                Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() },
                Modifier::Ring { token: Token::AccentRing, width: 3 },
            ]),
        ],
    }
}

/// Rename / new folder / new file. The field has the focus as the dialog
/// opens; Enter commits, Esc cancels.
fn name_dialog(lf: &Loft, kind: &NameKind) -> Widget {
    let (title, label, ok) = match kind {
        NameKind::Rename { .. } => (s().rename_title, s().rename_label, s().confirm_rename),
        NameKind::NewFolder => (s().folder_title, s().folder_label, s().create),
        NameKind::NewFile => (s().file_title, s().file_label, s().create),
    };
    let body = dialog_body(alloc::vec![
        Widget::Text { content: label.to_string(), style: TextStyle::Muted, modifiers: alloc::vec![] },
        dialog_field(&lf.name_buf, NODE_NAME, lf.focus_seq),
        dialog_buttons(Some((ok, prefab::ButtonStyle::Primary, name_ok(&lf.name_buf))), s().cancel),
    ]);
    prefab::dialog(title, body, None, 360)
}

fn confirm_dialog(targets: &[Target]) -> Widget {
    let question = match targets {
        [one] => fill(s().delete_one, &[basename(&one.name)]),
        _ => fill(s().delete_many, &[&num(targets.len())]),
    };
    let body = dialog_body(alloc::vec![
        Widget::Text { content: question, style: TextStyle::Body, modifiers: alloc::vec![] },
        Widget::Text { content: s().delete_hint.to_string(), style: TextStyle::Muted, modifiers: alloc::vec![] },
        dialog_buttons(Some((s().delete, prefab::ButtonStyle::Destructive, true)), s().cancel),
    ]);
    prefab::dialog(s().delete_title, body, None, 360)
}

fn properties_dialog(p: &Props) -> Widget {
    let mut rows = alloc::vec![
        prop_row(s().props_name, &p.name),
        prop_row(s().props_type, &p.kind),
        prop_row(s().props_size, &p.size),
    ];
    if let Some(files) = &p.files { rows.push(prop_row(s().props_files, files)); }
    rows.push(prop_row(s().props_modified, &p.modified));
    rows.push(prop_row(s().props_location, &p.location));
    rows.push(dialog_buttons(None, s().close));
    prefab::dialog(s().props_title, dialog_body(rows), None, 360)
}

fn open_with_dialog(target: &Target, apps: &[(String, String)], always: bool) -> Widget {
    let mut rows: Vec<Widget> = alloc::vec![Widget::Text {
        content:   basename(&target.name).to_string(),
        style:     TextStyle::Muted,
        modifiers: alloc::vec![],
    }];
    if apps.is_empty() {
        rows.push(Widget::Text { content: s().ow_none.to_string(), style: TextStyle::Body, modifiers: alloc::vec![] });
    }
    for (i, (_, display)) in apps.iter().enumerate() {
        let id = ACT_OW_BASE + i as u32;
        if id >= ACT_OW_END { break; }
        rows.push(prefab::button(display, prefab::ButtonStyle::Secondary, ActionId(id)));
    }
    if let Some(ext) = extension(&target.name) {
        rows.push(Widget::Row {
            children: alloc::vec![
                Widget::Checkbox { value: always, on_toggle: ActionId(ACT_OW_ALWAYS), modifiers: alloc::vec![] },
                Widget::Text { content: fill(s().ow_always, &[&ext]), style: TextStyle::Body, modifiers: alloc::vec![] },
            ],
            spacing:   Spacing::Sm.as_u16(),
            align:     Align::Center,
            modifiers: alloc::vec![],
        });
    }
    rows.push(dialog_buttons(None, s().cancel));
    prefab::dialog(s().ow_title, dialog_body(rows), None, 360)
}

fn batch_rename_dialog(lf: &Loft, targets: &[Target]) -> Widget {
    let muted = |t: &str| Widget::Text { content: t.to_string(), style: TextStyle::Muted, modifiers: alloc::vec![] };
    let mut rows = alloc::vec![
        muted(s().br_find),
        dialog_field(&lf.find_buf, NODE_FIND, lf.focus_seq),
        muted(s().br_replace),
        dialog_field(&lf.repl_buf, NODE_REPL, 0),
    ];
    let names = lf.batch_names(targets);
    let changed: Vec<(&Target, &String)> = targets.iter().zip(names.iter())
        .filter_map(|(t, n)| n.as_ref().map(|n| (t, n)))
        .collect();
    for (t, n) in changed.iter().take(PREVIEW_ROWS) {
        rows.push(Widget::Text {
            content:   alloc::format!("{}  ->  {}", basename(&t.name), n),
            style:     TextStyle::Caption,
            modifiers: alloc::vec![],
        });
    }
    if changed.len() > PREVIEW_ROWS {
        rows.push(muted(&fill(s().br_more, &[&num(changed.len() - PREVIEW_ROWS)])));
    }
    let ready = !changed.is_empty();
    rows.push(dialog_buttons(Some((s().confirm_rename, prefab::ButtonStyle::Primary, ready)), s().cancel));
    prefab::dialog(&fill(s().br_title, &[&num(targets.len())]), dialog_body(rows), None, 420)
}

fn clash_dialog(lf: &Loft, name: &str) -> Widget {
    let all = lf.paste.as_ref().is_some_and(|j| j.apply_all);
    let buttons = Widget::Row {
        children: alloc::vec![
            prefab::button(s().cf_skip, prefab::ButtonStyle::Ghost, ActionId(ACT_CF_SKIP)),
            Widget::Spacer { flex: 1 },
            prefab::button(s().cf_replace, prefab::ButtonStyle::Secondary, ActionId(ACT_CF_REPLACE)),
            prefab::button(s().cf_keep, prefab::ButtonStyle::Primary, ActionId(ACT_CF_KEEP)),
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![],
    };
    let body = dialog_body(alloc::vec![
        Widget::Text { content: fill(s().cf_text, &[name]), style: TextStyle::Body, modifiers: alloc::vec![] },
        Widget::Row {
            children: alloc::vec![
                Widget::Checkbox { value: all, on_toggle: ActionId(ACT_CF_ALL), modifiers: alloc::vec![] },
                Widget::Text { content: s().cf_all.to_string(), style: TextStyle::Body, modifiers: alloc::vec![] },
            ],
            spacing:   Spacing::Sm.as_u16(),
            align:     Align::Center,
            modifiers: alloc::vec![],
        },
        Widget::Text { content: s().cf_hint.to_string(), style: TextStyle::Muted, modifiers: alloc::vec![] },
        buttons,
    ]);
    prefab::dialog(s().cf_title, body, None, 400)
}

fn prop_row(label: &str, value: &str) -> Widget {
    Widget::Row {
        children: alloc::vec![
            Widget::Text {
                content:   label.to_string(),
                style:     TextStyle::Muted,
                modifiers: alloc::vec![Modifier::MinWidth(PROP_LABEL_W)],
            },
            Widget::Text { content: value.to_string(), style: TextStyle::Body, modifiers: alloc::vec![] },
        ],
        spacing:   Spacing::Md.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![],
    }
}

fn render_menu_bar() -> Widget {
    let labels: Vec<(String, ActionId)> = alloc::vec![
        (s().menu_file.to_string(), ActionId(ACT_MENU_FILE)),
        (s().menu_edit.to_string(), ActionId(ACT_MENU_EDIT)),
        (s().menu_view.to_string(), ActionId(ACT_MENU_VIEW)),
        (s().menu_go.to_string(), ActionId(ACT_MENU_GO)),
        (s().menu_help.to_string(), ActionId(ACT_MENU_HELP)),
    ];
    let anchors: Vec<NodeId> = alloc::vec![
        NodeId(NODE_MENU_FILE),
        NodeId(NODE_MENU_EDIT),
        NodeId(NODE_MENU_VIEW),
        NodeId(NODE_MENU_GO),
        NodeId(NODE_MENU_HELP),
    ];
    prefab::menu_bar_with_icon(IconId::Folders, &labels, &anchors)
}

/// Build the dropdown for the currently-open menu. Returns
/// `(anchor_node_id, content_widget)` so the caller can wrap the
/// content in a `Widget::Popover` against the matching menu label.
fn render_dropdown(lf: &Loft, kind: OpenMenu) -> (u32, Widget) {
    match kind {
        OpenMenu::File => {
            let mut m = MenuBuf::default();
            file_menu_items(lf, &mut m);
            (NODE_MENU_FILE, m.widget())
        }
        OpenMenu::Edit => {
            let mut m = MenuBuf::default();
            edit_menu_items(lf, &mut m);
            (NODE_MENU_EDIT, m.widget())
        }
        OpenMenu::View => {
            let mut m = MenuBuf::default();
            view_menu_items(lf, &mut m);
            (NODE_MENU_VIEW, m.widget())
        }
        OpenMenu::Go => {
            let mut m = MenuBuf::default();
            m.add(s().go_home, ACT_GO_HOME, "");
            m.add(s().go_filesystem, ACT_GO_FILESYSTEM, "");
            m.add(s().go_location, ACT_GO_LOCATION, "Ctrl+L");
            let marked = lf.bookmarks.contains(&lf.current);
            m.add(if marked { s().bm_remove } else { s().bm_here }, ACT_GO_BOOKMARK, "Ctrl+B");
            (NODE_MENU_GO, m.widget())
        }
        OpenMenu::Help => (
            NODE_MENU_HELP,
            prefab::popover_menu(&[
                (s().about.to_string(), ActionId(ACT_HELP_ABOUT)),
            ], None),
        ),
    }
}

fn render_toolbar(lf: &Loft) -> Widget {
    let search = search_input(&lf.query);
    let mut children = alloc::vec![
        prefab::icon_button(IconId::ArrowLeft,      16, Some(ActionId(ACT_TOOLBAR_BACK)),    None),
        prefab::icon_button(IconId::ArrowRight,     16, Some(ActionId(ACT_TOOLBAR_FORWARD)), None),
        prefab::icon_button(IconId::ArrowUp,        16, Some(ActionId(ACT_TOOLBAR_UP)),      None),
        prefab::icon_button(IconId::ArrowClockwise, 16, Some(ActionId(ACT_TOOLBAR_REFRESH)), None),
        select_toggle(lf.select_mode),
    ];
    match &lf.location {
        Some(loc) => children.push(location_input(loc, lf.focus_seq)),
        None => {
            children.push(breadcrumb_for(&lf.current));
            children.push(Widget::Spacer { flex: 1 });
        }
    }
    children.push(search);
    Widget::Row {
        children,
        spacing: Spacing::Sm.as_u16(),
        align:   Align::Center,
        // Own padding now that the outer Column is flush — keeps
        // back/forward/breadcrumbs + search bar off the chrome.
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

/// The typed location, in the breadcrumb's place. Enter goes there.
fn location_input(value: &str, focus_seq: u32) -> Widget {
    Widget::Row {
        children: alloc::vec![Widget::Input {
            value:       value.to_string(),
            placeholder: s().location_hint.to_string(),
            on_submit:   ActionId(ACT_LOCATION_SUBMIT),
            modifiers:   alloc::vec![
                Modifier::NodeId(NodeId(NODE_LOCATION)),
                Modifier::FocusRequest(focus_seq),
                Modifier::Flex(1),
            ],
        }],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Flex(1),
            Modifier::PaddingXY { x: 8, y: 0 },
            Modifier::MinHeight(FIELD_H),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() },
        ],
    }
}

/// Toolbar toggle for select mode, filled while it is on.
fn select_toggle(on: bool) -> Widget {
    let button = prefab::icon_button(IconId::Check, 16, Some(ActionId(ACT_TOOLBAR_SELECT)), None);
    let mut mods = alloc::vec![Modifier::Rounded(7)];
    if on {
        mods.push(Modifier::Background(Token::AccentMuted));
        mods.push(Modifier::Tint(Token::Accent));
    }
    Widget::Row { children: alloc::vec![button], spacing: 0, align: Align::Center, modifiers: mods }
}

/// Hand-rolled search input with always-visible chrome — `prefab::input`
/// blends with the panel by design (drun's launcher look), but loft's
/// toolbar wants the search bar to read as a discrete, framed widget.
/// Same magnifier prefix + Heading text + focus-accent border, plus a
/// baseline `SurfaceMuted` fill and a `Border` stroke that's visible
/// without focus too.
fn search_input(query: &str) -> Widget {
    let raw = Widget::Input {
        value:       query.to_string(),
        placeholder: s().search.to_string(),
        on_submit:   prefab::NO_ACTION,
        modifiers:   alloc::vec![Modifier::NodeId(NodeId(NODE_SEARCH))],
    };
    Widget::Row {
        children: alloc::vec![
            Widget::Icon {
                id:        IconId::MagnifyingGlass,
                size:      16,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
            },
            raw,
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![
            // Asymmetric on purpose: uniform Padding ties the side air to the
            // row height, so a 30 px field would squeeze the magnifier against
            // the border. (The Input adds ~4 px of its own chrome, so x=8
            // reads as the design's 12.)
            Modifier::PaddingXY { x: 8, y: 0 },
            Modifier::MinHeight(FIELD_H),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
            Modifier::MinWidth(230),
            // 1 px accent border plus a 3 px ring, per the design's
            // `text_field` focus state (docs/spec/UI_REFRESH.md §3).
            Modifier::Focus(alloc::vec![
                Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() },
                Modifier::Ring { token: Token::AccentRing, width: 3 },
            ]),
        ],
    }
}

/// Disk fill level at the foot of the sidebar: a slim accent-filled
/// track plus the percentage. Hidden when the kernel reports no
/// mounted filesystem.
fn storage_meter() -> Option<Widget> {
    let packed = fs_usage();
    if packed < 0 { return None; }
    let used  = ((packed as u64) >> 32) as u64;
    let total = (packed as u64) & 0xFFFF_FFFF;
    if total == 0 { return None; }
    let pct = ((used.saturating_mul(100)) / total).min(100) as u16;

    // The track is a fixed-width bar; the fill is the same bar clipped
    // to `pct` of that width, laid over it in a Stack.
    const TRACK_W: u16 = 108;
    let fill_w = ((TRACK_W as u32 * pct as u32) / 100).max(1) as u16;
    let mut pct_str = String::with_capacity(5);
    push_usize(&mut pct_str, pct as usize);
    pct_str.push('%');

    Some(Widget::Row {
        children: alloc::vec![
            Widget::Stack {
                children: alloc::vec![
                    prefab::mark(TRACK_W, 4, Some(Token::Border)),
                    prefab::mark(fill_w,  4, Some(Token::Accent)),
                ],
                modifiers: Vec::new(),
            },
            Widget::Spacer { flex: 1 },
            Widget::Text {
                content:   pct_str,
                style:     TextStyle::Mono,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceFaint)],
            },
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::Background(Token::SurfaceHover),
            Modifier::Rounded(Radius::Sm.as_u8()),
        ],
    })
}

fn render_body(lf: &Loft) -> Widget {
    // Sidebar — Places (Home/Documents/Downloads/Music/Pictures/Videos/Projects)
    // + Devices (Filesystem/Trash). `nav_row` selected-state lights
    // up when the current dir matches a sidebar path verbatim.
    let mut places_rows: Vec<Widget> = Vec::new();
    let mut bookmark_rows: Vec<Widget> = Vec::new();
    let mut devices_rows: Vec<Widget> = Vec::new();
    for (i, p) in lf.sidebar.iter().enumerate() {
        let selected = lf.sidebar_sel == Some(i);
        let mut row = prefab::nav_row(
            p.icon, &p.label, selected,
            Some(ActionId(ACT_SIDEBAR_CLICK_BASE + i as u32)),
            None,
        );
        add_modifier(&mut row, Modifier::DropTarget(ActionId(ACT_DROP_SIDEBAR_BASE + i as u32)));
        match p.section {
            Section::Places => places_rows.push(row),
            Section::Bookmarks => bookmark_rows.push(row),
            Section::Devices => devices_rows.push(row),
        }
    }
    // Sections scroll; the capacity meter is a fixed footer below that
    // scroll, so it stays glued to the bottom edge at any window height.
    // Inside the scrolled column (or with `prefab::sidebar_pane`, which
    // appends its own trailing Spacer) it would float in the middle of
    // the leftover space.
    let mut section_list = alloc::vec![prefab::sidebar_section(s().sec_places, places_rows)];
    if !bookmark_rows.is_empty() {
        section_list.push(prefab::sidebar_section(s().sec_bookmarks, bookmark_rows));
    }
    section_list.push(prefab::sidebar_section(s().sec_devices, devices_rows));
    let sections = Widget::Column {
        children:  section_list,
        spacing:   Spacing::None.as_u16(),
        align:     Align::Stretch,
        modifiers: alloc::vec![],
    };
    let mut pane: Vec<Widget> = alloc::vec![
        Widget::Scroll {
            child:     alloc::boxed::Box::new(sections),
            axis:      Axis::Vertical,
            // Flex(1) is what pins the footer down: the scroll swallows
            // all leftover height instead of the meter drifting up.
            modifiers: alloc::vec![Modifier::Flex(1)],
        },
    ];
    if let Some(meter) = storage_meter() {
        pane.push(meter);
    }
    let sidebar = Widget::Column {
        children:  pane,
        spacing:   Spacing::Xs.as_u16(),
        align:     Align::Stretch,
        modifiers: alloc::vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::MinWidth(SIDEBAR_W),
            Modifier::MaxWidth(SIDEBAR_W),
        ],
    };

    // Content — filtered grid OR list, plus two empty states
    // (genuinely empty directory vs. nothing matched the search).
    let content: Widget = if lf.filtered.is_empty() {
        let hint = if lf.query.is_empty() {
            s().empty_dir
        } else {
            s().no_matches
        };
        prefab::empty_state(hint)
    } else {
        match lf.view_mode {
            ViewMode::Grid => render_grid(lf),
            ViewMode::List => render_list(lf),
        }
    };

    // Wrap the file area in a vertical Scroll so a long listing scrolls
    // (mouse wheel) and is clipped to the body instead of overflowing and
    // pushing the footer off-screen in a small (¼-screen) window. The
    // overlay scrollbar only shows when the content actually overflows.
    // OnClick on the scroll makes the empty area behind the items a click
    // target, so a right-click there opens the background menu.
    let content = Widget::Scroll {
        child:     alloc::boxed::Box::new(content),
        axis:      Axis::Vertical,
        modifiers: alloc::vec![
            Modifier::Flex(1),
            Modifier::OnClick(ActionId(ACT_BACKGROUND)),
            Modifier::DropTarget(ActionId(ACT_DROP_HERE)),
        ],
    };
    let content = Widget::Column {
        children: alloc::vec![
            Widget::Row { children: Vec::new(), spacing: 0, align: Align::Start, modifiers: Vec::new() },
            content,
        ],
        spacing:   0,
        align:     Align::Stretch,
        modifiers: alloc::vec![Modifier::Flex(1)],
    };

    let mut columns = alloc::vec![sidebar, content];
    if lf.preview_on {
        columns.push(Widget::Column {
            children:  Vec::new(),
            spacing:   0,
            align:     Align::Stretch,
            modifiers: alloc::vec![
                Modifier::MinWidth(GRIP_W),
                Modifier::MaxWidth(GRIP_W),
                Modifier::Background(Token::Border),
                Modifier::Hover(alloc::vec![Modifier::Background(Token::Accent)]),
                Modifier::OnClick(ActionId(ACT_PREVIEW_GRIP)),
            ],
        });
        columns.push(render_preview(lf));
    }
    Widget::Row {
        children: columns,
        spacing: 0,
        align:   Align::Stretch,
        // Flex(1) makes the body absorb all leftover vertical space in
        // the parent Column. Sidebar inherits via Stretch align so its
        // SurfaceMuted bg reaches the bottom regardless of grid content
        // height. Without this the body is intrinsic-sized and the bg
        // ends where its tallest child does.
        modifiers: alloc::vec![Modifier::Flex(1)],
    }
}

/// The preview pane: the picture or the start of a text file, then the
/// item's facts. Text scrolls both ways in the space the facts leave.
/// Filled off the idle loop (`pump_preview`).
fn render_preview(lf: &Loft) -> Widget {
    let text = |t: String, style| Widget::Text { content: t, style, modifiers: alloc::vec![] };
    let pad = Padding::Md.as_u16();
    let inner = lf.preview_w.saturating_sub(pad * 2);
    let mut rows: Vec<Widget> = Vec::new();
    match (lf.cursor.and_then(|c| lf.entry_at(c)), &lf.preview) {
        (Some(e), Some(p)) if p.path == join(&lf.current, &e.name) => {
            rows.push(text(basename(&e.name).to_string(), TextStyle::Heading));
            if p.image {
                rows.push(Widget::Canvas {
                    id: CanvasId(PREVIEW_CANVAS), width: inner, height: inner,
                    modifiers: alloc::vec![],
                });
            }
            if let Some(lines) = &p.text {
                let body: Vec<Widget> = lines.iter()
                    .map(|l| Widget::Text { content: l.clone(), style: TextStyle::Mono, modifiers: alloc::vec![] })
                    .collect();
                rows.push(Widget::Scroll {
                    child: alloc::boxed::Box::new(Widget::Column {
                        children:  body,
                        spacing:   0,
                        align:     Align::Start,
                        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
                    }),
                    axis:      Axis::Both,
                    modifiers: alloc::vec![
                        Modifier::Flex(1),
                        Modifier::Background(Token::Page),
                        Modifier::Rounded(Radius::Sm.as_u8()),
                    ],
                });
            }
            rows.push(prop_row(s().props_type, &type_for(e)));
            if !e.is_dir || !e.stats_pending {
                rows.push(prop_row(s().props_size, &format_size(e.size)));
            }
            rows.push(prop_row(s().props_modified, &format_mtime(e.mtime)));
        }
        _ => rows.push(text(s().preview_none.to_string(), TextStyle::Muted)),
    }
    Widget::Column {
        children:  rows,
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Stretch,
        modifiers: alloc::vec![
            Modifier::Padding(pad),
            Modifier::MinWidth(lf.preview_w),
            Modifier::MaxWidth(lf.preview_w),
            Modifier::Background(Token::SurfaceMuted),
        ],
    }
}

fn render_grid(lf: &Loft) -> Widget {
    // Search hits carry their sub-path in `name`, so the label reads
    // "wallpapers/aurora" and shows where the match lives.
    let source = lf.source();
    let icon_px = ZOOM_STEPS[lf.zoom].1;
    let grid_children: Vec<Widget> = lf.filtered.iter().enumerate().map(|(ui_idx, &entry_idx)| {
        let e = &source[entry_idx];
        let look = item_look(lf, ui_idx);
        let mut item = prefab::grid_item(
            icon_for(e), &e.name,
            look.highlighted,
            Some(ActionId(ACT_GRID_CLICK_BASE + ui_idx as u32)),
            None,
        );
        if let Widget::Column { children, .. } = &mut item {
            if let Some(first) = children.first_mut() {
                match lf.thumb_of(e) {
                    Some(id) => *first = Widget::Canvas {
                        id:        CanvasId(id),
                        width:     icon_px,
                        height:    icon_px,
                        modifiers: alloc::vec![],
                    },
                    None => if let Widget::Icon { size, .. } = first { *size = icon_px; },
                }
            }
        }
        look.apply(&mut item);
        if e.is_dir { add_modifier(&mut item, Modifier::DropTarget(ActionId(ACT_DROP_ITEM_BASE + ui_idx as u32))); }
        item
    }).collect();
    prefab::grid(grid_children, lf.grid_cols())
}

/// How one item is drawn: highlighted when marked (or, with nothing
/// marked, when it is the cursor), ringed when it is the cursor among
/// marks.
struct ItemLook {
    highlighted: bool,
    ring:        bool,
}

fn item_look(lf: &Loft, ui: usize) -> ItemLook {
    let is_cursor = lf.cursor == Some(ui);
    let marking = !lf.marks.is_empty() || lf.select_mode;
    ItemLook {
        highlighted: lf.is_marked(ui) || (!marking && is_cursor),
        ring:        marking && is_cursor,
    }
}

impl ItemLook {
    fn apply(&self, w: &mut Widget) {
        if self.ring { add_modifier(w, Modifier::Ring { token: Token::AccentRing, width: 2 }); }
    }
}

/// Detail-list view: one row per entry, columns Name | Size | Files |
/// Type | Modified, spanning the full window width (Name flexes to fill
/// the slack). Headers are clickable — a click sorts by that column,
/// clicking the active column flips direction (▲/▼ marker).
fn render_list(lf: &Loft) -> Widget {
    let source = lf.source();
    // Folder size/count is only computed for the browse listing; in
    // search mode (recursive source) folders show "—" rather than a
    // misleading zero.
    let browsing = lf.query.is_empty();
    let mut rows: Vec<Widget> = Vec::with_capacity(lf.filtered.len() + 2);
    rows.push(list_header_row(lf));
    rows.push(Widget::Divider);
    for (ui_idx, &entry_idx) in lf.filtered.iter().enumerate() {
        let e = &source[entry_idx];
        let look = item_look(lf, ui_idx);
        let mut row = list_data_row(
            e, look.highlighted, browsing,
            ActionId(ACT_GRID_CLICK_BASE + ui_idx as u32),
        );
        look.apply(&mut row);
        if e.is_dir { add_modifier(&mut row, Modifier::DropTarget(ActionId(ACT_DROP_ITEM_BASE + ui_idx as u32))); }
        rows.push(row);
    }
    Widget::Column {
        children: rows,
        spacing: 0,
        align:   Align::Stretch,
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

fn list_header_row(lf: &Loft) -> Widget {
    Widget::Row {
        children: alloc::vec![
            header_cell(lf, s().col_name,     SortKey::Name,     ACT_HEADER_NAME,  COL_NAME_W,  true),
            header_cell(lf, s().col_size,     SortKey::Size,     ACT_HEADER_SIZE,  COL_SIZE_W,  false),
            header_cell(lf, s().col_files,    SortKey::Files,    ACT_HEADER_FILES, COL_FILES_W, false),
            header_cell(lf, s().col_type,     SortKey::Type,     ACT_HEADER_TYPE,  COL_TYPE_W,  false),
            header_cell(lf, s().col_modified, SortKey::Modified, ACT_HEADER_MTIME, COL_MTIME_W, false),
        ],
        spacing: Spacing::Md.as_u16(),
        align:   Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::MinHeight(HEADER_ROW_H),
        ],
    }
}

/// One clickable column header. Mono + faint, so the header band reads
/// as structure rather than as another row of data (docs/spec/UI_REFRESH.md §5).
/// Appends a ↑/↓ marker on the active sort column; `flex` lets the Name
/// header grow to fill the row.
fn header_cell(lf: &Loft, label: &str, key: SortKey, action: u32,
               min_w: u16, flex: bool) -> Widget {
    let mut content = String::from(label);
    let active = lf.sort_key == key;
    if active {
        content.push(' ');
        content.push(if lf.sort_asc { '↑' } else { '↓' });
    }
    let mut mods: Vec<Modifier> = alloc::vec![
        Modifier::MinWidth(min_w),
        Modifier::Tint(if active { Token::OnSurfaceMuted } else { Token::OnSurfaceFaint }),
        Modifier::OnClick(ActionId(action)),
        Modifier::Hover(alloc::vec![
            Modifier::Tint(Token::OnSurface),
        ]),
    ];
    if flex { mods.push(Modifier::Flex(1)); }
    Widget::Text { content, style: TextStyle::Mono, modifiers: mods }
}

fn list_data_row(e: &Entry, selected: bool, browsing: bool, on_click: ActionId) -> Widget {
    let icon = icon_for(e);
    // Name cell with icon + label. Flex(1) so the column absorbs the
    // row's slack and the fixed columns sit flush against the right edge.
    let name_cell = Widget::Row {
        children: alloc::vec![
            Widget::Icon {
                id: icon,
                size: 16,
                modifiers: alloc::vec![Modifier::Tint(
                    if selected { Token::Accent } else { Token::OnSurfaceMuted },
                )],
            },
            // Flex so a narrow window shrinks the name, which then ends in
            // an ellipsis, instead of pushing it over the Size column.
            Widget::Text {
                content:   e.name.clone(),
                style:     TextStyle::Body,
                modifiers: alloc::vec![Modifier::Flex(1)],
            },
        ],
        spacing: Spacing::Sm.as_u16(),
        align:   Align::Center,
        modifiers: alloc::vec![Modifier::MinWidth(COL_NAME_W), Modifier::Flex(1)],
    };
    // Folders show their recursive byte sum + file count once scanned
    // ("…" while pending, "—" in search mode where we don't compute it);
    // files show their own size and "—" in the Files column.
    let size_str = if e.is_dir {
        if !browsing { "—".to_string() }
        else if e.stats_pending { "…".to_string() }
        else { format_size(e.size) }
    } else {
        format_size(e.size)
    };
    let files_str = if e.is_dir {
        if !browsing { "—".to_string() }
        else if e.stats_pending { "…".to_string() }
        else {
            let mut s = String::with_capacity(8);
            push_usize(&mut s, e.files as usize);
            s
        }
    } else {
        "—".to_string()
    };
    let type_str   = type_for(e);
    let mtime_str  = format_mtime(e.mtime);
    // Selection reads as a tint plus a 2 px accent edge on the leading
    // side — never a boxed-in row (docs/spec/UI_REFRESH.md §3 `list_row`). The
    // edge occupies its space on every row so nothing shifts sideways
    // when the selection moves.
    let mut row_mods: Vec<Modifier> = alloc::vec![
        Modifier::Padding(Padding::Xs.as_u16()),
        Modifier::MinHeight(DATA_ROW_H),
        Modifier::OnClick(on_click),
        Modifier::Hover(alloc::vec![
            Modifier::Background(Token::SurfaceHover),
        ]),
    ];
    if selected {
        row_mods.push(Modifier::Background(Token::AccentMuted));
    }
    let edge = prefab::mark(2, DATA_ROW_H - 2 * ROW_PAD,
        if selected { Some(Token::Accent) } else { None });

    Widget::Row {
        children: alloc::vec![
            edge,
            name_cell,
            list_cell_text(&size_str,  COL_SIZE_W),
            list_cell_text(&files_str, COL_FILES_W),
            list_cell_text(&type_str,  COL_TYPE_W),
            list_cell_text(&mtime_str, COL_MTIME_W),
        ],
        spacing: Spacing::Md.as_u16(),
        align:   Align::Center,
        modifiers: row_mods,
    }
}

/// A metadata column. Mono + faint so the eye runs down the file names
/// and only lands on the numbers when it goes looking for them.
fn list_cell_text(text: &str, min_w: u16) -> Widget {
    Widget::Text {
        content: text.to_string(),
        style:   TextStyle::Mono,
        modifiers: alloc::vec![
            Modifier::MinWidth(min_w),
            Modifier::Tint(Token::OnSurfaceMuted),
        ],
    }
}

/// Sidebar width from the design (docs/spec/UI_REFRESH.md §5).
const SIDEBAR_W:    u16 = 176;
const FIELD_H:      u16 = 30;
const HEADER_ROW_H: u16 = 30;
const DATA_ROW_H:   u16 = 34;
const STATUS_H:     u16 = 24;
const PROP_LABEL_W: u16 = 90;
/// Vertical inset a list row adds around its content. Subtracted from the
/// selection edge so edge + padding lands exactly on DATA_ROW_H.
const ROW_PAD:      u16 = 4;

const COL_NAME_W:  u16 = 240;   // min — flexes to fill the row
const COL_SIZE_W:  u16 = 110;
const COL_FILES_W: u16 = 90;
const COL_TYPE_W:  u16 = 120;
const COL_MTIME_W: u16 = 170;

fn breadcrumb_for(path: &str) -> Widget {
    let mut segs: Vec<(String, ActionId)> = Vec::new();
    let mut acc = String::new();
    if path.is_empty() {
        segs.push(("/".to_string(), ActionId(ACT_BREADCRUMB_BASE)));
    } else {
        for (i, part) in path.split('/').enumerate() {
            if part.is_empty() { continue; }
            if !acc.is_empty() { acc.push('/'); }
            acc.push_str(part);
            let _ = i;
            // Each segment fires the same action base + segment count
            // so the dispatcher can rebuild the prefix from the path.
            // Simpler than embedding the path bytes in the ActionId.
            segs.push((part.to_string(),
                       ActionId(ACT_BREADCRUMB_BASE + segs.len() as u32 + 1)));
        }
    }
    prefab::breadcrumb(&segs)
}

// ── Event dispatch ────────────────────────────────────────────────────

enum Outcome { Idle, Rerender, Exit }

fn handle(lf: &mut Loft, ev: Event) -> Outcome {
    // Shift/Ctrl apply to the one event that follows `Mods`.
    if let Event::Mods { shift, ctrl } = ev {
        lf.mods = (shift, ctrl);
        return Outcome::Idle;
    }
    let mods = core::mem::take(&mut lf.mods);
    if let Event::Focus(true) = ev {
        if lf.paste.is_none() { lf.load_clip(); }
        return Outcome::Rerender;
    }
    if lf.dialog.is_some() {
        // An open waits for the dialog; it would change the folder under it.
        if let Event::Open(arg) = ev {
            lf.pending_open.push(arg);
            return Outcome::Idle;
        }
        let out = handle_dialog(lf, ev);
        if lf.dialog.is_none() {
            for arg in core::mem::take(&mut lf.pending_open) { lf.open_path(&arg); }
        }
        return out;
    }
    let (shift, ctrl) = mods;
    match ev {
        Event::Key(KeyCode::Up) if shift => { lf.extend_marks(-lf.row_stride()); return Outcome::Rerender; }
        Event::Key(KeyCode::Down) if shift => { lf.extend_marks(lf.row_stride()); return Outcome::Rerender; }
        Event::Key(KeyCode::Left) if shift && lf.view_mode == ViewMode::Grid => {
            lf.extend_marks(-1);
            return Outcome::Rerender;
        }
        Event::Key(KeyCode::Right) if shift && lf.view_mode == ViewMode::Grid => {
            lf.extend_marks(1);
            return Outcome::Rerender;
        }
        Event::Action(ActionId(id @ ACT_GRID_CLICK_BASE..ACT_GRID_CLICK_END)) if shift || ctrl => {
            lf.modified_click((id - ACT_GRID_CLICK_BASE) as usize, shift, ctrl);
            return Outcome::Rerender;
        }
        _ => {}
    }

    match ev {
        Event::Key(KeyCode::Escape) => escape(lf),
        Event::Key(KeyCode::Up)        => { lf.select_delta_y(-1); Outcome::Rerender }
        Event::Key(KeyCode::Down)      => { lf.select_delta_y( 1); Outcome::Rerender }
        // A focused search Input consumes Left/Right; reaching us means the
        // focus is elsewhere.
        Event::Key(KeyCode::Left)      => { lf.select_delta_x(-1); Outcome::Rerender }
        Event::Key(KeyCode::Right)     => { lf.select_delta_x( 1); Outcome::Rerender }
        Event::Key(KeyCode::Home)      => { lf.cursor_to(0); Outcome::Rerender }
        Event::Key(KeyCode::End)       => { lf.cursor_to(usize::MAX); Outcome::Rerender }
        Event::Key(KeyCode::PageUp)    => { lf.select_delta_y(-PAGE_ROWS); Outcome::Rerender }
        Event::Key(KeyCode::PageDown)  => { lf.select_delta_y( PAGE_ROWS); Outcome::Rerender }
        Event::Key(KeyCode::Enter)     => { lf.open_cursor(); Outcome::Rerender }
        Event::Key(KeyCode::F(2))      => { if lf.idle_fs() { lf.open_rename(); } Outcome::Rerender }
        Event::Key(KeyCode::F(5))      => { lf.refresh(); Outcome::Rerender }
        Event::Key(KeyCode::F(9))      => { lf.toggle_preview(); Outcome::Rerender }
        // Some keyboard paths send Delete as the DEL byte.
        Event::Key(KeyCode::Delete | KeyCode::Char(0x7F)) => {
            if lf.idle_fs() { lf.delete_targets(); }
            Outcome::Rerender
        }
        // Backspace in a non-empty search is consumed by the editor;
        // reaching us means "go up" (Finder convention).
        Event::Key(KeyCode::Backspace) => {
            if lf.location.is_some() { return Outcome::Idle; }
            lf.go_up();
            Outcome::Rerender
        }
        Event::Key(KeyCode::Char(b' ')) => {
            if let Some(c) = lf.cursor { lf.toggle_mark(c); }
            Outcome::Rerender
        }
        // Ctrl+A as a control byte (keyboard paths that send one).
        Event::Key(KeyCode::Char(0x01)) => { lf.select_all(); Outcome::Rerender }
        // Printable keys reach the app only while no text field has focus.
        Event::Key(KeyCode::Char(b)) if b.is_ascii_graphic() => { lf.type_ahead(b); Outcome::Rerender }
        Event::Chord { letter: b'a', .. } => { lf.select_all(); Outcome::Rerender }
        Event::Chord { letter: b'l', .. } => { lf.open_location(); Outcome::Rerender }
        Event::Chord { letter: b't', .. } => { let here = lf.current.clone(); lf.new_tab(here); Outcome::Rerender }
        Event::Chord { letter: b'w', .. } => {
            if lf.close_tab(lf.active) { Outcome::Rerender } else { Outcome::Exit }
        }
        Event::Chord { letter: b'b', .. } => { lf.toggle_bookmark_here(); Outcome::Rerender }
        Event::Open(arg) => { lf.open_path(&arg); Outcome::Rerender }
        Event::Chord { letter: b'h', .. } => { lf.toggle_hidden(); Outcome::Rerender }
        Event::Chord { letter: b'd', .. } => { if lf.idle_fs() { lf.duplicate(); } Outcome::Rerender }
        Event::Chord { letter: b'z', shift: false, .. } => { if lf.idle_fs() { lf.undo_last(); } Outcome::Rerender }
        Event::Zoom { delta } => {
            if lf.view_mode != ViewMode::Grid { return Outcome::Idle; }
            lf.set_zoom(delta);
            Outcome::Rerender
        }
        Event::Chord { letter: b'n', shift: true, .. } => {
            if !lf.in_trash_tree() && lf.idle_fs() { lf.open_new_folder(); }
            Outcome::Rerender
        }
        // Ctrl+C / X / V, delivered because loft is a clipboard sink and
        // its file area is not a text widget.
        Event::Clipboard(ClipKind::Copy)  => { lf.arm_clipboard(ClipMode::Copy); Outcome::Rerender }
        Event::Clipboard(ClipKind::Cut)   => {
            if !lf.in_trash_tree() && lf.idle_fs() { lf.arm_clipboard(ClipMode::Cut); }
            Outcome::Rerender
        }
        Event::Clipboard(ClipKind::Paste) => {
            if !lf.in_trash_tree() && lf.idle_fs() { lf.do_paste(); }
            Outcome::Rerender
        }
        // Every field carries a NodeId; the plain InputChange that comes
        // first says nothing about which field it was.
        Event::InputChangeAt { node: NODE_SEARCH, value } => {
            // Past QUERY_CAP we hard-cap; the compositor reconciles on the
            // next round-trip. At a character boundary: typed text is UTF-8.
            lf.query.clear();
            lf.query.push_str(nopeek_widgets::rt::str_clamp(&value, QUERY_CAP));
            lf.refilter();
            Outcome::Rerender
        }
        Event::InputChangeAt { node: NODE_LOCATION, value } => {
            if lf.location.is_some() { lf.location = Some(value); }
            Outcome::Idle
        }
        Event::Action(ActionId(ACT_PREVIEW_GRIP)) => { lf.grip = Grip::Armed; Outcome::Idle }
        Event::MouseButton { button: MouseButton::Left, down: true, x, y } => {
            lf.grip = if lf.grip == Grip::Armed { Grip::Drag { x0: x, w0: lf.preview_w, scale: host::pixel_scale_milli().max(1) as i32 } } else { Grip::Idle };
            lf.press = core::mem::take(&mut lf.press_armed).then_some((x, y));
            Outcome::Idle
        }
        Event::MouseMove { x, y } => {
            if lf.drag_grip(x) || lf.drag_item(x, y) { Outcome::Rerender } else { Outcome::Idle }
        }
        Event::MouseButton { button: MouseButton::Left, down: false, .. } => {
            let resized = matches!(lf.grip, Grip::Drag { w0, .. } if w0 != lf.preview_w);
            lf.grip = Grip::Idle;
            if resized { lf.save_settings(); }
            lf.press = None;
            let mut out = Outcome::Idle;
            // A plain click on one of several marked items selects just it.
            if core::mem::take(&mut lf.press_keep) && !lf.dragging {
                lf.marks.clear();
                out = Outcome::Rerender;
            }
            if core::mem::take(&mut lf.dragging) {
                lf.status = None;
                out = Outcome::Rerender;
            }
            out
        }
        Event::Navigate { forward: false } => { lf.go_back(); Outcome::Rerender }
        Event::Navigate { forward: true } => { lf.go_forward(); Outcome::Rerender }
        Event::Drop { target: ActionId(id), paths, copy } => {
            let Some(dest) = lf.drop_dest(id) else { return Outcome::Idle };
            lf.drop_into(dest, paths, copy);
            Outcome::Rerender
        }
        Event::ContextAction(ActionId(id)) => handle_context(lf, id),
        Event::Action(ActionId(id)) => handle_action(lf, id),
        _ => Outcome::Idle,
    }
}

/// The cancel ladder: menus, then marks and select mode, then the search,
/// then the window.
fn escape(lf: &mut Loft) -> Outcome {
    if lf.deleting.is_some() {
        lf.stop_delete();
    } else if lf.ctx.is_some() || lf.open_menu.is_some() {
        lf.close_menus();
    } else if lf.location.is_some() {
        lf.location = None;
    } else if !lf.marks.is_empty() || lf.select_mode {
        lf.marks.clear();
        lf.select_mode = false;
    } else if !lf.query.is_empty() {
        lf.query.clear();
        lf.refilter();
    } else {
        return Outcome::Exit;
    }
    Outcome::Rerender
}

/// A dialog is modal: it owns Enter/Esc and its buttons, InputChange feeds
/// the name field, everything else is swallowed.
fn handle_dialog(lf: &mut Loft, ev: Event) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) | Event::Action(ActionId(ACT_DIALOG_CANCEL)) => {
            lf.dialog = None;
            lf.stop_paste();
            Outcome::Rerender
        }
        Event::Key(KeyCode::Enter) | Event::Action(ActionId(ACT_DIALOG_SUBMIT)) => {
            lf.submit_dialog();
            Outcome::Rerender
        }
        Event::InputChangeAt { node: NODE_NAME, value } if matches!(lf.dialog, Some(Dialog::Name(_))) => {
            lf.set_name_buf(&value);
            Outcome::Rerender
        }
        Event::InputChangeAt { node: NODE_FIND, value } => { lf.find_buf = value; Outcome::Rerender }
        Event::InputChangeAt { node: NODE_REPL, value } => { lf.repl_buf = value; Outcome::Rerender }
        Event::Action(ActionId(ACT_OW_ALWAYS)) => {
            if let Some(Dialog::OpenWith { always, .. }) = &mut lf.dialog { *always = !*always; }
            Outcome::Rerender
        }
        Event::Action(ActionId(id @ ACT_OW_BASE..ACT_OW_END)) => {
            lf.open_with_pick((id - ACT_OW_BASE) as usize);
            Outcome::Rerender
        }
        Event::Action(ActionId(ACT_CF_ALL)) => {
            if let Some(job) = &mut lf.paste { job.apply_all = !job.apply_all; }
            Outcome::Rerender
        }
        Event::Action(ActionId(id @ ACT_CF_REPLACE..=ACT_CF_SKIP)) => {
            let answer = match id {
                ACT_CF_REPLACE => Clash::Replace,
                ACT_CF_KEEP => Clash::Keep,
                _ => Clash::Skip,
            };
            lf.dialog = None;
            lf.run_paste(Some(answer));
            Outcome::Rerender
        }
        _ => Outcome::Idle,
    }
}

/// Right-click dispatch: an item, the empty file area, or outside an open
/// menu (which only closes it).
fn handle_context(lf: &mut Loft, id: u32) -> Outcome {
    match id {
        ACT_GRID_CLICK_BASE..ACT_GRID_CLICK_END => {
            let ui = (id - ACT_GRID_CLICK_BASE) as usize;
            if ui >= lf.filtered.len() { return Outcome::Idle; }
            lf.open_item_menu(ui);
        }
        ACT_BACKGROUND => {
            lf.close_menus();
            lf.ctx = Some(CtxMenu::Background);
        }
        ACT_SIDEBAR_CLICK_BASE..ACT_SIDEBAR_CLICK_END => {
            let i = (id - ACT_SIDEBAR_CLICK_BASE) as usize;
            if i >= lf.sidebar.len() { return Outcome::Idle; }
            lf.close_menus();
            lf.ctx = Some(CtxMenu::Sidebar(i));
        }
        ACT_CTX_DISMISS | ACT_MENU_DISMISS => lf.close_menus(),
        _ => return Outcome::Idle,
    }
    Outcome::Rerender
}

/// Menu and context-menu file operations. Each closes the menu it came from.
fn handle_file_op(lf: &mut Loft, id: u32) {
    lf.close_menus();
    match id {
        ACT_EDIT_OPEN        => lf.open_cursor(),
        ACT_EDIT_COPY        => lf.arm_clipboard(ClipMode::Copy),
        ACT_EDIT_CUT         => lf.arm_clipboard(ClipMode::Cut),
        ACT_EDIT_PASTE       => lf.do_paste(),
        ACT_EDIT_RENAME      => lf.open_rename(),
        ACT_EDIT_TRASH       => lf.delete_targets(),
        ACT_EDIT_DELETE      => lf.request_delete_permanently(),
        ACT_EDIT_RESTORE     => lf.restore_targets(),
        ACT_EDIT_EMPTY_TRASH => lf.request_empty_trash(),
        ACT_EDIT_NEW_FOLDER  => lf.open_new_folder(),
        ACT_EDIT_SELECT_ALL  => lf.select_all(),
        ACT_EDIT_COPY_PATH   => lf.copy_path(),
        ACT_EDIT_PROPERTIES  => lf.open_properties(),
        ACT_EDIT_REFRESH     => lf.refresh(),
        ACT_EDIT_OPEN_WITH   => lf.open_with(),
        ACT_EDIT_NEW_FILE    => lf.open_new_file(),
        ACT_EDIT_DUPLICATE   => lf.duplicate(),
        ACT_EDIT_UNDO        => lf.undo_last(),
        ACT_EDIT_OPEN_TAB    => lf.open_cursor_in_tab(),
        ACT_EDIT_BOOKMARK    => {
            if let Some(t) = lf.cursor_target().filter(|t| t.is_dir) { lf.add_bookmark(t.full); }
        }
        _ => {}
    }
}

fn handle_action(lf: &mut Loft, id: u32) -> Outcome {
    if (ACT_EDIT_COPY..=ACT_EDIT_BOOKMARK).contains(&id) {
        // Changing files while a delete runs could race it; looking is fine.
        let looks = matches!(id, ACT_EDIT_OPEN | ACT_EDIT_SELECT_ALL | ACT_EDIT_COPY_PATH
            | ACT_EDIT_PROPERTIES | ACT_EDIT_REFRESH | ACT_EDIT_OPEN_WITH | ACT_EDIT_COPY
            | ACT_EDIT_OPEN_TAB | ACT_EDIT_BOOKMARK);
        if looks || lf.idle_fs() { handle_file_op(lf, id); }
        return Outcome::Rerender;
    }
    match id {
        ACT_TOOLBAR_BACK    => { lf.go_back();    Outcome::Rerender }
        ACT_TOOLBAR_FORWARD => { lf.go_forward(); Outcome::Rerender }
        ACT_TOOLBAR_UP      => { lf.go_up();      Outcome::Rerender }
        ACT_TOOLBAR_REFRESH => { lf.refresh();    Outcome::Rerender }
        ACT_TOOLBAR_SELECT  => { lf.select_mode = !lf.select_mode; Outcome::Rerender }
        // Menu-bar labels: toggle the matching dropdown. Clicking the
        // already-open menu's label re-fires this and closes it
        // (matches macOS / Files behavior). Clicking a different menu
        // switches dropdowns directly.
        ACT_MENU_FILE => { lf.ctx = None; lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::File); Outcome::Rerender }
        ACT_MENU_EDIT => { lf.ctx = None; lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::Edit); Outcome::Rerender }
        ACT_MENU_VIEW => { lf.ctx = None; lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::View); Outcome::Rerender }
        ACT_MENU_GO   => { lf.ctx = None; lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::Go);   Outcome::Rerender }
        ACT_MENU_HELP => { lf.ctx = None; lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::Help); Outcome::Rerender }
        // Click outside a popover, or on the "(no action)" placeholder.
        ACT_MENU_DISMISS | ACT_CTX_DISMISS => {
            if lf.open_menu.is_none() && lf.ctx.is_none() { return Outcome::Idle; }
            lf.close_menus();
            Outcome::Rerender
        }
        // Dropdown items.
        ACT_FILE_QUIT => Outcome::Exit,
        ACT_VIEW_GRID | ACT_VIEW_LIST => {
            lf.view_mode = if id == ACT_VIEW_GRID { ViewMode::Grid } else { ViewMode::List };
            lf.open_menu = None;
            lf.sync_thumbs();
            lf.save_settings();
            Outcome::Rerender
        }
        ACT_SORT_NAME..=ACT_SORT_MTIME => {
            lf.open_menu = None;
            lf.sort_from_menu(match id {
                ACT_SORT_NAME => SortKey::Name,
                ACT_SORT_SIZE => SortKey::Size,
                ACT_SORT_FILES => SortKey::Files,
                ACT_SORT_TYPE => SortKey::Type,
                _ => SortKey::Modified,
            });
            Outcome::Rerender
        }
        ACT_SORT_ASC | ACT_SORT_DESC => {
            lf.open_menu = None;
            lf.set_direction(id == ACT_SORT_ASC);
            Outcome::Rerender
        }
        ACT_VIEW_HIDDEN => { lf.open_menu = None; lf.toggle_hidden(); Outcome::Rerender }
        ACT_VIEW_PREVIEW => { lf.open_menu = None; lf.toggle_preview(); Outcome::Rerender }
        ACT_ZOOM_IN | ACT_ZOOM_OUT => {
            lf.set_zoom(if id == ACT_ZOOM_IN { 1 } else { -1 });
            Outcome::Rerender
        }
        ACT_GO_LOCATION => { lf.open_location(); Outcome::Rerender }
        ACT_GO_BOOKMARK => { lf.toggle_bookmark_here(); Outcome::Rerender }
        ACT_TAB_NEW => { let here = lf.current.clone(); lf.new_tab(here); Outcome::Rerender }
        ACT_TAB_BASE..ACT_TAB_CLOSE_BASE => {
            lf.close_menus();
            lf.switch_tab((id - ACT_TAB_BASE) as usize);
            Outcome::Rerender
        }
        ACT_TAB_CLOSE_BASE..ACT_TAB_CLOSE_END => {
            lf.close_menus();
            if lf.close_tab((id - ACT_TAB_CLOSE_BASE) as usize) { Outcome::Rerender } else { Outcome::Exit }
        }
        ACT_SB_OPEN_TAB..=ACT_SB_UP => {
            if let Some(CtxMenu::Sidebar(i)) = lf.ctx.clone() { lf.sidebar_op(i, id); }
            lf.close_menus();
            Outcome::Rerender
        }
        ACT_LOCATION_SUBMIT => { lf.submit_location(); Outcome::Rerender }
        ACT_GO_HOME => {
            lf.open_menu = None;
            let home = lf.home.clone();
            lf.navigate(home);
            Outcome::Rerender
        }
        ACT_GO_FILESYSTEM => {
            lf.open_menu = None;
            lf.navigate(String::new());
            Outcome::Rerender
        }
        ACT_HELP_ABOUT => {
            lf.open_menu = None;
            lf.say(alloc::format!("loft {}", env!("CARGO_PKG_VERSION")), false);
            Outcome::Rerender
        }
        // Left click on the empty file area clears the marks.
        ACT_BACKGROUND => {
            lf.close_menus();
            if !lf.select_mode { lf.marks.clear(); }
            Outcome::Rerender
        }
        // Column-header clicks → sort / toggle direction.
        ACT_HEADER_NAME  => { lf.set_sort(SortKey::Name);     Outcome::Rerender }
        ACT_HEADER_SIZE  => { lf.set_sort(SortKey::Size);     Outcome::Rerender }
        ACT_HEADER_FILES => { lf.set_sort(SortKey::Files);    Outcome::Rerender }
        ACT_HEADER_TYPE  => { lf.set_sort(SortKey::Type);     Outcome::Rerender }
        ACT_HEADER_MTIME => { lf.set_sort(SortKey::Modified); Outcome::Rerender }
        ACT_GRID_CLICK_BASE..ACT_GRID_CLICK_END => {
            // Not implemented: Shift/Ctrl+click ranges; clicks carry no modifier state.
            lf.click_item((id - ACT_GRID_CLICK_BASE) as usize);
            Outcome::Rerender
        }
        ACT_BREADCRUMB_BASE..ACT_TOOLBAR_BACK => {
            let n = (id - ACT_BREADCRUMB_BASE) as usize;
            let target = take_first_segments(&lf.current, n);
            if target == lf.current { return Outcome::Idle; }
            lf.navigate(target);
            Outcome::Rerender
        }
        ACT_SIDEBAR_CLICK_BASE..ACT_SIDEBAR_CLICK_END => {
            let Some(path) = lf.sidebar.get((id - ACT_SIDEBAR_CLICK_BASE) as usize)
                .map(|p| p.path.clone()) else { return Outcome::Idle };
            lf.navigate(path);
            Outcome::Rerender
        }
        _ => Outcome::Idle,
    }
}

// ── Sidebar helpers ───────────────────────────────────────────────────

fn default_sidebar(home: &str) -> Vec<Place> {
    use Section::{Devices, Places};
    let place = |label: &str, icon, path: String, section| Place { label: label.into(), icon, path, section };
    alloc::vec![
        place(s().place_home,      IconId::Home,       home.into(),                         Places),
        place(s().place_documents, IconId::FileText,   alloc::format!("{}/documents", home), Places),
        place(s().place_downloads, IconId::Download,   alloc::format!("{}/downloads", home), Places),
        place(s().place_music,     IconId::MusicNotes, alloc::format!("{}/music",     home), Places),
        place(s().place_pictures,  IconId::Image,      alloc::format!("{}/pictures",  home), Places),
        place(s().place_videos,    IconId::PlayCircle, alloc::format!("{}/videos",    home), Places),
        place(s().place_projects,  IconId::Folders,    alloc::format!("{}/projects",  home), Places),
        place(s().go_filesystem,   IconId::HardDrives, String::new(),                        Devices),
        place(s().place_trash,     IconId::Trash,      join(home, TRASH_DIR),                Devices),
    ]
}

/// Click on a menu-bar label: open it if no menu was open or a
/// different one was, close it if the same one was already open.
fn toggle_menu(current: Option<OpenMenu>, target: OpenMenu) -> Option<OpenMenu> {
    match current {
        Some(c) if c == target => None,
        _                       => Some(target),
    }
}

// ── Kernel-side calls ─────────────────────────────────────────────────

/// Parse `sys/config/associations` (optional) into (ext, app) pairs.
/// One mapping per line: `ext=app` (`#` comments + blanks skipped).
/// Absent file → empty (loft falls back to built-in defaults).
fn load_associations() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let key = "sys/config/associations";
    let Some(buf) = host::fetch(key) else { return out };
    if let Ok(s) = core::str::from_utf8(&buf) {
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            if let Some((ext, app)) = line.split_once('=') {
                let ext = ext.trim().to_ascii_lowercase();
                let app = app.trim().to_string();
                if !ext.is_empty() && !app.is_empty() { out.push((ext, app)); }
            }
        }
    }
    out
}

fn read_home_dir() -> String {
    // The username lives in the single encrypted `.system/config` blob,
    // not a fetchable `sys/config/name` object — ask the kernel for the
    // resolved home dir directly.
    match host::home_dir() {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => String::from("home"),
    }
}

/// Cheap order-sensitive signature of a directory listing — folds count,
/// names, sizes and is_dir of every entry (FNV-1a). Changes when a file
/// is added, removed, renamed or resized. Used for auto-refresh.
fn dir_signature(entries: &[Entry]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    h = (h ^ entries.len() as u64).wrapping_mul(0x0000_0100_0000_01b3);
    for e in entries {
        for &b in e.name.as_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h = (h ^ e.size).wrapping_mul(0x0000_0100_0000_01b3);
        h = (h ^ e.is_dir as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Listing of `prefix`; with `recursive` every descendant, its `name` the
/// sub-path under `prefix` (e.g. "wallpapers/aurora"), so a search hit
/// shows where the match lives.
fn list_dir_internal(prefix: &str, recursive: bool) -> Vec<Entry> {
    let Some(listing) = host::fs_list(prefix, recursive) else { return Vec::new() };
    let mut out: Vec<Entry> = Vec::new();
    for e in nopeek_widgets::fs::list_entries(&listing) {
        out.push(Entry {
            name:          e.name.to_string(),
            name_lc:       e.name.to_ascii_lowercase(),
            size:          e.size,
            is_dir:        e.is_dir,
            files:         0,
            stats_pending: false,
            mtime:         e.mtime,
        });
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    out
}

// ── Folder size/count + sorting ───────────────────────────────────────

/// Reset every folder row to "pending" so `pump_stats` will scan it.
/// Order-independent (just flips flags), so it's safe to call before the
/// sort. Files are left untouched (they already carry their own size).
fn init_folder_stats(entries: &mut [Entry]) {
    for e in entries.iter_mut() {
        if e.is_dir {
            e.size = 0;
            e.files = 0;
            e.stats_pending = true;
        }
    }
}

/// Indices of folders still flagged pending, in current (sorted) order.
fn pending_folder_indices(entries: &[Entry]) -> Vec<usize> {
    entries.iter().enumerate()
        .filter(|(_, e)| e.is_dir && e.stats_pending)
        .map(|(i, _)| i)
        .collect()
}

/// Recursive (size, file_count) of a single folder. One
/// `npk_fs_list(recursive=1)` scan, summed inline without allocating an
/// `Entry` per descendant — cheap even for large subtrees, and called one
/// folder at a time off the idle loop so no single scan stalls the app.
fn scan_folder_stats(path: &str) -> (u64, u64) {
    let Some(listing) = host::fs_list(path, true) else { return (0, 0) };
    let (mut bytes, mut files) = (0u64, 0u64);
    for e in nopeek_widgets::fs::list_entries(&listing) {
        if e.is_dir { continue; }                // directory → count files only
        bytes = bytes.saturating_add(e.size);
        files += 1;
    }
    (bytes, files)
}

/// Sort the browse listing: folders always grouped before files
/// (Thunar/Files idiom), then ordered within each group by `key`,
/// reversed for descending.
fn sort_entries(v: &mut [Entry], key: SortKey, asc: bool) {
    v.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => return core::cmp::Ordering::Less,
            (false, true) => return core::cmp::Ordering::Greater,
            _ => {}
        }
        let o = cmp_entries(a, b, key);
        if asc { o } else { o.reverse() }
    });
}

fn cmp_entries(a: &Entry, b: &Entry, key: SortKey) -> core::cmp::Ordering {
    let tie = a.name_lc.cmp(&b.name_lc);
    match key {
        SortKey::Name     => tie,
        SortKey::Size     => a.size.cmp(&b.size).then(tie),
        SortKey::Files    => a.files.cmp(&b.files).then(tie),
        SortKey::Type     => type_for(a).cmp(&type_for(b)).then(tie),
        SortKey::Modified => a.mtime.cmp(&b.mtime).then(tie),
    }
}

/// Drop sidebar entries whose path is not currently backed by a
/// `.dir` marker. Keeps "Filesystem" (empty path = npkFS root) — it
/// always exists by definition. Honest UI: if you can see it, you
/// can navigate into it without hitting an empty phantom.
fn filter_sidebar_to_existing(places: Vec<Place>) -> Vec<Place> {
    places.into_iter().filter(|p| {
        if p.path.is_empty() { return true; } // Filesystem root
        dir_exists(&p.path)
    }).collect()
}

fn dir_exists(path: &str) -> bool {
    host::fs_stat(path).is_some_and(|(_, is_dir, _)| is_dir)
}

// ── Path helpers for file operations ──────────────────────────────────

/// Join a directory path with a child name. npkFS uses slash paths and
/// the filesystem root is the empty string, so a join off root omits the
/// leading slash.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { alloc::format!("{}/{}", dir, name) }
}

/// Final path component of a (possibly relative, search-mode) name.
fn basename(name: &str) -> &str {
    match name.rsplit_once('/') {
        Some((_, b)) => b,
        None => name,
    }
}

/// True if any object (file or directory) exists at `path`.
fn path_exists(path: &str) -> bool {
    host::fs_stat(path).is_some()
}

/// Split a file name into (stem, extension-with-dot): "a.txt" → ("a",
/// ".txt"); "README" → ("README", ""). A leading dot (dotfile) stays in
/// the stem so the copy suffix lands before any real extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// A collision-free full path for `name` inside `dir`: "a.txt", then
/// "a (2).txt", "a (3).txt"… A folder name has no extension to keep.
/// Capped so a pathological directory can't spin.
fn unique_in(dir: &str, name: &str, is_dir: bool) -> String {
    let base = join(dir, name);
    if !path_exists(&base) { return base; }
    let (stem, ext) = if is_dir { (name, "") } else { split_ext(name) };
    let mut n = 2u32;
    loop {
        let cand = join(dir, &alloc::format!("{} ({}){}", stem, n, ext));
        if !path_exists(&cand) || n >= 999 { return cand; }
        n += 1;
    }
}

/// A dotfile, or anything inside a dot folder (search names are sub-paths).
fn is_hidden(name: &str) -> bool {
    name.split('/').any(|c| c.starts_with('.'))
}

/// Lowercased extension of a file name, without the dot.
/// The first lines of `path` if its start reads as text (no NUL byte),
/// whatever its name says.
fn text_head(path: &str) -> Option<Vec<String>> {
    let f = host::File::open(path)?;
    let mut buf = alloc::vec![0u8; PREVIEW_BYTES.min(f.size() as usize)];
    if buf.is_empty() { return Some(Vec::new()); }
    let n = f.read_at(0, &mut buf)?;
    let head = &buf[..n];
    if head.contains(&0) { return None; }
    let text = String::from_utf8_lossy(head);
    Some(text.lines().take(PREVIEW_LINES).map(|l| l.replace('\t', "    ")).collect())
}

/// Only plain `[a-z0-9]{1,16}` extensions count: the extension is written
/// into `sys/config/associations` as `ext=app`, and a name may hold `=`
/// or a newline.
fn extension(name: &str) -> Option<String> {
    match basename(name).rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() && (1..=16).contains(&e.len())
            && e.bytes().all(|b| b.is_ascii_alphanumeric()) => Some(e.to_ascii_lowercase()),
        _ => None,
    }
}

fn sort_key_name(k: SortKey) -> &'static str {
    match k {
        SortKey::Name => "name", SortKey::Size => "size", SortKey::Files => "files",
        SortKey::Type => "type", SortKey::Modified => "modified",
    }
}

fn sort_key_from(v: &str) -> Option<SortKey> {
    Some(match v {
        "name" => SortKey::Name, "size" => SortKey::Size, "files" => SortKey::Files,
        "type" => SortKey::Type, "modified" => SortKey::Modified,
        _ => return None,
    })
}

/// True if `path` lies inside directory `dir` (not `dir` itself).
fn is_below(path: &str, dir: &str) -> bool {
    if dir.is_empty() { return !path.is_empty(); }
    path.len() > dir.len() && path.starts_with(dir) && path.as_bytes()[dir.len()] == b'/'
}

/// Parse the trash index. Lines that are not three tab-separated fields
/// with a plain name and a non-empty origin are skipped.
fn parse_trash_index(bytes: &[u8]) -> Vec<TrashRecord> {
    let text = String::from_utf8_lossy(bytes);
    text.lines().filter_map(|line| {
        let mut f = line.split('\t');
        let (name, origin, time) = (f.next()?, f.next()?, f.next()?);
        if name.is_empty() || name.contains('/') || origin.is_empty() || f.next().is_some() {
            return None;
        }
        Some(TrashRecord {
            name:   name.to_string(),
            origin: origin.to_string(),
            time:   time.trim().parse().unwrap_or(0),
        })
    }).collect()
}

// ── Path helpers ──────────────────────────────────────────────────────

fn parent_path(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn take_first_segments(path: &str, n: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for part in path.split('/') {
        if part.is_empty() { continue; }
        if count >= n { break; }
        if !out.is_empty() { out.push('/'); }
        out.push_str(part);
        count += 1;
    }
    out
}

// ── Icon + type label ─────────────────────────────────────────────────

/// Human-readable type column for the list view. Mirrors the
/// `icon_for` taxonomy so the icon and the label always agree.
fn type_for(e: &Entry) -> String {
    if e.is_dir { return "Folder".to_string(); }
    let ext = e.name.rsplit('.').next().unwrap_or("");
    match ext {
        "md" | "txt" | "log" | "cfg" | "toml" | "json" | "yaml" | "yml" => "Text".to_string(),
        "rs" | "wasm" | "sh" | "py" | "c" | "h" | "hpp" | "cpp" | "go"  => "Code".to_string(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg"          => "Image".to_string(),
        ""    => "File".to_string(),
        other => alloc::format!("{} File", other.to_uppercase()),
    }
}

/// Render a Unix-second timestamp as "YYYY-MM-DD HH:MM" UTC. Zero
/// → "—" (mtime unknown — RTC was unreadable when the entry was
/// written). No std::time, no chrono — pure integer math against the
/// proleptic Gregorian calendar, matching what
/// `kernel/src/drivers/rtc.rs::datetime_to_unix` reverses.
fn format_mtime(secs: u64) -> String {
    if secs == 0 { return "—".to_string(); }
    let (y, mo, d, h, mi, _s) = unix_to_civil(secs);
    let mut s = String::with_capacity(16);
    push_zpad(&mut s, y as u64, 4); s.push('-');
    push_zpad(&mut s, mo as u64, 2); s.push('-');
    push_zpad(&mut s, d as u64, 2); s.push(' ');
    push_zpad(&mut s, h as u64, 2); s.push(':');
    push_zpad(&mut s, mi as u64, 2);
    s
}

/// `Howard Hinnant`-style civil_from_days. Converts Unix seconds to
/// (year, month [1..=12], day [1..=31], hour, minute, second) in UTC
/// without leap-second awareness (good enough for "modified" UI).
fn unix_to_civil(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem  = (secs % 86_400) as u32;
    let h    = rem / 3600;
    let mi   = (rem % 3600) / 60;
    let s    = rem % 60;

    // Shift epoch to 0000-03-01 to make leap math simple.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y_int = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp  = (5 * doy + 2) / 153;
    let d   = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo  = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y   = (y_int + if mo <= 2 { 1 } else { 0 }) as i32;
    (y, mo, d, h, mi, s)
}

fn push_zpad(s: &mut String, mut n: u64, width: usize) {
    let mut buf = [0u8; 20];
    let mut i = 0;
    if n == 0 { buf[0] = b'0'; i = 1; }
    while n > 0 { buf[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    while i < width { s.push('0'); i += 1; }
    let written: Vec<u8> = buf.iter().take_while(|&&b| b != 0).copied().collect();
    for &b in written.iter().rev() { s.push(b as char); }
}

/// Wrapper around the in-place `push_size` helper used by the
/// footer — returns an owned String for the list view's Size cell.
fn format_size(n: u64) -> String {
    let mut s = String::with_capacity(12);
    push_size(&mut s, n);
    s
}

fn icon_for(e: &Entry) -> IconId {
    if e.is_dir { return IconId::Folder; }
    let ext = e.name.rsplit('.').next().unwrap_or("");
    match ext {
        "md" | "txt" | "log" | "cfg" | "toml" | "json" | "yaml" | "yml" => IconId::FileText,
        "rs" | "wasm" | "sh" | "py" | "c" | "h" | "hpp" | "cpp" | "go" => IconId::Code,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" => IconId::Image,
        // The icon says what the file is, not what we can play.
        "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aac" => IconId::FileAudio,
        // No film icon in the atlas — `Image` is the closest that fits: a
        // film is a sequence of them.
        "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" => IconId::Image,
        _ => IconId::File,
    }
}

// ── Number formatters (no_std friendly) ───────────────────────────────

fn push_usize(s: &mut String, mut n: usize) {
    if n == 0 { s.push('0'); return; }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 { buf[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    while i > 0 { i -= 1; s.push(buf[i] as char); }
}

fn push_size(s: &mut String, bytes: u64) {
    // Powers of 1024 — KB / MB / GB. Two decimals once we leave bytes
    // ("2.4 GB" rather than "2456 MB"). Pure integer math (no f64 in
    // no_std without messing with the linker).
    const K: u64 = 1024;
    const M: u64 = K * 1024;
    const G: u64 = M * 1024;

    if bytes < K {
        push_usize(s, bytes as usize);
        s.push_str(" B");
    } else if bytes < M {
        push_decimal(s, bytes, K);
        s.push_str(" KB");
    } else if bytes < G {
        push_decimal(s, bytes, M);
        s.push_str(" MB");
    } else {
        push_decimal(s, bytes, G);
        s.push_str(" GB");
    }
}

fn push_decimal(s: &mut String, n: u64, unit: u64) {
    let whole = n / unit;
    let tenths = ((n % unit) * 10) / unit;
    push_usize(s, whole as usize);
    s.push('.');
    s.push((b'0' + tenths as u8) as char);
}

// ── Entry point ───────────────────────────────────────────────────────

fn commit_tree(lf: &mut Loft) {
    lf.drop_dirs = (0..lf.filtered.len())
        .map(|ui| lf.entry_at(ui).filter(|e| e.is_dir).map(|e| join(&lf.current, &e.name)))
        .collect();
    let lf = &*lf;
    let tree = render(lf);
    match wire::encode(&tree) {
        Ok(bytes) => { if !host::scene_commit(&bytes) { log("[loft] commit failed"); } }
        Err(_) => log("[loft] encode failed"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // No `npk_window_set_overlay` — loft is a regular tiled app, the
    // first commit creates its window via shade::create_widget_window.
    let mut loft = Loft::new();
    let mut event_buf = [0u8; EVENT_BUF_SIZE];
    let mut idle_ticks: u32 = 0;

    commit_tree(&mut loft);
    // The widget window exists after the first commit — opt into receiving
    // Ctrl+C/X/V as Event::Clipboard so the shortcuts drive file operations.
    window_set_clipboard_sink();

    loop {
        match poll_event(&mut event_buf) {
            PollResult::Event(ev) => {
                idle_ticks = 0;
                match handle(&mut loft, ev) {
                    Outcome::Idle => {}
                    Outcome::Rerender => commit_tree(&mut loft),
                    Outcome::Exit => { host::close_widget(); return; }
                }
            }
            PollResult::Empty => {
                host::sleep_ms(16);

                // Progressively fill folder sizes/counts a few per tick.
                // Skipped while searching (folder stats aren't shown then).
                let mut dirty = loft.expire_status();
                if loft.pump_delete(DELETE_BUDGET) { dirty = true; }
                if loft.pump_thumbs() { dirty = true; }
                if loft.pump_preview() { dirty = true; }
                if loft.query.is_empty() && !loft.stats_queue.is_empty()
                    && loft.pump_stats(STATS_PUMP_BUDGET)
                {
                    dirty = true;
                }
                if dirty { commit_tree(&mut loft); }

                idle_ticks += 1;
                if idle_ticks >= AUTO_REFRESH_TICKS {
                    idle_ticks = 0;
                    // Auto-refresh the browse view when the folder changed
                    // on disk (new screenshot, download, …). Skip while a
                    // search, menu or dialog is up so we don't disturb the user.
                    if loft.open_menu.is_none() && loft.ctx.is_none()
                        && loft.dialog.is_none() && loft.query.is_empty()
                        && loft.deleting.is_none()
                    {
                        // Probe with a throwaway listing; only a real change
                        // re-lists and re-renders.
                        let new_sig = dir_signature(&loft.list(&loft.current, false));
                        if new_sig != loft.dir_sig {
                            loft.refresh();
                            commit_tree(&mut loft);
                        }
                    }
                }
            }
            PollResult::WindowGone => return,
        }
    }
}

// Silence unused warning on app_meta::IconRef — referenced through
// the build.rs-generated AppMeta blob, not directly.
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
