# `kernel/src/microvm/devices/pci_bus.rs` @ 5e0102684

## L1-22 · `use super::virtio_blk_pci::VirtioBlk;`

```
//! PCI bus emulation for the Linux MicroVM guest.
//!
//! Type-1 config-space access via legacy PIO ports 0xCF8 (address) and
//! 0xCFC..0xCFF (data window with byte-lane select). No ACPI, no MMCFG —
//! the guest boots with `acpi=off` so it falls back to the legacy path.
//!
//! Layout:
//!
//! `​``text
//!   bus 0, slot 0, func 0  →  Intel i440FX-style host bridge (8086:1237)
//!   bus 0, slot 1, func 0  →  virtio-blk-pci  (1AF4:1042, rw profile.img → /dev/vda)
//!   bus 0, slot 2, func 0  →  virtio-net-pci  (1AF4:1041)
//!   bus 0, slot 3, func 0  →  virtio-gpu-pci  (1AF4:1050)
//!   bus 0, slot 4, func 0  →  virtio-input-pci (1AF4:1052)
//!   bus 0, slot 5, func 0  →  virtio-blk-pci  (1AF4:1042, ro sqfs → /dev/vdb)
//!   bus 0, slot 6, func 0  →  virtio-9p-pci   (1AF4:1049, npkFS share → mount -t 9p)
//!   everything else        →  vendor=0xFFFF (no device)
//! `​``
//!
//! Slot 1's full state — including BAR sizing handshake and the modern
//! virtio capability list — lives in `VirtioBlk`. This module just
//! routes config-space dwords to/from there.
```

## L35 · `pub struct PciBus {`

```
/// Per-VM PCI bus emulation state.
```

## L39-41 · `pub virtio_input: VirtioInput,`

```
// virtio-net AND virtio-gpu live out of VmShared/VM_BIG_LOCK (in
// `net_backend` / `gpu_backend`), so their off-vCPU workers can own the
// data-plane. Config-space dispatch reaches the GPU via `gpu_backend::lock()`.
```

## L43 · `pub virtio_blk_sqfs: VirtioBlk,`

```
/// Slot 5 — read-only squashfs userspace bundle (/dev/vdb).
```

## L45 · `pub virtio_9p: Virtio9p,`

```
/// Slot 6 — virtio-9p share onto npkFS home/<user>/ (mount -t 9p).
```

## L47 · `pub virtio_snd: VirtioSnd,`

```
/// Slot 7 — virtio-sound: bridges guest audio → kernel audio mailbox.
```

## L64-69 · `pub fn handle_pci_io(`

```
/// Dispatch a guest PIO access targeted at a PCI config-space port.
/// Returns `Some(value)` for IN reads, `None` for OUT writes (caller
/// leaves guest RAX alone in that case).
///
/// Caller must already have decided this port is in our PCI range
/// (`PCI_CONFIG_ADDR` or `PCI_CONFIG_DATA_START..=PCI_CONFIG_DATA_END`).
```

## L87 · `let lane_off = (port - PCI_CONFIG_DATA_START) as u8; // 0..=3`

```
// 0..=3
```

## L110-111 · `let val = if size == 4 && lane_off == 0 {`

```
// A byte or word write merges into its dword (read-modify-write):
// Linux writes the MSI-X message control as a 16-bit word.
```

## L158-159 · `fn host_bridge_config(reg: u8) -> u32 {`

```
/// Intel i440FX host bridge — minimum descriptor that satisfies the
/// Linux PCI enumerator. Class 06_00_00 = host bridge.
```

