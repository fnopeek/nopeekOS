# `tools/wasm/volume/src/lib.rs` @ 5e0102684

## L1-7 · `#![no_std]`

```
//! volume — a centred overlay volume slider, launched by the bar.
//!
//! drun-style: declares itself a modal overlay (`npk_window_set_overlay` +
//! `npk_window_set_modal`), renders a clickable level track, adjusts the
//! kernel master volume, and closes on Esc / the X / picking nothing. No
//! kernel support beyond the generic overlay host fns — the bar just
//! `launch`es this module on a speaker click.
```

## L24-26 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L66-68 · `const HEAP_SIZE: usize = 128 * 1024;`

```
// ── Bump allocator ───────────────────────────────────────────────────
// No heap state survives between frames (the slider reads its level from
// the statics below), so every commit resets to zero and rebuilds.
```

## L95 · `const MUTE: u32  = 2;`

```
// ── State + actions ──────────────────────────────────────────────────
```

## L98 · `const VOL_STEPS: u32 = 20;          // 5 %-steps`

```
// 5 %-steps
```

## L100 · `const W: i32 = 340;`

```
// Overlay size (px). Sized to the header + a 20-cell track.
```

## L105 · `static mut PRE_MUTE: u8 = 50;       // level restored on un-mute`

```
// level restored on un-mute
```

## L111-113 · `let mut cells: Vec<Widget> = Vec::with_capacity(VOL_STEPS as usize);`

```
// The slider track: VOL_STEPS clickable cells, filled (Accent) up to the
// current level, muted past it. No drag (the ABI gives clicks), so the
// cells double as the slider's discrete stops.
```

## L119 · `content: " ".to_string(),     // a space gives the cell its height`

```
// a space gives the cell its height
```

## L139 · `let header = Widget::Row {`

```
// Speaker → toggle mute. Esc / click-outside close (no chrome button).
```

## L161 · `unsafe { HEAP_POS = 0; }   // reclaim the previous frame's tree`

```
// reclaim the previous frame's tree
```

## L206-208 · `let packed = npk_screen_size();`

```
// Top-right, just below the bar (≈8 px under the ~40 px strut). The
// compositor clamps to the screen. Light-dismiss = close on a click
// outside; Esc closes too.
```

