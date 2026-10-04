# `tools/wasm/aml/harness/src/main.rs` @ 5e0102684

## L1-5 · `use aml_core::{ec_gpe, find_batteries, path_str, read_battery, Ec, Namespace};`

```
//! std dev-harness: load a real DSDT, run the AML battery methods against a
//! mock EC, and print what `_BST`/`_BIF` produce. Proves the interpreter
//! executes real firmware AML before we ship it as the wasm driver.
//!
//!   cargo run -p aml_harness -- ../dev/DSDT.aml
```

## L10-12 · `struct MockEc {`

```
/// Mock EC: returns realistic values at the HP Dragonfly battery offsets so the
/// percentage looks sane; everything else reads 0. On real hardware these reads
/// hit ports 0x62/0x66 instead.
```

## L21-22 · `mem.insert(0x84, 0x11);`

```
// @0x84: ADP(bit0)=AC present, BATP(bits4-7) battery-present mask;
// bit4 set => battery 0 present.
```

## L24 · `mem.insert(0x8D, 0x60);`

```
// BFC @0x8D = 6496 mAh (0x1960) full charge
```

## L27 · `mem.insert(0x89, 0x84);`

```
// BDC @0x89 = 7300 mAh design
```

## L30 · `mem.insert(0x99, 0x02);`

```
// BST @0x99 = 0x02 charging
```

## L32 · `mem.insert(0xA1, 0xEC);`

```
// BRC @0xA1 = 5100 mAh (0x13EC) remaining
```

## L35 · `mem.insert(0xA5, 0x14);`

```
// BPV @0xA5 = 7700 mV
```

