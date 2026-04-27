//! Compile-time ABI ordering lock.
//!
//! Postcard serializes enum variants by **declaration order**, not by
//! name. Inserting a variant in the middle of an ABI enum breaks every
//! serialized tree written before the change. Removing one does the
//! same.
//!
//! This module pins the ordering with two mechanisms:
//!
//!   1. `const _: () = assert!(...)` on `#[repr(u8/u16)]` enums — a
//!      reordering changes a discriminant, the compile fails.
//!
//!   2. Exhaustive `match` functions over field-carrying enums (`Widget`,
//!      `Modifier`, `Event`, …). Adding a variant without updating the
//!      match produces a non-exhaustive-match error. Within the defining
//!      crate, `#[non_exhaustive]` does not suppress this check.
//!
//! If you land here after a compile error: the ABI changed. Either
//! revert (variants must be appended only) or bump `WIRE_VERSION` and
//! update this file intentionally.

#![allow(dead_code)]

use super::abi::*;

// ── Integer-discriminant locks ────────────────────────────────────────

const _: () = {
    // Token — values frozen forever, appended only.
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

    // Role — values frozen, appended only.
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

    // TextStyle — values frozen.
    assert!(TextStyle::Body    as u8 == 0);
    assert!(TextStyle::Title   as u8 == 1);
    assert!(TextStyle::Caption as u8 == 2);
    assert!(TextStyle::Muted   as u8 == 3);
    assert!(TextStyle::Mono    as u8 == 4);
    assert!(TextStyle::Heading as u8 == 5);

    // IconId — u16, P10.9 Phosphor set frozen.
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
    assert!(IconId::Monitor           as u16 == 19);
    assert!(IconId::FileText          as u16 == 20);
    assert!(IconId::FolderOpen        as u16 == 21);
    assert!(IconId::Image             as u16 == 22);
    assert!(IconId::HardDrives        as u16 == 23);
    assert!(IconId::Code              as u16 == 24);
    assert!(IconId::Folders           as u16 == 25);
    assert!(IconId::CaretRight        as u16 == 26);
    assert!(IconId::ArrowClockwise    as u16 == 27);

    // Align / Axis — used inside Widget struct variants, positions frozen.
    assert!(Align::Start   as u8 == 0);
    assert!(Align::Center  as u8 == 1);
    assert!(Align::End     as u8 == 2);
    assert!(Align::Stretch as u8 == 3);

    assert!(Axis::Vertical   as u8 == 0);
    assert!(Axis::Horizontal as u8 == 1);
    assert!(Axis::Both       as u8 == 2);

    // Density — vocab v2 container-query buckets.
    assert!(Density::Compact  as u8 == 0);
    assert!(Density::Regular  as u8 == 1);
    assert!(Density::Spacious as u8 == 2);

    // MouseButton — Event payload, position frozen.
    assert!(MouseButton::Left   as u8 == 0);
    assert!(MouseButton::Right  as u8 == 1);
    assert!(MouseButton::Middle as u8 == 2);

    // Wire version — must be 0x01 for v1.
    assert!(WIRE_VERSION == 0x01);
};

// ── Exhaustive-match locks (fieldful enums) ───────────────────────────

/// Lock `Widget` variant order. Returns the wire position postcard will
/// write for each variant. If a variant is added, inserted, or removed,
/// this match becomes non-exhaustive (or the expected constant below
/// drifts) and compilation fails.
///
/// Never called — the match is evaluated at type-check time.
fn _widget_wire_position(w: &Widget) -> usize {
    match w {
        // Containers
        Widget::Column   { .. } => 0,
        Widget::Row      { .. } => 1,
        Widget::Stack    { .. } => 2,
        Widget::Scroll   { .. } => 3,
        // Leaves
        Widget::Text     { .. } => 4,
        Widget::Icon     { .. } => 5,
        Widget::Button   { .. } => 6,
        Widget::Input    { .. } => 7,
        Widget::Checkbox { .. } => 8,
        Widget::Spacer   { .. } => 9,
        Widget::Divider         => 10,
        Widget::Canvas   { .. } => 11,
        // Reserved (v2+)
        Widget::Popover  { .. } => 12,
        Widget::Tooltip  { .. } => 13,
        Widget::Menu     { .. } => 14,
    }
}

/// Lock `Modifier` variant order. Same mechanism as above.
fn _modifier_wire_position(m: &Modifier) -> usize {
    match m {
        // Active in v1
        Modifier::Padding(_)        => 0,
        Modifier::Margin(_)         => 1,
        Modifier::Background(_)     => 2,
        Modifier::Border { .. }     => 3,
        Modifier::Opacity(_)        => 4,
        Modifier::Transition(_)     => 5,
        Modifier::OnClick(_)        => 6,
        Modifier::OnHover(_)        => 7,
        // Reserved (v2+ effects, role)
        Modifier::Blur(_)           => 8,
        Modifier::Shadow(_)         => 9,
        Modifier::Effect(_)         => 10,
        Modifier::RoleOverride(_)   => 11,
        Modifier::Tint(_)           => 12,
        // Vocab v2 — Tailwind-style additions.
        Modifier::Hover(_)          => 13,
        Modifier::Focus(_)          => 14,
        Modifier::Active(_)         => 15,
        Modifier::Disabled(_)       => 16,
        Modifier::WhenDensity(_, _) => 17,
        Modifier::Scale(_)          => 18,
        Modifier::MinWidth(_)       => 19,
        Modifier::MaxWidth(_)       => 20,
        Modifier::Rounded(_)        => 21,
    }
}

/// Lock `Event` variant order.
fn _event_wire_position(e: &Event) -> usize {
    match e {
        Event::Key(_)              => 0,
        Event::Action(_)           => 1,
        Event::MouseMove { .. }    => 2,
        Event::MouseButton { .. }  => 3,
        Event::Focus(_)            => 4,
        Event::InputChange { .. }  => 5,
    }
}

/// Lock `Action` variant order.
fn _action_wire_position(a: &Action) -> usize {
    match a {
        Action::Idle     => 0,
        Action::Rerender => 1,
        Action::Exit     => 2,
    }
}

/// Lock `Transition` variant order.
fn _transition_wire_position(t: &Transition) -> usize {
    match t {
        Transition::Spring       => 0,
        Transition::Linear { .. }=> 1,
    }
}

/// Lock `Fill` variant order.
fn _fill_wire_position(f: &Fill) -> usize {
    match f {
        Fill::Solid(_) => 0,
    }
}
