//! Wire round-trip tests for nopeek_widgets.
//!
//! These run on the host with std (via `cargo test` in this crate dir)
//! and verify that a tree encoded on the SDK side decodes back to the
//! same tree with the same byte layout we'll eventually deserialize in
//! the kernel.

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;

use nopeek_widgets::*;
use nopeek_widgets::wire::{decode, encode, WireError, WIRE_VERSION};

fn sample_tree() -> Widget {
    Widget::Column {
        children: vec![
            Widget::Text {
                content: "Hello nopeekOS".to_string(),
                style: TextStyle::Title,
                modifiers: vec![Modifier::Padding(8)],
            },
            Widget::Row {
                children: vec![
                    Widget::Icon {
                        id: IconId::Folder,
                        size: 16,
                        modifiers: vec![],
                    },
                    Widget::Text {
                        content: "Documents".to_string(),
                        style: TextStyle::Body,
                        modifiers: vec![Modifier::Opacity(220)],
                    },
                    Widget::Spacer { flex: 1 },
                    Widget::Button {
                        label: "Open".to_string(),
                        icon: IconId::None,
                        on_click: ActionId(42),
                        modifiers: vec![
                            Modifier::Background(Token::Accent),
                            Modifier::Transition(Transition::Spring),
                        ],
                    },
                ],
                spacing: 4,
                align: Align::Center,
                modifiers: vec![],
            },
            Widget::Divider,
        ],
        spacing: 8,
        align: Align::Stretch,
        modifiers: vec![Modifier::Background(Token::Surface)],
    }
}

#[test]
fn round_trip_preserves_tree() {
    let t0 = sample_tree();
    let bytes = encode(&t0).expect("encode");
    let t1 = decode(&bytes).expect("decode");
    assert_eq!(t0, t1);
}

#[test]
fn version_byte_is_first() {
    let bytes = encode(&sample_tree()).expect("encode");
    assert_eq!(bytes[0], WIRE_VERSION);
}

#[test]
fn rejects_unknown_version() {
    let mut bytes = encode(&sample_tree()).expect("encode");
    bytes[0] = 0xFF;
    match decode(&bytes) {
        Err(WireError::VersionMismatch { got, want }) => {
            assert_eq!(got, 0xFF);
            assert_eq!(want, WIRE_VERSION);
        }
        other => panic!("expected VersionMismatch, got {:?}", other),
    }
}

#[test]
fn rejects_empty_buffer() {
    assert_eq!(decode(&[]), Err(WireError::Empty));
}

#[test]
fn rejects_truncated_body() {
    let bytes = encode(&sample_tree()).expect("encode");
    let truncated = &bytes[..3]; // version byte + 2 bytes of postcard
    assert_eq!(decode(truncated), Err(WireError::Postcard));
}

#[test]
fn reserved_slots_round_trip() {
    // Reserved widget slots must still round-trip — only the compositor
    // logs + rejects them, the wire format accepts them.
    let t0 = Widget::Popover {
        anchor: NodeId(7),
        child: alloc::boxed::Box::new(Widget::Divider),
        modifiers: vec![Modifier::RoleOverride(Role::Group)],
    };
    let bytes = encode(&t0).expect("encode");
    let t1 = decode(&bytes).expect("decode");
    assert_eq!(t0, t1);
}

#[test]
fn events_round_trip() {
    use nopeek_widgets::wire::WIRE_VERSION as _;
    let events = [
        Event::Key(KeyCode::Enter),
        Event::Key(KeyCode::Char(b'a')),
        Event::Key(KeyCode::F(5)),
        Event::Action(ActionId(99)),
        Event::MouseMove { x: 100, y: 200 },
        Event::MouseButton {
            button: MouseButton::Left,
            down: true,
            x: 10,
            y: 20,
        },
        Event::Focus(true),
        Event::InputChange { value: "hello".into() },
        Event::InputChange { value: String::new() },
    ];
    for e in events {
        let bytes = postcard::to_allocvec(&e).expect("ser");
        let back: Event = postcard::from_bytes(&bytes).expect("de");
        assert_eq!(e, back);
    }
}

#[test]
fn token_discriminants_frozen() {
    // Belt-and-braces — check_abi const-asserts should already guarantee
    // this, but surface any drift in a legible test failure too.
    assert_eq!(Token::Surface as u8, 0);
    assert_eq!(Token::Danger  as u8, 11);
}

#[test]
fn density_discriminants_frozen() {
    assert_eq!(Density::Compact  as u8, 0);
    assert_eq!(Density::Regular  as u8, 1);
    assert_eq!(Density::Spacious as u8, 2);
}

#[test]
fn vocab_v2_modifiers_round_trip() {
    // All vocab-v2 modifiers in a single tree — proves wire indices are
    // stable and nested Vec<Modifier> serializes correctly.
    let tree = Widget::Column {
        children: vec![
            Widget::Button {
                label: "Save".to_string(),
                icon: IconId::None,
                on_click: ActionId(1),
                modifiers: vec![
                    Modifier::Padding(Padding::Md.as_u16()),
                    Modifier::Rounded(Radius::Lg.as_u8()),
                    Modifier::Background(Token::Accent),
                    Modifier::Hover(vec![
                        Modifier::Background(Token::AccentMuted),
                        Modifier::Scale(264), // 1.03×
                    ]),
                    Modifier::Focus(vec![
                        Modifier::Border { token: Token::Accent, width: 2, radius: 12 },
                    ]),
                    Modifier::Active(vec![Modifier::Scale(248)]),
                    Modifier::Disabled(vec![Modifier::Opacity(128)]),
                    Modifier::Transition(Motion::Quick.as_transition()),
                ],
            },
            Widget::Row {
                children: vec![Widget::Spacer { flex: 1 }],
                spacing: 0,
                align: Align::Start,
                modifiers: vec![
                    Modifier::MinWidth(320),
                    Modifier::MaxWidth(960),
                    Modifier::WhenDensity(
                        Density::Compact,
                        vec![Modifier::Padding(Padding::Sm.as_u16())],
                    ),
                    Modifier::WhenDensity(
                        Density::Spacious,
                        vec![Modifier::Padding(Padding::Xl.as_u16())],
                    ),
                ],
            },
        ],
        spacing: 8,
        align: Align::Stretch,
        modifiers: vec![],
    };
    let bytes = encode(&tree).expect("encode");
    let back = decode(&bytes).expect("decode");
    assert_eq!(tree, back);
}

#[test]
fn motion_helper_lowers_to_linear_transition() {
    // Motion is SDK-only sugar — wire form must remain Transition::Linear.
    assert_eq!(
        Motion::Quick.as_transition(),
        Transition::Linear { ms: 120 }
    );
    assert_eq!(
        Motion::Normal.as_transition(),
        Transition::Linear { ms: 200 }
    );
}
