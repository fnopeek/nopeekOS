//! Prefab components — app-facing "how to build a nopeek UI" cookbook.
//! Apps assemble screens from these, never from raw Row/Column/Modifier.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::abi::{
    ActionId, Align, Axis, IconId, Modifier, TextStyle, Token, Widget,
};
use crate::style::{Padding, Radius, Spacing};

// Panel has no padding so dividers reach the window chrome for a
// closed ring; children set their own padding individually.
pub fn panel(children: Vec<Widget>) -> Widget {
    Widget::Column {
        children,
        spacing: Spacing::Sm.as_u16(),
        align:   Align::Stretch,
        modifiers: vec![],
    }
}

pub fn searchbar(query: &str, placeholder: &str, trailing: Option<Widget>) -> Widget {
    let (text, style) = if query.is_empty() {
        (placeholder.to_string(), TextStyle::Muted)
    } else {
        (query.to_string(), TextStyle::Body)
    };

    let mut children: Vec<Widget> = Vec::with_capacity(4);
    children.push(Widget::Icon {
        id:        IconId::MagnifyingGlass,
        size:      24,
        modifiers: vec![],
    });
    children.push(Widget::Text { content: text, style, modifiers: vec![] });
    children.push(Widget::Spacer { flex: 1 });
    if let Some(w) = trailing { children.push(w); }

    Widget::Row {
        children,
        spacing: Spacing::Sm.as_u16(),
        align:   Align::Center,
        modifiers: vec![Modifier::Padding(Padding::Md.as_u16())],
    }
}

pub fn list_row(
    icon: IconId,
    title: &str,
    subtitle: &str,
    selected: bool,
    on_click: Option<ActionId>,
    on_hover: Option<ActionId>,
) -> Widget {
    let mut row_mods: Vec<Modifier> = Vec::with_capacity(5);
    row_mods.push(Modifier::Padding(Padding::Sm.as_u16()));
    if let Some(id) = on_click { row_mods.push(Modifier::OnClick(id)); }
    if let Some(id) = on_hover { row_mods.push(Modifier::OnHover(id)); }
    if selected {
        row_mods.push(Modifier::Background(Token::AccentMuted));
        row_mods.push(Modifier::Border {
            token:  Token::Accent,
            width:  0,
            radius: Radius::Md.as_u8(),
        });
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

    let mut children: Vec<Widget> = Vec::with_capacity(4);
    children.push(Widget::Icon { id: icon, size: 24, modifiers: vec![] });
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
