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

    // Reserved (v2+) — compositor logs + rejects in v1.
    Popover {
        anchor:    NodeId,
        child:     Box<Widget>,
        modifiers: Vec<Modifier>,
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

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
