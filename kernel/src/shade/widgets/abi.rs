//! Widget ABI — frozen v1 wire contract.
//!
//! This module defines the shape of the widget tree as it crosses the WASM
//! sandbox boundary. Every type here is part of the persistent ABI: enum
//! variant order, #[repr] discriminants, and struct field order are all
//! frozen at v1 (WIRE_VERSION = 0x01).
//!
//! Rules (see docs/archive/PHASE10_WIDGETS.md "ABI stability & future-proofing"):
//!   - All ABI-visible enums carry #[non_exhaustive]
//!   - New variants appended only — never inserted, never reordered
//!   - Removing a variant = wire-version bump
//!   - Reserved variants use #[allow(dead_code)] to hold the slot
//!   - #[repr(u8)] or #[repr(u16)] where the variant index is the ABI
//!
//! Signatures and constants only, no logic.

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

/// A coloured run inside a `Widget::TextArea`'s `value` (byte offsets +
/// colour token). Mirror of the SDK `Span`. The compositor colours each
/// byte of the live buffer by the span covering it; uncovered → default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Span {
    pub start: u32,
    pub len:   u32,
    pub token: Token,
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

impl NodeId {
    /// Anchor a `Popover` at the point of the window's last right click
    /// instead of at a widget: where a context menu belongs.
    pub const POINTER: NodeId = NodeId(u32::MAX);
}

// ── Theme tokens ──────────────────────────────────────────────────────

/// Theme tokens. Apps never specify hex colors — the compositor resolves
/// tokens against the active palette at raster time.
///
/// Integer values frozen on v1 release. New tokens are appended only;
/// existing values are never reassigned.
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

    // UI-Refresh ramp extension (see docs/spec/UI_REFRESH.md §1).
    /// Content canvas — below `Surface`. Editor body, page, terminal.
    Page            = 12,
    /// Hover fill / chips — above `SurfaceMuted`.
    SurfaceHover    = 13,
    /// Third text level: section headings, meta columns, disabled.
    OnSurfaceFaint  = 14,
    /// Accent at 22 % over `Surface` — focus rings.
    AccentRing      = 15,
    /// Accent at 45 % over `Surface` — focused window border.
    AccentLine      = 16,

    // ── Code tokens (syntax highlighting) ─────────────────────────────
    //
    // A second, independent ramp. The tokens above describe chrome; these
    // describe source text. An editor needs both at once, and reusing
    // `Accent`/`Warning` for keywords and strings would tie the syntax
    // colours to the wallpaper. Resolved from the active code scheme
    // (`set code.scheme`), never from the accent.
    /// Declaration / storage keywords: `fn` `let` `def` `class` `int`.
    CodeKeyword     = 17,
    /// Control flow and imports: `if` `for` `return` `import` `match`.
    CodeControl     = 18,
    /// String and character literals, quotes included.
    CodeString      = 19,
    /// Comments, any syntax.
    CodeComment     = 20,
    /// Numeric literals.
    CodeNumber      = 21,
    /// Function names — declaration and call site.
    CodeFunction    = 22,
    /// Type / class names and markup tag names.
    CodeType        = 23,
    /// Attribute names, JSON keys, decorators.
    CodeVariable    = 24,
    /// Language constants (`true` `None` `null`) and escape sequences.
    CodeConstant    = 25,
    // Appended only — values frozen forever.
}

// ── Icons ─────────────────────────────────────────────────────────────

/// Icon identifier — an index into the Phosphor atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct IconId(pub u16);

/// The named icons. `IconId` is a number, not a closed enum: a module that
/// only passes an icon on (the dock showing an app's icon) must not have to
/// know every icon there is, or each new icon would mean rebuilding dock,
/// drun and bar. The atlas is looked up by number, and a number it lacks
/// draws nothing.
///
/// Wire format: postcard writes an enum variant index and a `u16` as the
/// same varint, so the numbers are wire-compatible with enum
/// discriminants. Values frozen; append only — and the atlas
/// (`tools/regen-icons`) must carry every number named here.
macro_rules! icon_ids {
    ($($name:ident = $n:expr,)*) => {
        #[allow(non_upper_case_globals)]
        impl IconId {
            $(pub const $name: IconId = IconId($n);)*
            /// Every named icon, for tests and tooling.
            pub const ALL: &'static [(&'static str, IconId)] =
                &[$((stringify!($name), IconId($n)),)*];
        }
    };
}

icon_ids! {
    None                  = 0,
    Folder                = 1,
    File                  = 2,
    ArrowLeft             = 3,
    ArrowRight            = 4,
    ArrowUp               = 5,
    ArrowDown             = 6,
    Home                  = 7,
    Download              = 8,
    MagnifyingGlass       = 9,
    X                     = 10,
    Check                 = 11,
    Gear                  = 12,
    Power                 = 13,
    Lock                  = 14,
    Terminal              = 15,
    Trash                 = 16,
    DotsThreeVertical     = 17,
    List                  = 18,
    Monitor               = 19,
    FileText              = 20,
    FolderOpen            = 21,
    Image                 = 22,
    HardDrives            = 23,
    Code                  = 24,
    Folders               = 25,
    CaretRight            = 26,
    ArrowClockwise        = 27,
    Globe                 = 28,
    Camera                = 29,
    BatteryEmpty          = 30,
    BatteryLow            = 31,
    BatteryMedium         = 32,
    BatteryHigh           = 33,
    BatteryFull           = 34,
    BatteryCharging       = 35,
    BatteryWarning        = 36,
    Plug                  = 37,
    SpeakerHigh           = 38,
    SpeakerLow            = 39,
    SpeakerX              = 40,
    Minus                 = 41,
    Plus                  = 42,
    Bird                  = 43,
    MusicNotes            = 44,
    Play                  = 45,
    Pause                 = 46,
    SkipBack              = 47,
    SkipForward           = 48,
    FileAudio             = 49,
    ArrowCounterClockwise = 50,
    PlayCircle            = 51,
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
    /// 18 px regular weight (appended).
    Heading = 5,
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

/// Named GPU effect reference, for a future GPU render backend.
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
    // wire indices do not shift when a GPU rasterizer
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
    Hover(#[serde(deserialize_with = "nested")] Vec<Modifier>),
    Focus(#[serde(deserialize_with = "nested")] Vec<Modifier>),
    Active(#[serde(deserialize_with = "nested")] Vec<Modifier>),
    Disabled(#[serde(deserialize_with = "nested")] Vec<Modifier>),
    /// Container query — apply inner modifiers only at the given density.
    WhenDensity(Density, #[serde(deserialize_with = "nested")] Vec<Modifier>),
    /// Uniform scale, Q8.8 fixed-point. 256 = 1.0× (identity).
    Scale(u16),
    /// Layout minimum width (px at 1× scale).
    MinWidth(u16),
    /// Layout maximum width (px at 1× scale).
    MaxWidth(u16),
    /// Corner radius (px at 1× scale) without a Border.
    Rounded(u8),
    /// CSS-style flex-grow on the main axis of the parent Row/Column.
    /// Adds the widget's intrinsic main size as a basis and absorbs a
    /// proportional share of the leftover alongside any
    /// `Spacer { flex }` siblings. `Flex(0)` = no flex.
    Flex(u8),
    /// Tag a widget with an app-chosen `NodeId`. Layout records the
    /// tagged widget's rect into a side table; `Widget::Popover`'s
    /// `anchor` field looks rects up there at render time.
    NodeId(NodeId),
    /// Focus ring — a stroke of `width` px drawn just outside the node's
    /// rect, under any Border. Mirrors CSS `box-shadow: 0 0 0 Npx`, which
    /// is how the design expresses focus. Costs no layout space; the
    /// caller must leave room (a row's gap is enough at width ≤ 3).
    Ring { token: Token, width: u8 },
    /// Layout minimum height (px at 1× scale). The design pins row
    /// heights (30/34/36/44); without this a row's height is whatever
    /// its tallest glyph plus padding happens to be.
    MinHeight(u16),
    /// Layout maximum height (px at 1× scale).
    MaxHeight(u16),
    /// Draw a line-number gutter down the left edge of a `TextArea`.
    /// The compositor owns that widget's viewport, so only it can keep
    /// the numbers in step with scrolling — an app-drawn column would
    /// drift the moment the buffer scrolls.
    LineNumbers(bool),
    /// Per-axis inner padding (px at 1× scale). `Padding` is uniform;
    /// the design routinely wants different insets per axis (a search
    /// field is `padding: 0 10px` with its height set by a clamp), and
    /// forcing one value makes the horizontal air hostage to the row
    /// height. Sums with any `Padding` on the same node.
    ///
    /// Appended after LineNumbers, not before it: inserting ahead of a
    /// shipped variant renumbers it on the wire, and an app built against
    /// the old order would send indices the compositor decodes as garbage.
    PaddingXY { x: u16, y: u16 },
    /// This text widget takes focus when its window first appears.
    /// Opt-in: auto-focusing the first `Input` is right for a launcher and
    /// wrong for anything with a search box, where a list would lose every
    /// arrow key to a field the user never clicked.
    Autofocus,
    /// Font size override in px for a `Widget::Text` — replaces the size
    /// its `TextStyle` resolves to; the face (proportional vs mono) still
    /// comes from the style. Layout measures at the override, so the text
    /// keeps a correct box. Clamped to `FONT_SIZE_RANGE`; ignored on every
    /// other widget.
    FontSize(u16),
    /// Pan a `Widget::Canvas`'s content by this many px, relative to the
    /// centred contain-fit position. Pairs with `Modifier::Scale`: scale
    /// decides how big the image is drawn, this decides which part of it
    /// the rect shows. Clamped to the overhang, so it can never push the
    /// content out of view. Ignored on every other widget.
    CanvasOffset { x: i32, y: i32 },
    /// Coloured runs over the text of a `Widget::Input`, in byte offsets —
    /// what `Widget::TextArea.spans` does for the editor.
    ///
    /// Built for the address bar: a browser highlights the registrable
    /// domain and dims the rest, and that is the anti-phishing display, not
    /// decoration. In `https://paypal.com.example.ru/login` the domain is
    /// `example.ru`, and without the highlight the eye reads the first thing
    /// that looks like a name. The app computes the span itself (e.g. from
    /// the Public Suffix List); the toolkit only colours what it is told and
    /// knows nothing about URLs.
    ///
    /// Uncovered bytes keep the default colour. Overlapping or unsorted
    /// spans are allowed; the later one wins.
    Spans(alloc::vec::Vec<Span>),
    /// Fire `Event::Action(id)` while the pointer moves over this widget,
    /// at most every [`MOTION_INTERVAL_MS`] — and at once when the pointer
    /// moves onto it from another `OnMotion` target (deepest wins, like
    /// `OnClick`). For "show the controls while the mouse is being used":
    /// apps never get raw pointer motion, and `OnHover` fires only on
    /// entering, so a pointer that rests and then moves again inside the
    /// same widget would go unnoticed.
    OnMotion(ActionId),
    /// The app paints a `Widget::Canvas` at the rect's own size
    /// (`npk_canvas_rect`), so its pixels are 1:1 with the screen. When the
    /// stored bitmap no longer matches the rect — the window was resized and
    /// the app has not committed the new size yet — it is drawn unscaled at
    /// the rect's top-left and its last column and row are extended over the
    /// uncovered part, instead of being contain-fit scaled. A scaled stale
    /// frame shrinks the whole picture for one frame and leaves the margins
    /// unpainted, which reads as flicker on every resize step. `Scale` and
    /// `CanvasOffset` are ignored while it is set. Ignored on every other
    /// widget.
    CanvasNative,
    /// Focus this `Widget::Input` or `Widget::TextArea` once, with its whole
    /// text selected, whenever the number differs from the one last honoured
    /// in this window. The app raises it when it wants the keyboard there —
    /// a browser's new tab or Ctrl+L — and re-committing the same number
    /// changes nothing, so the request survives the app re-rendering on
    /// every event without stealing focus back each time. 0 never requests.
    FocusRequest(u32),
    // Appended only.
}

/// Accepted range for `Modifier::FontSize`. The ceiling keeps a module
/// from filling the glyph cache with oversized bitmaps.
pub const FONT_SIZE_MIN: u16 = 6;
pub const FONT_SIZE_MAX: u16 = 64;

// ── Widget ────────────────────────────────────────────────────────────

/// Throttle of `Modifier::OnMotion`.
pub const MOTION_INTERVAL_MS: u32 = 200;

/// Upper end of `Widget::Slider::value` — per mille, fine enough for a
/// seek bar across a full-width window.
pub const SLIDER_MAX: u16 = 1000;

/// Widget tree node. A single `Widget` = root of a render commit.
///
/// Variant order frozen at v1. Reserved slots (`Popover`/`Tooltip`/`Menu`)
/// are declared so their wire indices do not shift when v2 implements
/// them — compositor rejects them with a log until then.
///
/// Deepest nesting a decoded tree may have, counting widgets inside widgets
/// and modifier lists inside modifiers alike. Every pass over the tree —
/// decode, layout, render, clone, drop — recurses once per level on the
/// caller's stack, and the tree comes from an app. Real interfaces stay
/// below 30.
pub const MAX_NESTING: u32 = 128;

/// Current nesting of the decode running on each core. A decode never
/// yields, so one counter per core is exclusive to it.
static NESTING: [core::sync::atomic::AtomicU32; 256] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 256];

/// `deserialize_with` for every recursive field: one level deeper, refused
/// past `MAX_NESTING` before the level is decoded.
fn nested<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    use core::sync::atomic::Ordering::Relaxed;
    struct Level(&'static core::sync::atomic::AtomicU32);
    impl Drop for Level {
        fn drop(&mut self) { self.0.fetch_sub(1, Relaxed); }
    }
    let cid = crate::smp::per_core::current_core_id() % NESTING.len();
    let counter = &NESTING[cid];
    let _level = Level(counter);
    if counter.fetch_add(1, Relaxed) >= MAX_NESTING {
        return Err(serde::de::Error::custom("widget tree nested too deeply"));
    }
    T::deserialize(d)
}

/// Struct-variant field order is also part of the ABI (postcard serializes
/// fields in declaration order).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Widget {
    // ── Containers ────────────────────────────────────────────────────
    Column {
        #[serde(deserialize_with = "nested")] 
        children:  Vec<Widget>,
        spacing:   u16,
        align:     Align,
        modifiers: Vec<Modifier>,
    },
    Row {
        #[serde(deserialize_with = "nested")] 
        children:  Vec<Widget>,
        spacing:   u16,
        align:     Align,
        modifiers: Vec<Modifier>,
    },
    Stack {
        #[serde(deserialize_with = "nested")] 
        children:  Vec<Widget>,
        modifiers: Vec<Modifier>,
    },
    Scroll {
        #[serde(deserialize_with = "nested")] 
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

    // ── Overlay widgets ───────────────────────────────────────────────
    // Out-of-flow rendering on top of the main tree. Cannot be
    // retrofitted without breaking every serialized tree. Do not
    // insert before these.

    /// Floating overlay anchored to a `Modifier::NodeId`-tagged widget
    /// elsewhere in the tree. Renders on top of everything (z-order)
    /// at `(anchor.x, anchor.y + anchor.h)`, flipping above the anchor
    /// when there is no room below. `on_dismiss` fires whenever a
    /// click lands outside both the popover content and the anchor
    /// rect.
    Popover {
        anchor:     NodeId,
        #[serde(deserialize_with = "nested")] 
        child:      alloc::boxed::Box<Widget>,
        on_dismiss: ActionId,
        modifiers:  Vec<Modifier>,
    },
    #[allow(dead_code)]
    Tooltip {
        text:      String,
        anchor:    NodeId,
        modifiers: Vec<Modifier>,
    },
    #[allow(dead_code)]
    Menu {
        #[serde(deserialize_with = "nested")] 
        items:     Vec<Widget>,
        modifiers: Vec<Modifier>,
    },
    /// Multi-line text editor. The compositor owns a 2-D caret (arrows
    /// move within/across lines, Enter inserts a newline, Home/End are
    /// line-relative, PageUp/PageDown scroll a viewport). `value` is the
    /// whole `\n`-separated document; buffer mutations emit
    /// `Event::InputChange { value }` like `Input`. No `on_submit` —
    /// Enter is a newline. See the SDK mirror for the full contract.
    TextArea {
        value:       String,
        placeholder: String,
        /// Syntax-highlight colour runs over `value` (byte offsets).
        spans:       Vec<Span>,
        modifiers:   Vec<Modifier>,
    },
    /// Horizontal value track with a draggable thumb. `value` runs from
    /// 0 to [`SLIDER_MAX`]. The compositor owns the drag: the thumb follows
    /// the pointer without a round trip, and the app hears
    /// `Event::Slide { action: on_change, .. }` — `done: false` while the
    /// value moves, once more with `done: true` on release. While a drag
    /// is in progress the compositor keeps its own value on screen, so an
    /// app that re-commits mid-drag (a player advancing its position)
    /// does not yank the thumb back.
    Slider {
        value:     u16,
        on_change: ActionId,
        modifiers: Vec<Modifier>,
    },
    // Appended only.
}

// ── Events (compositor → app) ─────────────────────────────────────────

/// Input event delivered to a WASM app via `npk_event_poll` /
/// `npk_event_wait`.
///
/// Note: `InputChange` carries an owned `String`, so this enum is
/// `Clone`-only — not `Copy`. All call sites pass `Event` by value
/// or clone explicitly; no fast-path lost.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Event {
    /// Keyboard input. Uses the existing KeyCode (already stable).
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
    /// The focused `Widget::Input` had its value mutated by the
    /// compositor's editor (printable key, Backspace, Delete). Carries
    /// the new buffer contents — the app typically mirrors `value` into
    /// its own state and re-commits the tree with the matching `value`.
    /// Cursor-only navigation (Left / Right / Home / End) does not
    /// fire this event; the caret is purely compositor-side state.
    InputChange { value: alloc::string::String },
    /// Right-click hit-test result. Same hit-test as `Action`, but fired
    /// for `MouseButton::Right` instead of `Left`. Apps use it to open
    /// context menus (Popover) without consuming the primary click.
    ContextAction(ActionId),
    /// "Open this resource" — delivered when `npk_open` targets an app
    /// already running (instead of spawning a duplicate). Payload is the
    /// launch argument (e.g. a file path). Enables singleton-with-tabs.
    Open(alloc::string::String),
    /// Mouse wheel over a focused app with no `Widget::Scroll` to consume it.
    /// `dy` is pixel-scaled (positive = scroll down). Canvas/surface apps
    /// scroll their own viewport; others ignore it.
    Wheel { dy: i32 },
    /// A clipboard chord (Ctrl+C / Ctrl+X / Ctrl+V) that no focused text
    /// widget consumed, delivered to the focused app so it can act on its
    /// own selection (e.g. loft copies/moves the selected file). Apps
    /// built against an older SDK fail to decode this appended variant and
    /// skip it — harmless. Must stay in lockstep with the SDK copy in
    /// `tools/wasm/sdk/widgets/src/abi.rs` (postcard variant order).
    Clipboard(ClipKind),
    /// A file-picker request this app started via `npk_pick` finished.
    /// `path` is the npkFS path the user chose, or empty if they
    /// cancelled. `tag` is the caller's own value from `npk_pick`,
    /// returned unchanged — the picker roundtrip is asynchronous, so an
    /// app running several dialogs (open / save-as / …) uses it to tell
    /// which one came back. The kernel never interprets it.
    Picked { path: alloc::string::String, tag: u32 },
    /// The user asked to close this window (Mod+Q, the title-bar X) and
    /// the app opted into being asked via `npk_window_set_close_guard`.
    /// The window is still open: save, prompt, then call
    /// `npk_close_widget` to go — or ignore it to stay.
    ///
    /// Not a promise of veto power. A second close gesture, or a few
    /// seconds of silence, closes the window anyway: an app must never be
    /// able to make its window unclosable.
    CloseRequest,
    /// A Ctrl chord the text editor doesn't own — Ctrl+S, Ctrl+O, … The
    /// editor keeps Ctrl+A/C/X/V for text; everything else reaches the app
    /// here. Ctrl is implied; `shift`/`alt` say what else was held, so
    /// Ctrl+Shift+S is distinguishable from Ctrl+S.
    ///
    /// `letter` is the lowercase ASCII letter, already normalized from the
    /// control byte some keyboard paths produce (0x13 → 's').
    ///
    /// Exists because `Event::Key` carries no modifiers: an app could not
    /// otherwise tell Ctrl+S from a typed "s", and while a text widget is
    /// focused it would never see the keystroke at all.
    Chord { letter: u8, shift: bool, alt: bool },
    /// Ctrl+wheel over the focused app — a zoom request. `delta` is
    /// positive for "bigger" (wheel up), negative for smaller; its
    /// magnitude is notches, not pixels, so the app picks the step.
    ///
    /// Separate from `Wheel` because that one carries no modifiers, and
    /// because Ctrl+wheel must not scroll: the compositor skips its own
    /// scroll handling and sends this instead. An app that ignores it
    /// simply doesn't zoom.
    Zoom { delta: i32 },
    /// Horizontal scrolling over the focused app that no `Widget::Scroll`
    /// with a horizontal axis consumed. `dx` is pixel-scaled, positive =
    /// to the right.
    ///
    /// A variant of its own rather than a field on `Wheel`: an added field
    /// would change an existing variant's ABI and break every app built
    /// against the old SDK. An appended variant only fails to decode for
    /// them and is skipped. Must stay in lockstep with the SDK copy in
    /// `tools/wasm/sdk/widgets/src/abi.rs`.
    WheelX { dx: i32 },
    /// A `Widget::Slider` moved. `action` is its `on_change`, `value` in
    /// 0..=[`SLIDER_MAX`]. `done: false` while the pointer drags (only
    /// when the value changed), `done: true` once on release — apps that
    /// do expensive work (seeking) wait for that one.
    Slide { action: ActionId, value: u16, done: bool },
    // Appended only.
}

/// Which clipboard chord fired. See `Event::Clipboard`.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ClipKind {
    Copy = 0,
    Cut  = 1,
    Paste = 2,
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
    /// Slot 0 = Token::Surface, slot 16 = Token::AccentLine, …
    pub colors: [u32; PALETTE_SLOTS],
}

/// Slots in a `Palette`. Must stay > the highest `Token` discriminant —
/// the rasterizer indexes with `token as usize` and does not bounds-check.
pub const PALETTE_SLOTS: usize = 32;

/// A planar 4:2:0 frame, borrowed from the canvas store. Every field was
/// validated by `canvas::commit_i420`, which is what lets the blit index
/// the planes without re-deriving a single bound.
pub struct I420Ref<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    /// Row strides in bytes — a decoder pads rows, so these are not `w`.
    pub ys: usize,
    pub cs: usize,
    pub coeffs: super::canvas::YuvCoeffs,
}

/// A raster destination. Either a tile in the GGTT slab, or a composition
/// layer — from the rasterizer's perspective they are identical: a BGRA32
/// pixel buffer with an origin offset in window coordinates.
///
/// The rasterizer receives `Rect`s and `Point`s in window coordinates
/// and subtracts `origin` internally to get the target-local position.
/// Draws are clipped to `size`, so drawing across tile boundaries works:
/// the left tile clips the right half away, and vice versa.
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
    /// Alpha (0..=255) applied to Background/Border fills. 255 = opaque
    /// (normal scenes, unchanged). A panel scene cleared transparent sets
    /// this to the chrome opacity so its pill backgrounds are translucent
    /// while glyphs stay full-coverage — the compositor then composites the
    /// scene over the wallpaper by per-pixel alpha (no halo).
    pub bg_alpha: u8,
    /// Owning widget window id — lets the render walker look up a
    /// `Widget::Canvas`'s committed bitmap in the canvas store.
    pub window_id: u32,
    /// Optional clip rectangle in target-local coords `(x0,y0,x1,y1)`.
    /// `None` = clip only to the target size (default). A `Widget::Scroll`
    /// sets this to its viewport rect for the duration of its subtree so
    /// overflowing content is masked instead of bleeding past the panel.
    pub clip: Option<(i32, i32, i32, i32)>,
}

/// Rasterizer backend. CPU in v1 (fontdue + gui/render.rs); a GPU backend
/// (SDF text atlas, fragment shaders for blur / shadow / effect) would
/// implement the same trait.
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

    /// Draw text at baseline point `p` (window coordinates) in `color`.
    fn text(&mut self, t: &mut RasterTarget, s: &str, style: TextStyle, color: Token, p: Point);

    /// `text` with the style's pixel size overridden (`Modifier::FontSize`).
    /// Default ignores the override so a backend can opt in.
    fn text_px(&mut self, t: &mut RasterTarget, s: &str, style: TextStyle, _size_px: u16,
               color: Token, p: Point) {
        self.text(t, s, style, color, p);
    }

    /// Draw an icon from the built-in atlas.
    fn icon(&mut self, t: &mut RasterTarget, id: IconId, size: u16, color: Token, p: Point);

    /// Copy app-supplied Canvas pixels (BGRA32) into the target.
    fn canvas_copy(&mut self, t: &mut RasterTarget, src: &[u8], w: u16, h: u16);

    /// Blit app-supplied BGRA32 pixels (`sw`×`sh`) contain-fit and centred
    /// into `rect` (window coordinates), with no background fill outside
    /// the fitted image. `zoom_q88` scales that fitted size (256 = 1.0× =
    /// plain fit, larger crops to the rect); it comes from `Modifier::Scale`
    /// on the Canvas. `pan` shifts the content from centred
    /// (`Modifier::CanvasOffset`) and is clamped to the overhang, so the
    /// rect never shows a gap. Default no-op.
    fn canvas_blit(&mut self, _t: &mut RasterTarget, _src: &[u8], _sw: u32, _sh: u32,
                   _rect: Rect, _zoom_q88: u32, _pan: (i32, i32)) {}

    /// Blit app-supplied BGRA32 pixels 1:1 at the top-left of `rect`
    /// (`Modifier::CanvasNative`). Where the bitmap is smaller than the
    /// rect, its last column and row are extended to the rect's edge, so a
    /// frame the app has not yet re-committed after a resize still fills
    /// the rect. Defaults to the contain-fit blit.
    fn canvas_blit_native(&mut self, t: &mut RasterTarget, src: &[u8], sw: u32, sh: u32,
                          rect: Rect) {
        self.canvas_blit(t, src, sw, sh, rect, 256, (0, 0));
    }

    /// The same blit for a planar 4:2:0 frame, converting Y′CbCr to BGRA
    /// on the way. Separate from `canvas_blit` rather than a format flag
    /// on it because the two walk different memory: one plane of 4-byte
    /// pixels against three planes at two resolutions. Default no-op.
    fn canvas_blit_i420(&mut self, _t: &mut RasterTarget, _p: &I420Ref, _sw: u32, _sh: u32,
                        _rect: Rect, _zoom_q88: u32, _pan: (i32, i32)) {}

    // ── Reserved (v2+, default no-op on CPU backend) ──────────────────

    /// Gaussian blur behind the given rect (acrylic/glass effect).
    fn blur(&mut self, _t: &mut RasterTarget, _r: Rect, _radius: u8) {}

    /// Drop shadow under the given rect.
    fn shadow(&mut self, _t: &mut RasterTarget, _r: Rect, _s: Shadow) {}

    /// Named GPU effect.
    fn effect(&mut self, _t: &mut RasterTarget, _r: Rect, _id: EffectId) {}
}
