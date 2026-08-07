//! Prefab components — app-facing "how to build a nopeek UI" cookbook.
//! Apps assemble screens from these, never from raw Row/Column/Modifier.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::abi::{
    ActionId, Align, Axis, IconId, Modifier, NodeId, TextStyle, Token, Widget,
};
use crate::style::{Elevation, Padding, Radius, Spacing};

// Small uniform inset (4 px) so children — including dividers — get
// breathing room from the window chrome instead of butting against
// the rounded border. The vertical inset combines with the leading /
// trailing zero-size widgets that `prefab::input` and `prefab::footer`
// install at their wrap-Column ends to keep the search row + footer
// row vertically symmetric between chrome and divider.
/// Centre a single child inside a fixed-size box.
///
/// `MinWidth` widens the box but leaves the child at the leading edge —
/// `Align` on a Row is the CROSS axis (vertical), not the main one. The
/// flex spacers are what actually centre it. Every fixed-size cell in the
/// design (workspace pill, tray icon, dock tile, toolbar button) needs
/// this; without it the glyph sits left and the fill runs off to the right.
pub fn center_box(child: Widget, modifiers: Vec<Modifier>) -> Widget {
    Widget::Row {
        children: vec![
            Widget::Spacer { flex: 1 },
            child,
            Widget::Spacer { flex: 1 },
        ],
        spacing: 0,
        align:   Align::Center,
        modifiers,
    }
}

/// Solid rectangle of an exact size — the design's fixed-size marks:
/// the dock's running dashes, a list row's 2 px selection edge, a
/// vertical separator inside a toolbar. `token: None` renders nothing
/// but still occupies the space, so a group of marks stays aligned
/// whether or not each one is shown.
pub fn mark(w: u16, h: u16, token: Option<Token>) -> Widget {
    let mut modifiers = vec![
        Modifier::MinWidth(w),
        Modifier::MaxWidth(w),
        Modifier::MinHeight(h),
        Modifier::MaxHeight(h),
    ];
    if let Some(t) = token {
        modifiers.push(Modifier::Background(t));
        if h > 2 || w > 2 {
            modifiers.push(Modifier::Rounded(1));
        }
    }
    Widget::Column {
        children: Vec::new(),
        spacing: 0,
        align: Align::Stretch,
        modifiers,
    }
}

pub fn panel(children: Vec<Widget>) -> Widget {
    Widget::Column {
        children,
        spacing: Spacing::Md.as_u16(),
        align:   Align::Stretch,
        modifiers: vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

// Selected rows are styled as a subtle elevated card with an accent
// border — the colour cue lives in the border + the icon tint, not in
// a strong fill that would clash with body text on top. Matches the
// "card-style highlight" that AI-generated UIs and modern launchers
// (Raycast, macOS Spotlight) reach for. Padding is Lg so the icon /
// title / subtitle / arrow have visible breathing room inside the
// border on a selected row instead of hugging the stroke.
pub fn list_row(
    icon: IconId,
    title: &str,
    subtitle: &str,
    selected: bool,
    on_click: Option<ActionId>,
    on_hover: Option<ActionId>,
) -> Widget {
    let mut row_mods: Vec<Modifier> = Vec::with_capacity(6);
    row_mods.push(Modifier::Padding(Padding::Lg.as_u16()));
    if let Some(id) = on_click { row_mods.push(Modifier::OnClick(id)); }
    if let Some(id) = on_hover { row_mods.push(Modifier::OnHover(id)); }
    if selected {
        row_mods.push(Modifier::Background(Token::SurfaceElevated));
        row_mods.push(Modifier::Border {
            token:  Token::Accent,
            width:  1,
            radius: Radius::Sm.as_u8(),
        });
    } else {
        // Non-selected rows get a subtle hover highlight + a focus
        // outline (Tab-nav). Selected rows skip both so hovering /
        // focusing an already-selected row doesn't compete with its
        // accent fill — keeps visual hierarchy stable.
        row_mods.push(Modifier::Hover(vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Rounded(Radius::Sm.as_u8()),
        ]));
        row_mods.push(Modifier::Focus(vec![
            Modifier::Border { token: Token::OnSurfaceMuted, width: 1, radius: Radius::Sm.as_u8() },
        ]));
    }

    // Always render subtitle (even when empty) so every row gets the
    // same Body+Muted line-height — keeps the hover bar, dividers and
    // the footer on a stable grid regardless of which entries have
    // descriptions. Empty string → fontdue emits zero glyphs but the
    // layout still reserves the Muted line-height slot.
    let subtitle_text = if subtitle.is_empty() { " ".to_string() } else { subtitle.to_string() };
    let text_col: Vec<Widget> = vec![
        Widget::Text {
            content: title.to_string(),
            style:   TextStyle::Body,
            modifiers: vec![],
        },
        Widget::Text {
            content: subtitle_text,
            style:   TextStyle::Muted,
            modifiers: vec![],
        },
    ];

    let icon_mods = if selected {
        vec![Modifier::Tint(Token::Accent)]
    } else {
        vec![]
    };
    let mut children: Vec<Widget> = Vec::with_capacity(4);
    children.push(Widget::Icon { id: icon, size: 24, modifiers: icon_mods });
    children.push(Widget::Column {
        children:  text_col,
        spacing:   Spacing::Xs.as_u16(),
        align:     Align::Start,
        modifiers: vec![],
    });
    children.push(Widget::Spacer { flex: 1 });
    if selected {
        children.push(Widget::Icon {
            id:        IconId::ArrowRight,
            size:      14,
            modifiers: vec![],
        });
    }

    Widget::Row {
        children,
        spacing: Spacing::Md.as_u16(),
        align:   Align::Center,
        modifiers: row_mods,
    }
}

pub fn badge(text: &str) -> Widget {
    Widget::Text {
        content: text.to_string(),
        style:   TextStyle::Caption,
        modifiers: vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::Background(Token::SurfaceMuted),
        ],
    }
}

pub fn footer(left: &str, right: &str) -> Widget {
    let row = Widget::Row {
        children: vec![
            Widget::Text {
                content: left.to_string(),
                style:   TextStyle::Muted,
                modifiers: vec![],
            },
            Widget::Spacer { flex: 1 },
            Widget::Text {
                content: right.to_string(),
                style:   TextStyle::Muted,
                modifiers: vec![],
            },
        ],
        spacing: 0,
        align:   Align::Center,
        modifiers: vec![Modifier::Padding(Padding::Md.as_u16())],
    };
    // Wrap with a trailing zero-size widget so the wrap-Column's
    // internal `Sm` spacing acts as BOTTOM-margin on the last row.
    // `row.Padding(Md=12) + this.spacing(Sm=8) + panel.Padding(Xs=4)
    // = 24 px` matches the symmetric 24 px above the footer text
    // (panel.spacing Md 12 + row top padding Md 12), keeping the
    // footer text centred between divider and chrome bottom.
    Widget::Column {
        children: vec![
            row,
            Widget::Icon {
                id:        IconId::None,
                size:      0,
                modifiers: vec![],
            },
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Stretch,
        modifiers: vec![],
    }
}

pub fn scroll_list(items: Vec<Widget>) -> Widget {
    Widget::Scroll {
        child: Box::new(Widget::Column {
            children:  items,
            spacing:   Spacing::Xxs.as_u16(),
            align:     Align::Stretch,
            modifiers: vec![],
        }),
        axis:      Axis::Vertical,
        modifiers: vec![],
    }
}

pub fn empty_state(text: &str) -> Widget {
    Widget::Text {
        content: text.to_string(),
        style:   TextStyle::Muted,
        modifiers: vec![Modifier::Padding(Padding::Lg.as_u16())],
    }
}

pub fn text_badge(text: String) -> Widget {
    badge(&text)
}

// Convenience converters — many apps format numbers into helper strings.
pub fn title_bar(title: &str) -> Widget {
    Widget::Text {
        content: title.to_string(),
        style:   TextStyle::Title,
        modifiers: vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

pub fn muted(text: &str) -> Widget {
    Widget::Text {
        content: text.to_string(),
        style:   TextStyle::Muted,
        modifiers: vec![],
    }
}

pub fn body(text: &str) -> Widget {
    Widget::Text {
        content: text.to_string(),
        style:   TextStyle::Body,
        modifiers: vec![],
    }
}

// ── File-browser / multi-pane prefabs (P10.11 loft) ───────────────────

/// Square tap-target with a single centred icon. Used for toolbar chrome
/// (back/forward/up, refresh) and in-row actions.
pub fn icon_button(icon: IconId, size: u16, on_click: Option<ActionId>, on_hover: Option<ActionId>) -> Widget {
    let mut mods: Vec<Modifier> = Vec::with_capacity(6);
    // A fixed square cell, not a padded glyph: the hit target then stays
    // the same whatever glyph size the caller asks for, and a row of
    // these lines up (UI_REFRESH.md §3 `toolbar_button`).
    mods.push(Modifier::MinWidth(TOOLBAR_BTN));
    mods.push(Modifier::MinHeight(TOOLBAR_BTN));
    mods.push(Modifier::Rounded(TOOLBAR_BTN_RADIUS));
    if let Some(id) = on_click { mods.push(Modifier::OnClick(id)); }
    if let Some(id) = on_hover { mods.push(Modifier::OnHover(id)); }
    mods.push(Modifier::Hover(vec![
        Modifier::Background(Token::SurfaceHover),
        Modifier::Rounded(TOOLBAR_BTN_RADIUS),
    ]));
    mods.push(Modifier::Active(vec![
        Modifier::Background(Token::AccentMuted),
        Modifier::Tint(Token::Accent),
        Modifier::Rounded(TOOLBAR_BTN_RADIUS),
    ]));
    mods.push(Modifier::Focus(vec![
        Modifier::Ring { token: Token::AccentRing, width: 2 },
    ]));
    center_box(Widget::Icon { id: icon, size, modifiers: vec![] }, mods)
}

const TOOLBAR_BTN: u16 = 28;
const TOOLBAR_BTN_RADIUS: u8 = 7;

/// Small uppercase section label above a group of `nav_row`s. Mono and
/// `OnSurfaceFaint` so it reads as structure, not content
/// (UI_REFRESH.md §5).
pub fn sidebar_section(label: &str, items: Vec<Widget>) -> Widget {
    let mut children: Vec<Widget> = Vec::with_capacity(items.len() + 1);
    children.push(Widget::Text {
        content: label.to_string(),
        style:   TextStyle::Mono,
        modifiers: vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::Tint(Token::OnSurfaceFaint),
        ],
    });
    children.extend(items);
    Widget::Column {
        children,
        spacing:   0,
        align:     Align::Stretch,
        modifiers: vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

/// One entry inside a sidebar. Selection is an accent tint plus accent
/// text and icon — no border, no full-strength fill (UI_REFRESH.md §3
/// `list_row`).
pub fn nav_row(
    icon: IconId,
    label: &str,
    selected: bool,
    on_click: Option<ActionId>,
    on_hover: Option<ActionId>,
) -> Widget {
    let mut mods: Vec<Modifier> = vec![
        Modifier::Padding(Padding::Xs.as_u16()),
        Modifier::MinHeight(NAV_ROW_H),
        Modifier::Rounded(NAV_ROW_RADIUS),
    ];
    if let Some(id) = on_click { mods.push(Modifier::OnClick(id)); }
    if let Some(id) = on_hover { mods.push(Modifier::OnHover(id)); }
    if selected {
        mods.push(Modifier::Background(Token::AccentMuted));
        mods.push(Modifier::Tint(Token::Accent));
    } else {
        mods.push(Modifier::Hover(vec![
            Modifier::Background(Token::SurfaceHover),
            Modifier::Rounded(NAV_ROW_RADIUS),
        ]));
        mods.push(Modifier::Focus(vec![
            Modifier::Ring { token: Token::AccentRing, width: 2 },
        ]));
    }

    let icon_mods = vec![Modifier::Tint(
        if selected { Token::Accent } else { Token::OnSurfaceMuted },
    )];

    Widget::Row {
        children: vec![
            // 16, not 24: the row is 30 px tall and padding + a 24 px
            // glyph alone overshoots it. 16 is atlas-native, so it stays
            // crisp (24 -> 17 would be a scaled blur).
            Widget::Icon { id: icon, size: 16, modifiers: icon_mods },
            Widget::Text { content: label.to_string(), style: TextStyle::Body, modifiers: vec![] },
            Widget::Spacer { flex: 1 },
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: mods,
    }
}

const NAV_ROW_H: u16 = 30;
const NAV_ROW_RADIUS: u8 = 7;

/// Horizontal toolbar with built-in padding. Children align centred.
pub fn toolbar(children: Vec<Widget>) -> Widget {
    Widget::Row {
        children,
        spacing: Spacing::Sm.as_u16(),
        align:   Align::Center,
        modifiers: vec![
            Modifier::Padding(Padding::Sm.as_u16()),
        ],
    }
}

/// Horizontal row of path segments joined by caret separators.
/// `segments` is a slice of (label, ActionId) — caller supplies a distinct
/// ActionId per segment so clicking one jumps to that depth.
pub fn breadcrumb(segments: &[(String, ActionId)]) -> Widget {
    let mut children: Vec<Widget> = Vec::with_capacity(segments.len() * 2);
    for (i, (label, action)) in segments.iter().enumerate() {
        if i > 0 {
            children.push(Widget::Icon {
                id:        IconId::CaretRight,
                size:      16,
                modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted)],
            });
        }
        // The last segment is where you are: a filled chip. The ones
        // behind it are the way back, and stay quiet.
        let here = i + 1 == segments.len();
        let mut mods = vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::Rounded(Radius::Sm.as_u8()),
            Modifier::OnClick(*action),
        ];
        if here {
            mods.push(Modifier::Background(Token::SurfaceHover));
        } else {
            mods.push(Modifier::Tint(Token::OnSurfaceMuted));
            mods.push(Modifier::Hover(vec![
                Modifier::Background(Token::SurfaceHover),
                Modifier::Rounded(Radius::Sm.as_u8()),
            ]));
        }
        children.push(Widget::Text {
            content: label.clone(),
            style:   TextStyle::Mono,
            modifiers: mods,
        });
    }
    Widget::Row {
        children,
        spacing: Spacing::Xxs.as_u16(),
        align:   Align::Center,
        modifiers: vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
        ],
    }
}

/// One cell in a grid. Centred large icon above a single-line label.
/// Accent tint + filled background when selected.
pub fn grid_item(
    icon: IconId,
    label: &str,
    selected: bool,
    on_click: Option<ActionId>,
    on_hover: Option<ActionId>,
) -> Widget {
    let mut mods: Vec<Modifier> = Vec::with_capacity(6);
    mods.push(Modifier::Padding(Padding::Sm.as_u16()));
    if let Some(id) = on_click { mods.push(Modifier::OnClick(id)); }
    if let Some(id) = on_hover { mods.push(Modifier::OnHover(id)); }
    if selected {
        mods.push(Modifier::Background(Token::SurfaceElevated));
        mods.push(Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() });
    } else {
        mods.push(Modifier::Hover(vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Rounded(Radius::Md.as_u8()),
        ]));
        mods.push(Modifier::Focus(vec![
            Modifier::Border { token: Token::OnSurfaceMuted, width: 1, radius: Radius::Md.as_u8() },
        ]));
    }

    let icon_mods = if selected { vec![Modifier::Tint(Token::Accent)] } else { vec![] };

    Widget::Column {
        children: vec![
            Widget::Icon { id: icon, size: 64, modifiers: icon_mods },
            Widget::Text {
                content: label.to_string(),
                style:   TextStyle::Body,
                modifiers: vec![],
            },
        ],
        spacing:   Spacing::Xs.as_u16(),
        align:     Align::Center,
        modifiers: mods,
    }
}

/// Wrap a flat list of `grid_item` widgets into fixed-width rows.
/// `per_row` controls how many cells fit horizontally.
pub fn grid(items: Vec<Widget>, per_row: usize) -> Widget {
    if items.is_empty() || per_row == 0 {
        return Widget::Column {
            children:  items,
            spacing:   Spacing::Md.as_u16(),
            align:     Align::Stretch,
            modifiers: vec![Modifier::Padding(Padding::Md.as_u16())],
        };
    }
    let mut rows: Vec<Widget> = Vec::new();
    let mut cursor = 0;
    while cursor < items.len() {
        let end = (cursor + per_row).min(items.len());
        let mut row_children: Vec<Widget> = Vec::with_capacity(per_row);
        for it in &items[cursor..end] {
            row_children.push(it.clone());
            row_children.push(Widget::Spacer { flex: 1 });
        }
        // Pad incomplete trailing rows with flex spacers so cells keep
        // the same width as full rows.
        for _ in end..(cursor + per_row) {
            row_children.push(Widget::Spacer { flex: 2 });
        }
        rows.push(Widget::Row {
            children:  row_children,
            spacing:   Spacing::Sm.as_u16(),
            align:     Align::Start,
            modifiers: vec![],
        });
        cursor = end;
    }
    Widget::Column {
        children:  rows,
        spacing:   Spacing::Md.as_u16(),
        align:     Align::Stretch,
        modifiers: vec![Modifier::Padding(Padding::Md.as_u16())],
    }
}

// ── Vocab v2 archetypes — modern Tailwind-style prefabs ─────────────
//
// These are the primary building blocks for new apps and AI-generated
// UI. They use the v2 modifier set (Hover, Rounded, WhenDensity) so
// callers get hover-feedback, responsive padding, and consistent
// elevation by default. The earlier prefabs above (panel, list_row,
// nav_row, ...) remain for backward compat with drun + loft and have
// been polished with hover-state in place.

/// Visual weight tier for `card`. Maps semantically to design tokens
/// rather than concrete pixel values so a future theme can retune all
/// cards in one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    /// Flat surface with a border. Use for inline groupings.
    Inset,
    /// Elevated surface above the window background. Default.
    Panel,
    /// Strongly elevated, e.g. modal dialogs. Maps to Floating elevation.
    Sheet,
}

/// Container with consistent padding, rounded corners, and surface
/// background. The visual workhorse of the v2 vocabulary — every
/// non-trivial app screen should contain at least one card.
///
/// Ignores `Elevation` for now (no shadow rendering yet); the kind
/// still determines the surface token + border treatment so card
/// hierarchy is visible even without shadows.
pub fn card(content: Widget, kind: CardKind) -> Widget {
    let (bg_tok, border) = match kind {
        CardKind::Inset => (Token::Surface, Some((Token::Border, 1u8))),
        CardKind::Panel => (Token::SurfaceElevated, None),
        CardKind::Sheet => (Token::SurfaceElevated, None),
    };
    let _elevation = match kind {
        CardKind::Inset => Elevation::Flat,
        CardKind::Panel => Elevation::Subtle,
        CardKind::Sheet => Elevation::Floating,
    };
    let mut mods: Vec<Modifier> = Vec::with_capacity(4);
    mods.push(Modifier::Padding(Padding::Lg.as_u16()));
    mods.push(Modifier::Background(bg_tok));
    mods.push(Modifier::Rounded(Radius::Lg.as_u8()));
    if let Some((tok, w)) = border {
        mods.push(Modifier::Border { token: tok, width: w, radius: Radius::Lg.as_u8() });
    }
    Widget::Column {
        children: vec![content],
        spacing:  Spacing::Md.as_u16(),
        align:    Align::Stretch,
        modifiers: mods,
    }
}

/// Visual variant for `button`. Defines the colour pair only; the
/// rest of the chrome (rounded corners, padding, hover lift) is
/// shared so all button styles feel like the same family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Solid accent fill — primary action of the screen.
    Primary,
    /// Soft elevated surface — secondary action.
    Secondary,
    /// No background, just the label — tertiary action.
    Ghost,
    /// Danger-coloured fill — destructive action.
    Destructive,
}

/// Themed button. Wraps `Widget::Button` with a coherent default
/// chrome plus interactive states (hover, active, focus) so every
/// call site feels like the same button family.
pub fn button(label: &str, style: ButtonStyle, on_click: ActionId) -> Widget {
    let (bg, hover_bg, active_bg) = match style {
        ButtonStyle::Primary     => (Token::Accent,          Token::AccentMuted,   Token::AccentMuted),
        ButtonStyle::Secondary   => (Token::SurfaceElevated, Token::SurfaceMuted,  Token::Border),
        ButtonStyle::Ghost       => (Token::Surface,         Token::SurfaceMuted,  Token::Border),
        ButtonStyle::Destructive => (Token::Danger,          Token::Warning,       Token::Warning),
    };
    // Text colour has to follow the fill, or a filled button paints
    // body-coloured text on its own Accent and reads as disabled. Ghost
    // and Secondary sit on surface colours, so they keep the default.
    let label_tint = match style {
        ButtonStyle::Primary | ButtonStyle::Destructive => Some(Token::OnAccent),
        ButtonStyle::Secondary | ButtonStyle::Ghost     => None,
    };
    let mut mods: Vec<Modifier> = Vec::with_capacity(7);
    mods.push(Modifier::Padding(Padding::Md.as_u16()));
    mods.push(Modifier::Background(bg));
    if let Some(tok) = label_tint { mods.push(Modifier::Tint(tok)); }
    mods.push(Modifier::Rounded(Radius::Md.as_u8()));
    mods.push(Modifier::Hover(vec![
        Modifier::Background(hover_bg),
    ]));
    mods.push(Modifier::Active(vec![
        Modifier::Background(active_bg),
    ]));
    mods.push(Modifier::Focus(vec![
        Modifier::Border { token: Token::Accent, width: 2, radius: Radius::Md.as_u8() },
    ]));
    Widget::Button {
        label:     label.to_string(),
        icon:      IconId::None,
        on_click,
        modifiers: mods,
    }
}

/// Semantic kind for `input`. Search adds a leading magnifier icon;
/// Password is a placeholder for masked rendering once the rasterizer
/// supports it (today renders as plain text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Text,
    Search,
    Password,
}

/// Themed text input with consistent padding, rounded corners, and
/// elevated surface. Search variant prepends a magnifier icon. An
/// optional `trailing` widget is rendered right-aligned (typical
/// uses: an app-name badge inside a search bar, a clear button).
///
/// `on_submit` is the ActionId fired when the user submits the input
/// (Enter while focused). Pass [`NO_ACTION`] to opt out — apps that
/// route Enter themselves (e.g. drun's launcher) typically do.
pub fn input(
    value: &str,
    placeholder: &str,
    kind: InputKind,
    on_submit: ActionId,
    trailing: Option<Widget>,
) -> Widget {
    input_maybe_focused(value, placeholder, kind, on_submit, trailing, false)
}

/// `input` that takes focus as soon as its window appears — for a
/// launcher or a dialog whose whole purpose is the field. Everything
/// else should stay unfocused so the window's own keys (arrows, Esc)
/// reach the app instead of a text box the user never clicked.
pub fn input_autofocus(
    value: &str,
    placeholder: &str,
    kind: InputKind,
    on_submit: ActionId,
    trailing: Option<Widget>,
) -> Widget {
    input_maybe_focused(value, placeholder, kind, on_submit, trailing, true)
}

fn input_maybe_focused(
    value: &str,
    placeholder: &str,
    kind: InputKind,
    on_submit: ActionId,
    trailing: Option<Widget>,
    autofocus: bool,
) -> Widget {
    let raw = Widget::Input {
        value:       value.to_string(),
        placeholder: placeholder.to_string(),
        on_submit,
        modifiers:   if autofocus { vec![Modifier::Autofocus] } else { vec![] },
    };
    // No background — the input blends with the dialog so the search bar
    // reads as part of the panel rather than as a stacked card on top.
    // Apps that want the elevated-card look can wrap the result in a
    // `card(..., CardKind::Inset)` or add `Modifier::Background` themselves.
    let mut wrap_mods: Vec<Modifier> = Vec::with_capacity(3);
    wrap_mods.push(Modifier::Padding(Padding::Md.as_u16()));
    wrap_mods.push(Modifier::Rounded(Radius::Md.as_u8()));
    wrap_mods.push(Modifier::Focus(vec![
        Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() },
    ]));

    let mut children: Vec<Widget> = Vec::with_capacity(4);
    if matches!(kind, InputKind::Search) {
        children.push(Widget::Icon {
            id:        IconId::MagnifyingGlass,
            // 24 is the atlas-native size — picks the unscaled glyph
            // for crisp 4K rendering (smaller sizes scale down from
            // the 24 px atlas slot and look fuzzy).
            size:      24,
            modifiers: vec![Modifier::Tint(Token::Accent)],
        });
    }
    children.push(raw);
    if let Some(w) = trailing {
        children.push(Widget::Spacer { flex: 1 });
        children.push(w);
    }

    let row = Widget::Row {
        children,
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: wrap_mods,
    };
    // Wrap with a leading zero-size widget so the wrap-Column's
    // internal `Sm` spacing acts as TOP-margin on the search row.
    // `panel.Padding(Xs=4) + this.spacing(Sm=8) + row.Padding(Md=12)
    // = 24 px` matches the symmetric 24 px below the search text
    // (row bottom padding 12 + panel.spacing Md 12), so the search
    // row stays vertically centred between chrome top and the
    // divider underneath. Mirror trick to `prefab::footer`.
    Widget::Column {
        children: vec![
            Widget::Icon { id: IconId::None, size: 0, modifiers: vec![] },
            row,
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Stretch,
        modifiers: vec![],
    }
}

/// Sentinel ActionId for "no action wired up" — useful as a default
/// for `on_submit` etc. when the app routes the event itself. Apps
/// must not use this id for their own actions.
pub const NO_ACTION: ActionId = ActionId(u32::MAX);

/// Modal dialog wrapper — title bar at the top, body in the middle,
/// optional footer hint at the bottom. Uses Sheet card styling.
///
/// `min_size` becomes a hard layout constraint so the dialog doesn't
/// collapse below readable dimensions even in a small tile.
pub fn dialog(
    title: &str,
    body: Widget,
    footer_hint: Option<&str>,
    min_w: u16,
) -> Widget {
    let mut children: Vec<Widget> = Vec::with_capacity(4);
    children.push(Widget::Text {
        content: title.to_string(),
        style:   TextStyle::Title,
        modifiers: vec![Modifier::Padding(Padding::Sm.as_u16())],
    });
    children.push(Widget::Divider);
    children.push(body);
    if let Some(hint) = footer_hint {
        children.push(Widget::Divider);
        children.push(Widget::Text {
            content: hint.to_string(),
            style:   TextStyle::Caption,
            modifiers: vec![
                Modifier::Padding(Padding::Sm.as_u16()),
            ],
        });
    }

    Widget::Column {
        children,
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Stretch,
        modifiers: vec![
            Modifier::Padding(Padding::Lg.as_u16()),
            Modifier::Background(Token::SurfaceElevated),
            Modifier::Rounded(Radius::Lg.as_u8()),
            Modifier::MinWidth(min_w),
            // Compact density: tighter padding so the dialog still fits
            // in a narrow tile.
            Modifier::WhenDensity(crate::abi::Density::Compact, vec![
                Modifier::Padding(Padding::Md.as_u16()),
            ]),
        ],
    }
}

/// Vertical sidebar container — `SurfaceMuted` background with
/// consistent padding. Children are typically `sidebar_section`s and
/// `nav_row`s; a trailing flex-Spacer is appended automatically so the
/// sections stack to the top and don't stretch.
pub fn sidebar_pane(sections: Vec<Widget>) -> Widget {
    let mut children: Vec<Widget> = sections;
    children.push(Widget::Spacer { flex: 1 });
    Widget::Column {
        children,
        spacing:   Spacing::None.as_u16(),
        align:     Align::Stretch,
        modifiers: vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Padding(Padding::Sm.as_u16()),
        ],
    }
}

/// Top menu-bar — flat row of clickable labels.
///
/// Each label gets its own `Padding(Sm)` so the click hit-rect is
/// generous; the Row's `Spacing::Md` keeps a clear gap *between*
/// labels so they don't read as one squished string. A trailing
/// flex-Spacer absorbs any leftover width on the right edge.
pub fn menu_bar(labels: &[(String, ActionId)]) -> Widget {
    menu_bar_with_anchors(labels, &[])
}

/// Menu bar variant that tags each label with a `NodeId` from
/// `anchors[i]`, so the app can attach a `Widget::Popover` to it
/// for the dropdown. `anchors` is matched positionally; pass `&[]`
/// for the no-NodeId case.
pub fn menu_bar_with_anchors(
    labels: &[(String, ActionId)],
    anchors: &[NodeId],
) -> Widget {
    menu_bar_with_icon(IconId::None, labels, anchors)
}

/// Menu bar with the app's own glyph at the leading edge — the window's
/// identity mark, the way every window in the design carries one
/// (UI_REFRESH.md §5). Pass `IconId::None` for a bare bar.
pub fn menu_bar_with_icon(
    icon: IconId,
    labels: &[(String, ActionId)],
    anchors: &[NodeId],
) -> Widget {
    let mut children: Vec<Widget> = Vec::with_capacity(labels.len() + 3);
    if !matches!(icon, IconId::None) {
        children.push(Widget::Row {
            children: vec![Widget::Icon {
                id: icon,
                size: 16,
                modifiers: vec![Modifier::Tint(Token::Accent)],
            }],
            spacing: 0,
            align:   Align::Center,
            modifiers: vec![Modifier::Padding(Padding::Xs.as_u16())],
        });
    }
    for (i, (label, action)) in labels.iter().enumerate() {
        let mut mods: Vec<Modifier> = Vec::with_capacity(5);
        mods.push(Modifier::Padding(Padding::Sm.as_u16()));
        mods.push(Modifier::Tint(Token::OnSurfaceMuted));
        mods.push(Modifier::OnClick(*action));
        mods.push(Modifier::Hover(vec![
            Modifier::Background(Token::SurfaceHover),
            Modifier::Tint(Token::OnSurface),
            Modifier::Rounded(Radius::Sm.as_u8()),
        ]));
        if let Some(id) = anchors.get(i) {
            mods.push(Modifier::NodeId(*id));
        }
        children.push(Widget::Text {
            content:   label.clone(),
            style:     TextStyle::Body,
            modifiers: mods,
        });
    }
    children.push(Widget::Spacer { flex: 1 });
    Widget::Row {
        children,
        spacing: Spacing::Xxs.as_u16(),
        align:   Align::Center,
        modifiers: vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::MinHeight(MENU_BAR_H),
            Modifier::Background(Token::SurfaceElevated),
        ],
    }
}

/// Menu-bar band height (UI_REFRESH.md §5).
const MENU_BAR_H: u16 = 36;

/// Build a popover-content surface from a list of menu items.
/// Each item is `(label, action_id)`. The wrapper is a SurfaceElevated
/// card with a 1 px Border and Md radius — same visual language as
/// `card(.., CardKind::Sheet)` but tighter padding for menu density.
/// `selected_index` flags the currently-active option (e.g. View →
/// Grid is the active mode) with an Accent tint.
pub fn popover_menu(
    items: &[(String, ActionId)],
    selected_index: Option<usize>,
) -> Widget {
    popover_menu_shortcuts(items, &[], selected_index)
}

/// `popover_menu` with a right-hand shortcut column ("Ctrl+S"). `hints`
/// is indexed alongside `items`; a short list or an empty string just
/// leaves that row without one. A menu is where people *learn* the
/// shortcut, so an app that binds keys should show them here.
pub fn popover_menu_shortcuts(
    items: &[(String, ActionId)],
    hints: &[&str],
    selected_index: Option<usize>,
) -> Widget {
    let mut rows: Vec<Widget> = Vec::with_capacity(items.len());
    for (i, (label, action)) in items.iter().enumerate() {
        let is_selected = selected_index == Some(i);
        let mut text_mods: Vec<Modifier> = Vec::with_capacity(2);
        text_mods.push(Modifier::Padding(Padding::Sm.as_u16()));
        if is_selected {
            // No background highlight — the leading Check icon is the
            // selection cue, matching macOS-style menu checkmarks.
        }
        let icon = if is_selected { IconId::Check } else { IconId::None };
        let row = Widget::Row {
            children: vec![
                Widget::Icon {
                    id: icon,
                    size: 16,
                    modifiers: vec![Modifier::Tint(Token::Accent)],
                },
                Widget::Text {
                    content:   label.clone(),
                    style:     TextStyle::Body,
                    modifiers: vec![],
                },
                // Push the shortcut to the right edge. The spacer is here
                // even without one, so labels line up across a mixed menu.
                Widget::Spacer { flex: 1 },
                Widget::Text {
                    content:   hints.get(i).copied().unwrap_or("").to_string(),
                    style:     TextStyle::Caption,
                    modifiers: vec![Modifier::Tint(Token::OnSurfaceFaint)],
                },
            ],
            spacing: Spacing::Sm.as_u16(),
            align:   Align::Center,
            modifiers: vec![
                Modifier::Padding(Padding::Sm.as_u16()),
                Modifier::OnClick(*action),
                Modifier::Hover(vec![
                    Modifier::Background(Token::SurfaceMuted),
                    Modifier::Rounded(Radius::Sm.as_u8()),
                ]),
            ],
        };
        rows.push(row);
    }
    Widget::Column {
        children: rows,
        spacing: Spacing::Xxs.as_u16(),
        align:   Align::Stretch,
        modifiers: vec![
            Modifier::Background(Token::SurfaceElevated),
            Modifier::Border {
                token:  Token::Border,
                width:  1,
                radius: Radius::Md.as_u8(),
            },
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::MinWidth(180),
        ],
    }
}
