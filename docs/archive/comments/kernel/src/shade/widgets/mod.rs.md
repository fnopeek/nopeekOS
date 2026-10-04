# `kernel/src/shade/widgets/mod.rs` @ 5e0102684

## L1-21 · `pub mod abi;`

```
//! Widget pipeline — declarative GUI for WASM apps.
//!
//! Apps describe **what** to render (widget tree); Shade owns **how**
//! (layout, rasterization, GPU compositing, animation, theming).
//!
//! See docs/archive/PHASE10_WIDGETS.md for the full spec.
//!
//! Phase map:
//!   P10.0 — abi, tile, check_abi, ggtt_layout constants
//!   P10.1 — SDK crate + font metrics (gui/text.rs)
//!   P10.2 — npk_scene_commit host fn + deserialize + serial dump
//!   P10.3 — layout (flexbox-lite) with real font metrics
//!   P10.4 — GGTT slab allocator
//!   P10.5 — CPU rasterization + first visible pixels
//!   P10.5b (this file) — widget-kind windows first-class in shade
//!   P10.6 — diff + per-app cache
//!   P10.7 — event routing
//!   P10.8 — animation (fixed-point Q16.16)
//!   P10.9 — icon atlas
//!   P10.10 — Canvas escape hatch
//!   P10.11 — first real app (file browser)
```

## L40-47 · `#[derive(Clone, Debug, Default)]`

```
// ── Focused-Input editor state ────────────────────────────────────────
//
// When a `Widget::Input` is the focus target, the compositor — not the
// app — owns the buffer + caret. Printable keys, Backspace, Delete,
// Left/Right/Home/End all mutate this struct without going through the
// WASM event queue, so cursor moves stay 60 Hz responsive even if the
// app is busy. The app sees a single `Event::InputChange { value }`
// per buffer mutation; pure cursor moves emit nothing.
```

## L51-55 · `pub value:  String,`

```
/// Current buffer contents — the source of truth while focused.
/// `Widget::Input.value` in the cached tree may lag by one round-
/// trip (the app hasn't echoed the InputChange back yet). The
/// render walker pulls from here, not from the tree, so the user
/// always sees what they just typed.
```

## L57-58 · `pub cursor: usize,`

```
/// Caret position as a byte index into `value`. Always at a UTF-8
/// boundary; v1 only inserts ASCII so `byte_index == char_index`.
```

## L60-67 · `pub pending: [u8; 4],`

```
/// Angefangene UTF-8-Folge. **Die Tastaturleitung traegt ein Byte je
/// Ereignis, und `ü` sind zwei** — das erste Byte allein ist noch kein
/// Zeichen und darf nicht in `value`, sonst stuende dort kein gueltiges
/// UTF-8. Hier liegt es, bis die Folge vollstaendig ist. Fuer ASCII
/// bleibt es immer leer.
///
/// Kernel-seitig und NICHT auf dem Draht: die App sieht nur fertige
/// Zeichen, ueber `Event::InputChange`.
```

## L70-73 · `pub sel_anchor: Option<usize>,`

```
/// Selection anchor (byte index) — the fixed end of a text selection;
/// the caret is the moving end. `None` = no selection. Set by Shift+
/// movement and mouse-drag, cleared by any plain (non-Shift) move or
/// edit. The selected range is `min(anchor, cursor)..max(anchor, cursor)`.
```

## L83 · `pub fn selection(&self) -> Option<(usize, usize)> {`

```
/// Selected byte range `[start, end)` if a non-empty selection exists.
```

## L94-100 · `pub struct WidgetScene {`

```
// ── Per-window widget scene storage ───────────────────────────────────
//
// Every widget-kind shade window has exactly one WidgetScene here,
// keyed by WindowId.0 (u32). The scene holds the last-rendered
// pixels (blitted into the window's content rect by shade's
// render_window) plus the tree + layout cached for resize-driven
// re-render (the app may have already exited).
```

## L106-108 · `pub origin_x:    i32,`

```
/// Content-rect origin in screen coordinates at the time of
/// the last render. Used to detect whether a re-render is
/// needed when shade redraws the window.
```

## L111-113 · `pub tree:        abi::Widget,`

```
/// Cached tree + layout so the compositor can re-layout on
/// resize without the app committing again. Widget-apps are
/// allowed to exit after a single commit.
```

## L116-120 · `pub anchors:     alloc::collections::BTreeMap<u32, abi::Rect>,`

```
/// NodeId → screen rect, populated for any widget tagged with
/// `Modifier::NodeId`. Used by `Widget::Popover`'s anchor lookup
/// at layout time and by the click-outside-dismiss test in
/// hit_test (clicks on an anchor must NOT fire on_dismiss —
/// the anchor's own OnClick handles toggle).
```

## L122-125 · `pub popovers:    Vec<layout::PopoverLayout>,`

```
/// Floating popover overlays in declaration order. Painted
/// last (top z-order); hit-tested before the main tree so a
/// click on a popover never falls through to whatever's
/// underneath it.
```

## L127-130 · `pub payload_hash: [u8; 32],`

```
/// blake3 hash of the postcard payload that produced this
/// scene. P10.6: lets scene_commit short-circuit when an app
/// resubmits the same tree (common with interactive apps that
/// re-commit on every event loop iteration).
```

## L132-135 · `pub hover_path:  Vec<u32>,`

```
/// Path of child indices from the root to the currently hovered
/// layout node — empty until the cursor enters the window. Used
/// by the render walker to merge `Modifier::Hover` inner mods on
/// the matching node.
```

## L137-139 · `pub focus_path:  Vec<u32>,`

```
/// Path of the currently focused widget (Tab-stop, keyboard
/// destination). Empty = no focus. Set by click-to-focus and
/// future Tab navigation.
```

## L141-143 · `pub active_path: Option<Vec<u32>>,`

```
/// Path of the widget under an active mouse-button press.
/// `None` = no button down. Cleared on button release. Drives
/// `Modifier::Active(…)` style.
```

## L145-147 · `pub density:     abi::Density,`

```
/// Compositor-classified container size bucket for this window —
/// drives `Modifier::WhenDensity(d, …)` matching. Recomputed on
/// commit and on resize.
```

## L149-152 · `pub has_pseudo:  bool,`

```
/// Cached: tree contains at least one Hover/Focus/Active/Disabled/
/// WhenDensity modifier. Lets `update_hover` skip re-rasterization
/// for trees that have no state-driven visuals — avoids
/// re-rendering on every mouse move.
```

## L154-156 · `pub parked_edit: Option<(Vec<u32>, InputEditState)>,`

```
/// Last edit buffer set aside when focus left a text widget, with
/// the path it belonged to. Refocusing that same widget resumes it
/// instead of rebuilding from the app's (one-round-trip-stale) tree.
```

## L158-161 · `pub input_edit:  Option<InputEditState>,`

```
/// Compositor-owned text-editor state, populated when `focus_path`
/// targets a `Widget::Input`. None means no Input is focused (or
/// the focused widget isn't an Input). Drives caret render +
/// keyboard intercept in `handle_input_key`.
```

## L163-165 · `pub scroll_y:     u32,`

```
/// Current vertical scroll offset (px) applied to this window's
/// `Widget::Scroll` content, driven by the mouse wheel. Clamped to
/// `max_scroll_y` after each layout.
```

## L167-169 · `pub scroll_x:     u32,`

```
/// Horizontal offset of the TextArea, in px. Only an editor scrolls
/// sideways — `Widget::Scroll` handles its own axis in layout, so this
/// lives here rather than in the layout pass.
```

## L171-172 · `pub max_scroll_y: u32,`

```
/// Largest legal `scroll_y` from the last layout (content − viewport
/// over the tallest vertical Scroll). Zero → nothing scrolls.
```

## L174-175 · `pub scroll_viewport: abi::Rect,`

```
/// Screen rect of the scrollable viewport (from layout) — the
/// compositor hit-tests its right edge for scrollbar dragging.
```

## L177-178 · `pub max_scroll_x: u32,`

```
/// Largest legal `scroll_x` (widest line − text column). Zero → the
/// document fits sideways and no horizontal bar is drawn.
```

## L180-181 · `pub scroll_viewport_x: abi::Rect,`

```
/// The text column `scroll_x` moves in — bar track and the strip the
/// compositor hit-tests along its bottom edge.
```

## L187-189 · `pub fn with_scene<F, R>(window_id: u32, f: F) -> Option<R>`

```
/// Look up a scene's pixel buffer for blitting. Returned pointer +
/// dimensions are valid as long as `SCENES` is not mutated — caller
/// must finish the blit before releasing the lock.
```

## L196-198 · `pub fn scroll_by(window_id: u32, delta: i32) -> bool {`

```
/// Adjust a widget window's vertical scroll by `delta` px (positive =
/// content moves up / scroll down). Clamped to `[0, max_scroll_y]`.
/// Re-renders + marks dirty on change. Returns true if the offset moved.
```

## L212-213 · `pub fn set_scroll(window_id: u32, y: u32) -> bool {`

```
/// Set the absolute vertical scroll offset (px), clamped. Used by the
/// scrollbar drag. Re-renders on change; returns true if it moved.
```

## L226-227 · `pub fn scroll_viewport_of(window_id: u32) -> Option<(abi::Rect, u32)> {`

```
/// Scrollable viewport rect + max offset (px) for a window's scrollbar,
/// or None if nothing scrolls. Used by the compositor's scrollbar hit-test.
```

## L235-236 · `pub fn scroll_by_x(window_id: u32, delta: i32) -> bool {`

```
/// Adjust the sideways offset by `delta` px (positive = content moves left).
/// Clamped to `[0, max_scroll_x]`. Returns true if it moved.
```

## L250-251 · `pub fn set_scroll_x(window_id: u32, x: u32) -> bool {`

```
/// Set the absolute sideways offset (px), clamped. Used by the horizontal
/// scrollbar drag. Returns true if it moved.
```

## L264-265 · `pub fn scroll_viewport_x_of(window_id: u32) -> Option<(abi::Rect, u32)> {`

```
/// Text column + max sideways offset for the horizontal scrollbar, or None
/// when the document fits. Counterpart to `scroll_viewport_of`.
```

## L273-274 · `pub fn remove_scene(window_id: u32) {`

```
/// Drop a scene. Called from compositor::close_window when the
/// widget-kind window is destroyed.
```

## L282-289 · `const MAX_EVENTS_PER_WINDOW: usize = 64;`

```
// ── Per-window event queues (P10.7) ───────────────────────────────────
//
// Widget apps poll events via `npk_event_poll`. Shade pushes Events
// here: mouse clicks after hit-testing against the scene's layout
// tree, keyboard-forwarded keys once focused, focus changes.
//
// Queue is bounded; on overflow we drop the oldest entry so a slow
// app can't wedge the compositor.
```

## L296-297 · `pub fn push_event(window_id: u32, event: abi::Event) {`

```
/// Push an event into the window's queue. Oldest is dropped on
/// overflow (bounded queue).
```

## L310-312 · `static EVENT_WAKERS: Mutex<BTreeMap<u32, crate::smp::fiber::Waker>> =`

```
/// The fiber of the app that owns each widget window, registered when the
/// app waits (`npk_wait`). An event wakes it at once instead of on its next
/// poll — the app used to look every 16 ms whether or not anything happened.
```

## L327 · `pub fn has_event(window_id: u32) -> bool {`

```
/// Is an event waiting for this window?
```

## L332 · `pub fn poll_event(window_id: u32) -> Option<abi::Event> {`

```
/// Non-blocking event pop. Returns None if queue is empty.
```

## L339-342 · `pub fn widget_window_exists(window_id: u32) -> bool {`

```
/// True if a widget-kind window with this id still exists in the
/// compositor. Host fn `npk_event_poll` uses this to distinguish
/// "queue empty" from "window closed" — the app turns the latter
/// into an exit signal rather than polling forever.
```

## L351-352 · `pub fn remove_event_queue(window_id: u32) {`

```
/// Called by compositor::close_window alongside remove_scene to
/// drop the now-orphaned queue.
```

## L355-356 · `wake_window_app(window_id);`

```
// The app may be parked in `npk_wait`: wake it so it sees the window is
// gone and leaves its loop.
```

## L362-366 · `if let Some(session) = take_pick(window_id) {`

```
// A picker closed by its window button (rather than by picking) still
// owes its requester an answer, or the app waits for a dialog that no
// longer exists. Report it as a cancel. `take_pick` first so the
// session is gone before `finish_pick` touches the event queue —
// EVENT_QUEUES is already released by then.
```

## L372-379 · `static CLIPBOARD_SINKS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());`

```
// ── Clipboard-sink opt-in ─────────────────────────────────────────────
//
// A window that manages its own selection (e.g. loft's file grid) opts in
// via `npk_window_set_clipboard_sink`. For a sink window, a Ctrl+C/X/V
// chord that a focused text widget can't act on (copy/cut with no text
// selection, paste into an empty single-line Input) is delivered to the
// app as `Event::Clipboard` instead of being swallowed. Non-sink windows
// keep the original behaviour, so existing apps' text paste is unaffected.
```

## L382 · `pub fn set_clipboard_sink(window_id: u32) {`

```
/// Mark `window_id` as a clipboard sink (idempotent).
```

## L387 · `pub fn is_clipboard_sink(window_id: u32) -> bool {`

```
/// True if `window_id` opted into `Event::Clipboard` delivery.
```

## L393-396 · `static WINDOW_CAPS: Mutex<BTreeMap<u32, crate::security::capability::CapId>> =`

```
/// Capability of the app that owns a widget window. Recorded on scene
/// commit, so a path grant can be routed to an app that is ALREADY
/// running — `npk_open` on a live singleton delivers `Event::Open`
/// rather than spawning, and the grant has to follow the same way.
```

## L408-419 · `static CLOSE_GUARDS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());`

```
// ── Close guard ───────────────────────────────────────────────────────
//
// Without this, Mod+Q and the title-bar X call `close_window` straight
// away and an app with unsaved work never gets asked — it only learns
// its window is gone when `npk_event_poll` returns -1, far too late to
// prompt. This is our `WM_DELETE_WINDOW`.
//
// Opt-in, so every existing app keeps closing instantly. A guarded
// window gets `Event::CloseRequest` and is expected to call
// `npk_close_widget` when it's done. It is NOT a veto: a second close
// gesture, or `CLOSE_GRACE_TICKS` of silence, closes it regardless — an
// app must never be able to pin a window open.
```

## L421 · `static CLOSE_GUARDS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());`

```
/// Windows that asked to be consulted before closing.
```

## L424-425 · `static PENDING_CLOSE: Mutex<BTreeMap<u32, (u64, bool)>> = Mutex::new(BTreeMap::new());`

```
/// Guarded windows with a close request outstanding: window → (tick it
/// was sent, whether the app has shown any sign of life since).
```

## L428 · `pub fn close_pending() -> bool {`

```
/// A close request waits for its answer (or its grace period).
```

## L433-439 · `pub const CLOSE_GRACE_TICKS: u64 = 200;`

```
/// How long a guarded window has to REACT to `CloseRequest` before the
/// compositor closes it anyway. Ticks run at 100 Hz, so ~2 s.
///
/// This is a liveness check, not a patience limit: the moment the app
/// commits a frame after being asked, the deadline is cancelled and it
/// can take as long as the user needs to answer the dialog. Only an app
/// that never reacts at all gets closed out from under itself.
```

## L451-453 · `pub fn begin_close_request(window_id: u32, now: u64) -> bool {`

```
/// Record that `window_id` was asked to close at `now`. Returns false if
/// a request was already outstanding — the caller takes that as "the user
/// asked twice" and closes for real.
```

## L461-463 · `pub fn note_close_request_alive(window_id: u32) {`

```
/// A guarded window drew a frame — proof it is processing events. Cancels
/// the liveness deadline while leaving the request outstanding, so a
/// second close gesture still closes immediately.
```

## L470 · `pub fn expired_close_requests(now: u64) -> alloc::vec::Vec<u32> {`

```
/// Guarded windows that never reacted within the deadline.
```

## L483-492 · `static PICK_SESSIONS: Mutex<BTreeMap<u32, PickSession>> = Mutex::new(BTreeMap::new());`

```
// ── File-picker portal ────────────────────────────────────────────────
//
// An app that wants to open or save a file calls `npk_pick`, which spawns
// the picker module in its own floating window. The picker browses npkFS
// (it needs READ; the requester does not) and reports the chosen path via
// `npk_pick_result`. The kernel then hands the requester an
// `Event::Picked` — so a user's click in a trusted picker, not a blanket
// right, is what points an app at a file.
//
// The picker never writes. It returns a path; the requester does the I/O.
```

## L494 · `static PICK_SESSIONS: Mutex<BTreeMap<u32, PickSession>> = Mutex::new(BTreeMap::new());`

```
/// Live picker sessions, keyed by the picker window's id.
```

## L499 · `pub requester: u32,`

```
/// Widget window of the app that asked for the dialog.
```

## L501 · `pub tag: u32,`

```
/// Caller-owned correlation value, returned in `Event::Picked`.
```

## L503-504 · `pub cap: crate::security::capability::CapId,`

```
/// Capability of the requesting instance — the grant for the chosen
/// path is written against it, so it dies when that instance does.
```

## L506 · `pub writable: bool,`

```
/// Save mode: the grant includes WRITE. Open mode: READ only.
```

## L510-511 · `pub fn register_pick(picker: u32, requester: u32, tag: u32,`

```
/// Register a picker window as belonging to `requester`. Called by
/// `npk_pick` right after it creates the picker's window.
```

## L517-518 · `pub fn has_open_pick(requester: u32) -> bool {`

```
/// True if `requester` already has a picker open — one dialog per app, so
/// a repeated click can't stack windows.
```

## L523-525 · `pub fn is_open_pick(picker: u32) -> bool {`

```
/// True if `picker` is a live picker window. Same authorisation check as
/// `take_pick`, without consuming the session — for verbs a dialog may
/// use repeatedly while it's open (`npk_pick_mkdir`).
```

## L530-537 · `pub fn take_picks_for_requester(requester: u32) -> alloc::vec::Vec<u32> {`

```
/// Drop every session belonging to `requester` and return their picker
/// windows, so the caller can close them.
///
/// A dialog belongs to the window that opened it: once the requester is
/// gone there is nobody left to hand a path to, and a picker left behind
/// still looks usable — the user picks a file and nothing happens. The
/// sessions are removed here, before the windows close, so the picker's
/// own teardown doesn't try to report a cancel to the dead requester.
```

## L548-551 · `pub fn take_pick(picker: u32) -> Option<PickSession> {`

```
/// Consume the session owned by picker window `picker`. Returns None if
/// the caller isn't a registered picker — which is also the authorisation
/// check for `npk_pick_result`: only a window the kernel itself spawned as
/// a picker can deliver a result.
```

## L556 · `pub fn finish_pick(session: PickSession, path: alloc::string::String) {`

```
/// Deliver the picker's answer to the requester. Empty `path` = cancelled.
```

## L558-560 · `if !path.is_empty() {`

```
// The click IS the authorisation: hand the requester a right to this
// one path, so it can open or save it without holding a blanket one.
// Nothing is granted on a cancel (empty path).
```

## L569-580 · `pub fn hit_test(window_id: u32, x: i32, y: i32) -> Option<abi::ActionId> {`

```
/// Deepest widget at (x, y) that declares an OnClick — returns the
/// ActionId the app should see. Screen-absolute coordinates (same
/// system as `LayoutNode.rect`).
///
/// Walk order: recurse into children first (deepest first), so a
/// Button inside a Row's rect wins over the Row itself. Falls back
/// to the variant's built-in click id (Widget::Button.on_click) if
/// no OnClick modifier is present.
///
/// Disabled-propagation: if any ancestor (inclusive) has
/// `Modifier::Disabled(_)`, the click is swallowed — disabled widgets
/// eat events for themselves AND their descendants.
```

## L585-591 · `for p in scene.popovers.iter().rev() {`

```
// Popovers (declared overlays) hit-test FIRST so a click inside
// an open menu dropdown lands on the menu item, not on whatever
// was rendered underneath. Iterate in reverse-declaration order
// so a popover declared later (= painted on top) wins. We don't
// recurse with `find_click_target` here because the popover's
// child tree is rooted at its own LayoutNode — same shape as the
// main pass, just an isolated subtree.
```

## L600-603 · `if !scene.popovers.is_empty() {`

```
// Click outside every popover BUT one or more popovers are open
// → fire the topmost popover's `on_dismiss`, unless the click
// landed on its anchor (the anchor's own OnClick handles toggle
// and we don't want the dismiss action to also fire and race).
```

## L608-609 · `return Some(scene.popovers.last().unwrap().on_dismiss);`

```
// Last-declared popover is the most-recently-opened —
// its on_dismiss matches what the app's state expects.
```

## L612-613 · `}`

```
// Click landed on an anchor — fall through to normal routing
// so the anchor's OnClick fires.
```

## L638-639 · `static LAST_HOVER: Mutex<BTreeMap<u32, abi::ActionId>> = Mutex::new(BTreeMap::new());`

```
// Deduplicated OnHover target per window. Compositor calls update_hover
// on every MouseMove; we push Event::Action only when the hit changes.
```

## L650-651 · `fn classify_density(window_w: u32) -> abi::Density {`

```
/// Compositor-side density classifier. Thresholds live here once;
/// apps reference them only via `Modifier::WhenDensity(Density, …)`.
```

## L658-661 · `fn find_hover_path(`

```
/// Walk the layout tree and collect the chain of child indices that
/// leads to the deepest node still containing (x, y). Returns the
/// path; empty path = (x, y) is inside the root only. `None` means
/// the point falls outside the root entirely.
```

## L706-709 · `fn find_focusable_path(`

```
/// Walk down to the deepest focusable widget under (x, y). Returns
/// the path of child indices from root, or `None` if no focusable
/// node lives there. Disabled widgets and their descendants are
/// skipped.
```

## L731 · `let kids = widget_children_ref(widget);`

```
// Children first — deepest focusable wins.
```

## L742 · `is_focusable(widget)`

```
// No child took it — am I focusable myself?
```

## L746-760 · `fn find_field_input_path(`

```
/// Path to a text field whose *surrounding row* was clicked.
///
/// An `Input` is only as wide as its text (over a 120 px floor), while the
/// chrome a user reads as "the field" — the border, the padding, the label
/// beside it — belongs to the container. Hit-testing the Input alone means
/// you have to strike the glyphs themselves; a click on the empty right
/// half of an address bar landed on the container, counted as "clicked
/// nothing", and cleared focus instead of granting it.
///
/// So: descend to the deepest node under the cursor, and treat it as a
/// field row if its subtree holds **exactly one** text widget and **no
/// other click target**. That covers a bordered Row wrapping one Input
/// (and makes its label clickable, like an HTML `<label for>`), while a
/// panel or toolbar — which always carries buttons or clickable rows —
/// is disqualified, so clicking empty chrome still releases focus.
```

## L778 · `let kids = widget_children_ref(widget);`

```
// Deepest node under the cursor wins, same order as the focus walk.
```

## L787 · `if subtree_click_targets(widget) > 0 { return false; }`

```
// Nothing deeper claimed it — is this node a field row?
```

## L799-800 · `fn subtree_click_targets(widget: &abi::Widget) -> usize {`

```
/// Widgets in this subtree that act on a click of their own (buttons,
/// anything carrying `OnClick`). A field row has none.
```

## L811-812 · `fn collect_text_widgets(`

```
/// Record the path to the single text widget in a subtree; `count` says
/// how many were seen, so callers can reject ambiguous rows.
```

## L831 · `fn widget_at_path_mut<'a>(tree: &'a mut abi::Widget, path: &[u32])`

```
/// Mutable twin of `widget_at_path`.
```

## L843-856 · `fn set_input_value_at(tree: &mut abi::Widget, path: &[u32], value: &str) -> bool {`

```
/// Write the editor's buffer into the compositor's copy of the tree.
///
/// The editor leads the app's tree by one round-trip by design: what you
/// typed lives in `input_edit`, and the app's `value` only catches up when
/// it re-commits. An app that doesn't re-commit on every keystroke (beak's
/// address bar, for one — a full chrome commit per character is not free)
/// therefore still holds the OLD value. While the field is focused nobody
/// notices, because the editor buffer is what gets painted. The moment
/// focus leaves, the paint falls back to the tree and the text appears to
/// vanish — though it was never lost, only unparked on the way back.
///
/// So on the way out we push the buffer into the tree. The next real
/// `scene_commit` overwrites it, which is correct: an app that states a
/// value outranks a stale keystroke.
```

## L871-872 · `motion_pulse(window_id, x, y);`

```
// First: every step below may return early when the hover target did
// not change — and an unchanged target is exactly when motion matters.
```

## L875-877 · `let new_path: Option<Vec<u32>> = {`

```
// Step 1 — recompute the hover path against the cached layout tree.
// Cheap (one descent), avoids re-rendering on hover moves that
// don't actually cross node boundaries.
```

## L887-890 · `let path_changed = {`

```
// Step 2 — diff against the cached hover_path. If unchanged, skip.
// If changed AND the tree has any pseudo-state-aware modifier,
// re-rasterize using the new path; otherwise just update the path
// (hover events still fire below).
```

## L907 · `if let Some(s) = SCENES.lock().get_mut(&window_id) {`

```
// Just bump the cached path so the next move diffs cleanly.
```

## L914-915 · `let new_id = hover_test(window_id, x, y);`

```
// Step 3 — fire the OnHover action event (existing semantics:
// dedup on ActionId, push only when target action changes).
```

## L935 · `static LAST_MOTION: Mutex<BTreeMap<u32, (abi::ActionId, i32, i32, u64)>> =`

```
// Last `OnMotion` pulse per window: (target, position, tick).
```

## L954-957 · `fn motion_pulse(window_id: u32, x: i32, y: i32) {`

```
/// `Modifier::OnMotion`: one Action per `MOTION_INTERVAL_MS` while the
/// pointer actually moves, immediately on a change of target. Called on
/// every hover update, which also runs when nothing moved — so the
/// position is compared, not just the clock.
```

## L981-983 · `fn rerender_scene_pixels(window_id: u32, hover_path: &[u32]) {`

```
/// Re-rasterize a scene's pixel buffer with the given hover path (focus /
/// active come from the cached scene). Does NOT request a repaint — the
/// caller decides full vs. damage-rect. Locks SCENES internally.
```

## L1032-1033 · `fn rerender_with_state(window_id: u32, hover_path: &[u32]) {`

```
/// Re-rasterize with the given hover path AND request a full repaint.
/// Used by focus/active state changes (rare, not the per-move hot path).
```

## L1039-1040 · `fn mark_dirty(window_id: u32) {`

```
/// Helper: mark the window dirty + request a render. Used by every
/// state-driven re-rasterize path.
```

## L1050-1061 · `#[must_use]`

```
/// Mouse-button-down at (x, y) on a widget window: move focus to the
/// deepest focusable widget at the cursor and start the active state.
/// Re-rasterizes the scene if it has any pseudo-state visuals.
///
/// Returns `true` if the scene was re-rasterized — caller MUST then
/// mark the window dirty. We do NOT call `with_compositor` here
/// because this function runs while the caller already holds the
/// compositor lock (deadlock-prone).
///
/// Doesn't push the click action — that's still hit_test's job at the
/// existing call site (kept separate so apps that wire their own
/// click-routing aren't affected by state mechanics).
```

## L1064-1078 · `let mut via_field_row = false;`

```
// Decide focus + active for the press.
//
// Focus is *only* moved when the click lands on a `Widget::Input`.
// For every other focusable (Button, OnClick'd container, sidebar
// nav_row, menu-bar label, …) we fire the click action but leave
// focus where it was — keeps the keyboard caret on the search
// input across mouse navigation, which is exactly what loft /
// settings-style apps want. Tab/Shift+Tab still walk every
// focusable; click is just no longer a way to *land* keyboard
// focus on a non-Input.
//
// Active still tracks the press target so `:active` state on
// buttons works visually during mouse-down.
// Set when focus came from the chrome around a field rather than from
// the glyphs — decides where the caret lands (see below).
```

## L1087-1091 · `let new_focus_opt: Option<Option<Vec<u32>>> = match press_path.as_ref() {`

```
// One rule, everywhere: a press inside a text widget focuses it,
// a press anywhere else releases focus. Previously only a press
// into a Canvas released it — which is why the browser (whose
// page IS a canvas) felt right while the file browser trapped the
// keyboard in its search box with no way out but Tab.
```

## L1097-1099 · `_ => match find_field_input_path(&scene.tree, &scene.layout_tree, x, y) {`

```
// Not on the text itself — but possibly on the field around
// it. A field is the whole row a user sees, not just the run
// of glyphs inside it.
```

## L1132-1133 · `let leaving = s.focus_path.clone();`

```
// Capture the OUTGOING path before it is overwritten — the
// parked buffer is keyed by the field it came from.
```

## L1139-1144 · `if s.focus_path.is_empty() {`

```
// The editor buffer leads the app's tree by one round-trip BY
// DESIGN, so discarding it on focus loss threw away whatever
// had been typed since the app last committed: the text
// vanished on click-away and only came back once Enter made
// the app re-commit. Park it under the field it belongs to,
// and hand it back when that same field is focused again.
```

## L1147-1148 · `if set_input_value_at(&mut s.tree, &leaving, &edit.value) {`

```
// Keep what was typed on screen — see
// `set_input_value_at`.
```

## L1160-1165 · `if via_field_row {`

```
// Click ON the text positions the caret exactly there
// (`text_select_begin`, which needs the press inside the
// Input's own rect). A click on the field's chrome has no
// glyph to aim at, so the caret goes to the end — where
// you'd continue typing. Overrides a resumed caret too:
// the click is the more recent intent.
```

## L1186-1191 · `#[must_use]`

```
/// Move focus to the next focusable widget in document order
/// (Tab semantics). Wraps around at the end. Returns `true` if a
/// re-render happened.
///
/// Called from outside the compositor lock (intent loop key path),
/// so this one is allowed to mark the window dirty itself.
```

## L1197 · `#[must_use]`

```
/// Move focus to the previous focusable widget (Shift+Tab).
```

## L1230 · `s.input_edit = compute_input_edit(&s.tree, &s.focus_path, None);`

```
// Tab onto an Input → init the editor; Tab off Input → drop it.
```

## L1238-1239 · `let needs_caret_render = SCENES.lock().get(&window_id)`

```
// Even without pseudo-state mods, we need to re-render to show
// the cursor caret on the newly-focused Input.
```

## L1250-1251 · `fn collect_focusable_paths(`

```
/// DFS-collect all focusable widget paths in document order. Used
/// for Tab traversal. Disabled subtrees are pruned.
```

## L1258 · `let _ = layout; // Same shape as the widget tree; kept for symmetry.`

```
// Same shape as the widget tree; kept for symmetry.
```

## L1272-1273 · `#[must_use]`

```
/// Mouse-button-up: clear active state. Focus persists. Returns
/// `true` if a re-render happened — caller marks the window dirty.
```

## L1295-1298 · `fn rerender_state_only(window_id: u32) {`

```
/// Re-rasterize using the cached focus/active/hover paths and update
/// the scene's pixel buffer. Does NOT touch the compositor (caller is
/// responsible for marking the window dirty). Pure scene-state work,
/// safe to call while the compositor lock is held.
```

## L1347-1354 · `pub fn suppress_hover(window_id: u32) {`

```
/// Drop the cached hover_path and the OnHover-action dedup so the next
/// render no longer applies Hover-state modifiers and the next mouse
/// move re-fires the OnHover action even at the same position.
///
/// Called when keyboard navigation should take visual precedence over
/// a stale mouse position — without this, moving the keyboard cursor
/// to row N leaves the original mouse-hovered row M still highlighted
/// and both states render at once.
```

## L1380-1382 · `if disabled { return; }`

```
// Disabled subtrees swallow clicks entirely — neither this node
// nor its children fire actions, even if a descendant has its own
// OnClick.
```

## L1385 · `for (cw, cl) in widget_children_ref(widget).iter().zip(layout.children.iter()) {`

```
// Children first — deepest hit wins.
```

## L1391-1392 · `for m in modifiers_of_ref(widget) {`

```
// Then self — check OnClick modifier + variant-native on_click
// (Button has a frozen on_click field).
```

## L1443-1444 · `fn widget_children_mut(w: &mut abi::Widget) -> alloc::vec::Vec<&mut abi::Widget> {`

```
/// Mutable twin of `widget_children_ref`. Same container set — anything
/// missing here would silently break path descent for the mutable walk.
```

## L1462 · `fn widget_at_path<'a>(tree: &'a abi::Widget, path: &[u32]) -> Option<&'a abi::Widget> {`

```
// ── Input self-editing helpers ────────────────────────────────────────
```

## L1464-1466 · `fn widget_at_path<'a>(tree: &'a abi::Widget, path: &[u32]) -> Option<&'a abi::Widget> {`

```
/// Walk `path` from `tree` and return the widget it targets. None if
/// any index is out-of-bounds or the path tries to descend through a
/// leaf.
```

## L1476-1479 · `fn find_first_input_path(tree: &abi::Widget) -> Option<Vec<u32>> {`

```
/// DFS for the first focusable text widget (`Input` or `TextArea`) in
/// document order. Used by `scene_commit` to auto-focus on first commit
/// so apps with a search bar (drun) or an editing surface (spell) never
/// need their own focus-priming host fn.
```

## L1483-1485 · `if matches!(w, abi::Widget::Input { .. } | abi::Widget::TextArea { .. })`

```
// Opt-in via `Modifier::Autofocus`. Grabbing the first Input we
// find is right for a launcher and wrong everywhere else: a file
// browser's search box swallowed every arrow key on open.
```

## L1504-1513 · `fn compute_input_edit(`

```
/// Decide the InputEditState that should sit on the scene after a
/// focus change or commit:
///
/// - `path` targets an Input AND `prev` already mirrors the same
///   buffer → keep `prev` (app round-tripped a prior InputChange,
///   cursor must not jump).
/// - `path` targets an Input with a different value (app overrode the
///   buffer programmatically) OR `prev` is None → fresh state with
///   the caret at the end of the new value.
/// - `path` doesn't target an Input → None.
```

## L1532-1537 · `fn line_start(s: &str, cursor: usize) -> usize {`

```
// ── Multi-line cursor helpers (TextArea) ──────────────────────────────
//
// Caret is a byte index into the whole `\n`-separated document. All
// helpers are UTF-8-safe (files carry umlauts) — results land on char
// boundaries, never mid-codepoint, so String::insert / remove can't
// panic.
```

## L1539 · `fn line_start(s: &str, cursor: usize) -> usize {`

```
/// Byte index of the start of the line containing `cursor`.
```

## L1544-1545 · `fn line_end(s: &str, cursor: usize) -> usize {`

```
/// Byte index of the end of the line containing `cursor` (the next
/// `\n`, or end-of-string).
```

## L1550 · `fn clamp_boundary(s: &str, mut i: usize) -> usize {`

```
/// Snap `i` down to the nearest char boundary ≤ len.
```

## L1557-1558 · `fn cursor_up(s: &str, cursor: usize) -> usize {`

```
/// Move the caret up one visual line, preserving the byte-column within
/// the line as closely as the shorter target line allows.
```

## L1561 · `if ls == 0 { return 0; } // already on the first line`

```
// already on the first line
```

## L1568 · `fn cursor_down(s: &str, cursor: usize) -> usize {`

```
/// Move the caret down one visual line, preserving byte-column.
```

## L1571 · `if le == s.len() { return s.len(); } // already on the last line`

```
// already on the last line
```

## L1579-1581 · `const CARET_MARGIN_PX: u32 = 24;`

```
/// Breathing room kept between the caret and the edge of the text column,
/// so typing at the end of a long line doesn't park the caret exactly on
/// the boundary where the next glyph would already be clipped.
```

## L1584-1592 · `fn caret_follow_scroll(window_id: u32) {`

```
/// Nudge a focused TextArea's scroll so the caret stays on screen — down
/// the lines and sideways along the current one. Called on every caret
/// move / edit (incl. paste, which can jump the caret far).
///
/// Vertical and horizontal differ in one way: the vertical offset is also
/// set by the wheel and the scrollbar drag, so it is clamped against
/// `max_scroll_y`. Sideways only the caret ever scrolls, and moving onto a
/// short line pulls the view back left on its own — so there is nothing to
/// clamp against and no `max_scroll_x` to keep in sync.
```

## L1597-1598 · `let (px, column) = match widget_at_path(&s.tree, &s.focus_path) {`

```
// Resolve the focused editor's metrics in one immutable pass — a zoomed
// editor scrolls in its own line height, not the default one.
```

## L1612 · `let (caret_line, caret_x) = match &s.input_edit {`

```
// Caret line + how far into that line it sits, in px.
```

## L1638-1639 · `if col_w > CARET_MARGIN_PX {`

```
// A column narrower than the margin can't hold the caret anywhere —
// leave the view where it is rather than fight over sub-pixels.
```

## L1652-1657 · `fn handle_clipboard_key(window_id: u32, letter: u8, is_textarea: bool) -> bool {`

```
/// Ctrl+C / X / V / A on a focused Input/TextArea. Copies/cuts the
/// selection to the kernel clipboard, pastes at the caret (replacing any
/// selection), or selects all. Returns `true` (the key is always consumed
/// once we're here). Emits `Event::InputChange` only when the buffer
/// actually changes (cut/paste), and always re-renders so a new selection
/// shows immediately.
```

## L1673 · `if !edit.value.is_empty() {`

```
// Select all (anchor at start, caret at end).
```

## L1702 · `Err(_) => return true, // ignore a non-UTF-8 paste`

```
// ignore a non-UTF-8 paste
```

## L1704 · `if !is_textarea {`

```
// An Input is single-line — strip newlines on paste.
```

## L1737-1742 · `use core::sync::atomic::{AtomicU32, Ordering};`

```
// ── Mouse text selection ──────────────────────────────────────────────
//
// Maps a screen pixel to a byte offset in the focused Input/TextArea, so a
// click positions the caret and a drag selects. The selection model
// (`sel_anchor` + `cursor`) is the same one Shift+movement and Ctrl+C/X/V
// already use — copy/paste come for free once the mouse sets it.
```

## L1746 · `static TEXT_DRAG: AtomicU32 = AtomicU32::new(0);`

```
/// Window id (+1) whose focused text widget is being drag-selected; 0 = none.
```

## L1749-1753 · `static LAST_CLICK: Mutex<Option<(u64, i32, i32, u32, u8)>> = Mutex::new(None);`

```
/// Der letzte Klick in ein Textfeld: `(Tick, x, y, Fenster, Zaehler)`.
///
/// Daraus wird der Doppel- und Dreifachklick abgeleitet. Die Uhr ist
/// `interrupts::ticks()` mit 100 Hz — 10 ms je Schritt, und ein Doppelklick
/// wird in Zehntelsekunden gemessen, nicht in Millisekunden.
```

## L1756-1757 · `const MULTI_CLICK_TICKS: u64 = 40;`

```
/// Zeitfenster fuer den naechsten Klick derselben Reihe: 40 Ticks = 400 ms.
/// Dasselbe, was jede Oberflaeche seit dreissig Jahren nimmt.
```

## L1759-1760 · `const MULTI_CLICK_SLOP: i32 = 4;`

```
/// Wie weit der Zeiger dabei wandern darf. Ohne diese Grenze wird aus zwei
/// Klicks an verschiedenen Stellen ein Doppelklick, und die Auswahl springt.
```

## L1763-1767 · `pub fn click_run_at(window_id: u32, x: i32, y: i32) -> u8 { click_run(window_id, x, y) }`

```
/// Der wievielte Klick dieser Reihe ist das? 1 = einzeln, 2 = doppelt,
/// 3 = dreifach (danach faengt es wieder bei 1 an).
///
/// **Wird bei JEDEM Druck gerufen, auch ausserhalb eines Textfelds** — sonst
/// zaehlt eine Reihe weiter, die laengst woanders stattfindet.
```

## L1785-1792 · `fn word_bounds(s: &str, at: usize) -> (usize, usize) {`

```
/// Die Wortgrenzen um `at` herum.
///
/// Ein „Wort" ist ein Lauf gleichartiger Zeichen: entweder alles
/// Buchstaben/Ziffern/`_`, oder alles andere. Genau so trennt jeder Browser
/// und jeder Editor, und fuer eine ADRESSE ist es die richtige Regel: ein
/// Doppelklick auf `arcade` in `https://www.arcade.ch/media/x.jpg` gibt
/// `arcade` und nicht die halbe Zeile, weil Punkt und Schraegstrich zur
/// anderen Klasse gehoeren.
```

## L1797-1798 · `let at = at.min(b.len());`

```
// Steht der Zeiger hinter dem letzten Zeichen, gehoert er zum Zeichen
// DAVOR — sonst waehlt ein Doppelklick am Zeilenende nichts aus.
```

## L1812-1815 · `fn padding_sum(mods: &[abi::Modifier]) -> u32 {`

```
/// Sum every `Modifier::Padding` — mirrors render's `leaf_padding` so the
/// inverse hit-mapping uses the same text origin the glyphs were drawn at.
/// Horizontal padding of a node — the caret hit-test needs the same
/// x-inset the renderer used, so it must count `PaddingXY.x` too.
```

## L1836-1838 · `fn byte_at_x(s: &str, style: abi::TextStyle, target_x: u32) -> usize {`

```
/// Byte offset in `s` whose glyph boundary sits nearest `target_x` px from
/// the text origin. Evaluates every char boundary (lines are short, so the
/// O(n) `measure` calls are cheap) and picks the closest.
```

## L1844-1845 · `fn byte_at_x_px(s: &str, style: abi::TextStyle, px: u16, target_x: u32) -> usize {`

```
/// `byte_at_x` at an explicit size — the zoomable TextArea needs the same
/// metrics the renderer drew with.
```

## L1848 · `let mut best_d = target_x; // distance at the 0-offset boundary (width 0)`

```
// distance at the 0-offset boundary (width 0)
```

## L1857 · `fn nth_line(s: &str, n: usize) -> (usize, &str) {`

```
/// (byte start, slice) of the `n`th '\n'-delimited line in `s`.
```

## L1867-1870 · `fn offset_at(scene: &WidgetScene, x: i32, y: i32, require_inside: bool) -> Option<usize> {`

```
/// Byte offset in the focused text widget's value under screen pixel (x,y).
/// Mirrors the render geometry (Input: Heading, single line; TextArea: Mono,
/// scrolled lines). `require_inside` rejects points outside the widget rect
/// (initial click); false clamps to the content (drag-extend).
```

## L1887-1888 · `let px = layout::font_size_of(style, modifiers_of_ref(widget));`

```
// Same px the renderer used — otherwise a click lands on the
// wrong line the moment the app zooms.
```

## L1894-1896 · `let (col_x, _) = render::textarea_text_column(`

```
// Same text column the renderer used, shifted by the same
// sideways offset — otherwise a click on a scrolled line lands
// on the character that WOULD be there at offset zero.
```

## L1911 · `pub fn text_selecting() -> bool { TEXT_DRAG.load(Ordering::Acquire) != 0 }`

```
/// True while a text-selection drag is in progress.
```

## L1914 · `pub fn text_select_cancel() { TEXT_DRAG.store(0, Ordering::Release); }`

```
/// Abandon any in-progress text drag (focus left the widget world).
```

## L1917-1918 · `pub fn text_select_begin(window_id: u32, x: i32, y: i32) -> bool {`

```
/// Position the caret at (x,y) and start a selection there. Returns true if
/// it began — the press must land inside the focused Input/TextArea.
```

## L1942-1949 · `pub fn text_select_run(window_id: u32, x: i32, y: i32, run: u8) -> bool {`

```
/// Ein Doppel- oder Dreifachklick: das WORT unter dem Zeiger auswaehlen,
/// beim dritten Klick die ganze Zeile.
///
/// Kein eigener Zieh-Zustand — die Auswahl steht danach fest, und das
/// Loslassen laesst sie in Ruhe (`text_select_end` raeumt nur eine
/// zusammengefallene Auswahl weg, und eine Wortauswahl ist keine).
///
/// `run` ist das Ergebnis von [`click_run`]: 2 = Wort, 3 = alles.
```

## L1971 · `TEXT_DRAG.store(0, Ordering::Release);`

```
// Der Zieh-Zustand bleibt AUS: ein Doppelklick waehlt, er zieht nicht.
```

## L1981-1983 · `pub fn text_select_extend(window_id: u32, x: i32, y: i32) -> bool {`

```
/// Move the caret (selection's moving end) to (x,y). Returns true if it
/// changed (caller re-renders). Clamps to content so a drag past the edge
/// keeps extending.
```

## L2006-2007 · `pub fn text_select_end(window_id: u32) {`

```
/// End a text drag. A collapsed (single-click) selection is cleared so a
/// plain click leaves no highlight.
```

## L2018-2023 · `struct SlideDrag {`

```
// ── Slider drag ───────────────────────────────────────────────────────
//
// The compositor moves the thumb itself: an app busy decoding must not
// make the bar lag behind the pointer. The live value is written into the
// cached tree, and `scene_commit` re-applies it to every tree the app
// commits while the drag lasts.
```

## L2028 · `track:  abi::Rect,`

```
/// Screen rect of the slider at press time.
```

## L2060-2061 · `fn set_slider_value(tree: &mut abi::Widget, action: abi::ActionId, value: u16) -> bool {`

```
/// Set every slider with `on_change == action` to `value`, popover
/// contents included. Returns true if anything changed.
```

## L2073 · `static SLIDE_DIRTY: AtomicU32 = AtomicU32::new(0);`

```
/// Window (+1) whose slider moved since the last frame; 0 = none.
```

## L2076-2077 · `fn set_scene_slider(scene: &mut WidgetScene, action: abi::ActionId, value: u16) -> bool {`

```
/// The value lives in the tree AND in the popover copies the pixel-only
/// repaint draws from, so both are set.
```

## L2086-2089 · `fn slide_repaint(window_id: u32) {`

```
/// Not drawn here: a drag step arrives with every mouse packet, and a
/// raster of the whole window (a scaled video frame included) in the input
/// path makes the pointer stutter. `slide_flush` draws once per frame,
/// after the queued mouse events are through.
```

## L2095-2096 · `pub fn slide_flush() {`

```
/// Draw the slider that moved since the last frame, if any. Value changes
/// leave the layout as it is, so the pixel-only path is enough.
```

## L2102-2103 · `pub fn slide_begin(window_id: u32, x: i32, y: i32) -> bool {`

```
/// A left press at (x,y): start dragging the slider under it, if any.
/// The press already counts as a move — clicking the track jumps there.
```

## L2123 · `pub fn slide_move(x: i32) {`

```
/// Pointer moved with the button held.
```

## L2140 · `pub fn slide_end() {`

```
/// Button released: report the final value once more, with `done`.
```

## L2143-2146 · `if let Some(s) = SCENES.lock().get_mut(&d.window) { s.payload_hash = [0; 32]; }`

```
// The cached tree now shows a value the app never committed. Forget
// the payload hash so its next commit is applied even if it is
// byte-identical to the one before the drag (an app that declined
// the new value must be able to put the thumb back).
```

## L2151-2153 · `fn apply_slide_override(window_id: u32, tree: &mut abi::Widget) -> Option<u16> {`

```
/// Keep a drag's live value in a tree the app commits mid-drag. Returns
/// the value it put in, so the commit can check afterwards whether the
/// drag moved on while it was laying out.
```

## L2161-2163 · `fn reapply_slide_after_commit(window_id: u32, applied: Option<u16>) {`

```
/// A commit runs on a worker core and stores its scene only after layout
/// and raster. A drag step that landed in between was overwritten by it —
/// put the thumb back where the pointer is.
```

## L2175-2176 · `pub fn clipboard_copy(window_id: u32) -> bool { widget_clipboard(window_id, b'c') }`

```
/// Ctrl+Shift+C / V routed from the compositor: copy or paste the focused
/// text widget's selection via the existing clipboard handler.
```

## L2191-2207 · `pub fn handle_input_key(`

```
/// Compositor-side keyboard intercept for a focused `Widget::Input` or
/// `Widget::TextArea`. Returns `true` iff the key was consumed — caller
/// must skip the usual `push_event(Event::Key)` route to the app.
///
/// Edits the buffer + caret in place. Buffer-mutating keys (printable,
/// Backspace, Delete, and — for TextArea — Enter→newline) emit a single
/// `Event::InputChange { value }` carrying the whole document so the app
/// can mirror its own state. Pure caret moves consume the key silently.
///
/// Input vs TextArea divergence:
///   - Input: Enter fires `Event::Action(on_submit)` (NO_ACTION = let it
///     fall through to the app as `Event::Key`); Home/End jump to buffer
///     start/end; Up/Down are NOT consumed (fall through to the app — e.g.
///     loft's grid navigation).
///   - TextArea: Enter inserts `\n`; Home/End are line-relative;
///     Up/Down/PageUp/PageDown move the caret across lines and ARE
///     consumed (an editor owns its arrows; super+arrow stays WM nav).
```

## L2215-2217 · `let (is_textarea, on_submit, win_h, has_sel, value_empty) = {`

```
// Phase 1: confirm a focused text widget exists; capture its kind
// (Input on_submit, or TextArea) + the window height for paging.
// Drop the read lock before mutating.
```

## L2227-2228 · `let (has_sel, value_empty) = match scene.input_edit.as_ref() {`

```
// Selection presence + emptiness decide whether a clipboard chord
// is "text" or should fall through to the app (below).
```

## L2240-2249 · `let clip_letter = match key {`

```
// Phase 1b: Ctrl+C / X / V / A — clipboard + select-all. Applies to
// both Input and TextArea. The keyboard driver maps Ctrl+C to control
// byte 0x03; the others arrive as the plain letter — both carry
// `mods.ctrl`, so normalize a control byte back to its letter.
// Clipboard / select-all shortcuts. Two ways the combo arrives:
//  - Ctrl+C is translated by the PS/2 layer to control byte 0x03 (the
//    SIGINT convention), produced ONLY when Ctrl was held — so detect it
//    directly, immune to `mods.ctrl` being sampled a tick late (the
//    buffered key is often processed after Ctrl is already released).
//  - Ctrl+V/X/A arrive as the plain letter; they need mods.ctrl.
```

## L2256-2268 · `let text_op = if is_clipboard_sink(window_id) {`

```
// For a clipboard-sink window (loft), decide whether this chord is a
// TEXT op (consume it here) or should fall through to the app as a
// file/object clipboard op. A focused text field only owns the chord
// when it can act on text:
//   - Select-all: always text.
//   - Copy / Cut: only with a live selection — nothing selected means
//     nothing to copy as text, so let the app copy the selected file.
//   - Paste: a TextArea (an editor) always pastes text; a single-line
//     Input pastes text only when it's non-empty or has a selection.
//     An idle empty Input (loft's search box) lets the paste fall
//     through so the app pastes a file.
// Non-sink windows keep the original always-consume behaviour, so
// text paste in every other app is unchanged.
```

## L2283-2284 · `return false;`

```
// Not a text op — return false so the caller's clipboard routing
// delivers `Event::Clipboard` to the focused app.
```

## L2307-2311 · `K::Char(b) if b >= 0x20 && b != 0x7F => Op::Insert(b),`

```
// **Nicht mehr nur ASCII.** 0x20..0x7F war woertlich „keine
// Umlaute" — auf einer Deutschschweizer Tastatur liess sich damit
// kein einziges `ä` in ein Feld tippen. Alles ab 0x80 ist ein Stueck
// einer UTF-8-Folge und wird unten zusammengesetzt; 0x7F (DEL) ist
// kein Text und bleibt draussen.
```

## L2323-2326 · `K::Enter if is_textarea                  => Op::Newline,`

```
// TextArea: Enter is a newline. Input: Enter only swallowed when
// it declared a real on_submit; otherwise it falls through to
// `Event::Key` so window-level Enter handlers (drun's launcher)
// keep working without wiring on_submit just to receive it.
```

## L2328-2329 · `K::Tab   if is_textarea                  => Op::Indent,`

```
// Tab in a TextArea indents (4 spaces) instead of moving focus;
// for an Input it falls through so Tab still navigates fields.
```

## L2335-2336 · `let (value_changed, new_value) = {`

```
// Phase 2: apply the edit, snapshot the new buffer for the
// InputChange event.
```

## L2347-2348 · `edit.cursor = clamp_boundary(&edit.value, edit.cursor);`

```
// Defensive: clamp cursor in case a malformed prev state slipped
// through (a wild or mid-codepoint index would panic insert/remove).
```

## L2350-2352 · `let line_h = (crate::gui::text::line_height(abi::TextStyle::Mono) as u32).max(1);`

```
// One viewport of lines for PageUp/PageDown, derived from the
// window height at Mono metrics (a close-enough approximation —
// the TextArea's own rect isn't threaded here).
```

## L2357-2361 · `let sel = edit.selection();`

```
// Selection-aware editing. Movement ops update or clear the anchor
// (Shift extends; a plain move drops the selection). An edit op with
// an active selection replaces it: delete the range first, then the
// op (insert / newline / indent) applies at the collapsed caret;
// Backspace/Delete are satisfied by the range removal alone.
```

## L2392-2394 · `if b >= 0xC0 { edit.pending_len = 0; }   // neue Folge`

```
// **`b as char` waere hier LATIN-1, nicht UTF-8** — aus
// dem Fuehrungsbyte 0xC3 wuerde `Ã`. Gesammelt wird, bis
// die Folge steht, und dann als ZEICHEN eingefuegt.
```

## L2395 · `if b >= 0xC0 { edit.pending_len = 0; }   // neue Folge`

```
// neue Folge
```

## L2401 · `edit.pending_len = 0;   // laenger als jede Folge — verwerfen`

```
// laenger als jede Folge — verwerfen
```

## L2456-2459 · `if is_textarea {`

```
// Phase 3b: caret-follow scroll. The render no longer pulls the view to
// the caret every frame (that defeated wheel/drag scrolling), so do it
// HERE — only on a caret move — for the focused TextArea: nudge
// scroll_y just enough to keep the caret line on screen.
```

## L2464 · `match op {`

```
// Phase 3: push the right event(s).
```

## L2478-2480 · `rerender_state_only(window_id);`

```
// Phase 4: re-rasterize so the new buffer + caret position appear.
// Same machinery as a hover-state change: pure local re-render
// using the cached state slices, no recommit needed.
```

## L2487 · `pub fn scene_commit(bytes: &[u8], window_id: u32, module_name: &str) -> i32 {`

```
// ── Scene commit ──────────────────────────────────────────────────────
```

## L2489-2503 · `pub fn scene_commit(bytes: &[u8], window_id: u32, module_name: &str) -> i32 {`

```
/// Deserialize a wire-framed widget tree from an app's commit payload
/// and render it into `window_id`'s per-window scene buffer. Shade's
/// next render cycle will blit the scene through `render_window`.
///
/// `window_id == 0` means "no widget window yet" — this is the first
/// commit from an app. We create a widget-kind window via shade,
/// return the new id to the caller (stored in HostState), then do
/// the actual render.
///
/// Return protocol (i32):
///   > 0 → success, new widget window id (caller stores for reuse)
///   == 0 → success, reused `window_id` as-is
///   -1 → version mismatch or cap denied
///   -2 → postcard decode failure
///   -3 → couldn't allocate a window
```

## L2512-2514 · `if window_id != 0 { note_close_request_alive(window_id); }`

```
// Committing a frame proves the app is still turning its event loop.
// If it was asked to close, that cancels the liveness deadline — the
// user may now take as long as they like with the prompt.
```

## L2532-2534 · `let (target_id, new_window) = match window_id {`

```
// Obtain or create the widget window. `new_window` is Some iff we
// just created one — return its id to the caller so the next
// commit reuses the same slot.
```

## L2537-2539 · `let title = if module_name.is_empty() { "widget" } else { module_name };`

```
// Title the window with the module name so launch paths can
// find an already-running instance (re-focus / singleton +
// npk_open tab routing) and the bar shows the app name.
```

## L2568-2569 · `let prev_scroll_y = SCENES.lock().get(&target_id).map(|s| s.scroll_y).unwrap_or(0);`

```
// Preserve the wheel scroll position across re-commits so an app that
// re-renders on every event doesn't snap back to the top.
```

## L2583-2589 · `let (prev_hover, prev_focus, prev_active, prev_input_edit, prev_parked,`

```
// Preserve hover/focus/active across re-commits so an interactive
// app re-rendering on every event doesn't lose its state-merged
// pixels mid-frame. Falls back to empty for first commit.
// `is_first_commit` is "no scene yet for this window" — true even
// when the window itself was created earlier by
// `npk_window_set_overlay` (drun's path), which is exactly when we
// want to auto-focus.
```

## L2606-2607 · `let scroll_x = prev_scroll_x.min(max_scroll_x);`

```
// A commit that shortens the longest line must pull the view back with
// it, or the editor stays parked past the end of its own text.
```

## L2610-2615 · `let focus_path: Vec<u32> = if is_first_commit {`

```
// First commit auto-focuses the first focusable Widget::Input so
// search bars / launchers Just Work without the user having to
// click into the input first — keeps the "type as soon as it
// opens" UX every keyboard-driven dialog needs. Re-commits keep
// whatever focus the user navigated to via Tab / click; we never
// auto-jump focus during a session.
```

## L2622-2628 · `let input_edit: Option<InputEditState> = if focus_path.is_empty() {`

```
// Reconcile the input editor against the new tree:
//   - focus targets an Input with the same value the editor already
//     holds → keep the prev state (the app just echoed our buffer
//     back, the cursor must NOT reset).
//   - focus targets an Input with a different value → app overrode
//     it programmatically; rebuild from the tree value.
//   - focus elsewhere → drop the editor.
```

## L2645-2646 · `SCENES.lock().insert(target_id, WidgetScene {`

```
// Store into the per-window scene map. Keep a clone of the tree
// + layout for future resize re-renders (typical tree < 1 KB).
```

## L2674-2676 · `crate::shade::with_compositor(|c| {`

```
// Mark the window dirty so shade paints it in the next render,
// then request a full render on Core 0. scene_commit may run on
// a worker core — we never touch MMIO directly from here.
```

## L2690-2702 · `fn rasterize_buffer_with_overlays(`

```
/// Alloc a BGRA back buffer, clear to Surface, run the render walker,
/// return the pixel vec. Used by `scene_commit`, the relayout path
/// (resize / re-render from cached tree), and the pseudo-state
/// re-renders (hover/focus/active).
///
/// Each `*_path: Option<&[u32]>` follows the protocol from
/// `render::render_with_state`: `None` = state not in this subtree,
/// `Some([])` = root IS the state target, `Some([i,…])` = descend
/// into child `i`.
/// Rasterize the main tree, then paint each popover overlay on top
/// in declaration order. Popovers are rendered without state-paths
/// (no hover/focus carry-through to their content) — overlay state
/// is short-lived and the next commit rebuilds the popover anyway.
```

## L2720-2726 · `let bg_alpha: u8 = if matches!(tree, abi::Widget::Stack { .. }) {`

```
// Translucent-panel detection: the bar (and only the bar) roots its tree
// in a `Stack`. Such a scene clears TRANSPARENT and fills backgrounds at
// the chrome opacity, so glyphs stay full-coverage and the compositor
// composites the whole scene over the wallpaper by per-pixel alpha (no
// halo). Every other app roots in Row/Column → opaque, byte-identical to
// before. Derived from the tree so it needs no extra plumbing or locks
// (relayout_scene rasterises while holding the SCENES lock).
```

## L2733-2735 · `if bg_alpha >= 255 {`

```
// Opaque scene → clear to the Surface token (covers unpainted areas).
// Translucent panel scene → clear TRANSPARENT (alpha 0) so gaps and
// glyph edges composite correctly over the wallpaper.
```

## L2760-2762 · `for p in popovers {`

```
// Overlays — paint after the main tree so they sit on top of any
// pixels the main pass wrote into the same screen region. No
// pseudo-state paths — popovers are transient by definition (scroll 0).
```

## L2770 · `drop(target);`

```
// `target` drops here, releasing the &mut on `pixels`.
```

## L2775-2781 · `pub fn refresh_all_scenes() {`

```
/// Re-render a scene at new dimensions — called by shade when a
/// widget-kind window's content rect changes (resize / retile).
/// Uses the cached tree + re-runs layout so we don't need the app
/// to commit again. Returns false if no scene exists for that id.
/// Re-rasterize every live widget scene without changing its geometry.
/// Called after theme-affecting events (wallpaper change → new accent /
/// surface palette) so cached pixels pick up the fresh token colours.
```

## L2790-2793 · `pub fn rerender_window(wid: u32) {`

```
/// Re-rasterize a single window's cached scene in place (same geometry +
/// stored pseudo-state). Used after something the rasteriser reads
/// changed without an app commit — a theme swap, or a committed
/// `Widget::Canvas` bitmap (`npk_canvas_commit`). No-op if no scene.
```

## L2834-2845 · `pub fn rerender_window_pixels(wid: u32) {`

```
/// Repaint a window from its EXISTING layout.
///
/// For a canvas commit the widget tree is untouched — only the pixels a
/// `Widget::Canvas` holds changed — so `layout_scrolled` would produce the
/// same tree it produced last time. At 30 frames a second that is the most
/// expensive thing in the path, done for nothing.
///
/// The rasterisation itself still runs over the whole buffer, and that is
/// deliberate: painting only the canvas rect would have to know what sits
/// ON TOP of it (an open menu, a popover, a scroll clip), and a repaint
/// that gets z-order wrong erases the menu instead of the frame. Skipping
/// the layout is safe without asking that question at all.
```

## L2888 · `return true; // no-op, nothing moved`

```
// no-op, nothing moved
```

## L2895-2898 · `let new_lo = layout::layout_scrolled(&tree, new_rect, scroll_y);`

```
// Resize invalidates the cached hover_path AND active_path —
// coordinates of the old layout no longer match. Focus survives
// (a focused input stays focused after resize). Active is
// mouse-tied so it gets cleared.
```

