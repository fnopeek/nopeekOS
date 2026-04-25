//! Widget ABI — frozen v1 wire contract.
//!
//! This module defines the shape of the widget tree as it crosses the WASM
//! sandbox boundary. Every type here is part of the persistent ABI: enum
//! variant order, #[repr] discriminants, and struct field order are all
//! frozen at v1 (WIRE_VERSION = 0x01).
//!
//! Rules (see PHASE10_WIDGETS.md "ABI stability & future-proofing"):
//!   - All ABI-visible enums carry #[non_exhaustive]
//!   - New variants appended only — never inserted, never reordered
//!   - Removing a variant = wire-version bump
//!   - Reserved variants use #[allow(dead_code)] to hold the slot
//!   - #[repr(u8)] or #[repr(u16)] where the variant index is the ABI
//!
//! P10.0 scope: signatures + constants, no logic. Serialization lands in
//! P10.1, deserialization in P10.2.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;

// ── Wire version ──────────────────────────────────────────────────────

/// Wire protocol version byte. Prefixed to every `npk_scene_commit` payload.
/// Compositor rejects unknown versions with `-1`.
///
/// Bump when the wire contract changes incompatibly. Forward-compatible
/// additions (Option<T> at struct tail, appended variants) do not bump.
pub const WIRE_VERSION: u8 = 0x01;

// ── Geometry ──────────────────────────────────────────────────────────

/// Point in window coordinates (px).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Size in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

/// Rectangle in window coordinates (px).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

// ── Identifiers ───────────────────────────────────────────────────────

/// App-defined action identifier. Attached to `on_click`, `on_submit`,
/// `on_toggle` modifiers. Echoed back via `Event::Action(ActionId)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ActionId(pub u32);

/// Canvas leaf identifier. Matches the `canvas_id` passed to
/// `npk_canvas_commit` for pixel delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CanvasId(pub u32);

/// Node identifier inside a tree (structural path hash, compositor-assigned).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NodeId(pub u32);

// ── Theme tokens ──────────────────────────────────────────────────────

/// Theme tokens. Apps never specify hex colors — the compositor resolves
/// tokens against the active palette at raster time.
///
/// Integer values frozen on v1 release. New tokens **appended only** —
/// existing values never reassigned.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Token {
    // Surfaces
    Surface         = 0,
    SurfaceElevated = 1,
    SurfaceMuted    = 2,

    // Text
    OnSurface       = 3,
    OnSurfaceMuted  = 4,
    OnAccent        = 5,

    // Accent
    Accent          = 6,
    AccentMuted     = 7,

    // Semantic
    Border          = 8,
    Success         = 9,
    Warning         = 10,
    Danger          = 11,
    // Appended only — values frozen forever.
}

// ── Icons ─────────────────────────────────────────────────────────────

/// Curated icon identifier. Atlas rasterized at build time (P10.9).
/// `#[repr(u16)]` — values frozen. Adding new icons appends.
///
/// P10.0 scaffolding: only `None` placeholder + a few core variants from
/// the file-browser example. Real atlas populated in P10.9.
#[repr(u16)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IconId {
    None                = 0,
    Folder              = 1,
    File                = 2,
    ArrowLeft           = 3,
    ArrowRight          = 4,
    ArrowUp             = 5,
    ArrowDown           = 6,
    Home                = 7,
    Download            = 8,
    // P10.9 additions — Phosphor Regular subset
    MagnifyingGlass     = 9,
    X                   = 10,
    Check               = 11,
    Gear                = 12,
    Power               = 13,
    Lock                = 14,
    Terminal            = 15,
    Trash               = 16,
    DotsThreeVertical   = 17,
    List                = 18,
    // P10.11 file-browser additions (loft)
    Monitor             = 19,
    FileText            = 20,
    FolderOpen          = 21,
    Image               = 22,
    HardDrives          = 23,
    Code                = 24,
    Folders             = 25,
    CaretRight          = 26,
    ArrowClockwise      = 27,
    // Appended only.
}

// ── Accessibility roles ───────────────────────────────────────────────

/// A11y role tag. v1 stores but does not consume. Freezing the enum now
/// avoids a wire-version bump when screen readers / UI automation land.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Role {
    /// Decorative only — skip in traversal.
    None      = 0,
    Button    = 1,
    Link      = 2,
    TextInput = 3,
    List      = 4,
    ListItem  = 5,
    Heading   = 6,
    Image     = 7,
    Separator = 8,
    Group     = 9,
    Status    = 10,
    // Appended only.
}

// ── Text style ────────────────────────────────────────────────────────

/// Typographic style token. Maps to Inter Variable weight/size tuple at
/// raster time. `Mono` routes to the Spleen bitmap font (terminal look).
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TextStyle {
    Body    = 0,
    Title   = 1,
    Caption = 2,
    Muted   = 3,
    Mono    = 4,
    // Appended only.
}

// ── Fill (rasterizer-side only) ───────────────────────────────────────

/// Fill description passed to the rasterizer. Never appears in the wire
/// tree directly — constructed from Modifier tokens during raster setup.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Fill {
    Solid(Token),
    // Appended only (gradients etc. in future versions).
}

// ── Effect IDs (reserved) ─────────────────────────────────────────────

/// Named GPU effect reference. Populated in Phase 12 (Xe render engine).
/// CPU rasterizer treats `.effect(_)` as no-op.
#[repr(u16)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectId {
    /// Reserved placeholder — no effects registered in v1.
    None = 0,
    // Appended only.
}

// ── Layout primitives ─────────────────────────────────────────────────

/// Row/Column alignment on the cross axis.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Align {
    Start   = 0,
    Center  = 1,
    End     = 2,
    Stretch = 3,
    // Appended only.
}

/// Scroll container axis.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Axis {
    Vertical   = 0,
    Horizontal = 1,
    Both       = 2,
    // Appended only.
}

// ── Container-query density ───────────────────────────────────────────

/// Compositor-classified window size bucket. Apps reference these via
/// `Modifier::WhenDensity(Density, ...)` to adapt layout to the available
/// space without picking pixel breakpoints. Thresholds live once in the
/// compositor (Compact <600 px, Regular 600–1200 px, Spacious >1200 px).
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Density {
    Compact  = 0,
    Regular  = 1,
    Spacious = 2,
    // Appended only.
}

// ── Animation ─────────────────────────────────────────────────────────

/// Transition curve. Deterministic fixed-point math lives in the
/// compositor; the wire form just carries the curve choice.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Transition {
    /// Spring physics with default stiffness/damping (compositor-owned).
    Spring,
    /// Linear interpolation over `ms` milliseconds.
    Linear { ms: u16 },
    // Appended only.
}

/// Drop-shadow parameters (reserved — Modifier::Shadow).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Shadow {
    pub offset: Point,
    pub blur:   u8,
    pub token:  Token,
}

// ── Modifier ──────────────────────────────────────────────────────────

/// Modifier applied to a widget node. Modifiers are carried as a `Vec` on
/// each `Widget` so ordering is preserved (affects rendering: padding
/// outside background, border on top, etc.).
///
/// Variant order frozen at v1. Reserved slots below are declared so their
/// wire indices do not shift when v2 implements them.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Modifier {
    // ── Active in v1 ──────────────────────────────────────────────────
    /// Inner padding (px at 1× scale, scaled at raster time).
    Padding(u16),
    /// Outer margin (px at 1× scale).
    Margin(u16),
    /// Background token fill.
    Background(Token),
    /// Border: token, width (px), corner radius (px).
    Border {
        token:  Token,
        width:  u8,
        radius: u8,
    },
    /// Opacity 0..=255 (0 = fully transparent, 255 = opaque). Fixed-point
    /// on purpose — no float non-determinism.
    Opacity(u8),
    /// Declares that this widget animates when its props change.
    Transition(Transition),
    /// Click handler — compositor synthesizes `Event::Action(id)` on hit.
    OnClick(ActionId),
    /// Hover handler.
    OnHover(ActionId),

    // ── Reserved slots (v2+) ──────────────────────────────────────────
    // CPU rasterizer treats these as no-ops in v1. Slots declared now so
    // wire indices do not shift when the GPU rasterizer (Phase 12)
    // implements them. Do not insert before these.
    #[allow(dead_code)]
    Blur(u8),
    #[allow(dead_code)]
    Shadow(Shadow),
    #[allow(dead_code)]
    Effect(EffectId),
    /// A11y role override (v1 reads but does not consume).
    #[allow(dead_code)]
    RoleOverride(Role),
    /// Paint an Icon in the given Token color instead of OnSurface.
    Tint(Token),

    // ── Vocab v2 (Tailwind-style modifiers) ───────────────────────────
    // Pseudo-state modifier lists — compositor merges the inner list onto
    // the widget when the state matches; the tree itself stays static
    // across hovers (no app round-trip).
    Hover(Vec<Modifier>),
    Focus(Vec<Modifier>),
    Active(Vec<Modifier>),
    Disabled(Vec<Modifier>),
    /// Container query — apply inner modifiers only at the given density.
    WhenDensity(Density, Vec<Modifier>),
    /// Uniform scale, Q8.8 fixed-point. 256 = 1.0× (identity).
    Scale(u16),
    /// Layout minimum width (px at 1× scale).
    MinWidth(u16),
    /// Layout maximum width (px at 1× scale).
    MaxWidth(u16),
    /// Corner radius (px at 1× scale) without a Border.
    Rounded(u8),
    // Appended only.
}

// ── Widget ────────────────────────────────────────────────────────────

/// Widget tree node. A single `Widget` = root of a render commit.
///
/// Variant order frozen at v1. Reserved slots (`Popover`/`Tooltip`/`Menu`)
/// are declared so their wire indices do not shift when v2 implements
/// them — compositor rejects them with a log until then.
///
/// Struct-variant field order is also part of the ABI (postcard serializes
/// fields in declaration order).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Widget {
    // ── Containers ────────────────────────────────────────────────────
    Column {
        children:  Vec<Widget>,
        spacing:   u16,
        align:     Align,
        modifiers: Vec<Modifier>,
    },
    Row {
        children:  Vec<Widget>,
        spacing:   u16,
        align:     Align,
        modifiers: Vec<Modifier>,
    },
    Stack {
        children:  Vec<Widget>,
        modifiers: Vec<Modifier>,
    },
    Scroll {
        child:     alloc::boxed::Box<Widget>,
        axis:      Axis,
        modifiers: Vec<Modifier>,
    },

    // ── Leaves ────────────────────────────────────────────────────────
    Text {
        content:   String,
        style:     TextStyle,
        modifiers: Vec<Modifier>,
    },
    Icon {
        id:        IconId,
        size:      u16,
        modifiers: Vec<Modifier>,
    },
    Button {
        /// Text label (empty for icon-only buttons).
        label:     String,
        /// Optional icon (IconId::None = no icon).
        icon:      IconId,
        on_click:  ActionId,
        modifiers: Vec<Modifier>,
    },
    Input {
        value:       String,
        placeholder: String,
        on_submit:   ActionId,
        modifiers:   Vec<Modifier>,
    },
    Checkbox {
        value:     bool,
        on_toggle: ActionId,
        modifiers: Vec<Modifier>,
    },
    Spacer {
        flex:      u8,
    },
    Divider,
    Canvas {
        id:        CanvasId,
        width:     u16,
        height:    u16,
        modifiers: Vec<Modifier>,
    },

    // ── Reserved slots (v2+) ──────────────────────────────────────────
    // Out-of-window overlay widgets. Cannot be retrofitted without
    // breaking every serialized tree. Compositor logs + rejects in v1.
    // Do not insert before these.
    #[allow(dead_code)]
    Popover {
        anchor:    NodeId,
        child:     alloc::boxed::Box<Widget>,
        modifiers: Vec<Modifier>,
    },
    #[allow(dead_code)]
    Tooltip {
        text:      String,
        anchor:    NodeId,
        modifiers: Vec<Modifier>,
    },
    #[allow(dead_code)]
    Menu {
        items:     Vec<Widget>,
        modifiers: Vec<Modifier>,
    },
    // Appended only.
}

// ── Events (compositor → app) ─────────────────────────────────────────

/// Input event delivered to a WASM app via `npk_event_poll` /
/// `npk_event_wait`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Event {
    /// Keyboard input. Uses the existing Phase 8 KeyCode (already stable).
    Key(crate::input::KeyCode),
    /// User-defined action (synthesized from `OnClick` / `OnHover`).
    Action(ActionId),
    /// Mouse pointer moved (window-local coords).
    MouseMove { x: i32, y: i32 },
    /// Mouse button pressed/released at position.
    MouseButton {
        button: MouseButton,
        down:   bool,
        x:      i32,
        y:      i32,
    },
    /// Window focus changed.
    Focus(bool),
    // Appended only.
}

#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MouseButton {
    Left   = 0,
    Right  = 1,
    Middle = 2,
    // Appended only.
}

// ── Actions (app → compositor, via App trait return value) ────────────

/// Action returned by `App::handle(event) -> Action`. The SDK uses this
/// to decide whether to re-render; it does not cross the wire verbatim.
/// Declared here so the enum lives alongside its counterpart `Event`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Action {
    /// No state change — do not re-render.
    Idle,
    /// State changed — SDK calls render() and commits a new tree.
    Rerender,
    /// App wants to exit (window close).
    Exit,
    // Appended only.
}

// ── Rasterizer abstraction ────────────────────────────────────────────

/// Palette lookup — maps Token → concrete BGRA32 at raster time.
/// Defined here as a shape placeholder; the concrete `Palette` type
/// lives in `gui/color.rs` and is re-exported when the rasterizer lands.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// BGRA32 color per Token, indexed by `Token as u8`.
    /// Slot 0 = Token::Surface, slot 11 = Token::Danger, …
    pub colors: [u32; 16],
}

/// A raster destination. Either a tile in the GGTT slab, or a composition
/// layer — from the rasterizer's perspective they are identical: a BGRA32
/// pixel buffer with an origin offset in window coordinates.
///
/// The rasterizer receives `Rect`s and `Point`s in **window** coordinates
/// and subtracts `origin` internally to get the target-local position.
/// Draws are clipped to `size`. This is what makes tile-boundary drawing
/// Just Work — the left tile clips the right half away, and vice versa.
pub struct RasterTarget<'a> {
    /// Backing pixel buffer (BGRA32, packed u32 per pixel).
    pub pixels:  &'a mut [u32],
    /// Pixels per row (may exceed `size.w` for aligned allocations).
    pub stride:  u32,
    /// Target size in pixels.
    pub size:    Size,
    /// Target top-left in window coordinates.
    /// Tiles:  `(tx * TILE_SIZE_PX, ty * TILE_SIZE_PX)`
    /// Layers: node's layout rect top-left.
    pub origin:  Point,
    /// HiDPI factor (1 or 2).
    pub scale:   u8,
    /// Active theme palette — Token → concrete BGRA.
    pub palette: &'a Palette,
}

/// Rasterizer backend. CPU in v1 (fontdue + gui/render.rs). GPU in v2+
/// (Intel Xe Render engine, SDF text atlas, fragment shaders for blur /
/// shadow / effect).
///
/// Non-negotiable: no call site in the widget pipeline references CPU or
/// GPU specifics. Switching backends = replacing `Box<dyn Rasterizer>`.
pub trait Rasterizer: Send + Sync {
    /// Fill the entire target with a theme-token color.
    fn clear(&mut self, t: &mut RasterTarget, color: Token);

    /// Draw a filled rectangle. `r` is in window coordinates.
    fn rect(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill);

    /// Fill a rounded rectangle. Default falls back to sharp rect.
    fn rect_rounded(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill, _radius: u8) {
        self.rect(t, r, fill);
    }

    /// Stroke a rounded-rect outline `width`-thick. Default draws 4 sharp
    /// rects ignoring radius.
    fn stroke_rounded(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill, width: u8, _radius: u8) {
        let w = width as u32;
        if w == 0 { return; }
        let wi = w as i32;
        self.rect(t, Rect { x: r.x, y: r.y, w: r.w, h: w }, fill);
        self.rect(t, Rect { x: r.x, y: r.y + r.h as i32 - wi, w: r.w, h: w }, fill);
        self.rect(t, Rect { x: r.x, y: r.y, w: w, h: r.h }, fill);
        self.rect(t, Rect { x: r.x + r.w as i32 - wi, y: r.y, w: w, h: r.h }, fill);
    }

    /// Draw text at baseline point `p` (window coordinates).
    fn text(&mut self, t: &mut RasterTarget, s: &str, style: TextStyle, p: Point);

    /// Draw an icon from the built-in atlas.
    fn icon(&mut self, t: &mut RasterTarget, id: IconId, size: u16, color: Token, p: Point);

    /// Copy app-supplied Canvas pixels (BGRA32) into the target.
    fn canvas_copy(&mut self, t: &mut RasterTarget, src: &[u8], w: u16, h: u16);

    // ── Reserved (v2+, default no-op on CPU backend) ──────────────────

    /// Gaussian blur behind the given rect (acrylic/glass effect).
    fn blur(&mut self, _t: &mut RasterTarget, _r: Rect, _radius: u8) {}

    /// Drop shadow under the given rect.
    fn shadow(&mut self, _t: &mut RasterTarget, _r: Rect, _s: Shadow) {}

    /// Named GPU effect.
    fn effect(&mut self, _t: &mut RasterTarget, _r: Rect, _id: EffectId) {}
}
