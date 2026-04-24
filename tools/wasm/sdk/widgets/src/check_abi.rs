//! Compile-time ABI ordering lock — SDK-side mirror of the kernel's
//! `kernel/src/shade/widgets/check_abi.rs`. Same mechanisms, same
//! expected constants. If these two files disagree, deserialization on
//! either side will silently misinterpret variants.
//!
//! If you land here after a compile error: the ABI changed. Revert
//! (append-only) or bump `WIRE_VERSION` on both sides intentionally.

#![allow(dead_code)]

use crate::abi::*;
use crate::wire::WIRE_VERSION;

const _: () = {
    // Token
    assert!(Token::Surface         as u8 == 0);
    assert!(Token::SurfaceElevated as u8 == 1);
    assert!(Token::SurfaceMuted    as u8 == 2);
    assert!(Token::OnSurface       as u8 == 3);
    assert!(Token::OnSurfaceMuted  as u8 == 4);
    assert!(Token::OnAccent        as u8 == 5);
    assert!(Token::Accent          as u8 == 6);
    assert!(Token::AccentMuted     as u8 == 7);
    assert!(Token::Border          as u8 == 8);
    assert!(Token::Success         as u8 == 9);
    assert!(Token::Warning         as u8 == 10);
    assert!(Token::Danger          as u8 == 11);

    // Role
    assert!(Role::None      as u8 == 0);
    assert!(Role::Button    as u8 == 1);
    assert!(Role::Link      as u8 == 2);
    assert!(Role::TextInput as u8 == 3);
    assert!(Role::List      as u8 == 4);
    assert!(Role::ListItem  as u8 == 5);
    assert!(Role::Heading   as u8 == 6);
    assert!(Role::Image     as u8 == 7);
    assert!(Role::Separator as u8 == 8);
    assert!(Role::Group     as u8 == 9);
    assert!(Role::Status    as u8 == 10);

    // TextStyle
    assert!(TextStyle::Body    as u8 == 0);
    assert!(TextStyle::Title   as u8 == 1);
    assert!(TextStyle::Caption as u8 == 2);
    assert!(TextStyle::Muted   as u8 == 3);
    assert!(TextStyle::Mono    as u8 == 4);

    // IconId
    assert!(IconId::None              as u16 == 0);
    assert!(IconId::Folder            as u16 == 1);
    assert!(IconId::File              as u16 == 2);
    assert!(IconId::ArrowLeft         as u16 == 3);
    assert!(IconId::ArrowRight        as u16 == 4);
    assert!(IconId::ArrowUp           as u16 == 5);
    assert!(IconId::ArrowDown         as u16 == 6);
    assert!(IconId::Home              as u16 == 7);
    assert!(IconId::Download          as u16 == 8);
    assert!(IconId::MagnifyingGlass   as u16 == 9);
    assert!(IconId::X                 as u16 == 10);
    assert!(IconId::Check             as u16 == 11);
    assert!(IconId::Gear              as u16 == 12);
    assert!(IconId::Power             as u16 == 13);
    assert!(IconId::Lock              as u16 == 14);
    assert!(IconId::Terminal          as u16 == 15);
    assert!(IconId::Trash             as u16 == 16);
    assert!(IconId::DotsThreeVertical as u16 == 17);
    assert!(IconId::List              as u16 == 18);

    // Align / Axis
    assert!(Align::Start   as u8 == 0);
    assert!(Align::Center  as u8 == 1);
    assert!(Align::End     as u8 == 2);
    assert!(Align::Stretch as u8 == 3);

    assert!(Axis::Vertical   as u8 == 0);
    assert!(Axis::Horizontal as u8 == 1);
    assert!(Axis::Both       as u8 == 2);

    // MouseButton
    assert!(MouseButton::Left   as u8 == 0);
    assert!(MouseButton::Right  as u8 == 1);
    assert!(MouseButton::Middle as u8 == 2);

    // Wire version
    assert!(WIRE_VERSION == 0x01);

    // AppMeta wire version (tracked separately from widget wire).
    assert!(crate::app_meta::APP_META_WIRE == 0x01);
};

fn _icon_ref_wire_position(r: &crate::app_meta::IconRef) -> usize {
    match r {
        crate::app_meta::IconRef::Builtin(_) => 0,
    }
}

// ── Exhaustive-match locks (fieldful enums) ───────────────────────────

fn _widget_wire_position(w: &Widget) -> usize {
    match w {
        Widget::Column   { .. } => 0,
        Widget::Row      { .. } => 1,
        Widget::Stack    { .. } => 2,
        Widget::Scroll   { .. } => 3,
        Widget::Text     { .. } => 4,
        Widget::Icon     { .. } => 5,
        Widget::Button   { .. } => 6,
        Widget::Input    { .. } => 7,
        Widget::Checkbox { .. } => 8,
        Widget::Spacer   { .. } => 9,
        Widget::Divider         => 10,
        Widget::Canvas   { .. } => 11,
        Widget::Popover  { .. } => 12,
        Widget::Tooltip  { .. } => 13,
        Widget::Menu     { .. } => 14,
    }
}

fn _modifier_wire_position(m: &Modifier) -> usize {
    match m {
        Modifier::Padding(_)      => 0,
        Modifier::Margin(_)       => 1,
        Modifier::Background(_)   => 2,
        Modifier::Border { .. }   => 3,
        Modifier::Opacity(_)      => 4,
        Modifier::Transition(_)   => 5,
        Modifier::OnClick(_)      => 6,
        Modifier::OnHover(_)      => 7,
        Modifier::Blur(_)         => 8,
        Modifier::Shadow(_)       => 9,
        Modifier::Effect(_)       => 10,
        Modifier::RoleOverride(_) => 11,
    }
}

fn _event_wire_position(e: &Event) -> usize {
    match e {
        Event::Key(_)             => 0,
        Event::Action(_)          => 1,
        Event::MouseMove { .. }   => 2,
        Event::MouseButton { .. } => 3,
        Event::Focus(_)           => 4,
    }
}

fn _action_wire_position(a: &Action) -> usize {
    match a {
        Action::Idle     => 0,
        Action::Rerender => 1,
        Action::Exit     => 2,
    }
}

fn _transition_wire_position(t: &Transition) -> usize {
    match t {
        Transition::Spring        => 0,
        Transition::Linear { .. } => 1,
    }
}

fn _fill_wire_position(f: &Fill) -> usize {
    match f {
        Fill::Solid(_) => 0,
    }
}

fn _keycode_wire_position(k: &KeyCode) -> usize {
    match k {
        KeyCode::Char(_)    => 0,
        KeyCode::Enter      => 1,
        KeyCode::Backspace  => 2,
        KeyCode::Tab        => 3,
        KeyCode::Escape     => 4,
        KeyCode::Delete     => 5,
        KeyCode::Insert     => 6,
        KeyCode::Up         => 7,
        KeyCode::Down       => 8,
        KeyCode::Left       => 9,
        KeyCode::Right      => 10,
        KeyCode::Home       => 11,
        KeyCode::End        => 12,
        KeyCode::PageUp     => 13,
        KeyCode::PageDown   => 14,
        KeyCode::F(_)       => 15,
    }
}
