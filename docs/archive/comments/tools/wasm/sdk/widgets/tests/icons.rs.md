# `tools/wasm/sdk/widgets/tests/icons.rs` @ 5e0102684

## L1-4 · `use nopeek_widgets::{IconId, Widget};`

```
//! `IconId` is a number since it stopped being an enum. These tests stand
//! in for the compile-time check the enum gave: every named icon exists in
//! the atlas the kernel draws from, the atlas holds nothing unnamed, and
//! the bytes on the wire did not change.
```

## L8 · `fn atlas_ids() -> Vec<u16> {`

```
/// Icon ids in `release/assets/phosphor.atlas` (format: tools/regen-icons).
```

## L44 · `assert_eq!(postcard::to_allocvec(&IconId::PlayCircle).unwrap(), vec![51]);`

```
// A variant index and a u16 are the same varint in postcard.
```

