# `kernel/src/gui/icons.rs` @ 5e0102684

## L1-22 · `#![allow(dead_code)]`

```
//! Phosphor icon atlas — alpha-only, loaded from npkFS at boot.
//!
//! Packed binary format produced by `tools/regen-icons`:
//!
//! `​``text
//! 0x00  magic      : [u8; 8] = b"NPKIATLS"
//! 0x08  version    : u16 LE = 1
//! 0x0A  num_icons  : u16 LE
//! 0x0C  num_sizes  : u16 LE
//! 0x0E  _pad       : u16
//! 0x10  sizes      : [u16; num_sizes]
//! ....  index      : num_icons × { id: u16, _pad: u16, offsets: [u32; num_sizes] }
//! ....  data       : alpha bytes; icon at size S contributes S*S bytes.
//! `​``
//!
//! Parsing lives here; the ATLAS bytes themselves arrive via
//! `npkfs::fetch("sys/icons/phosphor")`, BLAKE3-verified at a higher
//! layer if a hash is frozen for a specific release.
//!
//! `alpha(IconId, size)` returns a slice view into the loaded atlas;
//! slice lifetime is static (atlas Vec<u8> is held forever once
//! loaded — same ownership model as the Inter Variable font bytes).
```

## L38-39 · `pub struct Atlas {`

```
/// Parsed atlas header + raw bytes. Stored once at load time; we keep
/// the owned `Vec<u8>` to keep returned slices valid forever.
```

## L48 · `offsets: Vec<u32>,   // one per size`

```
// one per size
```

## L58-60 · `pub fn init() {`

```
/// Load the atlas from npkFS. Logs + returns silently on any issue —
/// icons are a nice-to-have, missing atlas means CpuRasterizer falls
/// back to the stub square. Call after npkfs::mount, after login.
```

## L92-97 · `pub fn alpha_for(icon: IconId, size_px: u16) -> Option<(u16, Vec<u8>)> {`

```
/// Look up the alpha bitmap for `icon` at the requested `size_px`.
/// Returns the size that was actually used (nearest >= requested in
/// the atlas) plus the packed alpha byte slice (`S*S` bytes).
///
/// Returns None if atlas not loaded, icon not present, or requested
/// size unreasonable.
```

## L103 · `let entry = atlas.entries.iter().find(|e| e.id == icon.0)?;`

```
// Find entry by id.
```

## L106 · `let (idx, size) = best_size(&atlas.sizes, size_px)?;`

```
// Pick smallest atlas size >= requested, else largest available.
```

