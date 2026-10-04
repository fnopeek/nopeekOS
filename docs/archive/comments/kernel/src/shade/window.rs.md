# `kernel/src/shade/window.rs` @ 5e0102684

## L1-5 · `use alloc::string::String;`

```
//! Window management for shade compositor.
//!
//! Windows are metadata (position, size, title, state). No per-window pixel
//! buffers — the compositor renders directly to the framebuffer shadow buffer.
//! This is efficient for tiling WMs where windows don't overlap.
```

## L9 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// Unique window identifier.
```

## L13 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// Window state.
```

## L16 · `Tiled,`

```
/// Normal tiled window.
```

## L18 · `Floating,`

```
/// Floating (manual position).
```

## L20 · `Fullscreen,`

```
/// Fullscreen (covers entire workspace area).
```

## L24-31 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// What kind of content the window renders.
///
/// - `Terminal`: classic `loop` window with a per-window terminal
///   buffer (existing behaviour — keyboard input, text output,
///   intent prompt). Uses `terminal_idx`.
/// - `Widget`: Phase 10 declarative GUI app window. Content is a
///   BGRA pixel buffer rendered by the widget pipeline from the
///   last committed tree. Doesn't own a terminal buffer.
```

## L36-40 · `Surface,`

```
/// Raw-bitmap window fed by an external pixel source (a microvm's
/// virtio-gpu framebuffer; later any Canvas-escape-hatch app).
/// No terminal buffer, no widget tree — its content is a
/// `GuestSurface` double-buffer keyed by WindowId. Composited as a
/// tile like any other window (tiling invariant; never fullscreen).
```

## L44-45 · `#[allow(dead_code)]`

```
/// A single window managed by the compositor.
/// No pixel buffer — rendering goes directly to the shadow buffer.
```

## L50 · `pub x: u32,`

```
/// Position relative to screen origin (set by WM layout).
```

## L53 · `pub width: u32,`

```
/// Outer dimensions (including border).
```

## L56 · `pub bg_color: u32,`

```
/// Background color for the content area.
```

## L61 · `pub dirty: bool,`

```
/// True if content changed since last render.
```

## L63 · `pub workspace: u8,`

```
/// Workspace index (0-based).
```

## L65 · `pub terminal_idx: u8,`

```
/// Terminal buffer index (separate session per window).
```

## L67 · `pub resize_w: i32,`

```
/// Resize delta for tiling split adjustment (pixels, can be negative).
```

## L70-75 · `pub split_from: Option<WindowId>,`

```
/// Dwindle: the window this one SPLIT when it opened, and which way.
///
/// Together these two fields are the whole layout tree — a dwindle tree is
/// fully determined by "who did each window split, and in which direction",
/// so no separate structure has to be kept in sync with the window list.
/// `None` = this window owns the workspace's whole area (the root).
```

## L77-79 · `pub split_beside: bool,`

```
/// `true` = the split put the new window BESIDE its parent, `false` = below.
/// Decided once, from the parent's shape at the moment of the split, so the
/// layout stays stable when windows are later resized or closed.
```

## L81 · `pub pid: u32,`

```
/// Process ID in process table (0 = not registered).
```

## L83 · `pub kind: WindowKind,`

```
/// Content kind — Terminal (classic loop) or Widget (Phase 10 GUI).
```

## L85-86 · `pub is_overlay: bool,`

```
/// Overlay window: skipped by retile, keeps its own geometry,
/// rendered on top. Set by the app via `npk_window_set_overlay`.
```

## L88-90 · `pub modal: bool,`

```
/// Modal window: while this window is on the active workspace,
/// shade-actions that would shift focus or reshape the grid are
/// suppressed. Set by the app via `npk_window_set_modal`.
```

## L92-95 · `pub is_dock: bool,`

```
/// Dock window: an auto-hide overlay anchored to the bottom edge
/// (`npk_window_set_dock`). Implies `is_overlay`. Excluded from the
/// focus cycle; the compositor owns its reveal/hide slide and keeps
/// its `workspace` synced to the active one (global across spaces).
```

## L97-100 · `pub is_bar: bool,`

```
/// Bar window: a top-edge strut panel (`npk_window_set_panel(Top,
/// Strut)`). Implies `is_overlay`. Positioned by the compositor into
/// the bar band, always visible, global across workspaces, never
/// focused. Rendered with the translucent-tray blit like the dock.
```

## L102-104 · `pub light_dismiss: bool,`

```
/// Light-dismiss: close this window when a click lands outside it
/// (opt-in via `npk_window_set_light_dismiss`). Used by transient
/// overlays like the volume slider. Independent of `modal`.
```

## L110 · `pub fn new(id: WindowId, title: &str, x: u32, y: u32, w: u32, h: u32) -> Self {`

```
/// Create a new window (metadata only, no pixel buffer).
```

## L140 · `pub fn content_x(&self, border: u32) -> u32 {`

```
/// Content area origin (inside border).
```

## L149 · `pub fn content_w(&self, border: u32) -> u32 {`

```
/// Content area dimensions (excluding border).
```

## L158 · `pub fn render_to(&self, shadow: *mut u8, info: &crate::framebuffer::FbInfo, border: u32) {`

```
/// Render the window content area directly to the shadow buffer.
```

## L167 · `crate::gui::render::fill_rect(shadow, info, cx, cy, cw, ch, self.bg_color);`

```
// Fill content area with background color
```

