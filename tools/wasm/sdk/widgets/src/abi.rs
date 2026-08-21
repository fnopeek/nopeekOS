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

/// A coloured run inside a `Widget::TextArea`'s `value`. `start`/`len`
/// are byte offsets into the (UTF-8) buffer; `token` is the colour. The
/// app recomputes spans on every edit (syntax highlighting); the
/// compositor renders the live buffer and colours each byte by the span
/// covering it (uncovered bytes use the default text colour). Spans
/// should be sorted by `start` and non-overlapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub len:   u32,
    pub token: Token,
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
    /// Content canvas — below `Surface`. Editor body, page, terminal.
    Page            = 12,
    /// Hover fill / chips — above `SurfaceMuted`.
    SurfaceHover    = 13,
    /// Third text level: section headings, meta columns, disabled.
    OnSurfaceFaint  = 14,
    /// Accent at 22 % over `Surface` — focus rings.
    AccentRing      = 15,
    /// Accent at 45 % over `Surface` — focused window border.

    // ── Code tokens (syntax highlighting) ─────────────────────────────
    //
    // A second, independent ramp. The tokens above describe *chrome*;
    // these describe *source text*. An editor needs both at once, and
    // reusing `Accent`/`Warning` for keywords and strings tied the
    // syntax colours to the wallpaper — a whole language got three
    // colours. Resolved from the active code scheme (`set code.scheme`),
    // never from the accent.
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
    AccentLine      = 16,
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
    Globe             = 28,
    Camera            = 29,
    // Battery states (bar plugin) — Phosphor horizontal set.
    BatteryEmpty      = 30,
    BatteryLow        = 31,
    BatteryMedium     = 32,
    BatteryHigh       = 33,
    BatteryFull       = 34,
    BatteryCharging   = 35,
    BatteryWarning    = 36,
    Plug              = 37,
    SpeakerHigh       = 38,
    SpeakerLow        = 39,
    SpeakerX          = 40,
    Minus             = 41,
    Plus              = 42,
    Bird              = 43,
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
    /// Focus ring — a stroke of `width` px drawn just OUTSIDE the node's
    /// rect, under any Border. Mirrors CSS `box-shadow: 0 0 0 Npx`. Costs
    /// no layout space; leave room yourself (a row gap suffices at ≤ 3).
    Ring { token: Token, width: u8 },
    /// Layout minimum height (px at 1× scale).
    MinHeight(u16),
    /// Layout maximum height (px at 1× scale).
    MaxHeight(u16),
    /// Draw a line-number gutter down the left edge of a `TextArea`.
    LineNumbers(bool),
    /// Per-axis inner padding (px at 1× scale). Sums with `Padding`.
    /// Appended AFTER LineNumbers — inserting ahead of a shipped variant
    /// renumbers it on the wire and breaks the running app.
    PaddingXY { x: u16, y: u16 },
    /// This text widget takes focus when its window first appears.
    /// Without it a window opens with nothing focused.
    Autofocus,
    /// Font size override in px for a `Widget::Text` — replaces the size
    /// its `TextStyle` resolves to; the face (proportional vs mono) still
    /// comes from the style. Layout measures at the override, so the text
    /// keeps a correct box. Clamped to 6..=64 by the compositor; ignored
    /// on every other widget.
    FontSize(u16),
    /// Pan a `Widget::Canvas`'s content by this many px, relative to the
    /// centred contain-fit position. Pairs with `Modifier::Scale`: scale
    /// decides how big the image is drawn, this decides which part of it
    /// the rect shows. The compositor clamps it to the overhang, so it can
    /// never push the content out of view. Ignored on every other widget.
    CanvasOffset { x: i32, y: i32 },
    // Appended only.
}

/// Accepted range for `Modifier::FontSize`; the compositor clamps to it.
/// Mirror of the kernel copy.
pub const FONT_SIZE_MIN: u16 = 6;
pub const FONT_SIZE_MAX: u16 = 64;

/// Default size of `TextStyle::Mono` — the editor's starting point.
pub const MONO_SIZE_PX: u16 = 13;

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
    /// Multi-line text editor. Unlike `Input` (single line, on_submit on
    /// Enter), the compositor owns a 2-D caret: arrows move within / across
    /// lines, Enter inserts a newline, Home/End are line-relative,
    /// PageUp/PageDown scroll by a viewport. `value` is the whole document
    /// (`\n`-separated). Buffer mutations emit `Event::InputChange { value }`
    /// (the entire document) exactly like `Input`; only one widget is ever
    /// focused, so there is no ambiguity. There is intentionally no
    /// `on_submit` — Enter is a newline, not a submit. Rendered with
    /// `TextStyle::Mono`; the visible window scrolls to keep the caret in
    /// view. Apps typically wrap it in `Modifier::Flex(1)` so it fills the
    /// space between toolbar and footer.
    TextArea {
        value:       String,
        placeholder: String,
        /// Syntax-highlight colour runs over `value` (byte offsets). The
        /// app recomputes these on every edit; empty = plain text.
        spans:       Vec<Span>,
        modifiers:   Vec<Modifier>,
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
    /// Right-click hit-test result. Same hit-test as `Action`, but
    /// fired for `MouseButton::Right`. Apps use it to open context
    /// menus (Popover) without consuming the primary click.
    ContextAction(ActionId),
    /// "Open this resource" — delivered when `npk_open` targets an app
    /// that is ALREADY running (instead of spawning a duplicate). The
    /// payload is the launch argument (e.g. a file path). Lets an app
    /// be a singleton with tabs: a second open routes here as a new tab.
    Open(String),
    /// Mouse wheel over a focused app that has no `Widget::Scroll` to consume
    /// it. `dy` is already scaled to pixels (positive = scroll down). Apps
    /// that render their own surface (e.g. the browser's Canvas) scroll their
    /// own viewport in response; apps that ignore it are unaffected.
    Wheel { dy: i32 },
    /// A clipboard chord (Ctrl+C / Ctrl+X / Ctrl+V) reached the focused
    /// app because no focused text widget consumed it. Apps that manage
    /// their own selection (e.g. loft's file grid) act on it — copy/cut
    /// the selection, or paste into the current context. Apps that don't
    /// care ignore it; the variant is append-only, so an app built
    /// against an older SDK simply fails to decode it and skips (its
    /// `postcard::from_bytes` returns `Err`, treated as "no event").
    Clipboard(ClipKind),
    /// A file-picker request this app started via `npk_pick` finished.
    /// `path` is the npkFS path the user chose, or **empty if they
    /// cancelled**. `tag` is the caller's own value from `npk_pick`,
    /// returned unchanged — the picker roundtrip is asynchronous, so an
    /// app running several dialogs (open / save-as / …) uses it to tell
    /// which one came back. The kernel never interprets it.
    Picked { path: String, tag: u32 },
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
    /// here. **Ctrl is implied**; `shift`/`alt` say what else was held, so
    /// Ctrl+Shift+S is distinguishable from Ctrl+S.
    ///
    /// `letter` is the lowercase ASCII letter, already normalized from the
    /// control byte some keyboard paths produce (0x13 → 's').
    ///
    /// Exists because `Event::Key` carries no modifiers: an app could not
    /// otherwise tell Ctrl+S from a typed "s" — and while a text widget is
    /// focused it never saw the keystroke at all.
    Chord { letter: u8, shift: bool, alt: bool },
    /// Ctrl+wheel over the focused app — a zoom request. `delta` is
    /// positive for "bigger" (wheel up), negative for smaller; its
    /// magnitude is notches, not pixels, so the app picks the step.
    ///
    /// Separate from `Wheel` because that one carries no modifiers, and
    /// because Ctrl+wheel must NOT scroll: the compositor skips its own
    /// scroll handling and sends this instead. An app that ignores it
    /// simply doesn't zoom.
    Zoom { delta: i32 },
    // Appended only.
}

/// Which clipboard chord fired. See `Event::Clipboard`.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipKind {
    Copy = 0,
    Cut  = 1,
    Paste = 2,
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
    pub colors: [u32; PALETTE_SLOTS],
}

/// Slots in a `Palette` — must stay > the highest `Token` discriminant.
pub const PALETTE_SLOTS: usize = 32;
