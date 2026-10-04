# `kernel/src/vga.rs` @ 5e0102684

## L1-4 · `const VGA_BUFFER: *mut u8 = 0xB8000 as *mut u8;`

```
//! VGA Text Mode
//!
//! Visual boot status indicator for QEMU/VirtualBox window.
//! Will be replaced by framebuffer canvas in Phase 8.
```

## L10 · `const COLOR_NORMAL: u8 = 0x07;  // light gray on black`

```
// light gray on black
```

## L11 · `const COLOR_BRIGHT: u8 = 0x0F;  // white on black`

```
// white on black
```

## L14 · `const COLOR_DARK: u8 = 0x08;    // dark gray on black`

```
// dark gray on black
```

## L21-22 · `pub fn debug_mark(ch: u8) {`

```
/// Write a single debug character to top-right corner of VGA.
/// Call this at each boot stage to track progress on real hardware.
```

## L27 · `let offset = col * 2; // Row 0`

```
// Row 0
```

