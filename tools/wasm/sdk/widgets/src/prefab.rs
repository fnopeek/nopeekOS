//! Prefab components — app-facing "how to build a nopeek UI" cookbook.
//! Apps assemble screens from these, never from raw Row/Column/Modifier.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::abi::{
    ActionId, Align, Axis, IconId, Modifier, TextStyle, Token, Widget,
};
use crate::style::{Elevation, Padding, Radius, Spacing};

// Panel has no padding so dividers reach the window chrome for a
// closed ring; children set their own padding individually.
pub fn panel(children: Vec<Widget>) -> Widget {
    Widget::Column {
        children,
        spacing: Spacing::Md.as_u16(),
        align:   Align::Stretch,
        modifiers: vec![],
    }
}

// Selected-row padding is generous so the accent fill has breathing
// room around the text. list_row itself uses Md; the generous reading
// means Lg for the row container overall.
pub fn list_row(
    icon: IconId,
    title: &str,
    subtitle: &str,
    selected: bool,
    on_click: Option<ActionId>,
    on_hover: Option<ActionId>,
) -> Widget {
    let mut row_mods: Vec<Modifier> = Vec::with_capacity(6);
    row_mods.push(Modifier::Padding(Padding::Md.as_u16()));
    if let Some(id) = on_click { row_mods.push(Modifier::OnClick(id)); }
    if let Some(id) = on_hover { row_mods.push(Modifier::OnHover(id)); }
    if selected {
        row_mods.push(Modifier::Background(Token::AccentMuted));
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
        spacing:   Spacing::Xxs.as_u16(),
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
    Widget::Row {
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
    let mut mods: Vec<Modifier> = Vec::with_capacity(5);
    mods.push(Modifier::Padding(Padding::Sm.as_u16()));
    if let Some(id) = on_click { mods.push(Modifier::OnClick(id)); }
    if let Some(id) = on_hover { mods.push(Modifier::OnHover(id)); }
    mods.push(Modifier::Hover(vec![
        Modifier::Background(Token::SurfaceMuted),
        Modifier::Rounded(Radius::Sm.as_u8()),
    ]));
    mods.push(Modifier::Focus(vec![
        Modifier::Border { token: Token::OnSurfaceMuted, width: 1, radius: Radius::Sm.as_u8() },
    ]));
    Widget::Icon { id: icon, size, modifiers: mods }
}

/// Small uppercase section label shown above a group of `nav_row`s in a
/// sidebar. Matches the "PLACES" / "DEVICES" look from the Thunar mockup.
pub fn sidebar_section(label: &str, items: Vec<Widget>) -> Widget {
    let mut children: Vec<Widget> = Vec::with_capacity(items.len() + 1);
    children.push(Widget::Text {
        content: label.to_string(),
        style:   TextStyle::Caption,
        modifiers: vec![
            Modifier::Padding(Padding::Xs.as_u16()),
        ],
    });
    children.extend(items);
    Widget::Column {
        children,
        spacing:   Spacing::Xxs.as_u16(),
        align:     Align::Stretch,
        modifiers: vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

/// One entry inside a sidebar. Icon on the left, label to the right,
/// full-width accent fill when selected.
pub fn nav_row(
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
        mods.push(Modifier::Background(Token::AccentMuted));
        mods.push(Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Sm.as_u8() });
    } else {
        mods.push(Modifier::Hover(vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Rounded(Radius::Sm.as_u8()),
        ]));
        mods.push(Modifier::Focus(vec![
            Modifier::Border { token: Token::OnSurfaceMuted, width: 1, radius: Radius::Sm.as_u8() },
        ]));
    }

    let icon_mods = if selected { vec![Modifier::Tint(Token::Accent)] } else { vec![] };

    Widget::Row {
        children: vec![
            Widget::Icon { id: icon, size: 24, modifiers: icon_mods },
            Widget::Text { content: label.to_string(), style: TextStyle::Body, modifiers: vec![] },
            Widget::Spacer { flex: 1 },
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: mods,
    }
}

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
        children.push(Widget::Text {
            content: label.clone(),
            style:   if i + 1 == segments.len() { TextStyle::Body } else { TextStyle::Muted },
            modifiers: vec![
                Modifier::Padding(Padding::Xs.as_u16()),
                Modifier::OnClick(*action),
            ],
        });
    }
    Widget::Row {
        children,
        spacing: Spacing::Xxs.as_u16(),
        align:   Align::Center,
        modifiers: vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Sm.as_u8() },
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
        mods.push(Modifier::Background(Token::AccentMuted));
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
    let mut mods: Vec<Modifier> = Vec::with_capacity(6);
    mods.push(Modifier::Padding(Padding::Md.as_u16()));
    mods.push(Modifier::Background(bg));
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
    let raw = Widget::Input {
        value:       value.to_string(),
        placeholder: placeholder.to_string(),
        on_submit,
        modifiers:   vec![],
    };
    let mut wrap_mods: Vec<Modifier> = Vec::with_capacity(4);
    wrap_mods.push(Modifier::Padding(Padding::Md.as_u16()));
    wrap_mods.push(Modifier::Background(Token::SurfaceElevated));
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

    Widget::Row {
        children,
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: wrap_mods,
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
pub fn menu_bar(labels: &[(String, ActionId)]) -> Widget {
    let mut children: Vec<Widget> = Vec::with_capacity(labels.len() + 1);
    for (label, action) in labels {
        children.push(Widget::Text {
            content: label.clone(),
            style:   TextStyle::Body,
            modifiers: vec![
                Modifier::Padding(Padding::Sm.as_u16()),
                Modifier::OnClick(*action),
            ],
        });
    }
    children.push(Widget::Spacer { flex: 1 });
    Widget::Row {
        children,
        spacing: Spacing::None.as_u16(),
        align:   Align::Center,
        modifiers: vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::Background(Token::SurfaceElevated),
        ],
    }
}
