# `kernel/src/gpu/gop.rs` @ 5e0102684

## L1-4 · `use super::{FramebufferInfo, GpuError, GpuHal, ModeInfo};`

```
//! GOP (Graphics Output Protocol) Fallback Driver
//!
//! Uses the framebuffer provided by the bootloader via Multiboot2.
//! No modesetting — resolution is fixed at boot by GRUB/UEFI.
```

## L17 · `Err(GpuError::UnsupportedMode) // GOP can't switch modes`

```
// GOP can't switch modes
```

## L28 · `fn flip(&mut self, _surface_addr: u64) { /* GOP: no hardware flip */ }`

```
/* GOP: no hardware flip */
```

## L29 · `fn wait_vblank(&self) { /* GOP: no vblank access */ }`

```
/* GOP: no vblank access */
```

## L34-36 · `pub fn from_boot_info(boot_info: &crate::boot_info::BootInfo) -> Option<Self> {`

```
/// Pull the framebuffer details from the BootInfo struct that
/// `boot_uefi::efi_main` populated via the UEFI Graphics Output
/// Protocol. Returns `None` if no framebuffer was discovered.
```

## L48-50 · `let fb_size = pitch as u64 * height as u64;`

```
// Map framebuffer pages identity. UEFI already exposed them at
// their physical address, but we install our own page tables
// later and need the entries.
```

## L71 · `alloc::vec![ModeInfo {`

```
// GOP only has the one mode the bootloader selected
```

