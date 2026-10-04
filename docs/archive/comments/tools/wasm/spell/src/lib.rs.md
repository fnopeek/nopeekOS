# `tools/wasm/spell/src/lib.rs` @ 5e0102684

## L1-14 · `#![no_std]`

```
//! spell 0.1 — text editor with markdown preview, in loft's visual
//! language.
//!
//! Layout (top → bottom):
//!   menu_bar   — Datei / Ansicht / Hilfe
//!   toolbar    — file name (+ dirty marker) · mode toggle · save
//!   body       — TextArea (edit) OR rendered preview (read-only)
//!   footer     — line + byte counts · file kind · saved/modified
//!
//! Editing uses `Widget::TextArea`: the compositor owns the 2-D caret
//! (arrows / Enter / Home-End / PageUp-Down), the app only mirrors the
//! document via `Event::InputChange`. Syntax highlight + markdown live
//! in the preview pane (read-only `Text` + `Tint` spans) because an
//! editable field renders flat text by design.
```

## L35-46 · `#[unsafe(link_section = ".npk.caps")]`

```
// Declared capabilities: read + render. **No WRITE** — an editor that can
// overwrite any file in the store is exactly what the file-dialog portal
// exists to avoid.
//
// Saving still works, through two narrower routes the kernel grants:
//   - the path the user picked in the dialog (`npk_pick` records it
//     against this instance — the click IS the authorisation), and
//   - `sys/config/spell`, our own settings file, whose name the kernel
//     derives from the module name so we can't claim someone else's.
//
// Browsing is likewise not ours: the dialog runs in its own module and
// hands back a single path.
```

## L51-53 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L69-71 · `struct Strings {`

```
// ── Strings ───────────────────────────────────────────────────────────
// English is the source language; a new one is one more `const` below.
// See `nopeek_widgets::i18n`.
```

## L87 · `zoom_reset:    &'static str,`

```
/// "Actual size ({} px)" — shows where you'd land back.
```

## L89-90 · `cfg_header:    &'static str,`

```
/// Header written above the settings, so the file explains itself
/// when someone opens it in spell.
```

## L94 · `untitled_file: &'static str,`

```
/// Default filename offered by the save dialog (no extension).
```

## L102 · `step_of:       &'static str,`

```
/// "{} of {}" — position in the quit sequence.
```

## L153 · `fn fill(template: &str, value: &str) -> String {`

```
/// Substitute the single `{}` placeholder in a catalog string.
```

## L179-180 · `fn step_label(n: usize, total: usize) -> String {`

```
/// "(2 of 3)" — `step_of` carries two placeholders, one more than
/// `fill` handles, so substitute both here.
```

## L190-191 · `fn zoom_reset_label() -> String {`

```
/// "Originalgröße (13 px)" — naming the target makes the entry a status
/// line too: you can see how far you have zoomed without counting.
```

## L208-213 · `const EVENT_BUF_SIZE: usize = 512 * 1024;`

```
// ── Buffers ───────────────────────────────────────────────────────────
//
// The event buffer must hold a full `InputChange` — its payload is the
// WHOLE document. `npk_event_poll` drops (does not truncate) an event
// that overflows, which would silently desync our mirror from the
// compositor's edit buffer, so size it well above any realistic file.
```

## L217 · `const FETCH_BUF_SIZE: usize = 512 * 1024;`

```
// Scratch for npk_fetch (open) — same ceiling as the edit buffer.
```

## L221-222 · `const TEXT_CAP: usize = 256 * 1024;`

```
// Pre-allocate the document so ordinary edits stay within capacity and
// don't churn the bump allocator (mirrors loft's `query` discipline).
```

## L225-230 · `const PAYLOAD_CAP: usize = 512 * 1024;`

```
// An event's owned String (Open path / InputChange value) is allocated on
// the bump heap during poll, ABOVE persistent_mark — so `alloc_reset`
// before `handle` frees it and the first allocation in `handle` clobbers
// it (a use-after-free). We copy such payloads into this STATIC buffer
// (outside the bump heap) before the reset, and hand `handle` a &str into
// it. Sized to hold a whole-document InputChange.
```

## L261-263 · `const HEAP_SIZE: usize = 4 * 1024 * 1024;`

```
// ── Bump allocator (4 MB — a document plus its transient widget tree
//    and preview spans). State alloc'd before `persistent_mark`
//    survives `alloc_reset`; everything above the mark is per-frame. ──
```

## L292 · `const ACT_MENU_FILE: u32 = 5_000;`

```
// ── Action / node ids ─────────────────────────────────────────────────
```

## L304 · `const ACT_CLOSE_SAVE:    u32 = 6_007;`

```
// Unsaved-changes-on-close dialog.
```

## L316 · `const PICK_OPEN: i32 = 0;`

```
// `npk_pick` modes.
```

## L320-322 · `const TAG_OPEN:            u32 = 1;`

```
// Tags handed to `npk_pick` and returned in `Event::Picked`. The save tags
// carry the tab index, so a reply lands on the document it was asked for
// rather than on whatever happens to be active when it arrives.
```

## L327 · `const ACT_TAB_BASE:       u32 = 8_000;`

```
// Tab bar: select tab i / close tab i.
```

## L335 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
// ── State ─────────────────────────────────────────────────────────────
```

## L343-345 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// What the file's extension says it is. A buffer with no filename yet is
/// `Untyped`: no highlighting, no preview toggle, no assumed extension —
/// it becomes a kind when the user names it.
```

## L349-350 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// Languages with a preview highlighter. Markup = HTML/XML (tag-based);
/// the rest share the C-like scanner parameterised by `syntax_for`.
```

## L354 · `struct Doc {`

```
/// One open document = one tab.
```

## L356 · `path:  Option<String>,`

```
/// npkFS path of the open file, or None for an unsaved buffer.
```

## L358 · `title: String,`

```
/// Basename shown on the tab.
```

## L360 · `text:  String,`

```
/// The whole document (`\n`-separated). Pre-allocated to `TEXT_CAP`.
```

## L363 · `mode:  Mode,`

```
/// Markdown raw↔rendered toggle (per document).
```

## L394 · `None                                   => Kind::Untyped, // unnamed buffer`

```
// unnamed buffer
```

## L395 · `_                                      => Kind::Plain,   // txt/log/toml/yaml/…`

```
// txt/log/toml/yaml/…
```

## L405-406 · `fn is_pristine(&self) -> bool {`

```
/// `true` for a fresh, never-edited Unbenannt tab — opening a file
/// reuses it instead of stacking a blank tab (VS Code behaviour).
```

## L413 · `docs:         Vec<Doc>,`

```
/// Open documents, one per tab.
```

## L415 · `active:       usize,`

```
/// Index of the active tab.
```

## L418-419 · `confirm_close: Option<usize>,`

```
/// Tab index pending an unsaved-changes confirmation before close.
/// `Some(i)` shows the "save changes?" dialog for tab `i`.
```

## L421-422 · `quitting:      bool,`

```
/// A quit is in progress: the confirm dialog is walking the dirty
/// tabs one by one, and clearing the last one closes the app.
```

## L424-425 · `font_px:       u16,`

```
/// Editor font size in px. Ctrl+wheel moves it; every tab shares one
/// setting, like a view preference rather than a per-file property.
```

## L427-430 · `font_px_saved: u16,`

```
/// What is actually on disk. A wheel spin fires a dozen zoom events
/// and each npkFS write costs an encrypt + hash + flush, so the file
/// is written when leaving, not per notch — this says whether that
/// write is still owed.
```

## L432-434 · `quit_total:    usize,`

```
/// How many dirty tabs the quit started with, and how many are done.
/// Counted at the start because the live count shrinks as tabs close
/// — deriving the step from it would show "1 of 3", "1 of 2", "1 of 1".
```

## L454 · `let mut argbuf = [0u8; 512];`

```
// Launched to open a specific file (loft file association)?
```

## L470 · `let mut d = Doc::empty();`

```
// No file argument → welcome tab.
```

## L480-481 · `fn open_path(&mut self, path: &str) {`

```
/// Open a file: focus its tab if already open (VS Code behaviour),
/// else load it into a new tab (reusing a pristine Unbenannt tab).
```

## L514-515 · `fn request_close(&mut self, i: usize) {`

```
/// Close tab `i`, but if it has unsaved changes show the confirm
/// dialog first (focusing it) instead of discarding silently.
```

## L526-531 · `fn begin_quit(&mut self) -> bool {`

```
// ── Quitting ──────────────────────────────────────────────────────
//
// The window manager asks before closing us (`Event::CloseRequest`),
// so this is where unsaved work gets its say. One dialog per dirty
// tab, in order, with a counter — the same shape VS Code uses. Any
// Cancel abandons the whole quit, not just that one file.
```

## L533 · `fn begin_quit(&mut self) -> bool {`

```
/// Begin quitting. Returns true if we can go right now.
```

## L558-559 · `fn advance_quit(&mut self) -> bool {`

```
/// One dirty tab is settled; move to the next or finish. Returns true
/// when nothing is left and the app should close.
```

## L569-571 · `fn zoom(&mut self, to: Option<i32>) -> bool {`

```
/// Step the editor font. `to = None` returns to the default — the
/// "actual size" entry every editor and browser has, because after a
/// few notches nobody remembers where they started.
```

## L583-584 · `fn save_config(&mut self) {`

```
/// Persist settings if they drifted from the file. Cheap no-op when
/// nothing changed, so it can sit on every exit path.
```

## L611 · `fn save_or_name(&mut self) {`

```
/// Save the active doc, or ask where to put it if it has no file yet.
```

## L627-630 · `if path == CONFIG_PATH {`

```
// Editing the settings file in spell itself is a stated use of
// having one. Adopt what was just saved — otherwise our in-memory
// value would be written back over it on the way out and the edit
// would silently undo itself.
```

## L638-643 · `fn ask_save_target(&mut self, doc: usize, then_close: bool) {`

```
/// Ask the picker where to save tab `doc`. `then_close` remembers that
/// this save came from the close dialog, so the tab goes away once the
/// file is written.
///
/// The dialog is modal, so the tab indices encoded in the tag can't
/// shift while it's up.
```

## L647-648 · `let start = d.path.as_deref().map(dirname).unwrap_or("");`

```
// Open where the file already lives; a fresh buffer starts wherever
// the picker defaults to (the kernel resolves "" to the user's home).
```

## L650-652 · `let suggest = d.path.as_deref().map(basename).unwrap_or(s().untitled_file);`

```
// Suggest a name so the dialog's Save is usable straight away.
// No extension for a fresh buffer — the file has no type until the
// user gives it one, and whatever they type decides it.
```

## L658 · `fn save_as(&mut self, doc: usize, path: &str) {`

```
/// Adopt `path` as tab `doc`'s file and write it there.
```

## L671 · `fn pick(mode: i32, start: &str, suggest: &str, tag: u32) {`

```
/// Ask the kernel for a file dialog. `tag` comes back in `Event::Picked`.
```

## L683 · `fn read_file(path: &str) -> Option<String> {`

```
/// Fetch a file's contents as a String, or None on error / non-UTF-8.
```

## L695-701 · `const CONFIG_PATH: &str = "sys/config/spell";`

```
// ── Config ────────────────────────────────────────────────────────────
//
// `sys/config/spell`, same shape as `sys/config/bar`: one `key: value`
// per line, `#` starts a comment. Meant to be opened and edited in spell
// itself, so it carries a header explaining what is in it and unknown
// keys are ignored rather than rejected — a newer spell's settings must
// not make an older one refuse the file.
```

## L707-708 · `fn read_config_font() -> u16 {`

```
/// Read the saved font size. Anything missing or unparseable falls back
/// to the default — a broken config should never keep the editor shut.
```

## L730-731 · `fn write_config(font_px: u16) {`

```
/// Write the config back, header and all. Called when settings change,
/// not on every wheel notch — see `Event::Zoom`.
```

## L745 · `fn basename(path: &str) -> &str {`

```
// ── Path helpers ──────────────────────────────────────────────────────
```

## L761 · `fn render(sp: &Spell) -> Widget {`

```
// ── Render ────────────────────────────────────────────────────────────
```

## L769 · `render_body(sp),       // Flex(1) — fills (no footer; removed as noise)`

```
// Flex(1) — fills (no footer; removed as noise)
```

## L806-807 · `OpenMenu::File => (`

```
// The menu is where the shortcuts are learned, so they're listed
// next to the entry that binds them.
```

## L839-844 · `const TAB_W: u16 = 200;`

```
/// Tab bar — one tab per open document, active tab highlighted, each
/// with a dirty dot and a close (×). Trailing "+" opens a new tab.
/// Replaces the old filename+icons toolbar (save lives in the Datei
/// menu, the markdown view toggle in the Ansicht menu).
/// Fixed tab width — names ellipsize rather than letting the strip
/// reflow as you open files (docs/spec/UI_REFRESH.md §3 `tab`).
```

## L847-848 · `const TAB_RADIUS: u8 = 8;`

```
/// Rounded on top only would need per-corner radii; the strip clips the
/// bottom edge visually because the active tab shares the body's colour.
```

## L864-865 · `mods.push(Modifier::Background(Token::Page));`

```
// The active tab carries the colour of the content below it,
// so the two read as one surface.
```

## L874-875 · `let trailing = if d.dirty {`

```
// Unsaved work shows as an accent dot; only a tab you can act on
// offers the ×. A saved, inactive tab shows neither.
```

## L917 · `tabs.push(Widget::Text {`

```
// New-tab button.
```

## L945 · `fn tab_icon(d: &Doc) -> IconId {`

```
/// Tab glyph by file kind — plain text vs. source.
```

## L957-959 · `let doc = sp.cur();`

```
// Markdown is the special case: a rendered preview you toggle to.
// Everything else is the live-highlighted editor — code gets colour
// spans the compositor paints while you type, no toggle needed.
```

## L990-995 · `fn render_confirm_dialog(sp: &Spell) -> Widget {`

```
/// Unsaved-changes confirmation shown when closing a dirty tab. Buttons
/// only (no text input), so no focus dance needed.
///
/// While quitting this is one step of a sequence, so it carries a
/// "(2 of 3)" counter — otherwise a second dialog appearing right after
/// the first reads like the button didn't take.
```

## L1053 · `fn markdown_preview(text: &str) -> Widget {`

```
// ── Markdown preview ──────────────────────────────────────────────────
```

## L1119-1121 · `fn paragraph(text: &str) -> Widget {`

```
/// Paragraph with inline `code` spans rendered as Mono + muted tint.
/// Other inline markup (**bold**, *italic*) is left as literal text in
/// v1 — there is no bold weight in the text vocab yet.
```

## L1123 · `let mut spans: Vec<Widget> = Vec::new();`

```
// Odd split segments sit between backticks → inline code.
```

## L1160-1171 · `fn push_span(out: &mut Vec<Span>, start: usize, len: usize, token: Token) {`

```
// ── Syntax highlighting → colour spans (live in the TextArea) ─────────
//
// One pass over the whole buffer, not line by line. Block comments,
// triple-quoted strings and JS templates all run across newlines, and a
// per-line scanner has to guess at every line start which construct it
// is standing inside — it guesses wrong on exactly the files people
// write.
//
// Spans cover only what is NOT default-coloured; uncovered bytes render
// in `OnSurface`, which is most of a file. They come out sorted by
// `start` (the scan only ever moves forward), which the compositor's
// renderer relies on.
```

## L1178-1183 · `struct Syntax {`

```
/// Per-language lexer knobs.
///
/// `decl` and `ctrl` are split the way the themes split them —
/// storage/declaration against control flow and imports. VSCodium paints
/// those two different colours, and collapsing them into one bucket was
/// half of why source here read as flat.
```

## L1188 · `types:     &'static [&'static str],`

```
/// Built-in type names that are not keywords: `u32`, `str`, `bytes`.
```

## L1190 · `builtins:  &'static [&'static str],`

```
/// Names that are callable without parentheses (shell builtins).
```

## L1192 · `line:      &'static str,`

```
/// Line-comment marker, or "" for none.
```

## L1195 · `nest_block: bool,`

```
/// Rust nests `/* /* */ */`; C and JS do not.
```

## L1197 · `squote:    bool,`

```
/// `'x'` is a string. False for Rust, where `'a` is a lifetime.
```

## L1199 · `char_lit:  bool,`

```
/// `'x'` is a char literal — only when a closing quote really follows.
```

## L1201 · `backtick:  bool,`

```
/// `` `…` `` template literal, newlines allowed.
```

## L1203 · `triple:    bool,`

```
/// `"""…"""` / `'''…'''`.
```

## L1205 · `raw_str:   bool,`

```
/// `r"…"`, `r#"…"#`, `b"…"`, `br#"…"#`.
```

## L1207 · `dollar:    bool,`

```
/// `$VAR`, `${VAR}`, `$1`, `$?`.
```

## L1209 · `decorator: bool,`

```
/// `@name` decorator.
```

## L1211 · `preproc:   bool,`

```
/// `#directive`, and `<stdio.h>` after `#include`.
```

## L1213-1214 · `str_prefix: &'static [&'static str],`

```
/// Literal prefixes that belong to the string they precede:
/// `f"…"`, `b'…'`, `r#"…"#`.
```

## L1216-1218 · `subst:     bool,`

```
/// `$(…)` is a command substitution — inside a double-quoted shell
/// string it opens a fresh quoting context, so the string does not
/// end at the next quote it contains.
```

## L1248 · `"include","define","undef","ifdef","ifndef","endif","pragma","elif","error"],`

```
// preprocessor directives, reached through the '#' branch
```

## L1315 · `Lang::Json | Lang::Markup => &RUST,`

```
// Handled by their own scanners — never reached.
```

## L1320 · `fn code_spans(text: &str, lang: Lang) -> Vec<Span> {`

```
/// Tokenise the whole document into colour spans for the given language.
```

## L1329 · `fn char_len(s: &str, i: usize) -> usize {`

```
// ── Scanning primitives ───────────────────────────────────────────────
```

## L1344-1346 · `fn scan_block(text: &str, start: usize, open: &str, close: &str, nest: bool) -> usize {`

```
/// End offset of a block comment starting at `start`. Unterminated runs
/// to EOF — that is what an editor should show while you are still
/// typing the closing marker.
```

## L1363-1365 · `fn scan_string(text: &str, start: usize, quote: u8, triple: bool, subst: bool) -> usize {`

```
/// End offset of a string starting at `start`. A plain quoted string
/// stops at the newline if it is unterminated, so one stray `"` cannot
/// paint the rest of the file.
```

## L1384-1385 · `if subst && b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'(' {`

```
// `"$(cd "$dir" && pwd)"` is ONE string — the quotes inside the
// substitution belong to it, not to us. Skip to the balanced `)`.
```

## L1407-1408 · `fn scan_raw_string(text: &str, at: usize) -> Option<usize> {`

```
/// Rust `r"…"` / `r#"…"#`. `at` points just past the `r`/`b`/`br` prefix.
/// Returns `None` if no raw string actually follows.
```

## L1428-1429 · `fn scan_char_lit(text: &str, start: usize) -> Option<usize> {`

```
/// `'x'` / `'\n'` / `'\u{1F600}'` — but NOT `'a`, which is a lifetime.
/// `None` unless a closing quote really follows.
```

## L1454 · `if b[i] == b'0' && i + 1 < b.len() && matches!(b[i + 1] | 0x20, b'x' | b'b' | b'o') {`

```
// 0x… / 0b… / 0o… — one run, digits and separators alike.
```

## L1461 · `if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {`

```
// A fraction, but `1..2` is a range — the digit after the dot decides.
```

## L1474 · `while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }`

```
// Type suffix: 1u32, 3.14f64, 10UL.
```

## L1479-1482 · `fn classify(w: &str, b: &[u8], after: usize, sx: &Syntax) -> Option<Token> {`

```
/// Which colour an identifier gets. There is no symbol table here, so
/// after the keyword lists it is shape that carries the signal: a name
/// with a `(` behind it is a call, `HttpRequest` is a type, `MAX_LEN` is
/// a constant, and `count` is none of the three and stays default.
```

## L1489-1491 · `let first = w.as_bytes()[0];`

```
// Shape is checked BEFORE the call site: `new Error(…)` and
// `Downloader(url)` are a type being used, and colouring them as
// calls made every constructor in the file look like a free function.
```

## L1503 · `fn clike_spans(text: &str, sx: &Syntax) -> Vec<Span> {`

```
// ── The C-like scanner ────────────────────────────────────────────────
```

## L1509-1510 · `let mut include_line = false;`

```
// Set while inside a `#include` line, so `<stdio.h>` reads as a path
// and not as two comparisons.
```

## L1560-1561 · `if i + 1 < b.len() && b[i + 1] == b'(' { i += 2; continue; }`

```
// `$(…)` is a command substitution. Stepping over just the
// `$(` lets the command inside keep its own colours.
```

## L1568 · `if j == i + 1 && j < b.len() { j += 1; }`

```
// $? $@ $# $* — one punctuation byte is still a variable.
```

## L1588 · `i += 1;`

```
// A lifetime. Step over the quote and read the name normally.
```

## L1605 · `if sx.str_prefix.contains(&w) && j < b.len() {`

```
// `f"…"`, `b'…'`, `r#"…"#` — the prefix is part of the literal.
```

## L1633 · `fn json_spans(text: &str) -> Vec<Span> {`

```
// ── JSON ──────────────────────────────────────────────────────────────
```

## L1635-1636 · `fn json_spans(text: &str) -> Vec<Span> {`

```
/// Keys and values are both quoted strings, so the next non-space byte
/// after a string is what tells them apart.
```

## L1675 · `fn markup_spans(text: &str) -> Vec<Span> {`

```
// ── HTML / XML ────────────────────────────────────────────────────────
```

## L1677-1680 · `fn markup_spans(text: &str) -> Vec<Span> {`

```
/// Tag names → type colour, attribute names → variable, attribute values
/// → string, `<!-- -->` → comment, `&entity;` → constant. The angle
/// brackets themselves stay default: colouring the punctuation too turns
/// a page of markup into one solid block.
```

## L1736 · `enum Outcome { Idle, Rerender, Exit }`

```
// ── Events ────────────────────────────────────────────────────────────
```

## L1742-1745 · `Event::Key(KeyCode::Escape) => {`

```
// Esc backs out of whatever is open. It does NOT quit: in an
// editor Esc is the cancel key, and a stray press should never
// end the session. Closing is Mod+Q, the window's ×, or
// File → Close.
```

## L1751 · `Event::CloseRequest => {`

```
// The window manager is asking whether it may close us.
```

## L1756-1759 · `let d = sp.cur_mut();`

```
// `payload` is the stabilized buffer value (the event's own
// String was freed by alloc_reset). The TextArea is the only
// editable widget we render — the file dialogs live in their
// own window now.
```

## L1765-1766 · `Event::Picked { tag, .. } => {`

```
// The file dialog came back. `payload` is the chosen path; empty
// means the user cancelled.
```

## L1781-1782 · `Event::Open(_) => {`

```
// Another file was opened while we're running (loft association,
// singleton routing) → open it as a tab. `payload` = the path.
```

## L1787-1788 · `Event::Chord { letter, shift, .. } => match letter {`

```
// Ctrl chords. The editor keeps Ctrl+A/C/X/V for text, so those
// never arrive here.
```

## L1799-1800 · `b'+' | b'=' => { sp.zoom(Some(1)); Outcome::Rerender }`

```
// Zoom. '=' comes along because on a US layout the '+' key is
// Shift+'=', and people press Ctrl+'=' without the Shift.
```

## L1806-1807 · `Event::Zoom { delta } => {`

```
// Ctrl+wheel. One px per notch: small enough to land on the size
// you want, and the compositor clamps the ends anyway.
```

## L1832-1833 · `let start = sp.cur().path.as_deref().map(dirname).unwrap_or("").to_string();`

```
// Start where the current file lives; the picker falls back to
// the user's home for an unnamed buffer.
```

## L1844 · `ACT_CLOSE_DISCARD => {`

```
// Unsaved-changes dialog buttons.
```

## L1852-1854 · `ACT_CLOSE_CANCEL => { sp.cancel_quit(); Outcome::Rerender }`

```
// Cancel abandons the whole quit, not just this one file — the
// user said "no" to closing, and losing the other tabs' prompts
// would be a surprise.
```

## L1864-1865 · `sp.ask_save_target(i, true);`

```
// No filename yet → ask where to put it. The chain
// resumes when the picker replies.
```

## L1874-1876 · `if matches!(sp.cur().kind(), Kind::Markdown) {`

```
// Only markdown has a preview. Leaving the mode on Edit for
// everything else keeps the menu's radio dot honest instead of
// marking a view that never renders.
```

## L1884-1886 · `if read_file(CONFIG_PATH).is_none() { write_config(sp.font_px); }`

```
// Write it first when it doesn't exist yet, so "Settings"
// always opens something — an empty editor with no file would
// just raise the question of where to put it.
```

## L1900 · `if id >= ACT_TAB_CLOSE_BASE {`

```
// Ranges, highest base first.
```

## L1919 · `fn commit_tree(sp: &Spell) {`

```
// ── Commit + main loop ────────────────────────────────────────────────
```

## L1923-1924 · `match wire::encode(&tree) {`

```
// `wire::encode` prepends the WIRE_VERSION byte that the compositor's
// `scene_commit` checks — a raw postcard payload is rejected (-1).
```

## L1933-1936 · `let mut sp = Spell::new();`

```
// Tiled app — the first commit creates the window; the compositor
// auto-focuses the first text widget (our TextArea) so the user can
// type immediately. See the loft bump-allocator notes for the
// persistent_mark / alloc_reset lifecycle.
```

## L1941-1942 · `unsafe { let _ = npk_window_set_close_guard(1); }`

```
// After the first commit — the window has to exist before it can be
// guarded. From here Mod+Q and the × ask us first.
```

## L1948-1951 · `let plen = match &ev {`

```
// Stabilize heap-backed payloads (Open path, InputChange
// value) into the static buffer BEFORE alloc_reset frees
// the event — otherwise handle's first allocation clobbers
// them (use-after-free).
```

## L1968-1969 · `PollResult::WindowGone => { sp.save_config(); return; }`

```
// Window pulled out from under us (hard close): the settings
// write still goes through, npkFS doesn't need the window.
```

## L1975 · `#[allow(dead_code)]`

```
// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
```

