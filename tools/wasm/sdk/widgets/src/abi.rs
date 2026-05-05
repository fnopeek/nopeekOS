//! Widget ABI — wire contract mirror of `kernel/src/shade/widgets/abi.rs`.
//!
//! **Every change here must be mirrored to the kernel side, and vice
//! versa.** Variant order, struct-variant field order, and `#[repr]`
//! discriminants are all part of the wire format. Postcard serializes by
//! declaration position, so drift between the two copies would produce
//! silent deserialization corruption.
//!
//! The `check_abi` module at the crate root enforces ordering invariants
//! at compile time (same mechanism as the kernel's check_abi.rs).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

// ── Geometry ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

// ── Identifiers ───────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CanvasId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u32);

// ── Theme tokens ──────────────────────────────────────────────────────

#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Token {
    Surface         = 0,
    SurfaceElevated = 1,
    SurfaceMuted    = 2,
    OnSurface       = 3,
    OnSurfaceMuted  = 4,
    OnAccent        = 5,
    Accent          = 6,
    AccentMuted     = 7,
    Border          = 8,
    Success         = 9,
    Warning         = 10,
    Danger          = 11,
    // Appended only.
}

// ── Icons ─────────────────────────────────────────────────────────────

#[repr(u16)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconId {
    None              = 0,
    Folder            = 1,
    File              = 2,
    ArrowLeft         = 3,
    ArrowRight        = 4,
    ArrowUp           = 5,
    ArrowDown         = 6,
    Home              = 7,
    Download          = 8,
    // P10.9 Phosphor Regular set
    MagnifyingGlass   = 9,
    X                 = 10,
    Check             = 11,
    Gear              = 12,
    Power             = 13,
    Lock              = 14,
    Terminal          = 15,
    Trash             = 16,
    DotsThreeVertical = 17,
    List              = 18,
    // P10.11 file-browser additions (loft)
    Monitor           = 19,
    FileText          = 20,
    FolderOpen        = 21,
    Image             = 22,
    HardDrives        = 23,
    Code              = 24,
    Folders           = 25,
    CaretRight        = 26,
    ArrowClockwise    = 27,
    // Appended only.
}

// ── Accessibility roles ───────────────────────────────────────────────

#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
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

#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextStyle {
    Body    = 0,
    Title   = 1,
    Caption = 2,
    Muted   = 3,
    Mono    = 4,
    /// 18 px regular weight — between `Body` (14) and `Title` (24, bold).
    /// Used for non-bold display text such as input placeholders /
    /// values where Body reads too small but Title's 600-weight bold
    /// is too heavy. (Appended for vocab-v3.)
    Heading = 5,
    // Appended only.
}

// ── Fill (rasterizer-side only) ───────────────────────────────────────

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fill {
    Solid(Token),
    // Appended only.
}

// ── Effect IDs (reserved) ─────────────────────────────────────────────

#[repr(u16)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectId {
    None = 0,
    // Appended only.
}

// ── Layout primitives ─────────────────────────────────────────────────

#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    Start   = 0,
    Center  = 1,
    End     = 2,
    Stretch = 3,
    // Appended only.
}

#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    Vertical   = 0,
    Horizontal = 1,
    Both       = 2,
    // Appended only.
}

// ── Container-query density ───────────────────────────────────────────

/// Compositor-classified window size bucket. Apps reference these via
/// `Modifier::WhenDensity(Density, ...)` to adapt layout to the available
/// space without picking pixel breakpoints. The compositor owns the
/// thresholds (Compact <600 px, Regular 600–1200 px, Spacious >1200 px)
/// so apps never see raw pixel widths.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Density {
    Compact  = 0,
    Regular  = 1,
    Spacious = 2,
    // Appended only.
}

// ── Animation ─────────────────────────────────────────────────────────

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transition {
    Spring,
    Linear { ms: u16 },
    // Appended only.
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shadow {
    pub offset: Point,
    pub blur:   u8,
    pub token:  Token,
}

// ── Modifier ──────────────────────────────────────────────────────────

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Modifier {
    // Active in v1
    Padding(u16),
    Margin(u16),
    Background(Token),
    Border {
        token:  Token,
        width:  u8,
        radius: u8,
    },
    Opacity(u8),
    Transition(Transition),
    OnClick(ActionId),
    OnHover(ActionId),
    // Reserved (v2+) — CPU rasterizer treats as no-op.
    Blur(u8),
    Shadow(Shadow),
    Effect(EffectId),
    RoleOverride(Role),
    Tint(Token),
    // ── Vocab v2 (Tailwind-style modifiers) ───────────────────────────
    // Pseudo-state modifier lists. Compositor merges the inner list onto
    // the widget when the state matches; tree stays static across hovers.
    Hover(Vec<Modifier>),
    Focus(Vec<Modifier>),
    Active(Vec<Modifier>),
    Disabled(Vec<Modifier>),
    /// Container query — apply inner modifiers only at the given density.
    WhenDensity(Density, Vec<Modifier>),
    /// Uniform scale, Q8.8 fixed-point. 256 = 1.0× (identity).
    Scale(u16),
    /// Layout minimum width (px at 1× scale). Compositor honors as a hard
    /// floor; if the parent allots less, the widget overflows visibly
    /// rather than collapsing.
    MinWidth(u16),
    /// Layout maximum width (px at 1× scale).
    MaxWidth(u16),
    /// Corner radius (px at 1× scale) without a Border. Use this when
    /// rounding is needed without a stroked outline.
    Rounded(u8),
    /// CSS-style flex-grow on the main axis of the parent Row/Column.
    /// The widget keeps its intrinsic main size as a basis and absorbs
    /// a proportional share of the leftover space alongside any
    /// `Spacer { flex }` siblings (Spacer = Flex with intrinsic 0 in
    /// this scheme). Use case: a body Row that should fill the
    /// remaining vertical space below the toolbar so its sidebar bg
    /// reaches the footer divider, even when the grid content is
    /// short. `Flex(0)` is identical to no Flex at all (intrinsic only).
    Flex(u8),
    /// Tag a widget with an app-chosen `NodeId`. The compositor's
    /// layout pass records the laid-out rect of every NodeId-tagged
    /// widget into a side table; `Widget::Popover { anchor }` then
    /// looks the rect up to position itself relative to the anchor.
    /// IDs are app-private — the compositor only echoes them back
    /// internally for anchor lookups, never to other apps. Multiple
    /// widgets with the same id is undefined behavior (last wins).
    NodeId(NodeId),
    // Appended only.
}

// ── Widget ────────────────────────────────────────────────────────────

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Widget {
    // Containers
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
        child:     Box<Widget>,
        axis:      Axis,
        modifiers: Vec<Modifier>,
    },

    // Leaves
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
        label:     String,
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
        flex: u8,
    },
    Divider,
    Canvas {
        id:        CanvasId,
        width:     u16,
        height:    u16,
        modifiers: Vec<Modifier>,
    },

    /// Floating overlay anchored to a `Modifier::NodeId`-tagged
    /// widget elsewhere in the tree. Renders on top of everything
    /// (z-order) at `(anchor.x, anchor.y + anchor.h)` — flips above
    /// the anchor when there is no room below. Apps emit a Popover
    /// only while the overlay should be visible; toggle by adding /
    /// removing it from the tree. `on_dismiss` fires whenever the
    /// user clicks outside both the popover content AND the anchor
    /// rect — apps route this to their "close" state transition.
    Popover {
        anchor:     NodeId,
        child:      Box<Widget>,
        on_dismiss: ActionId,
        modifiers:  Vec<Modifier>,
    },
    Tooltip {
        text:      String,
        anchor:    NodeId,
        modifiers: Vec<Modifier>,
    },
    Menu {
        items:     Vec<Widget>,
        modifiers: Vec<Modifier>,
    },
    // Appended only.
}

// ── Events / Actions ──────────────────────────────────────────────────

/// Mirror of `kernel::input::KeyCode`. Field shape frozen as part of the
/// Phase 8 ABI — kernel-side and SDK-side must stay in sync.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyCode {
    Char(u8),
    Enter,
    Backspace,
    Tab,
    Escape,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
    // Appended only.
}

/// Note: `InputChange` carries an owned `String`, so this enum is
/// `Clone`-only — not `Copy`. Apps match `Event` by value (move) or
/// clone explicitly when keeping it across iterations.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    Key(KeyCode),
    Action(ActionId),
    MouseMove { x: i32, y: i32 },
    MouseButton {
        button: MouseButton,
        down:   bool,
        x:      i32,
        y:      i32,
    },
    Focus(bool),
    /// The focused `Widget::Input`'s value was mutated by the
    /// compositor (printable key, Backspace, Delete). `value` is the
    /// new buffer contents — apps typically mirror it into their state
    /// and re-commit the tree with `Widget::Input { value, ... }`
    /// matching. Cursor-only navigation (Left/Right/Home/End) does
    /// not fire this event.
    InputChange { value: String },
    // Appended only.
}

#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left   = 0,
    Right  = 1,
    Middle = 2,
    // Appended only.
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Idle,
    Rerender,
    Exit,
    // Appended only.
}

// ── Palette (for app-side token → color query via npk_theme_token) ────

/// Received by the app if it queries the active palette. The concrete
/// RGBA values are compositor-resolved; the app never picks hex colors.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Palette {
    pub colors: [u32; 16],
}
