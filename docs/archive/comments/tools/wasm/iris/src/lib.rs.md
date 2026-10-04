# `tools/wasm/iris/src/lib.rs` @ 5e0102684

## L1-16 · `#![no_std]`

```
//! iris — minimal image viewer for nopeekOS, in loft's visual language.
//!
//! Layout (top → bottom):
//!   toolbar — icon · file name · spacer · "i/m · W×H"
//!   body    — Widget::Canvas (the decoded image, contain-fit)
//!   footer  — full npkFS path
//!
//! Navigation: left-click = next image, right-click = previous (also
//! ←/→ arrow keys). The image set is every `.png` in the file's folder,
//! sorted by name. Launched with a file argument (loft double-click) or
//! routed an `Event::Open` when already running (singleton).
//!
//! Display uses the P10.10 canvas escape hatch: iris decodes PNG → BGRA
//! in WASM and uploads it with `npk_canvas_commit`; the compositor blits
//! it contain-fit into the `Widget::Canvas` rect. iris never touches the
//! framebuffer — it only holds the CANVAS capability.
```

## L37 · `#[unsafe(link_section = ".npk.caps")]`

```
// Read images + render + upload pixels. No WRITE, no EXEC.
```

## L42-44 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L68-71 · `fn log_ms(label: &str, ms: i64) {`

```
/// Log "<label>: <ms> ms". Permanent, not scaffolding: the decode runs
/// under a WASM interpreter, so knowing whether the seconds go into
/// inflate, un-filtering or the colour swap is the difference between
/// fixing the slow thing and rewriting the fast one. 10 ms resolution.
```

## L103 · `const EVENT_BUF_SIZE: usize = 8 * 1024;`

```
// ── Buffers ───────────────────────────────────────────────────────────
```

## L107 · `const FETCH_BUF_SIZE: usize = 32 * 1024 * 1024;`

```
// Compressed PNG file scratch (a screen-sized PNG is a few MB).
```

## L117-119 · `const PAYLOAD_CAP: usize = 4 * 1024;`

```
// Event payloads (Open path) live on the bump heap above the persistent
// mark; alloc_reset before handling frees them, so copy into this static
// first (use-after-free lesson, see spell).
```

## L150-155 · `const HEAP_SIZE: usize = 160 * 1024 * 1024;`

```
// ── Bump allocator ────────────────────────────────────────────────────
//
// A 4K PNG decode needs ~3× its raw size live at once (decompressed +
// unfiltered + BGRA). 256 MB matches wallpaper's headroom; everything
// above `persistent_mark` (the per-frame scene + the decode buffers) is
// freed each loop iteration so navigation doesn't leak.
```

## L183-209 · `const CACHE_MAX: usize = 320 * 1024 * 1024;`

```
// ── Decoded-bitmap cache ──────────────────────────────────────────────
//
// Decoding is the whole cost of showing an image (~1.3 s for a 1080p
// PNG under the interpreter), so a picture already decoded must never be
// decoded twice. The cache lives OUTSIDE the bump heap — `alloc_reset`
// would otherwise pull it out from under us every event.
//
// **Budgeted in bytes, not in pictures.** A count that fits 1080p (8 MB
// each) buys 300 MB at 4K (33 MB each); the same mistake a per-stylesheet
// cap made in beak. With a byte budget the depth adapts by itself.
//
// **And grown on demand, not reserved.** A `static` array would be part
// of the module's linear memory and therefore allocated and zeroed at
// launch — half a gigabyte for a folder holding three pictures. Instead
// the arena is claimed with `memory.grow` as pictures actually arrive,
// and only up to what this folder can use: nine images at the size the
// first decode turned out to be. Three small pictures cost a few dozen
// megabytes and never more.
//
// The growth is contiguous because we are the only caller of
// `memory.grow` in this module — the bump allocator hands out slices of
// a fixed array and never asks the runtime for more.
//
// The arena is a ring: allocations run forward, wrap when the tail is
// too short, and evict whatever they land on. In sequential browsing
// that means the pictures furthest behind you go first, which is exactly
// the right eviction order and costs no compaction.
```

## L217-218 · `fn arena_reserve(want: usize) -> usize {`

```
/// Claim linear memory until the arena holds at least `want` bytes (never
/// past `CACHE_MAX`). Returns the capacity actually available.
```

## L226 · `if prev == usize::MAX { return cap; }   // runtime refused; keep what we have`

```
// runtime refused; keep what we have
```

## L231-234 · `cache_clear();`

```
// Someone else claimed memory in between, so the arena is no
// longer one run. Nothing else in this module does that today;
// if it ever starts to, drop what we hold and re-base rather
// than address across the gap.
```

## L252-255 · `const SKIP_SLOTS: usize = 8;`

```
/// Pictures that did not make it into the cache — too big for the budget,
/// or undecodable. Without this list prefetching would pick the same
/// target every round and re-decode it forever: a hundred percent of a
/// core, spent on an image that can never land.
```

## L272-273 · `fn cache_clear() {`

```
/// Everything cached is dropped when the folder changes — the entries are
/// keyed by position in `files`, and that meaning changes with the list.
```

## L293-299 · `fn cache_put(idx: usize, bgra: &[u8], w: u32, h: u32, wanted: usize) {`

```
/// Store a decoded bitmap, growing the arena if this folder can use the
/// room. `wanted` is how many pictures are worth holding — the prefetch
/// window, capped by how many the folder actually has, so a two-picture
/// folder never claims space for nine.
///
/// Skips pictures too big to share the arena: one of those would evict
/// everything else on every navigation.
```

## L307 · `if start + len > cap { start = 0; }   // wrap; tail is wasted`

```
// wrap; tail is wasted
```

## L310 · `for slot in entries.iter_mut() {`

```
// Anything the new bytes land on is gone.
```

## L319-320 · `match entries.iter_mut().find(|s| s.is_none()) {`

```
// Prefer a free slot; if the table is full the oldest region is
// the one nearest the write head, so drop the first entry.
```

## L329 · `struct Strings {`

```
// ── Strings ───────────────────────────────────────────────────────────
```

## L376 · `const ACT_MENU_FILE:    u32 = 1;`

```
// ── Actions / anchors ─────────────────────────────────────────────────
```

## L389 · `const ACT_OPEN_FILE_BASE: u32 = 1000;`

```
/// Picking a file from the Open list: base + index.
```

## L399 · `struct Iris {`

```
// ── State ─────────────────────────────────────────────────────────────
```

## L401 · `dir:    String,        // folder being browsed (npkFS path, no trailing /)`

```
// folder being browsed (npkFS path, no trailing /)
```

## L402 · `files:  Vec<String>,   // image file names in dir, sorted`

```
// image file names in `dir`, sorted
```

## L404 · `w:      u32,           // current image dims (0 = none / failed)`

```
// current image dims (0 = none / failed)
```

## L406 · `failed: bool,          // decode failed for the current file`

```
// decode failed for the current file
```

## L407 · `menu:   Option<OpenMenu>,`

```
// Chrome state. Copy-only, so it may be mutated after the alloc mark.
```

## L409 · `picking: bool,         // File → Open… list is showing`

```
// File → Open… list is showing
```

## L410-412 · `zoom:   u16,`

```
/// Zoom as Q8.8 relative to contain-fit: 256 = the whole image in the
/// window. The compositor scales the bitmap it already holds, so a
/// zoom step costs no decode and no upload.
```

## L414 · `pan:    (i32, i32),`

```
/// Pan in px from centred. Clamped by the compositor to the overhang.
```

## L416 · `press:  Option<(i32, i32)>,`

```
/// Where the primary button went down, while it is still down.
```

## L418 · `dragged: bool,`

```
/// The pointer moved far enough since the press to call it a drag.
```

## L420 · `forward: bool,`

```
/// Last navigation direction — prefetch follows it first.
```

## L422-424 · `loading: bool,`

```
/// A decode is about to run. Committed as a scene BEFORE the decode
/// starts, so the footer says what is happening during the seconds
/// the picture takes — the window is otherwise silent.
```

## L426-431 · `swallow_press: bool,`

```
/// A widget consumed this click, so the raw press that follows it is
/// not ours. The compositor sends BOTH for one physical click: first
/// `Action(id)` for the button/menu that was hit, then the raw
/// `MouseButton` for position-sensitive apps. Without this the
/// toolbar's ◀ would page back on the Action and forward again on the
/// release, and a menu would close in the same click that opened it.
```

## L436 · `const ZOOM_MIN: u16 = 64;      // 25 %`

```
// 25 %
```

## L437 · `const ZOOM_MAX: u16 = 4096;    // 16×`

```
// 16×
```

## L438 · `const ZOOM_STEP_NUM: u32 = 5;`

```
/// One wheel notch — a bit under 1.25×, so a few clicks feel linear.
```

## L441-442 · `const DRAG_SLOP: i32 = 4;`

```
/// Movement (px, Manhattan) below which a press+release is still a click
/// and not a drag. Generous enough that a shaky hand still pages.
```

## L445-446 · `const PREFETCH_DEPTH: usize = 4;`

```
/// How far ahead and behind to decode. The byte budget decides how many
/// of these actually fit — at 4K only the nearest one or two will.
```

## L448-450 · `const QUIET_POLLS: u32 = 20;`

```
/// Empty polls (16 ms each) before prefetching starts. Decoding is a
/// solid second of CPU, so it must never race the user: while clicks are
/// still arriving, answering them wins.
```

## L469 · `let mut argbuf = [0u8; 512];`

```
// Launched to open a specific file?
```

## L478 · `let home = read_home_dir();`

```
// No argument → default to the screenshots folder.
```

## L485 · `fn point_at(&mut self, path: &str) {`

```
/// Set the folder + selection from a full file path.
```

## L493 · `fn refresh(&mut self) {`

```
/// Re-list `dir` for image files.
```

## L495-496 · `cache_clear();`

```
// Entries are keyed by position in `files`; a new listing gives
// those positions a different meaning.
```

## L507-512 · `fn load(&mut self) {`

```
/// Fetch + decode the current image and upload it to the canvas.
/// Mutates only Copy fields (w/h/failed) + the kernel canvas store —
/// nothing heap-persistent — so it's safe to run after the per-frame
/// alloc mark (its big decode buffers are transient, freed next reset).
/// Show the current image. A cache hit is the whole point: the only
/// work left is handing the bitmap to the compositor.
```

## L528 · `fn decode_into_view(&mut self) {`

```
/// Decode `idx` off disk, show it, and keep it for next time.
```

## L538-544 · `match decode_png(bytes) {`

```
// Nothing is shown until the picture is whole. Handing over each
// band as it landed did work — first pixels after a tenth of the
// time — but watching an image wipe in over two seconds reads
// worse than a moment of quiet, and while browsing it replaced a
// finished picture with a half-black one. The decoder stays
// resumable (it costs nothing and is checked bit-identical); it
// simply keeps its intermediate states to itself.
```

## L553 · `log_ms("=== open -> displayed", now_ms() - t_start);`

```
// The number that matters: click → pixels on screen.
```

## L565-567 · `fn cache_slots(&self) -> usize {`

```
/// How many pictures are worth holding: the prefetch window in both
/// directions plus the current one, but never more than the folder
/// has. Three pictures in a folder must not claim room for nine.
```

## L572-574 · `fn prefetch_target(&self) -> Option<usize> {`

```
/// The next neighbour worth decoding ahead, nearest first and in the
/// direction of travel — so a fast click in the way you were already
/// going is the case that is covered first.
```

## L593-594 · `fn prefetch_one(&mut self, idx: usize) {`

```
/// Decode ONE neighbour into the cache. Never touches the view, so a
/// half-finished round of prefetching leaves nothing behind.
```

## L604-605 · `if cache_get(idx).is_none() { skip_mark(idx); }`

```
// Did not land (undecodable, or larger than the budget allows) —
// remember that, or we would try it again every round forever.
```

## L609-610 · `fn next(&mut self) {`

```
// A new image always starts fit to the window — carrying a 4× zoom
// into the next picture leaves you staring at somebody's corner.
```

## L629-630 · `fn set_zoom(&mut self, z: u16) {`

```
/// Zooming keeps the pan; the compositor re-clamps it to the new
/// overhang, so zooming out simply pulls the image back to centre.
```

## L636-644 · `fn zoom_at_cursor(&mut self, z1: u16) {`

```
/// Zoom anchored on the pointer: whatever pixel sits under the cursor
/// stays under it. Without this, zooming in on a detail means zooming
/// to the middle and then dragging the detail back — twice the work
/// for the thing you actually wanted.
///
/// A point `p` of the image (in fit-units from the canvas centre) is
/// drawn at offset `s = p·z/256 + pan`. Holding `s` fixed at the
/// cursor offset `c` while z0 → z1 gives `pan1 = c − (c − pan0)·z1/z0`.
/// Falls back to centred zoom when the pointer is elsewhere.
```

## L665 · `Event::Key(KeyCode::Escape) => {`

```
// Escape closes an open menu first, the window only when none is.
```

## L673 · `Event::Key(KeyCode::Char(b'+')) | Event::Key(KeyCode::Char(b'=')) => {`

```
// Keyboard zoom, the usual trio.
```

## L679-680 · `Event::Wheel { dy } => {`

```
// Wheel zoom. `dy` is the compositor's scroll step, sign only —
// up (negative) enlarges, like every other viewer.
```

## L685-687 · `Event::Action(ActionId(id)) => {`

```
// A widget (menu label, menu item, toolbar button) took this
// click. Swallow the raw press that the compositor sends right
// after it, or the same physical click would act twice.
```

## L692-695 · `Event::MouseButton { button: MouseButton::Left, down: true, x, y } => {`

```
// Press just arms a possible drag — what it MEANT is decided on
// release, because the same button both pans and pages. That is
// how every image viewer resolves this: a drag moves the picture,
// a click without movement is still a click.
```

## L701-702 · `if iris.menu.is_some() || iris.picking { return Outcome::Idle; }`

```
// Belt and braces: a click with a dropdown open is the
// dismissing click and belongs to the Popover, not to us.
```

## L713-714 · `Event::MouseMove { x, y } => {`

```
// Motion only arrives while the button is held (the compositor
// forwards drags, never hover), so this IS the pan.
```

## L719 · `return Outcome::Idle;   // still a click, not yet a drag`

```
// still a click, not yet a drag
```

## L723 · `if iris.zoom == ZOOM_FIT { return Outcome::Idle; } // nothing to pan`

```
// nothing to pan
```

## L728 · `Event::Open(_) => { iris.point_at(payload); Outcome::Reload }`

```
// Another image opened while running (loft / singleton routing).
```

## L735 · `if id >= ACT_OPEN_FILE_BASE {`

```
// Picking a file out of the Open list.
```

## L743 · `ACT_MENU_FILE => { iris.picking = false; iris.menu = toggle(iris.menu, OpenMenu::File); Outcome::Render }`

```
// A menu label toggles its own dropdown and closes any other.
```

## L764-768 · `const NAV_BTN: u16 = 28;`

```
// ── Scene ─────────────────────────────────────────────────────────────
//
// Same three bands as loft / spell / beak: menu bar · toolbar · body ·
// footer, all built from prefabs so the chrome tracks the design system
// instead of drifting on its own hardcoded paddings.
```

## L770 · `const NAV_BTN: u16 = 28;`

```
/// Toolbar chrome sizing — beak's `toolbar_button` (docs/spec/UI_REFRESH.md §5).
```

## L774-776 · `fn nav_button(icon: IconId, action: u32, enabled: bool) -> Widget {`

```
/// Navigation button: bare at rest, `SurfaceHover` under the cursor,
/// accent tint while pressed. Disabled-looking (faint, no handler) when
/// there is nowhere to go.
```

## L832-833 · `fn render_footer(iris: &Iris) -> Widget {`

```
/// Footer: full npkFS path left, dimensions (or the failure reason) right
/// — loft's `prefab::footer` split.
```

## L845 · `alloc::format!("{}×{} · {} %", iris.w, iris.h, iris.zoom as u32 * 100 / 256)`

```
// Percent of the fitted size — what the wheel actually changed.
```

## L879 · `fn render_open_list(iris: &Iris) -> Widget {`

```
/// File → Open…: every image in the folder, current one check-marked.
```

## L901-902 · `if iris.zoom != ZOOM_FIT {`

```
// The compositor scales the bitmap it already holds — no re-decode,
// no re-upload, so a wheel notch is a repaint and nothing more.
```

## L926 · `if iris.picking {`

```
// The Open list replaces the File dropdown at the same anchor.
```

## L955 · `#[unsafe(no_mangle)]`

```
// ── Entry ─────────────────────────────────────────────────────────────
```

## L960-962 · `iris.loading = true;`

```
// The window appears immediately and says what it is busy with; the
// first picture has nothing cached behind it and takes the full
// decode, which is the longest wait iris ever shows anyone.
```

## L964 · `commit_scene(&iris); // creates the window (assigns widget_window_id)`

```
// creates the window (assigns widget_window_id)
```

## L965 · `iris.load();         // window exists now → canvas_commit works`

```
// window exists now → canvas_commit works
```

## L967 · `commit_scene(&iris); // refresh chrome with dims`

```
// refresh chrome with dims
```

## L969 · `let mut quiet: u32 = 0;`

```
// Consecutive empty polls — the quiet period that gates prefetching.
```

## L982 · `mark = alloc_mark(); // small state mutations persist`

```
// small state mutations persist
```

## L987-989 · `iris.loading = true;`

```
// Say "loading" first, then do the work: a decode
// blocks this loop for seconds and nothing else
// would tell the user anything.
```

## L1000-1002 · `if quiet >= QUIET_POLLS {`

```
// Decode a neighbour once the user has stopped for a
// moment — ONE per round, then straight back to polling,
// so a click never waits behind more than a single image.
```

## L1007 · `alloc_reset(mark);   // decode buffers are transient`

```
// decode buffers are transient
```

## L1019 · `fn read_home_dir() -> String {`

```
// ── npkFS helpers ─────────────────────────────────────────────────────
```

## L1028-1029 · `fn cursor_pos() -> Option<(i32, i32)> {`

```
/// Pointer position in screen coordinates — the same space the mouse
/// events report. `None` while we don't have focus (the kernel refuses).
```

## L1036-1037 · `fn canvas_rect() -> Option<(i32, i32, i32, i32)> {`

```
/// The canvas widget's laid-out rect (screen coords). `None` until the
/// first scene with a Canvas has been laid out.
```

## L1054 · `fn list_images(dir: &str) -> Vec<String> {`

```
/// List `.png` files (non-recursive) in `dir`, sorted by name.
```

## L1076 · `fn split_path(path: &str) -> (&str, &str) {`

```
/// Split "a/b/c.png" → ("a/b", "c.png"). No slash → ("", name).
```

## L1084-1085 · `fn decode_png(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {`

```
// ── PNG decoder (ported from wallpaper) ───────────────────────────────
// 8-bit RGB/RGBA, non-interlaced. Returns (BGRA, width, height).
```

## L1090-1100 · `fn decode_png_cb<F>(data: &[u8], mut on_rows: F) -> Option<(Vec<u8>, u32, u32)>`

```
/// Decode with progress. `on_rows` is handed the (still incomplete) BGRA
/// buffer every time another band of scanlines is finished, so a viewer
/// can put pixels on screen long before the picture is done.
///
/// Why bands and not a low-resolution preview: a PNG holds no smaller
/// version of itself, and the pixels cannot be sampled — the whole file
/// is ONE deflate stream, and every scanline's filter refers to the one
/// above it. Row 500 is unreachable except through rows 0..499. What the
/// format does give us is that the stream arrives in order, so the top of
/// the picture is genuinely ready while the bottom is still compressed.
/// We were simply throwing that away until the last byte arrived.
```

## L1129 · `if d[10] != 0 || d[11] != 0 || d[12] != 0 { return None; } // compression/filter/interlace`

```
// compression/filter/interlace
```

## L1137 · `pos = end + 4; // +4 CRC`

```
// +4 CRC
```

## L1155-1161 · `let mut bgra = alloc::vec![0u8; pixel_count * 4];`

```
// Opaque black underneath, so the part that has not arrived yet reads
// as a neutral band rather than as transparent garbage.
// No alpha pre-fill: the conversion writes all four bytes of every
// pixel. Pre-filling was needed while half-decoded pictures went on
// screen; now that nothing partial is shown it was two million loop
// iterations of pure waste per image — and it sat inside the phase we
// have spent all afternoon trying to speed up.
```

## L1174-1178 · `let bands = 12usize;`

```
// Feed the compressed stream in slices so finished scanlines can be
// handed on while the rest is still packed. Twelve bands is a
// compromise: fine enough that the first pixels show up in about a
// tenth of the total time, coarse enough that the extra full-buffer
// uploads stay in the noise.
```

## L1222-1223 · `fn unfilter_rows(decompressed: &[u8], unfiltered: &mut [u8],`

```
/// Un-filter scanlines `[from, to)` in place. Resumable: everything it
/// needs about earlier rows is already in `unfiltered`.
```

## L1232-1234 · `let (done, rest) = unfiltered.split_at_mut(dst);`

```
// Split so the row above is an immutable slice and the current row
// a mutable one: the predictors then read from a plain slice
// instead of re-indexing the whole image buffer.
```

## L1241-1243 · `fn rows_to_bgra(unfiltered: &[u8], bgra: &mut [u8],`

```
/// Convert scanlines `[from, to)` to BGRA. One 32-bit store per pixel
/// instead of four byte stores; BGRA in memory is little-endian
/// B | G<<8 | R<<16 | A<<24.
```

## L1271-1283 · `fn unfilter_row(filter: u8, cur: &mut [u8], src: &[u8], above: Option<&[u8]>, channels: usize) {`

```
// ── PNG un-filtering, one row at a time ───────────────────────────────
//
// The filter type is constant per scanline, so it is resolved once per
// row, not once per byte. Inside a row the two left-hand predictors (`a`
// = the pixel just written, `c` = the one above it) are carried in
// locals instead of being read back out of the image buffer — only `b`
// is an actual load. The channel count is a const parameter so the
// per-channel loop unrolls and the array indices become constants.
//
// This is the hot loop of the whole viewer: real PNGs use Paeth and
// Average for nearly every row (measured on our own wallpapers: 886 of
// 1080 rows Paeth, 166 Average, none unfiltered), so the naive version
// spent ~1.5 s per image here.
```

## L1286 · `if channels == 3 {`

```
// color_type is validated as 2 (RGB) or 6 (RGBA) before we get here.
```

## L1296-1297 · `(0, _) | (2, None) => cur.copy_from_slice(src),`

```
// None — and every predictor on the first row where "above"
// reads as zero and the left one is absent.
```

## L1300-1301 · `(1, _) | (4, None) => {`

```
// Sub: left neighbour. Paeth on the first row is the same thing,
// because paeth(a, 0, 0) == a.
```

## L1313 · `(2, Some(up)) => {`

```
// Up: the byte directly above. No left-hand state at all.
```

## L1320 · `(3, up) => {`

```
// Average: mean of left and above (floor).
```

## L1349 · `(4, Some(up)) => {`

```
// Paeth: left / above / above-left predictor.
```

## L1367-1368 · `_ => cur.copy_from_slice(src),`

```
// Unknown filter byte — treat the row as unfiltered rather than
// failing the whole image.
```

## L1381 · `#[allow(dead_code)]`

```
// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
```

