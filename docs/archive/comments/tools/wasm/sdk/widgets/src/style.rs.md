# `tools/wasm/sdk/widgets/src/style.rs` @ 5e0102684

## L1-2 · `#[non_exhaustive]`

```
//! Design tokens — size scales apps reference by name instead of
//! hardcoded pixels. A future theme swap retunes the whole UI here.
```

## L63-66 · `#[non_exhaustive]`

```
/// Motion duration token — semantic timing for `Transition::Linear`.
/// SDK-only convenience: mapped to milliseconds at modifier-construction
/// time, so the wire form stays the existing `Transition::Linear { ms }`
/// (no new variant, no wire-version bump).
```

## L80-81 · `pub const fn as_transition(self) -> crate::abi::Transition {`

```
/// Lower this token to a wire-format `Transition`. Use as
/// `Modifier::Transition(Motion::Quick.as_transition())`.
```

