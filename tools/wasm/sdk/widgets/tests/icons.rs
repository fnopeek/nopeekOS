//! `IconId` is a number since it stopped being an enum. These tests stand
//! in for the compile-time check the enum gave: every named icon exists in
//! the atlas the kernel draws from, the atlas holds nothing unnamed, and
//! the bytes on the wire did not change.

use nopeek_widgets::{IconId, Widget};

/// Icon ids in `release/assets/phosphor.atlas` (format: tools/regen-icons).
fn atlas_ids() -> Vec<u16> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../release/assets/phosphor.atlas");
    let b = std::fs::read(path).expect("read atlas");
    assert_eq!(&b[0..8], b"NPKIATLS");
    let n = u16::from_le_bytes([b[10], b[11]]) as usize;
    let sizes = u16::from_le_bytes([b[12], b[13]]) as usize;
    let index = 16 + sizes * 2;
    let entry = 4 + sizes * 4;
    (0..n).map(|i| {
        let at = index + i * entry;
        u16::from_le_bytes([b[at], b[at + 1]])
    }).collect()
}

#[test]
fn every_named_icon_is_in_the_atlas_and_back() {
    let atlas = atlas_ids();
    for (name, id) in IconId::ALL {
        if *id == IconId::None { continue; }
        assert!(atlas.contains(&id.0), "{name} = {} fehlt im Atlas", id.0);
    }
    for a in &atlas {
        assert!(IconId::ALL.iter().any(|(_, id)| id.0 == *a), "Atlas-Eintrag {a} hat keinen Namen");
    }
}

#[test]
fn numbers_are_contiguous_from_zero() {
    for (i, (name, id)) in IconId::ALL.iter().enumerate() {
        assert_eq!(id.0 as usize, i, "{name}");
    }
}

#[test]
fn wire_is_what_the_enum_wrote() {
    // A variant index and a u16 are the same varint in postcard.
    assert_eq!(postcard::to_allocvec(&IconId::PlayCircle).unwrap(), vec![51]);
    assert_eq!(postcard::to_allocvec(&IconId(300)).unwrap(), vec![0xAC, 0x02]);
}

#[test]
fn an_unknown_icon_passes_through() {
    let w = Widget::Icon { id: IconId(9999), size: 16, modifiers: Vec::new() };
    let bytes = postcard::to_allocvec(&w).unwrap();
    let back: Widget = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(back, w);
}
