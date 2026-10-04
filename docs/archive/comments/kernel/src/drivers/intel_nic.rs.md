# `kernel/src/drivers/intel_nic.rs` @ 5e0102684

## L1-5 · `use core::sync::atomic::{AtomicBool, Ordering};`

```
//! Intel Ethernet Driver (e1000 / e1000e / I219 family), and the front door
//! for I225/I226: those are handed to `igc` after BAR0 is mapped.
//!
//! MMIO via BAR0. Legacy RX/TX descriptor rings with DMA, polled.
//! Exposes same API as virtio_net.
```

## L18 · `const KNOWN_IDS: &[u16] = &[`

```
// Known Intel NIC device IDs
```

## L20 · `0x15F3, // I225-V`

```
// I225-V
```

## L21 · `0x15F2, // I225-LM`

```
// I225-LM
```

## L22 · `0x125C, // I226-V`

```
// I226-V
```

## L23 · `0x125B, // I226-LM`

```
// I226-LM
```

## L24 · `0x15BC, // I219-V (Cannon Lake)`

```
// I219-V (Cannon Lake)
```

## L25 · `0x15BD, // I219-LM`

```
// I219-LM
```

## L26 · `0x15BE, // I219-V`

```
// I219-V
```

## L27 · `0x0D4F, // I219-LM (Comet Lake)`

```
// I219-LM (Comet Lake)
```

## L28 · `0x0D4E, // I219-V (Comet Lake)`

```
// I219-V (Comet Lake)
```

## L29 · `0x1A1E, // I219-LM (Alder Lake)`

```
// I219-LM (Alder Lake)
```

## L30 · `0x1A1F, // I219-V (Alder Lake)`

```
// I219-V (Alder Lake)
```

## L31 · `0x550A, // I219-V (Raptor Lake)`

```
// I219-V (Raptor Lake)
```

## L32 · `0x550B, // I219-LM (Raptor Lake)`

```
// I219-LM (Raptor Lake)
```

## L33 · `0x100E, // 82540EM (QEMU default e1000)`

```
// Classic e1000/e1000e
```

## L34 · `0x100E, // 82540EM (QEMU default e1000)`

```
// 82540EM (QEMU default e1000)
```

## L35 · `0x100F, // 82545EM`

```
// 82545EM
```

## L36 · `0x10D3, // 82574L`

```
// 82574L
```

## L37 · `0x153A, // I217-LM`

```
// I217-LM
```

## L38 · `0x153B, // I217-V`

```
// I217-V
```

## L41 · `const CTRL: u32     = 0x0000;  // Device Control`

```
// Common registers
```

## L42 · `const CTRL: u32     = 0x0000;  // Device Control`

```
// Device Control
```

## L43 · `const STATUS: u32   = 0x0008;  // Device Status`

```
// Device Status
```

## L44 · `const EERD: u32     = 0x0014;  // EEPROM Read`

```
// EEPROM Read
```

## L45 · `const ICR: u32      = 0x00C0;  // Interrupt Cause Read (clear on read)`

```
// Interrupt Cause Read (clear on read)
```

## L46 · `const IMC: u32      = 0x00D8;  // Interrupt Mask Clear`

```
// Interrupt Mask Clear
```

## L47 · `const RCTL: u32     = 0x0100;  // Receive Control`

```
// Receive Control
```

## L48 · `const TCTL: u32     = 0x0400;  // Transmit Control`

```
// Transmit Control
```

## L49 · `const TIPG: u32     = 0x0410;  // Transmit IPG`

```
// Transmit IPG
```

## L50 · `const RAL: u32      = 0x5400;  // Receive Address Low`

```
// Receive Address Low
```

## L51 · `const RAH: u32      = 0x5404;  // Receive Address High`

```
// Receive Address High
```

## L52 · `const MTA: u32      = 0x5200;  // Multicast Table Array (128 entries)`

```
// Multicast Table Array (128 entries)
```

## L54 · `const E1000_RDBAL: u32 = 0x2800;`

```
// e1000 classic queue registers
```

## L66 · `const STATUS_LU: u32 = 1 << 1;  // Link Up`

```
// Status register bits
```

## L67 · `const STATUS_LU: u32 = 1 << 1;  // Link Up`

```
// Link Up
```

## L69 · `const IGC_IDS: &[u16] = &[0x15F3, 0x15F2, 0x125C, 0x125B];`

```
// I225/I226 device IDs — driven by `igc`
```

## L72 · `const CTRL_RST: u32     = 1 << 26;`

```
// CTRL bits
```

## L74 · `const CTRL_SLU: u32     = 1 << 6;   // Set Link Up`

```
// Set Link Up
```

## L75 · `const CTRL_ASDE: u32    = 1 << 5;   // Auto-Speed Detection Enable`

```
// Auto-Speed Detection Enable
```

## L77 · `const RCTL_EN: u32      = 1 << 1;   // Receiver Enable`

```
// RCTL bits
```

## L78 · `const RCTL_EN: u32      = 1 << 1;   // Receiver Enable`

```
// Receiver Enable
```

## L80 · `const RCTL_SBP: u32     = 1 << 2;   // Store Bad Packets`

```
// Store Bad Packets
```

## L82 · `const RCTL_UPE: u32     = 1 << 3;   // Unicast Promiscuous`

```
// Unicast Promiscuous
```

## L84 · `const RCTL_MPE: u32     = 1 << 4;   // Multicast Promiscuous`

```
// Multicast Promiscuous
```

## L85 · `const RCTL_BAM: u32     = 1 << 15;  // Broadcast Accept Mode`

```
// Broadcast Accept Mode
```

## L87 · `const RCTL_BSIZE_4K: u32 = (3 << 16) | (1 << 25); // Buffer size 4096`

```
// Buffer size 4096
```

## L89 · `const RCTL_BSIZE_2K: u32 = 0;       // Buffer size 2048 (default)`

```
// Buffer size 2048 (default)
```

## L90 · `const RCTL_SECRC: u32   = 1 << 26;  // Strip Ethernet CRC`

```
// Strip Ethernet CRC
```

## L92 · `const TCTL_EN: u32      = 1 << 1;   // Transmit Enable`

```
// TCTL bits
```

## L93 · `const TCTL_EN: u32      = 1 << 1;   // Transmit Enable`

```
// Transmit Enable
```

## L94 · `const TCTL_PSP: u32     = 1 << 3;   // Pad Short Packets`

```
// Pad Short Packets
```

## L95 · `const TCTL_CT_SHIFT: u32 = 4;       // Collision Threshold`

```
// Collision Threshold
```

## L96 · `const TCTL_COLD_SHIFT: u32 = 12;    // Collision Distance`

```
// Collision Distance
```

## L98 · `const RXD_STAT_DD: u8   = 1 << 0;   // Descriptor Done`

```
// RX descriptor status bits
```

## L99 · `const RXD_STAT_DD: u8   = 1 << 0;   // Descriptor Done`

```
// Descriptor Done
```

## L101 · `const RXD_STAT_EOP: u8  = 1 << 1;   // End of Packet`

```
// End of Packet
```

## L103 · `const TXD_CMD_EOP: u8   = 1 << 0;   // End of Packet`

```
// TX descriptor command bits
```

## L104 · `const TXD_CMD_EOP: u8   = 1 << 0;   // End of Packet`

```
// End of Packet
```

## L105 · `const TXD_CMD_IFCS: u8  = 1 << 1;   // Insert FCS/CRC`

```
// Insert FCS/CRC
```

## L106 · `const TXD_CMD_RS: u8    = 1 << 3;   // Report Status`

```
// Report Status
```

## L107 · `const TXD_STAT_DD: u8   = 1 << 0;   // Descriptor Done`

```
// Descriptor Done
```

## L109-115 · `const NUM_RX_DESC: usize = 32;`

```
// 32. Briefly bumped to 256 in v0.225.24 (to give the microvm RX producer more
// headroom — it drains this shared ring only every ~2.7 ms on a shared core).
// REVERTED in v0.225.28: it correlated with intermittent HOST OTA-download
// truncation ("size mismatch"), and it is the ONLY host-NIC change in that
// window. The host's own download path busy-spins the drain so it never needed
// the extra depth; the microvm's ring-depth need is better met by a faster
// producer cadence (dedicated core + spin), NOT by enlarging this shared HW ring.
```

## L120 · `#[repr(C)]`

```
/// Legacy RX Descriptor (16 bytes, for e1000)
```

## L132 · `#[repr(C)]`

```
/// Legacy TX Descriptor (16 bytes, for e1000)
```

## L169 · `static IS_IGC: AtomicBool = AtomicBool::new(false);`

```
/// The card is an I225/I226 and `igc` owns its rings.
```

## L180 · `pub fn init() -> bool {`

```
/// Detect and initialize Intel NIC.
```

## L182 · `let dev = KNOWN_IDS.iter().find_map(|&did| pci::find_device(INTEL_VENDOR, did));`

```
// Find by specific device IDs
```

## L185 · `let dev = dev.or_else(|| {`

```
// Fallback: find by class (02:00 = Ethernet, vendor Intel)
```

## L201 · `let cmd = pci::read16(dev.addr, 0x04);`

```
// Also enable memory space access
```

## L203 · `pci::write32(dev.addr, 0x04, (cmd | 0x06) as u32); // Bus Master + Memory Space`

```
// Bus Master + Memory Space
```

## L205 · `if dev.addr.bus > 0 {`

```
// Enable bus mastering on PCIe bridge if device is behind one
```

## L215 · `if pci::read8(ba, 0x0E) & 0x7F == 1 {`

```
// Header type 1 = PCI-PCI bridge
```

## L228 · `let bar0_raw = pci::read32(dev.addr, 0x10);`

```
// BAR0 (MMIO) — check if 32-bit or 64-bit
```

## L231 · `pci::read_bar64(dev.addr, 0x10)`

```
// 64-bit BAR
```

## L234 · `(bar0_raw & 0xFFFF_FFF0) as u64`

```
// 32-bit BAR
```

## L249 · `let map_size = 128 * 1024u64;`

```
// Map BAR0 (128KB for modern Intel NICs)
```

## L264-265 · `w32(mmio, IMC, 0xFFFF_FFFF);`

```
// Don't full-reset — UEFI firmware already configured the PHY.
// Just disable interrupts (we poll).
```

## L267 · `let _ = r32(mmio, ICR); // Clear pending`

```
// Clear pending
```

## L269 · `let ctrl = r32(mmio, CTRL);`

```
// Preserve UEFI link config, ensure link-up + auto-speed
```

## L273 · `let ral = r32(mmio, RAL);`

```
// Read MAC from RAL/RAH
```

## L285 · `let mac = if mac == [0; 6] {`

```
// If MAC is all zeros, try EEPROM
```

## L295 · `for i in 0..128 {`

```
// Clear multicast table
```

## L316 · `let rx_ring_size = NUM_RX_DESC * 16; // 16 bytes per desc`

```
// === Setup RX ===
```

## L317 · `let rx_ring_size = NUM_RX_DESC * 16; // 16 bytes per desc`

```
// 16 bytes per desc
```

## L325 · `let rx_buf_pages = (NUM_RX_DESC * RX_BUF_SIZE + 4095) / 4096;`

```
// Allocate RX buffers (NUM_RX_DESC * RX_BUF_SIZE)
```

## L337 · `for i in 0..NUM_RX_DESC {`

```
// Legacy e1000 RX init
```

## L353 · `let tx_ring_size = NUM_TX_DESC * 16;`

```
// === Setup TX ===
```

## L362 · `let tx_buf_pages = (NUM_TX_DESC * RX_BUF_SIZE + 4095) / 4096;`

```
// Allocate TX buffers (2KB aligned per buffer, not MTU)
```

## L381 · `kprintln!("[npk] intel-nic: RX descs={:#x} bufs={:#x}", rx_descs, rx_bufs);`

```
// Debug: show buffer addresses
```

## L395 · `fn wait_for_link(mmio: u64) {`

```
/// Wait for link up (max 3 seconds).
```

## L409 · `pub fn tso_capable() -> bool { IS_IGC.load(Ordering::Acquire) }`

```
/// TCPv4 segmentation offload — I225/I226 only.
```

## L417 · `pub fn rx_irq_vector() -> u8 {`

```
/// LAPIC vector of the RX interrupt; 0 for the polled e1000 path.
```

## L426-428 · `pub fn link_up() -> bool {`

```
/// Live link (carrier) state: reads STATUS.LU right now, so a pulled/plugged
/// cable is reflected immediately. Cheap MMIO read — but call from Core 0 (it
/// takes the DEVICE lock). False if the NIC isn't present.
```

## L454 · `let buf_addr = dev.tx_bufs + (i * RX_BUF_SIZE) as u64; // 2KB aligned`

```
// 2KB aligned
```

## L476 · `core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);`

```
// Write barrier: ensure descriptor is visible to NIC before tail bump
```

## L487 · `pub fn debug_stats() {`

```
/// Debug: print TX/RX packet counts
```

## L524 · `fn read_mac_eeprom(mmio: u64) -> [u8; 6] {`

```
/// Read MAC address from EEPROM (for NICs that don't expose it via RAL/RAH).
```

## L528 · `w32(mmio, EERD, (i << 8) | 1); // Start read at address i`

```
// Start read at address i
```

## L529 · `for _ in 0..10_000u32 {`

```
// Wait for done
```

## L532 · `if val & (1 << 4) != 0 { // Done bit`

```
// Done bit
```

