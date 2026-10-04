# `tools/wasm/drun/src/lib.rs` @ 5e0102684

## L1 · `#![no_std]`

```
//! drun — Mod+D app launcher.
```

## L20-22 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L77-79 · `const HEAP_SIZE: usize = 256 * 1024;`

```
// Bump allocator reset every rerender; state kept across frames must be
// allocated before `persistent_mark` with enough capacity that push()
// never reallocates past the mark.
```

## L118-121 · `const CLICK_BASE: u32 = 1;`

```
// ActionId encoding:
//   0          → reserved sentinel (prefab::NO_ACTION-style; never fired)
//   1..CLICK_BASE+N    = row click  (offset by CLICK_BASE so 0 stays free)
//   HOVER_BASE+N       = row hover
```

## L137-139 · `let entries = app_catalog::load(&["drun", "dock", "bar", "volume", "pick"]);`

```
// Installed modules + built-in intents, sorted. Exclude drun
// itself and the dock (the dock is launcher infrastructure, not a
// user app — listing it would let a click spawn a second dock).
```

## L200-205 · `let list = Widget::Column {`

```
// Stack the rows in their own tight Column so the inter-row gap
// doesn't follow the panel's top-level rhythm. Reads as a single
// grouped list instead of widely-spaced standalone items.
// Xs padding keeps the selected-row's accent border off the chrome
// edge — dividers above/below stay near-full-bleed (panel padding
// only), the list itself reads as an inset block.
```

## L257-261 · `Event::InputChange { value } => {`

```
// Search-buffer mutation lives in the compositor now —
// we just mirror the new value back into our pre-mark
// heap slot so it survives `alloc_reset(persistent_mark)`
// before the next commit. Past QUERY_CAP we hard-cap;
// the compositor will reconcile on the next round-trip.
```

## L286 · `Outcome::Idle`

```
// ActionId(0) — input on_submit sentinel; ignored.
```

## L297-299 · `fn entry_matches(e: &AppEntry, query_lower: &str) -> bool {`

```
// Case-insensitive substring match over display + launch name. Lives in
// drun (not the SDK) because filtering is launcher-specific; the dock has
// no search box.
```

