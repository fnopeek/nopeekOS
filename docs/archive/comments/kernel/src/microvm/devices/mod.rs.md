# `kernel/src/microvm/devices/mod.rs` @ 5e0102684

## L1-10 · `pub mod guest_fetch;`

```
//! MicroVM virtual devices.
//!
//! Pure-software emulation of devices the Linux guest expects to see.
//! Vendor-neutral — both VMX and SVM exit handlers thread the same
//! state through.
//!
//! Phase 12.2 starts here with PCI config-space + virtio-blk emulation.
//! 12.2.2 adds BAR sizing, the modern virtio cap chain, MMIO BAR0
//! emulation and a minimal x86 MOV decoder for SVM-side MMIO traps.
//! Real I/O paths (virtqueue parsing, IRQ injection) land in 12.2.3.
```

