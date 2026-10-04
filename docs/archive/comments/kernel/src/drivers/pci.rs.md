# `kernel/src/drivers/pci.rs` @ 5e0102684

## L1-4 · `use crate::serial::{outl, inl};`

```
//! PCI Configuration Space Access
//!
//! Standard x86 PCI bus via I/O ports 0xCF8 (address) / 0xCFC (data).
//! Scans for VirtIO and other PCI devices.
```

## L30 · `unsafe {`

```
// SAFETY: PCI config space port I/O, standard x86 mechanism
```

## L38 · `unsafe {`

```
// SAFETY: PCI config space port I/O
```

## L65 · `pub fn find_device(vendor: u16, device: u16) -> Option<PciDevice> {`

```
/// Find first PCI device matching vendor + device ID
```

## L99-106 · `pub fn report_mass_storage() {`

```
/// Find first PCI device matching class + subclass
/// Print every PCI mass-storage controller (class 01h) with its subclass
/// and prog-if.
///
/// The storage drivers bind by EXACT class, so on a machine where none
/// matches, the installer halted with "No block device found" and nothing
/// to go on — and without a disk there is no npkFS, so `dmesg prev` cannot
/// answer it either. The subclass alone usually names the cause.
```

## L148-159 · `pub fn find_by_class_n(class: u8, subclass: u8, index: u32) -> Option<PciDevice> {`

```
/// The `index`-th device of this class, in PCI scan order.
///
/// Eine Klasse kann mehrfach besetzt sein, und welcher Treffer zuerst
/// kommt, ist Zufall der Busreihenfolge. Fast jede Maschine hat ZWEI
/// HD-Audio-Controller — den der GPU (HDMI/DP) und den der Southbridge
/// (Lautsprecher) —, und `find_by_class` gab immer den ersten. Damit lief
/// der Ton in einen DisplayPort, an dem nichts haengt.
///
/// Der Kernel entscheidet dabei NICHT, welcher der richtige ist: er reicht
/// den n-ten heraus, und welcher taugt, weiss nur der Treiber (hier: der
/// Codec mit einem analogen Ausgangspin). Dieselbe Trennung wie bei
/// `drivers::report` — der Kernel traegt, er urteilt nicht.
```

## L198 · `pub fn read_bar64(addr: PciAddr, bar_offset: u8) -> u64 {`

```
/// Read 64-bit BAR (BAR0 + BAR1 for 64-bit MMIO devices like NVMe)
```

## L202 · `(high << 32) | (low & 0xFFFF_FFF0)`

```
// Clear type/prefetch bits from low word
```

## L206 · `pub fn enable_bus_master(addr: PciAddr) {`

```
/// Enable PCI bus mastering (required for DMA)
```

## L212-213 · `fn find_bridge_for_bus(bus: u8) -> Option<PciAddr> {`

```
/// Die Bridge finden, hinter der `bus` haengt: Sekundaerbus <= bus <=
/// Subordinatbus. Erst Bus 0 (der Normalfall), dann der volle Durchgang.
```

## L245-257 · `pub fn enable_bus_master_path(addr: PciAddr) {`

```
/// Bus-Mastering auf JEDER Bridge zwischen `addr` und der Wurzel einschalten.
///
/// Eine PCI-Bridge leitet eine Transaktion von ihrem Sekundaerbus nur dann
/// nach oben weiter, wenn in IHREM Kommandoregister Bus Master gesetzt ist
/// (PCI-zu-PCI-Bridge-Spezifikation 1.2, §3.2.5.3). Steht es dort nicht,
/// schickt das Geraet seine Leseanfrage ab und bekommt einen Master Abort —
/// waehrend MMIO von der CPU nach unten tadellos funktioniert, weil das die
/// andere Richtung ist.
///
/// Das trifft genau die Geraete, die die Firmware NICHT selbst benutzt hat:
/// NVMe (Boot) und xHCI (Tastatur) kommen mit eingeschalteten Bridges aus
/// UEFI, eine WLAN-Karte nicht. Gefunden am RTL8822CE im IdeaPad, nachdem
/// drei andere Erklaerungen gemessen und verworfen waren.
```

## L260-261 · `for _ in 0..8 {`

```
// Acht Ebenen sind mehr, als eine reale Topologie tief wird; der Deckel
// ist gegen einen Ring in kaputten Bus-Nummern, nicht gegen Tiefe.
```

## L282-284 · `pub fn assign_bar_mmio(dev: PciAddr, bar_offset: u8) -> u64 {`

```
/// Assign an MMIO address to an unassigned BAR and configure the parent bridge.
/// `bar_offset` is the config space offset of the BAR (e.g. 0x18 for BAR2).
/// Returns the assigned physical address, or 0 on failure.
```

## L286 · `let cmd = read32(dev, 0x04);`

```
// ── Step 1: Probe BAR size ──────────────────────────────────
```

## L288 · `write32(dev, 0x04, cmd & !0x06); // disable memory + bus master`

```
// disable memory + bus master
```

## L308 · `write32(dev, bar_offset, saved_lo);`

```
// Restore and bail
```

## L315-316 · `static NEXT_ADDR: core::sync::atomic::AtomicU32 =`

```
// ── Step 2: Pick address (simple bump allocator) ────────────
// Use 0xFD000000 region — above typical TOLUD, below APIC/ECAM
```

## L322 · `addr = (addr + align - 1) & !(align - 1); // align up`

```
// align up
```

## L325 · `write32(dev, bar_offset, addr);`

```
// ── Step 3: Write BAR ───────────────────────────────────────
```

## L329 · `write32(dev, 0x04, cmd | 0x06);`

```
// Re-enable memory space + bus master
```

## L335 · `if dev.bus > 0 {`

```
// ── Step 4: Configure parent bridge memory window ───────────
```

## L343-344 · `fn configure_bridge_window(target_bus: u8, addr: u32, size: u32) {`

```
/// Find the PCIe root port (bridge on bus 0) that forwards to `target_bus`
/// and set its Memory Base/Limit to include `addr..addr+size`.
```

## L355 · `let hdr = read8(bridge, 0x0E) & 0x7F;`

```
// Check: is this a PCI-PCI bridge? (header type = 0x01)
```

## L362 · `let sec_bus = read8(bridge, 0x19);`

```
// Check: does this bridge's secondary bus match?
```

## L369-370 · `let base_val = ((addr >> 16) & 0xFFF0) as u16;`

```
// Found the bridge! Set Memory Base/Limit (offset 0x20)
// Format: bits [15:4] = address [31:20], granularity = 1MB
```

## L377 · `let bcmd = read32(bridge, 0x04);`

```
// Enable memory forwarding on the bridge
```

## L389 · `pub fn msix_enabled(addr: PciAddr) -> bool {`

```
/// Check if MSI-X is enabled (not just present) — affects legacy VirtIO config offset
```

## L404-412 · `pub fn program_msix(dev: PciAddr, entry: u16, vector: u8, dest_apic: u32) -> bool {`

```
/// Program MSI-X table `entry` of `dev` to deliver `vector` to the LAPIC of
/// `dest_apic` (physical destination, fixed delivery, edge), unmask that
/// entry, and enable MSI-X (+ clear the global function mask). Returns false
/// if the device has no MSI-X capability or `entry` is out of range.
///
/// MSI-X writes go straight to the LAPIC (message address
/// `0xFEE0_0000 | apic<<12`) — no PIC/IOAPIC involved, so the HP firmware
/// PIC-reinit SMI trap is irrelevant. The MSI-X table lives in a device BAR
/// (the cap's Table Offset/BIR); we write it via identity-mapped MMIO.
```

## L414 · `if read16(dev, 0x06) & (1 << 4) == 0 {`

```
// Walk the capability list for MSI-X (cap ID 0x11).
```

## L433 · `let ctrl_dword = read32(dev, cap);`

```
// Message Control (high 16 bits of the dword at `cap`): table size, enable.
```

## L436 · `let table_size = (msg_ctrl & 0x7FF) + 1; // encoded as N-1`

```
// encoded as N-1
```

## L443 · `let table = read32(dev, cap + 4);`

```
// Table Offset / BIR (cap + 4): low 3 bits = BAR index, rest = byte offset.
```

## L462-464 · `let page = entry_addr & !0xFFF;`

```
// Defensively identity-map the MSI-X table page NO_CACHE — the device's
// BAR may not be mapped if its driver only mapped a register window.
// Avoids a #PF on the writes below (AlreadyMapped is fine).
```

## L474 · `let msg_addr_lo: u32 = 0xFEE0_0000 | ((dest_apic & 0xFF) << 12);`

```
// MSI-X table entry: addr_lo, addr_hi, data, vector-control (bit0 = mask).
```

## L476-477 · `unsafe {`

```
// SAFETY: MSI-X table MMIO inside the device BAR (identity-mapped). The
// entry was bounds-checked against the table size above.
```

## L480 · `core::ptr::write_volatile(p.add(0), msg_addr_lo); // Message Address (low)`

```
// Message Address (low)
```

## L481 · `core::ptr::write_volatile(p.add(1), 0); // Message Address (high)`

```
// Message Address (high)
```

## L482 · `core::ptr::write_volatile(p.add(2), vector as u32); // Message Data = vector`

```
// Message Data = vector
```

## L483 · `core::ptr::write_volatile(p.add(3), 0); // Vector Control: unmasked`

```
// Vector Control: unmasked
```

## L486-487 · `let new_ctrl = (msg_ctrl | (1 << 15)) & !(1 << 14);`

```
// Enable MSI-X (bit 15) + clear global function mask (bit 14), preserving
// cap ID + next ptr in the low 16 bits of the dword.
```

## L494-498 · `pub fn msix_set_dest(dev: PciAddr, entry: u16, dest_apic: u32) {`

```
/// Re-point an already-programmed MSI-X table `entry` of `dev` to deliver to
/// LAPIC `dest_apic` — rewrites ONLY the message address (one MMIO write).
/// Lets the IRQ subsystem route a device's interrupt to whichever core is
/// about to wait on it (so the IRQ wakes the right core out of HLT). The entry
/// must already be programmed + the table page mapped (via `program_msix`).
```

## L529-530 · `unsafe { core::ptr::write_volatile(entry_addr as *mut u32, msg_addr_lo); }`

```
// SAFETY: MSI-X table MMIO (mapped when `program_msix` first ran). Writing
// the message-address low word re-points delivery; no enable/mask change.
```

## L534 · `fn find_cap(dev: PciAddr, id: u8) -> u8 {`

```
/// Offset of capability `id` in `dev`'s list, or 0 if it has none.
```

## L551 · `pub fn has_msix(dev: PciAddr) -> bool {`

```
/// Does `dev` have an MSI-X capability?
```

## L556-566 · `pub fn program_msi(dev: PciAddr, vector: u8, dest_apic: u32) -> bool {`

```
/// Program `dev`'s plain MSI capability (0x05) for ONE vector delivered to
/// LAPIC `dest_apic`, and enable it. For devices without MSI-X — the
/// RTL8822CE among them: rtw88 asks Linux for exactly one MSI vector
/// (`pci_alloc_irq_vectors(pdev, 1, 1, PCI_IRQ_MSI | PCI_IRQ_INTX)`).
///
/// Layout (PCI 3.0 §6.8.1): Message Control in the high half of the cap's
/// first dword (bit 0 enable, bits 6:4 multiple-message enable, bit 7 64-bit
/// address, bit 8 per-vector masking); address at +4, then either data at +8
/// (32-bit) or address-high at +8 and data at +0xC; mask bits after the data.
/// MSI is disabled while it is programmed, and INTx is switched off as
/// Linux' `pci_intx_for_msi` does.
```

## L578 · `let off = ctrl & !1 & !(0b111 << 4);`

```
// Disabled, one message (MME = 0) while the address/data are written.
```

## L589-590 · `write32(dev, data_off, vector as u32);`

```
// Message Data = vector (fixed delivery, edge). The upper half is
// extended data / reserved: 0.
```

## L597-598 · `let cmd = read32(dev, 0x04) & 0xFFFF;`

```
// INTx off (Command bit 10). Write only the command half: the status
// half above it is write-1-to-clear.
```

## L606-607 · `pub fn msi_set_dest(dev: PciAddr, dest_apic: u32) {`

```
/// Re-point `dev`'s plain MSI to LAPIC `dest_apic` — one config write of the
/// message address (the counterpart of `msix_set_dest`).
```

## L615 · `#[derive(Clone, Copy)]`

```
/// Live MSI-X state for diagnostics (read on demand, e.g. from `disk`).
```

## L619 · `pub ctrl: u16,       // Message Control: bit15 enable, bit14 function-mask, [10:0] size-1`

```
// Message Control: bit15 enable, bit14 function-mask, [10:0] size-1
```

## L624 · `pub pci_cmd: u16,    // PCI command reg (bit2 = bus master, needed to issue the MSI write)`

```
// PCI command reg (bit2 = bus master, needed to issue the MSI write)
```

## L625 · `pub e_addr: u32,     // table entry: message address low`

```
// table entry: message address low
```

## L626 · `pub e_data: u32,     // table entry: message data (= delivered vector)`

```
// table entry: message data (= delivered vector)
```

## L627 · `pub e_vctrl: u32,    // table entry: vector control (bit0 = masked)`

```
// table entry: vector control (bit0 = masked)
```

## L630-632 · `pub fn msix_debug(dev: PciAddr, entry: u16) -> Option<MsixDebug> {`

```
/// Re-read `dev`'s MSI-X capability + table `entry` live. None if no MSI-X.
/// Lets `disk` show whether our programming stuck (entry unmasked, enabled,
/// addr/data correct) without scrolling the boot log.
```

## L660 · `let (e_addr, e_data, e_vctrl) = unsafe {`

```
// SAFETY: MSI-X table MMIO (mapped at program_msix time), read-only here.
```

## L683 · `pub fn scan() -> u16 {`

```
/// Scan PCI bus, print all devices, return count
```

