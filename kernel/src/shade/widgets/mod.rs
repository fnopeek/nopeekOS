//! Widget pipeline — declarative GUI for WASM apps.
//!
//! Apps describe **what** to render (widget tree); Shade owns **how**
//! (layout, rasterization, GPU compositing, animation, theming).
//!
//! See PHASE10_WIDGETS.md for the full spec.
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

pub mod abi;
pub mod tile;
pub mod debug;
pub mod layout;
pub mod palette;
pub mod render;
pub mod raster;
pub mod animation;

mod check_abi;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ── Focused-Input editor state ────────────────────────────────────────
//
// When a `Widget::Input` is the focus target, the compositor — not the
// app — owns the buffer + caret. Printable keys, Backspace, Delete,
// Left/Right/Home/End all mutate this struct without going through the
// WASM event queue, so cursor moves stay 60 Hz responsive even if the
// app is busy. The app sees a single `Event::InputChange { value }`
// per buffer mutation; pure cursor moves emit nothing.

#[derive(Clone, Debug, Default)]
pub struct InputEditState {
    /// Current buffer contents — the source of truth while focused.
    /// `Widget::Input.value` in the cached tree may lag by one round-
    /// trip (the app hasn't echoed the InputChange back yet). The
    /// render walker pulls from here, not from the tree, so the user
    /// always sees what they just typed.
    pub value:  String,
    /// Caret position as a byte index into `value`. Always at a UTF-8
    /// boundary; v1 only inserts ASCII so `byte_index == char_index`.
    pub cursor: usize,
}

impl InputEditState {
    fn from_value(v: &str) -> Self {
        InputEditState { value: v.into(), cursor: v.len() }
    }
}

// ── Per-window widget scene storage ───────────────────────────────────
//
// Every widget-kind shade window has exactly one WidgetScene here,
// keyed by WindowId.0 (u32). The scene holds the last-rendered
// pixels (blitted into the window's content rect by shade's
// render_window) plus the tree + layout cached for resize-driven
// re-render (the app may have already exited).

pub struct WidgetScene {
    pub pixels:      Vec<u32>,
    pub width:       u32,
    pub height:      u32,
    /// Content-rect origin in screen coordinates at the time of
    /// the last render. Used to detect whether a re-render is
    /// needed when shade redraws the window.
    pub origin_x:    i32,
    pub origin_y:    i32,
    /// Cached tree + layout so the compositor can re-layout on
    /// resize without the app committing again. Widget-apps are
    /// allowed to exit after a single commit.
    pub tree:        abi::Widget,
    pub layout_tree: layout::LayoutNode,
    /// NodeId → screen rect, populated for any widget tagged with
    /// `Modifier::NodeId`. Used by `Widget::Popover`'s anchor lookup
    /// at layout time and by the click-outside-dismiss test in
    /// hit_test (clicks on an anchor must NOT fire on_dismiss —
    /// the anchor's own OnClick handles toggle).
    pub anchors:     alloc::collections::BTreeMap<u32, abi::Rect>,
    /// Floating popover overlays in declaration order. Painted
    /// last (top z-order); hit-tested before the main tree so a
    /// click on a popover never falls through to whatever's
    /// underneath it.
    pub popovers:    Vec<layout::PopoverLayout>,
    /// blake3 hash of the postcard payload that produced this
    /// scene. P10.6: lets scene_commit short-circuit when an app
    /// resubmits the same tree (common with interactive apps that
    /// re-commit on every event loop iteration).
    pub payload_hash: [u8; 32],
    /// Path of child indices from the root to the currently hovered
    /// layout node — empty until the cursor enters the window. Used
    /// by the render walker to merge `Modifier::Hover` inner mods on
    /// the matching node.
    pub hover_path:  Vec<u32>,
    /// Path of the currently focused widget (Tab-stop, keyboard
    /// destination). Empty = no focus. Set by click-to-focus and
    /// future Tab navigation.
    pub focus_path:  Vec<u32>,
    /// Path of the widget under an active mouse-button press.
    /// `None` = no button down. Cleared on button release. Drives
    /// `Modifier::Active(…)` style.
    pub active_path: Option<Vec<u32>>,
    /// Compositor-classified container size bucket for this window —
    /// drives `Modifier::WhenDensity(d, …)` matching. Recomputed on
    /// commit and on resize.
    pub density:     abi::Density,
    /// Cached: tree contains at least one Hover/Focus/Active/Disabled/
    /// WhenDensity modifier. Lets `update_hover` skip re-rasterization
    /// for trees that have no state-driven visuals — avoids
    /// re-rendering on every mouse move.
    pub has_pseudo:  bool,
    /// Compositor-owned text-editor state, populated when `focus_path`
    /// targets a `Widget::Input`. None means no Input is focused (or
    /// the focused widget isn't an Input). Drives caret render +
    /// keyboard intercept in `handle_input_key`.
    pub input_edit:  Option<InputEditState>,
}

static SCENES: Mutex<BTreeMap<u32, WidgetScene>> = Mutex::new(BTreeMap::new());

/// Look up a scene's pixel buffer for blitting. Returned pointer +
/// dimensions are valid as long as `SCENES` is not mutated — caller
/// must finish the blit before releasing the lock.
pub fn with_scene<F, R>(window_id: u32, f: F) -> Option<R>
where F: FnOnce(&WidgetScene) -> R,
{
    SCENES.lock().get(&window_id).map(f)
}

/// Drop a scene. Called from compositor::close_window when the
/// widget-kind window is destroyed.
pub fn remove_scene(window_id: u32) {
    SCENES.lock().remove(&window_id);
    LAST_HOVER.lock().remove(&window_id);
}

// ── Per-window event queues (P10.7) ───────────────────────────────────
//
// Widget apps poll events via `npk_event_poll`. Shade pushes Events
// here: mouse clicks after hit-testing against the scene's layout
// tree, keyboard-forwarded keys once focused, focus changes.
//
// Queue is bounded; on overflow we drop the oldest entry so a slow
// app can't wedge the compositor.

const MAX_EVENTS_PER_WINDOW: usize = 64;

static EVENT_QUEUES: Mutex<BTreeMap<u32, VecDeque<abi::Event>>> =
    Mutex::new(BTreeMap::new());

/// Push an event into the window's queue. Oldest is dropped on
/// overflow (bounded queue).
pub fn push_event(window_id: u32, event: abi::Event) {
    let mut queues = EVENT_QUEUES.lock();
    let q = queues.entry(window_id).or_insert_with(VecDeque::new);
    if q.len() >= MAX_EVENTS_PER_WINDOW {
        q.pop_front();
    }
    q.push_back(event);
}

/// Non-blocking event pop. Returns None if queue is empty.
pub fn poll_event(window_id: u32) -> Option<abi::Event> {
    EVENT_QUEUES.lock()
        .get_mut(&window_id)
        .and_then(|q| q.pop_front())
}

/// True if a widget-kind window with this id still exists in the
/// compositor. Host fn `npk_event_poll` uses this to distinguish
/// "queue empty" from "window closed" — the app turns the latter
/// into an exit signal rather than polling forever.
pub fn widget_window_exists(window_id: u32) -> bool {
    crate::shade::with_compositor(|comp| {
        comp.windows.iter().any(|w|
            w.id.0 == window_id
            && w.kind == crate::shade::window::WindowKind::Widget)
    }).unwrap_or(false)
}

/// Called by compositor::close_window alongside remove_scene to
/// drop the now-orphaned queue.
pub fn remove_event_queue(window_id: u32) {
    EVENT_QUEUES.lock().remove(&window_id);
}

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
pub fn hit_test(window_id: u32, x: i32, y: i32) -> Option<abi::ActionId> {
    let scenes = SCENES.lock();
    let scene = scenes.get(&window_id)?;

    // Popovers (declared overlays) hit-test FIRST so a click inside
    // an open menu dropdown lands on the menu item, not on whatever
    // was rendered underneath. Iterate in reverse-declaration order
    // so a popover declared later (= painted on top) wins. We don't
    // recurse with `find_click_target` here because the popover's
    // child tree is rooted at its own LayoutNode — same shape as the
    // main pass, just an isolated subtree.
    for p in scene.popovers.iter().rev() {
        if rect_contains(p.layout.rect, x, y) {
            let mut out = None;
            find_click_target(&p.child, &p.layout, x, y, false, &mut out);
            return out;
        }
    }

    // Click outside every popover BUT one or more popovers are open
    // → fire the topmost popover's `on_dismiss`, unless the click
    // landed on its anchor (the anchor's own OnClick handles toggle
    // and we don't want the dismiss action to also fire and race).
    if !scene.popovers.is_empty() {
        let on_anchor = scene.popovers.iter()
            .any(|p| rect_contains(p.anchor_rect, x, y));
        if !on_anchor {
            // Last-declared popover is the most-recently-opened —
            // its on_dismiss matches what the app's state expects.
            return Some(scene.popovers.last().unwrap().on_dismiss);
        }
        // Click landed on an anchor — fall through to normal routing
        // so the anchor's OnClick fires.
    }

    let mut out = None;
    find_click_target(&scene.tree, &scene.layout_tree, x, y, false, &mut out);
    out
}

fn is_disabled(w: &abi::Widget) -> bool {
    modifiers_of_ref(w).iter().any(|m| matches!(m, abi::Modifier::Disabled(_)))
}

fn is_focusable(w: &abi::Widget) -> bool {
    if is_disabled(w) { return false; }
    if matches!(w,
        abi::Widget::Button { .. }
        | abi::Widget::Input { .. }
        | abi::Widget::Checkbox { .. }
    ) {
        return true;
    }
    modifiers_of_ref(w).iter().any(|m| matches!(m, abi::Modifier::OnClick(_)))
}

// Deduplicated OnHover target per window. Compositor calls update_hover
// on every MouseMove; we push Event::Action only when the hit changes.
static LAST_HOVER: Mutex<BTreeMap<u32, abi::ActionId>> = Mutex::new(BTreeMap::new());

pub fn hover_test(window_id: u32, x: i32, y: i32) -> Option<abi::ActionId> {
    let scenes = SCENES.lock();
    let scene = scenes.get(&window_id)?;
    let mut out = None;
    find_hover_target(&scene.tree, &scene.layout_tree, x, y, &mut out);
    out
}

/// Compositor-side density classifier. Thresholds live here once;
/// apps reference them only via `Modifier::WhenDensity(Density, …)`.
fn classify_density(window_w: u32) -> abi::Density {
    if window_w < 600 { abi::Density::Compact }
    else if window_w < 1200 { abi::Density::Regular }
    else { abi::Density::Spacious }
}

/// Walk the layout tree and collect the chain of child indices that
/// leads to the deepest node still containing (x, y). Returns the
/// path; empty path = (x, y) is inside the root only. `None` means
/// the point falls outside the root entirely.
fn find_hover_path(
    widget: &abi::Widget,
    layout: &layout::LayoutNode,
    x: i32, y: i32,
) -> Option<Vec<u32>> {
    if !rect_contains(layout.rect, x, y) { return None; }
    let mut path: Vec<u32> = Vec::new();
    descend_hover(widget, layout, x, y, &mut path);
    Some(path)
}

fn descend_hover(
    widget: &abi::Widget,
    layout: &layout::LayoutNode,
    x: i32, y: i32,
    out: &mut Vec<u32>,
) {
    let kids = widget_children_ref(widget);
    for (i, (cw, cl)) in kids.iter().zip(layout.children.iter()).enumerate() {
        if rect_contains(cl.rect, x, y) {
            out.push(i as u32);
            descend_hover(cw, cl, x, y, out);
            return;
        }
    }
}

fn find_hover_target(
    widget: &abi::Widget,
    layout: &layout::LayoutNode,
    x: i32, y: i32,
    out: &mut Option<abi::ActionId>,
) {
    if !rect_contains(layout.rect, x, y) { return; }
    if is_disabled(widget) { return; }
    for (cw, cl) in widget_children_ref(widget).iter().zip(layout.children.iter()) {
        find_hover_target(cw, cl, x, y, out);
        if out.is_some() { return; }
    }
    for m in modifiers_of_ref(widget) {
        if let abi::Modifier::OnHover(id) = m { *out = Some(*id); return; }
    }
}

/// Walk down to the deepest focusable widget under (x, y). Returns
/// the path of child indices from root, or `None` if no focusable
/// node lives there. Disabled widgets and their descendants are
/// skipped.
fn find_focusable_path(
    widget: &abi::Widget,
    layout: &layout::LayoutNode,
    x: i32, y: i32,
) -> Option<Vec<u32>> {
    if !rect_contains(layout.rect, x, y) { return None; }
    if is_disabled(widget) { return None; }
    let mut path: Vec<u32> = Vec::new();
    if descend_focusable(widget, layout, x, y, &mut path) {
        Some(path)
    } else {
        None
    }
}

fn descend_focusable(
    widget: &abi::Widget,
    layout: &layout::LayoutNode,
    x: i32, y: i32,
    out: &mut Vec<u32>,
) -> bool {
    // Children first — deepest focusable wins.
    let kids = widget_children_ref(widget);
    for (i, (cw, cl)) in kids.iter().zip(layout.children.iter()).enumerate() {
        if !rect_contains(cl.rect, x, y) { continue; }
        if is_disabled(cw) { continue; }
        out.push(i as u32);
        if descend_focusable(cw, cl, x, y, out) {
            return true;
        }
        out.pop();
    }
    // No child took it — am I focusable myself?
    is_focusable(widget)
}

pub fn update_hover(window_id: u32, x: i32, y: i32) {
    // Step 1 — recompute the hover path against the cached layout tree.
    // Cheap (one descent), avoids re-rendering on hover moves that
    // don't actually cross node boundaries.
    let new_path: Option<Vec<u32>> = {
        let scenes = SCENES.lock();
        match scenes.get(&window_id) {
            Some(s) => find_hover_path(&s.tree, &s.layout_tree, x, y),
            None    => None,
        }
    };
    let new_path = new_path.unwrap_or_default();

    // Step 2 — diff against the cached hover_path. If unchanged, skip.
    // If changed AND the tree has any pseudo-state-aware modifier,
    // re-rasterize using the new path; otherwise just update the path
    // (hover events still fire below).
    let path_changed = {
        let scenes = SCENES.lock();
        match scenes.get(&window_id) {
            Some(s) => s.hover_path != new_path,
            None    => false,
        }
    };

    if path_changed {
        let needs_rerender = {
            let scenes = SCENES.lock();
            scenes.get(&window_id).map(|s| s.has_pseudo).unwrap_or(false)
        };
        if needs_rerender {
            rerender_with_state(window_id, &new_path);
        } else {
            // Just bump the cached path so the next move diffs cleanly.
            if let Some(s) = SCENES.lock().get_mut(&window_id) {
                s.hover_path = new_path.clone();
            }
        }
    }

    // Step 3 — fire the OnHover action event (existing semantics:
    // dedup on ActionId, push only when target action changes).
    let new_id = hover_test(window_id, x, y);
    let mut last = LAST_HOVER.lock();
    let prev = last.get(&window_id).copied();
    let changed = match (prev, new_id) {
        (Some(a), Some(b)) => a != b,
        (None, None)       => false,
        _                  => true,
    };
    if !changed { return; }
    match new_id {
        Some(id) => { last.insert(window_id, id); }
        None     => { last.remove(&window_id); }
    }
    drop(last);
    if let Some(id) = new_id {
        push_event(window_id, abi::Event::Action(id));
    }
}

/// Re-rasterize a scene with the given hover path. Other state paths
/// (focus, active) come from the cached scene values. Caller must
/// hold no lock on SCENES; we lock internally.
fn rerender_with_state(window_id: u32, hover_path: &[u32]) {
    let (tree, rect, density, focus_path, active_path, input_edit) = {
        let scenes = SCENES.lock();
        match scenes.get(&window_id) {
            Some(s) => (
                s.tree.clone(),
                abi::Rect { x: s.origin_x, y: s.origin_y, w: s.width, h: s.height },
                s.density,
                s.focus_path.clone(),
                s.active_path.clone(),
                s.input_edit.clone(),
            ),
            None => return,
        }
    };
    let lo = layout::layout(&tree, rect);
    let layout_tree = lo.tree;
    let anchors = lo.anchors;
    let popovers = lo.popovers;
    let h = if hover_path.is_empty() { None } else { Some(hover_path) };
    let f: Option<&[u32]> = if focus_path.is_empty() { None } else { Some(&focus_path) };
    let a: Option<&[u32]> = active_path.as_deref();
    let pixels = rasterize_buffer_with_overlays(
        &tree, &layout_tree, &popovers, rect, h, f, a, density,
        input_edit.as_ref(),
    );

    if let Some(s) = SCENES.lock().get_mut(&window_id) {
        s.pixels      = pixels;
        s.layout_tree = layout_tree;
        s.anchors     = anchors;
        s.popovers    = popovers;
        s.hover_path  = hover_path.to_vec();
    }
    mark_dirty(window_id);
}

/// Helper: mark the window dirty + request a render. Used by every
/// state-driven re-rasterize path.
fn mark_dirty(window_id: u32) {
    crate::shade::with_compositor(|c| {
        if let Some(win) = c.windows.iter_mut().find(|w| w.id.0 == window_id) {
            win.dirty = true;
        }
    });
    crate::shade::request_render();
}

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
#[must_use]
pub fn press_at(window_id: u32, x: i32, y: i32) -> bool {
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
    let (new_focus_opt, new_active, has_pseudo) = {
        let scenes = SCENES.lock();
        let scene = match scenes.get(&window_id) {
            Some(s) => s,
            None    => return false,
        };
        let press_path = find_focusable_path(&scene.tree, &scene.layout_tree, x, y);
        let new_focus_opt: Option<Option<Vec<u32>>> = match press_path.as_ref() {
            // Input under cursor → focus moves to it.
            Some(p) if matches!(widget_at_path(&scene.tree, p), Some(abi::Widget::Input { .. })) => {
                Some(Some(p.clone()))
            }
            // Non-Input focusable under cursor → focus untouched.
            // `None` means "no change to focus_path", distinct from
            // `Some(None)` which would mean "clear focus".
            Some(_) => None,
            // Click into empty space → focus untouched too. Apps that
            // want "click outside an input dismisses focus" can
            // implement that explicitly later.
            None    => None,
        };
        (new_focus_opt, press_path, scene.has_pseudo)
    };

    let (focus_changed, active_changed) = {
        let scenes = SCENES.lock();
        let scene = match scenes.get(&window_id) {
            Some(s) => s,
            None    => return false,
        };
        let f_changed = match &new_focus_opt {
            Some(Some(p))  => *p != scene.focus_path,
            Some(None)     => !scene.focus_path.is_empty(),
            None           => false,
        };
        let a_changed = match (&new_active, &scene.active_path) {
            (Some(p), Some(cur)) => p != cur,
            (None, None)         => false,
            _                    => true,
        };
        (f_changed, a_changed)
    };

    if !(focus_changed || active_changed) { return false; }

    let edit_present = if let Some(s) = SCENES.lock().get_mut(&window_id) {
        if let Some(new_focus) = new_focus_opt {
            match new_focus {
                Some(p) => s.focus_path = p,
                None    => s.focus_path.clear(),
            }
            // Re-derive input_edit against the new focus target. We
            // only ever reach this path when focus actually moved
            // (onto an Input or back to none) — pure non-Input clicks
            // leave both focus_path and input_edit untouched.
            s.input_edit = if s.focus_path.is_empty() {
                None
            } else {
                compute_input_edit(&s.tree, &s.focus_path, None)
            };
        }
        s.active_path = new_active;
        s.input_edit.is_some()
    } else { false };

    if has_pseudo || edit_present {
        rerender_state_only(window_id);
        crate::shade::request_render();
        return true;
    }
    false
}

/// Move focus to the next focusable widget in document order
/// (Tab semantics). Wraps around at the end. Returns `true` if a
/// re-render happened.
///
/// Called from outside the compositor lock (intent loop key path),
/// so this one is allowed to mark the window dirty itself.
#[must_use]
pub fn next_focus(window_id: u32) -> bool {
    advance_focus(window_id, 1)
}

/// Move focus to the previous focusable widget (Shift+Tab).
#[must_use]
pub fn prev_focus(window_id: u32) -> bool {
    advance_focus(window_id, -1)
}

fn advance_focus(window_id: u32, delta: isize) -> bool {
    let (paths, current, has_pseudo) = {
        let scenes = SCENES.lock();
        let scene = match scenes.get(&window_id) {
            Some(s) => s,
            None    => return false,
        };
        let mut paths: Vec<Vec<u32>> = Vec::new();
        let mut cursor: Vec<u32> = Vec::new();
        collect_focusable_paths(&scene.tree, &scene.layout_tree, &mut cursor, &mut paths);
        (paths, scene.focus_path.clone(), scene.has_pseudo)
    };
    if paths.is_empty() { return false; }

    let cur_idx = paths.iter().position(|p| *p == current);
    let next_idx = match cur_idx {
        Some(i) => {
            let n = paths.len() as isize;
            ((i as isize + delta).rem_euclid(n)) as usize
        }
        None => if delta >= 0 { 0 } else { paths.len() - 1 },
    };
    let new_path = paths[next_idx].clone();
    if new_path == current { return false; }

    if let Some(s) = SCENES.lock().get_mut(&window_id) {
        s.focus_path = new_path;
        // Tab onto an Input → init the editor; Tab off Input → drop it.
        s.input_edit = compute_input_edit(&s.tree, &s.focus_path, None);
    }
    if has_pseudo {
        rerender_state_only(window_id);
        mark_dirty(window_id);
        return true;
    }
    // Even without pseudo-state mods, we need to re-render to show
    // the cursor caret on the newly-focused Input.
    let needs_caret_render = SCENES.lock().get(&window_id)
        .map(|s| s.input_edit.is_some()).unwrap_or(false);
    if needs_caret_render {
        rerender_state_only(window_id);
        mark_dirty(window_id);
        return true;
    }
    false
}

/// DFS-collect all focusable widget paths in document order. Used
/// for Tab traversal. Disabled subtrees are pruned.
fn collect_focusable_paths(
    widget: &abi::Widget,
    layout: &layout::LayoutNode,
    cursor: &mut Vec<u32>,
    out: &mut Vec<Vec<u32>>,
) {
    let _ = layout; // Same shape as the widget tree; kept for symmetry.
    if is_disabled(widget) { return; }
    if is_focusable(widget) {
        out.push(cursor.clone());
    }
    let kids = widget_children_ref(widget);
    for (i, cw) in kids.iter().enumerate() {
        let cl = match layout.children.get(i) { Some(c) => c, None => continue };
        cursor.push(i as u32);
        collect_focusable_paths(cw, cl, cursor, out);
        cursor.pop();
    }
}

/// Mouse-button-up: clear active state. Focus persists. Returns
/// `true` if a re-render happened — caller marks the window dirty.
#[must_use]
pub fn release_at(window_id: u32) -> bool {
    let (had_active, has_pseudo) = {
        let scenes = SCENES.lock();
        match scenes.get(&window_id) {
            Some(s) => (s.active_path.is_some(), s.has_pseudo),
            None    => return false,
        }
    };
    if !had_active { return false; }
    if let Some(s) = SCENES.lock().get_mut(&window_id) {
        s.active_path = None;
    }
    if has_pseudo {
        rerender_state_only(window_id);
        crate::shade::request_render();
        return true;
    }
    false
}

/// Re-rasterize using the cached focus/active/hover paths and update
/// the scene's pixel buffer. Does NOT touch the compositor (caller is
/// responsible for marking the window dirty). Pure scene-state work,
/// safe to call while the compositor lock is held.
fn rerender_state_only(window_id: u32) {
    let (tree, rect, density, hover_path, focus_path, active_path, input_edit) = {
        let scenes = SCENES.lock();
        match scenes.get(&window_id) {
            Some(s) => (
                s.tree.clone(),
                abi::Rect { x: s.origin_x, y: s.origin_y, w: s.width, h: s.height },
                s.density,
                s.hover_path.clone(),
                s.focus_path.clone(),
                s.active_path.clone(),
                s.input_edit.clone(),
            ),
            None => return,
        }
    };
    let lo = layout::layout(&tree, rect);
    let layout_tree = lo.tree;
    let anchors = lo.anchors;
    let popovers = lo.popovers;
    let h: Option<&[u32]> = if hover_path.is_empty() { None } else { Some(&hover_path) };
    let f: Option<&[u32]> = if focus_path.is_empty() { None } else { Some(&focus_path) };
    let a: Option<&[u32]> = active_path.as_deref();
    let pixels = rasterize_buffer_with_overlays(
        &tree, &layout_tree, &popovers, rect, h, f, a, density,
        input_edit.as_ref(),
    );
    if let Some(s) = SCENES.lock().get_mut(&window_id) {
        s.pixels      = pixels;
        s.layout_tree = layout_tree;
        s.anchors     = anchors;
        s.popovers    = popovers;
    }
}

/// Drop the cached hover_path and the OnHover-action dedup so the next
/// render no longer applies Hover-state modifiers and the next mouse
/// move re-fires the OnHover action even at the same position.
///
/// Called when keyboard navigation should take visual precedence over
/// a stale mouse position — without this, moving the keyboard cursor
/// to row N leaves the original mouse-hovered row M still highlighted
/// and both states render at once.
pub fn suppress_hover(window_id: u32) {
    let needs_rerender = {
        let scenes = SCENES.lock();
        match scenes.get(&window_id) {
            Some(s) => s.has_pseudo && !s.hover_path.is_empty(),
            None    => false,
        }
    };
    if needs_rerender {
        rerender_with_state(window_id, &[]);
    } else if let Some(s) = SCENES.lock().get_mut(&window_id) {
        s.hover_path.clear();
    }
    LAST_HOVER.lock().remove(&window_id);
}

fn find_click_target(
    widget: &abi::Widget,
    layout: &layout::LayoutNode,
    x: i32, y: i32,
    ancestor_disabled: bool,
    out: &mut Option<abi::ActionId>,
) {
    if !rect_contains(layout.rect, x, y) { return; }
    let disabled = ancestor_disabled || is_disabled(widget);
    // Disabled subtrees swallow clicks entirely — neither this node
    // nor its children fire actions, even if a descendant has its own
    // OnClick.
    if disabled { return; }

    // Children first — deepest hit wins.
    for (cw, cl) in widget_children_ref(widget).iter().zip(layout.children.iter()) {
        find_click_target(cw, cl, x, y, disabled, out);
        if out.is_some() { return; }
    }

    // Then self — check OnClick modifier + variant-native on_click
    // (Button has a frozen on_click field).
    for m in modifiers_of_ref(widget) {
        if let abi::Modifier::OnClick(id) = m { *out = Some(*id); return; }
    }
    if let abi::Widget::Button { on_click, .. } = widget {
        *out = Some(*on_click);
    }
}

fn rect_contains(r: abi::Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w as i32 && y >= r.y && y < r.y + r.h as i32
}

fn modifiers_of_ref(w: &abi::Widget) -> &[abi::Modifier] {
    match w {
        abi::Widget::Column   { modifiers, .. } |
        abi::Widget::Row      { modifiers, .. } |
        abi::Widget::Stack    { modifiers, .. } |
        abi::Widget::Scroll   { modifiers, .. } |
        abi::Widget::Text     { modifiers, .. } |
        abi::Widget::Icon     { modifiers, .. } |
        abi::Widget::Button   { modifiers, .. } |
        abi::Widget::Input    { modifiers, .. } |
        abi::Widget::Checkbox { modifiers, .. } |
        abi::Widget::Canvas   { modifiers, .. } |
        abi::Widget::Popover  { modifiers, .. } |
        abi::Widget::Tooltip  { modifiers, .. } |
        abi::Widget::Menu     { modifiers, .. } => modifiers,
        _ => &[],
    }
}

fn widget_children_ref(w: &abi::Widget) -> alloc::vec::Vec<&abi::Widget> {
    let mut out = alloc::vec::Vec::new();
    match w {
        abi::Widget::Column { children, .. } |
        abi::Widget::Row    { children, .. } |
        abi::Widget::Stack  { children, .. } |
        abi::Widget::Menu   { items: children, .. } => {
            for c in children { out.push(c); }
        }
        abi::Widget::Scroll { child, .. } | abi::Widget::Popover { child, .. } => {
            out.push(child.as_ref());
        }
        _ => {}
    }
    out
}

// ── Input self-editing helpers ────────────────────────────────────────

/// Walk `path` from `tree` and return the widget it targets. None if
/// any index is out-of-bounds or the path tries to descend through a
/// leaf.
fn widget_at_path<'a>(tree: &'a abi::Widget, path: &[u32]) -> Option<&'a abi::Widget> {
    let mut cur = tree;
    for &i in path {
        let kids = widget_children_ref(cur);
        cur = *kids.get(i as usize)?;
    }
    Some(cur)
}

/// DFS for the first focusable `Widget::Input` in document order.
/// Used by `scene_commit` to auto-focus on first commit so apps with
/// search bars (drun, future settings dialogs) never need their own
/// focus-priming host fn.
fn find_first_input_path(tree: &abi::Widget) -> Option<Vec<u32>> {
    fn walk(w: &abi::Widget, cursor: &mut Vec<u32>, out: &mut Option<Vec<u32>>) {
        if out.is_some() || is_disabled(w) { return; }
        if matches!(w, abi::Widget::Input { .. }) {
            *out = Some(cursor.clone());
            return;
        }
        for (i, c) in widget_children_ref(w).iter().enumerate() {
            cursor.push(i as u32);
            walk(c, cursor, out);
            cursor.pop();
        }
    }
    let mut cursor: Vec<u32> = Vec::new();
    let mut out: Option<Vec<u32>> = None;
    walk(tree, &mut cursor, &mut out);
    out
}

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
fn compute_input_edit(
    tree: &abi::Widget,
    path: &[u32],
    prev: Option<&InputEditState>,
) -> Option<InputEditState> {
    let target = widget_at_path(tree, path)?;
    let value = match target {
        abi::Widget::Input { value, .. } => value.as_str(),
        _ => return None,
    };
    if let Some(p) = prev {
        if p.value == value {
            return Some(p.clone());
        }
    }
    Some(InputEditState::from_value(value))
}

/// Compositor-side keyboard intercept for the focused `Widget::Input`.
/// Returns `true` iff the key was consumed — caller must skip the
/// usual `push_event(Event::Key)` route to the app.
///
/// Edits the buffer + caret in place. Buffer-mutating keys
/// (printable, Backspace, Delete) emit a single
/// `Event::InputChange { value }` so the app can mirror its own
/// state. Pure caret moves (Left / Right / Home / End) consume the
/// key silently — no event, no app round-trip. Enter fires
/// `Event::Action(on_submit)` if the Input declared one
/// (`NO_ACTION` sentinel = ignore).
pub fn handle_input_key(window_id: u32, key: crate::input::KeyCode) -> bool {
    use crate::input::KeyCode as K;

    // Phase 1: confirm a focused Input exists and capture its
    // on_submit. Drop the read lock before mutating.
    let on_submit = {
        let scenes = SCENES.lock();
        let scene = match scenes.get(&window_id) {
            Some(s) => s,
            None    => return false,
        };
        if scene.focus_path.is_empty() || scene.input_edit.is_none() {
            return false;
        }
        match widget_at_path(&scene.tree, &scene.focus_path) {
            Some(abi::Widget::Input { on_submit, .. }) => *on_submit,
            _ => return false,
        }
    };

    enum Op {
        Insert(u8),
        Backspace,
        Delete,
        Left,
        Right,
        Home,
        End,
        Submit,
    }

    let op = match key {
        K::Char(b) if (0x20..0x7F).contains(&b) => Op::Insert(b),
        K::Backspace                             => Op::Backspace,
        K::Delete                                => Op::Delete,
        K::Left                                  => Op::Left,
        K::Right                                 => Op::Right,
        K::Home                                  => Op::Home,
        K::End                                   => Op::End,
        // Enter only swallowed when the Input declared a real
        // on_submit; otherwise it falls through to `Event::Key` so
        // apps that route Enter at the window level (drun's launcher,
        // any "press Enter to confirm selection" UI) keep working
        // without needing to wire on_submit just to receive it.
        K::Enter if on_submit.0 != u32::MAX => Op::Submit,
        _                                        => return false,
    };

    // Phase 2: apply the edit, snapshot the new buffer for the
    // InputChange event.
    let (value_changed, new_value) = {
        let mut scenes = SCENES.lock();
        let scene = match scenes.get_mut(&window_id) {
            Some(s) => s,
            None    => return false,
        };
        let edit = match scene.input_edit.as_mut() {
            Some(e) => e,
            None    => return false,
        };
        // Defensive: clamp cursor in case a malformed prev state slipped
        // through (shouldn't happen, but a wild index would panic
        // String::insert / remove).
        if edit.cursor > edit.value.len() { edit.cursor = edit.value.len(); }
        let mut changed = false;
        match op {
            Op::Insert(b) => {
                edit.value.insert(edit.cursor, b as char);
                edit.cursor += 1;
                changed = true;
            }
            Op::Backspace => {
                if edit.cursor > 0 {
                    let idx = edit.cursor - 1;
                    edit.value.remove(idx);
                    edit.cursor = idx;
                    changed = true;
                }
            }
            Op::Delete => {
                if edit.cursor < edit.value.len() {
                    edit.value.remove(edit.cursor);
                    changed = true;
                }
            }
            Op::Left  => if edit.cursor > 0 { edit.cursor -= 1; }
            Op::Right => if edit.cursor < edit.value.len() { edit.cursor += 1; }
            Op::Home  => edit.cursor = 0,
            Op::End   => edit.cursor = edit.value.len(),
            Op::Submit => {}
        }
        (changed, edit.value.clone())
    };

    // Phase 3: push the right event(s).
    match op {
        Op::Submit => {
            if on_submit.0 != u32::MAX {
                push_event(window_id, abi::Event::Action(on_submit));
            }
        }
        _ => {
            if value_changed {
                push_event(window_id, abi::Event::InputChange { value: new_value });
            }
        }
    }

    // Phase 4: re-rasterize so the new buffer + caret position appear.
    // Same machinery as a hover-state change: pure local re-render
    // using the cached state slices, no recommit needed.
    rerender_state_only(window_id);
    mark_dirty(window_id);
    crate::shade::request_render();
    true
}

// ── Scene commit ──────────────────────────────────────────────────────

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
pub fn scene_commit(bytes: &[u8], window_id: u32) -> i32 {
    let (&version, body) = match bytes.split_first() {
        Some(v) => v,
        None => return -1,
    };
    if version != abi::WIRE_VERSION {
        return -1;
    }
    let incoming_hash: [u8; 32] = *blake3::hash(bytes).as_bytes();
    if window_id != 0 {
        if let Some(cached_hash) = SCENES.lock().get(&window_id).map(|s| s.payload_hash) {
            if cached_hash == incoming_hash {
                return 0;
            }
        }
    }

    let tree: abi::Widget = match postcard::from_bytes(body) {
        Ok(t) => t,
        Err(_) => return -2,
    };

    // Obtain or create the widget window. `new_window` is Some iff we
    // just created one — return its id to the caller so the next
    // commit reuses the same slot.
    let (target_id, new_window) = match window_id {
        0 => {
            let id = crate::shade::with_compositor(|c| c.create_widget_window("widget"))
                .ok_or(-3i32);
            match id {
                Ok(id) => (id.0, Some(id.0)),
                Err(_) => return -3,
            }
        }
        id => (id, None),
    };

    let rect = crate::shade::with_compositor(|c| {
        let win = c.windows.iter().find(|w| w.id.0 == target_id)?;
        let border = c.border;
        Some((
            win.content_x(border) as i32,
            win.content_y(border) as i32,
            win.content_w(border),
            win.content_h(border),
        ))
    }).flatten();
    let (win_x, win_y, win_w, win_h) = match rect {
        Some(r) => r,
        None    => return -3,
    };
    if win_w == 0 || win_h == 0 { return -3; }

    let layout_rect = abi::Rect { x: win_x, y: win_y, w: win_w, h: win_h };
    let lo = layout::layout(&tree, layout_rect);
    let layout_tree = lo.tree;
    let anchors = lo.anchors;
    let popovers = lo.popovers;
    let density = classify_density(win_w);
    let has_pseudo = render::tree_has_pseudo_state(&tree);

    // Preserve hover/focus/active across re-commits so an interactive
    // app re-rendering on every event doesn't lose its state-merged
    // pixels mid-frame. Falls back to empty for first commit.
    // `is_first_commit` is "no scene yet for this window" — true even
    // when the window itself was created earlier by
    // `npk_window_set_overlay` (drun's path), which is exactly when we
    // want to auto-focus.
    let (prev_hover, prev_focus, prev_active, prev_input_edit, is_first_commit) = {
        let scenes = SCENES.lock();
        match scenes.get(&target_id) {
            Some(s) => (
                s.hover_path.clone(),
                s.focus_path.clone(),
                s.active_path.clone(),
                s.input_edit.clone(),
                false,
            ),
            None    => (Vec::new(), Vec::new(), None, None, true),
        }
    };

    // First commit auto-focuses the first focusable Widget::Input so
    // search bars / launchers Just Work without the user having to
    // click into the input first — keeps the "type as soon as it
    // opens" UX every keyboard-driven dialog needs. Re-commits keep
    // whatever focus the user navigated to via Tab / click; we never
    // auto-jump focus during a session.
    let focus_path: Vec<u32> = if is_first_commit {
        find_first_input_path(&tree).unwrap_or_default()
    } else {
        prev_focus
    };

    // Reconcile the input editor against the new tree:
    //   - focus targets an Input with the same value the editor already
    //     holds → keep the prev state (the app just echoed our buffer
    //     back, the cursor must NOT reset).
    //   - focus targets an Input with a different value → app overrode
    //     it programmatically; rebuild from the tree value.
    //   - focus elsewhere → drop the editor.
    let input_edit: Option<InputEditState> = if focus_path.is_empty() {
        None
    } else {
        compute_input_edit(&tree, &focus_path, prev_input_edit.as_ref())
    };

    let hover_slice:  Option<&[u32]> = if prev_hover.is_empty() { None } else { Some(&prev_hover) };
    let focus_slice:  Option<&[u32]> = if focus_path.is_empty() { None } else { Some(&focus_path) };
    let active_slice: Option<&[u32]> = prev_active.as_deref();

    let pixels = rasterize_buffer_with_overlays(
        &tree, &layout_tree, &popovers, layout_rect,
        hover_slice, focus_slice, active_slice,
        density, input_edit.as_ref(),
    );

    // Store into the per-window scene map. Keep a clone of the tree
    // + layout for future resize re-renders (typical tree < 1 KB).
    SCENES.lock().insert(target_id, WidgetScene {
        pixels,
        width:       win_w,
        height:      win_h,
        origin_x:    win_x,
        origin_y:    win_y,
        tree:        tree.clone(),
        layout_tree,
        anchors,
        popovers,
        payload_hash: incoming_hash,
        hover_path:  prev_hover,
        focus_path,
        active_path: prev_active,
        density,
        has_pseudo,
        input_edit,
    });

    // Mark the window dirty so shade paints it in the next render,
    // then request a full render on Core 0. scene_commit may run on
    // a worker core — we never touch MMIO directly from here.
    crate::shade::with_compositor(|c| {
        if let Some(win) = c.windows.iter_mut().find(|w| w.id.0 == target_id) {
            win.dirty = true;
        }
    });
    crate::shade::request_render();

    match new_window {
        Some(id) => id as i32,
        None     => 0,
    }
}

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
fn rasterize_buffer_with_overlays(
    tree: &abi::Widget,
    layout_tree: &layout::LayoutNode,
    popovers: &[layout::PopoverLayout],
    rect: abi::Rect,
    hover_path: Option<&[u32]>,
    focus_path: Option<&[u32]>,
    active_path: Option<&[u32]>,
    density: abi::Density,
    input_edit: Option<&InputEditState>,
) -> Vec<u32> {
    let pixel_count = (rect.w as usize) * (rect.h as usize);
    let mut pixels: Vec<u32> = alloc::vec![0u32; pixel_count];

    // Clear to Surface token — covers areas not painted by any widget.
    let bg = palette::resolve(abi::Token::Surface);
    for p in pixels.iter_mut() { *p = bg; }

    let pal = palette::current();
    let mut target = abi::RasterTarget {
        pixels:  &mut pixels,
        stride:  rect.w,
        size:    abi::Size { w: rect.w, h: rect.h },
        origin:  abi::Point { x: rect.x, y: rect.y },
        scale:   1,
        palette: &pal,
    };
    let mut rast = raster::cpu::CpuRasterizer::new();
    render::render_with_state(
        &mut rast, &mut target, tree, layout_tree,
        hover_path, focus_path, active_path, density,
        input_edit,
    );

    // Overlays — paint after the main tree so they sit on top of any
    // pixels the main pass wrote into the same screen region. No
    // pseudo-state paths — popovers are transient by definition.
    for p in popovers {
        render::render_with_state(
            &mut rast, &mut target, &p.child, &p.layout,
            None, None, None, density, None,
        );
    }

    // `target` drops here, releasing the &mut on `pixels`.
    drop(target);
    pixels
}

/// Re-render a scene at new dimensions — called by shade when a
/// widget-kind window's content rect changes (resize / retile).
/// Uses the cached tree + re-runs layout so we don't need the app
/// to commit again. Returns false if no scene exists for that id.
/// Re-rasterize every live widget scene without changing its geometry.
/// Called after theme-affecting events (wallpaper change → new accent /
/// surface palette) so cached pixels pick up the fresh token colours.
pub fn refresh_all_scenes() {
    let keys: alloc::vec::Vec<u32> = SCENES.lock().keys().copied().collect();
    for wid in keys {
        let (tree, rect, hover_path, focus_path, active_path, density, input_edit) = match SCENES.lock().get(&wid) {
            Some(s) => (
                s.tree.clone(),
                abi::Rect { x: s.origin_x, y: s.origin_y, w: s.width, h: s.height },
                s.hover_path.clone(),
                s.focus_path.clone(),
                s.active_path.clone(),
                s.density,
                s.input_edit.clone(),
            ),
            None => continue,
        };
        let new_lo = layout::layout(&tree, rect);
        let h: Option<&[u32]> = if hover_path.is_empty() { None } else { Some(&hover_path) };
        let f: Option<&[u32]> = if focus_path.is_empty() { None } else { Some(&focus_path) };
        let a: Option<&[u32]> = active_path.as_deref();
        let new_pixels = rasterize_buffer_with_overlays(&tree, &new_lo.tree, &new_lo.popovers, rect, h, f, a, density, input_edit.as_ref());
        if let Some(scene) = SCENES.lock().get_mut(&wid) {
            scene.pixels      = new_pixels;
            scene.layout_tree = new_lo.tree;
            scene.anchors     = new_lo.anchors;
            scene.popovers    = new_lo.popovers;
        }
        crate::shade::with_compositor(|c| {
            if let Some(win) = c.windows.iter_mut().find(|w| w.id.0 == wid) {
                win.dirty = true;
            }
        });
    }
    crate::shade::request_render();
}

pub fn relayout_scene(window_id: u32, new_x: i32, new_y: i32, new_w: u32, new_h: u32) -> bool {
    let mut guard = SCENES.lock();
    let scene = match guard.get_mut(&window_id) {
        Some(s) => s,
        None    => return false,
    };
    if scene.width == new_w && scene.height == new_h
       && scene.origin_x == new_x && scene.origin_y == new_y {
        return true; // no-op, nothing moved
    }
    let new_rect = abi::Rect { x: new_x, y: new_y, w: new_w, h: new_h };
    let tree = scene.tree.clone();
    let new_density = classify_density(new_w);
    // Resize invalidates the cached hover_path AND active_path —
    // coordinates of the old layout no longer match. Focus survives
    // (a focused input stays focused after resize). Active is
    // mouse-tied so it gets cleared.
    let new_lo = layout::layout(&tree, new_rect);
    let focus_path = scene.focus_path.clone();
    let input_edit = scene.input_edit.clone();
    let f: Option<&[u32]> = if focus_path.is_empty() { None } else { Some(&focus_path) };
    let new_pixels = rasterize_buffer_with_overlays(
        &tree, &new_lo.tree, &new_lo.popovers, new_rect, None, f, None, new_density,
        input_edit.as_ref(),
    );
    scene.pixels      = new_pixels;
    scene.width       = new_w;
    scene.height      = new_h;
    scene.origin_x    = new_x;
    scene.origin_y    = new_y;
    scene.layout_tree = new_lo.tree;
    scene.anchors     = new_lo.anchors;
    scene.popovers    = new_lo.popovers;
    scene.density     = new_density;
    scene.hover_path.clear();
    scene.active_path = None;
    true
}
