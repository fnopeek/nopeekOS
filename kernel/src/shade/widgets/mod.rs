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
use alloc::vec::Vec;
use spin::Mutex;

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
    /// blake3 hash of the postcard payload that produced this
    /// scene. P10.6: lets scene_commit short-circuit when an app
    /// resubmits the same tree (common with interactive apps that
    /// re-commit on every event loop iteration).
    pub payload_hash: [u8; 32],
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
}

/// True if any widget scenes are currently allocated.
pub fn any_scenes() -> bool {
    !SCENES.lock().is_empty()
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
pub fn hit_test(window_id: u32, x: i32, y: i32) -> Option<abi::ActionId> {
    let scenes = SCENES.lock();
    let scene = scenes.get(&window_id)?;
    let mut out = None;
    find_click_target(&scene.tree, &scene.layout_tree, x, y, &mut out);
    out
}

fn find_click_target(
    widget: &abi::Widget,
    layout: &layout::LayoutNode,
    x: i32, y: i32,
    out: &mut Option<abi::ActionId>,
) {
    if !rect_contains(layout.rect, x, y) { return; }

    // Children first — deepest hit wins.
    for (cw, cl) in widget_children_ref(widget).iter().zip(layout.children.iter()) {
        find_click_target(cw, cl, x, y, out);
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
        None => {
            crate::kprintln!("[npk] scene_commit: empty payload");
            return -1;
        }
    };
    if version != abi::WIRE_VERSION {
        crate::kprintln!(
            "[npk] scene_commit: wire version mismatch (got {:#x}, want {:#x})",
            version, abi::WIRE_VERSION,
        );
        return -1;
    }
    // P10.6: hash the payload and short-circuit if it matches the
    // cached scene. Fast path for apps that re-commit identical
    // trees (happens on every event poll iteration in an idle app).
    // Hash the full `bytes` (version byte + body) so any wire-level
    // change (incl. version bump) invalidates the cache.
    let incoming_hash: [u8; 32] = *blake3::hash(bytes).as_bytes();
    if window_id != 0 {
        if let Some(cached_hash) = SCENES.lock().get(&window_id).map(|s| s.payload_hash) {
            if cached_hash == incoming_hash {
                crate::kprintln!(
                    "[npk] scene_commit: {} bytes → cache hit, skip re-render",
                    bytes.len(),
                );
                return 0;
            }
        }
    }

    let tree: abi::Widget = match postcard::from_bytes(body) {
        Ok(t) => t,
        Err(e) => {
            crate::kprintln!("[npk] scene_commit: postcard decode failed: {:?}", e);
            return -2;
        }
    };
    crate::kprintln!("[npk] scene_commit: {} bytes → tree decoded", bytes.len());
    debug::print_tree(&tree);

    // Obtain or create the widget window. `new_window` is Some iff we
    // just created one — return its id to the caller so the next
    // commit reuses the same slot.
    let (target_id, new_window) = match window_id {
        0 => {
            let id = match crate::shade::with_compositor(|c| c.create_widget_window("widget")) {
                Some(id) => id,
                None => {
                    crate::kprintln!("[npk] scene_commit: shade not available");
                    return -3;
                }
            };
            (id.0, Some(id.0))
        }
        id => (id, None),
    };

    // Look up the window's current content rect. retile() may have
    // moved it since the last commit, so always re-query.
    let rect = match crate::shade::with_compositor(|c| {
        let win = c.windows.iter().find(|w| w.id.0 == target_id)?;
        let border = c.border;
        Some((
            win.content_x(border) as i32,
            win.content_y(border) as i32,
            win.content_w(border),
            win.content_h(border),
        ))
    }).flatten() {
        Some(r) => r,
        None => {
            crate::kprintln!("[npk] scene_commit: widget window {} not found", target_id);
            return -3;
        }
    };
    let (win_x, win_y, win_w, win_h) = rect;
    if win_w == 0 || win_h == 0 {
        crate::kprintln!("[npk] scene_commit: window has zero-size content rect");
        return -3;
    }

    // Lay out the tree inside the resolved rect, then rasterize into a
    // fresh back buffer. The per-window scene takes ownership of the
    // buffer so shade can re-blit on every render pass.
    let layout_rect = abi::Rect { x: win_x, y: win_y, w: win_w, h: win_h };
    let layout_tree = layout::layout(&tree, layout_rect);
    debug::print_layout(&tree, &layout_tree);

    let pixels = rasterize_to_buffer(&tree, &layout_tree, win_x, win_y, win_w, win_h);

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
        payload_hash: incoming_hash,
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

    crate::kprintln!(
        "[npk] scene_commit: rendered {}x{} into widget window #{}",
        win_w, win_h, target_id,
    );

    // On first commit, tell the caller its new window id so it can
    // store it in HostState and reuse on the next commit.
    match new_window {
        Some(id) => id as i32,
        None     => 0,
    }
}

/// Alloc a BGRA back buffer, clear to Surface, run the render walker,
/// return the pixel vec. Used by both `scene_commit` (fresh) and the
/// relayout path (resize / re-render from cached tree).
fn rasterize_to_buffer(
    tree: &abi::Widget,
    layout_tree: &layout::LayoutNode,
    win_x: i32, win_y: i32, win_w: u32, win_h: u32,
) -> Vec<u32> {
    let pixel_count = (win_w as usize) * (win_h as usize);
    let mut pixels: Vec<u32> = alloc::vec![0u32; pixel_count];

    // Clear to Surface token — covers areas not painted by any widget.
    let bg = palette::resolve(abi::Token::Surface);
    for p in pixels.iter_mut() { *p = bg; }

    let pal = palette::current();
    let mut target = abi::RasterTarget {
        pixels:  &mut pixels,
        stride:  win_w,
        size:    abi::Size { w: win_w, h: win_h },
        origin:  abi::Point { x: win_x, y: win_y },
        scale:   1,
        palette: &pal,
    };
    let mut rast = raster::cpu::CpuRasterizer::new();
    render::render(&mut rast, &mut target, tree, layout_tree);

    // `target` drops here, releasing the &mut on `pixels`.
    drop(target);
    pixels
}

/// Re-render a scene at new dimensions — called by shade when a
/// widget-kind window's content rect changes (resize / retile).
/// Uses the cached tree + re-runs layout so we don't need the app
/// to commit again. Returns false if no scene exists for that id.
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
    let new_layout = layout::layout(&tree, new_rect);
    let new_pixels = rasterize_to_buffer(&tree, &new_layout, new_x, new_y, new_w, new_h);
    scene.pixels      = new_pixels;
    scene.width       = new_w;
    scene.height      = new_h;
    scene.origin_x    = new_x;
    scene.origin_y    = new_y;
    scene.layout_tree = new_layout;
    true
}
