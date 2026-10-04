# `tools/regen-icons/src/main.rs` @ 5e0102684

## L1-30 · `use std::fs;`

```
//! regen-icons — rasterize Phosphor SVGs into a nopeekOS icon atlas.
//!
//! Reads `icons/phosphor/*.svg` (relative to repo root), rasterizes
//! each via resvg at 5 sizes (16/24/32/48/64 actual px, alpha only),
//! packs into a binary blob at `release/assets/phosphor.atlas`. That
//! blob then ships with the installer and lands at
//! `sys/icons/phosphor` in npkFS.
//!
//! Atlas format (little-endian throughout):
//!
//! `​``text
//! 0x00  magic      : [u8; 8] = b"NPKIATLS"
//! 0x08  version    : u16 = 1
//! 0x0A  num_icons  : u16
//! 0x0C  num_sizes  : u16
//! 0x0E  _pad       : u16 = 0
//! 0x10  sizes      : [u16; num_sizes]            — px edge length
//! ....  index      : [IconEntry; num_icons]
//!         IconEntry { icon_id: u16, _pad: u16, offsets: [u32; num_sizes] }
//! ....  data       : alpha bytes, per-icon per-size, concatenated
//!                    size S contributes exactly S*S bytes
//! `​``
//!
//! `IconId` values match the frozen enum in
//! `kernel/src/shade/widgets/abi.rs` — see the `ICONS` table below.
//! Appending a new IconId = add a row, regen, commit atlas + kernel
//! ABI extension in the same release.
//!
//! Run from repo root: `cargo run --release --manifest-path tools/regen-icons/Cargo.toml`
//! Output: `release/assets/phosphor.atlas` (regenerate + commit).
```

## L35-36 · `struct IconEntry {`

```
/// One icon in the atlas — which SVG filename supplies its shape,
/// which IconId discriminant it corresponds to in the kernel ABI.
```

## L40 · `name: &'static str,`

```
/// Label for logging only.
```

## L44-45 · `const ICONS: &[IconEntry] = &[`

```
/// The v1 icon set. Order matches the kernel's IconId enum; new
/// icons must be appended, not inserted.
```

## L100 · `const SIZES: &[u16] = &[16, 24, 32, 48, 64];`

```
/// Rasterized sizes, in actual pixels (not HiDPI-scaled).
```

## L111 · `let svg_size = tree.size();`

```
// Fit SVG into target square, centred, preserving aspect ratio.
```

## L124-126 · `pixmap.pixels().iter().map(|p| p.alpha()).collect()`

```
// Phosphor SVGs use `currentColor` (usually black); resvg paints
// opaque black. We keep just the alpha channel and let the kernel
// tint per theme token at composite time.
```

## L131 · `let repo_root = PathBuf::from(std::env::current_dir().expect("cwd"));`

```
// Resolve paths relative to the repo root (cargo run CWD = repo root).
```

## L140 · `let mut alpha_per_icon_size: Vec<Vec<Vec<u8>>> = Vec::with_capacity(ICONS.len());`

```
// Pass 1: rasterize every (icon, size) tuple, stash alpha bytes.
```

## L157 · `let num_icons = ICONS.len() as u16;`

```
// Pass 2: layout the atlas. Header + sizes + index + data.
```

## L161 · `let header_len   = 8 + 2 + 2 + 2 + 2;                // magic + version + num_icons + num_sizes + pad`

```
// magic + version + num_icons + num_sizes + pad
```

## L163 · `let entry_len    = 2 + 2 + SIZES.len() * 4;          // id + pad + offsets`

```
// id + pad + offsets
```

## L169 · `blob.extend_from_slice(MAGIC);`

```
// Header
```

## L176 · `for &sz in SIZES { blob.extend_from_slice(&sz.to_le_bytes()); }`

```
// Sizes
```

## L179-180 · `let index_offset = blob.len();`

```
// Index — written after we know per-(icon, size) offsets. Reserve
// space first, then back-patch.
```

## L184 · `let mut data_cursor: u32 = data_start as u32;`

```
// Data — concatenate, track offsets.
```

## L199 · `for (i, icon) in ICONS.iter().enumerate() {`

```
// Back-patch index.
```

## L210 · `blob.extend_from_slice(&data_bytes);`

```
// Append data.
```

