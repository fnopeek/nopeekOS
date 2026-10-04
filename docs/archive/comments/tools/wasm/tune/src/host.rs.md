# `tools/wasm/tune/src/host.rs` @ 5e0102684

## L1-3 · `#[link(wasm_import_module = "env")]`

```
//! Host functions — WASM imports from the `env` module, resolved by the
//! kernel at instantiation. Naming the module explicitly is what makes them
//! imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L48 · `pub fn pick_open(start: &str) -> i32 {`

```
/// Open the system file dialog. The answer arrives as `Event::Picked`.
```

## L62-72 · `#[allow(dead_code)]`

```
/// Bytes still sitting in the slot's ring.
///
/// Unused until tune plays a film WITH sound — then this, not the wall
/// clock, is what the picture follows. Declared now because it is the one
/// piece of the A/V-sync design that lives in the kernel and it should be
/// visible here when someone builds that half. It costs nothing in the
/// shipped module: with no caller, the import is stripped (checked — it is
/// NOT in tune.wasm's import list).
///
/// The honest play clock: the wall clock and the audio crystal drift apart,
/// and over a film that shows.
```

## L78-82 · `pub fn canvas_commit_yuv(canvas_id: i32, y: &[u8], u: &[u8], v: &[u8],`

```
/// Upload a planar 4:2:0 frame. The colour conversion happens in the
/// compositor, at the size the canvas really has — doing it here would cost
/// more than decoding the frame (measured: 145 % of a core at 1080p30).
///
/// `flags`: bit 0 = Rec. 709, bit 1 = full range.
```

