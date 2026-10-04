# `tools/wasm/sdk/widgets/src/app_meta.rs` @ 5e0102684

## L1-2 · `use alloc::string::String;`

```
//! AppMeta — launcher-visible metadata embedded per app as a WASM
//! custom section (`.npk.app_meta`), cached by the installer.
```

## L16 · `}`

```
// Appended only.
```

## L49-54 · `Err(_) => decode_lenient(body),`

```
// An app built against a NEWER SDK may name an icon this build has
// never heard of, and serde rejects the whole record for it. Losing
// the icon is a blemish; losing the app is a bug — tune shipped with
// IconId 44 and vanished from the launcher and the dock of every
// system whose drun predated that icon. The name and the description
// sit in front of the icon on the wire, so they are still readable.
```

## L59-60 · `fn decode_lenient(body: &[u8]) -> Result<AppMeta, AppMetaError> {`

```
/// Read only what every wire version puts first: two length-prefixed
/// strings. Anything after them is left to the strict decoder.
```

## L67 · `fn take_str(b: &[u8]) -> Result<(String, &[u8]), AppMetaError> {`

```
/// postcard string: varint byte length, then UTF-8.
```

## L103-104 · `#[test]`

```
/// The exact failure that hid `tune`: an icon the reader has never
/// heard of must cost the icon, not the app.
```

