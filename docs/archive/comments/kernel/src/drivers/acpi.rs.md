# `kernel/src/drivers/acpi.rs` @ 5e0102684

## L1-3 · `use core::sync::atomic::{AtomicU16, Ordering};`

```
//! Minimal ACPI parser for power management
//!
//! Finds RSDP → RSDT/XSDT → FADT → PM1a_CNT_BLK port for S5 power-off.
```

## L7 · `static PM1A_CNT_PORT: AtomicU16 = AtomicU16::new(0);`

```
/// Cached PM1a control block I/O port (0 = not found yet)
```

## L9 · `static SLP_TYP_S5: AtomicU16 = AtomicU16::new(5);`

```
/// Cached SLP_TYPa value for S5 state (default 5, works on most Intel)
```

## L11 · `static RESET_PORT: AtomicU16 = AtomicU16::new(0);`

```
/// Cached ACPI reset register info (address_space, address, value)
```

## L15 · `static RSDP_ADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);`

```
/// Cached RSDP physical address (set by UEFI stub via `set_rsdp`).
```

## L18-19 · `pub fn set_rsdp(rsdp_phys: u64) {`

```
/// Stash the ACPI RSDP address handed to us by the UEFI stub. Called
/// from `kernel_main` before `init`.
```

## L24 · `pub fn init() {`

```
/// Initialize ACPI: find FADT and cache PM1a_CNT_BLK port.
```

## L40-43 · `pub fn enable_acpi_mode() {`

```
/// Take the platform out of legacy (SMM) mode into ACPI mode — ACPICA
/// `acpi_enable` → `acpi_hw_set_mode(ACPI_SYS_MODE_ACPI)`: if PM1_CNT.SCI_EN
/// is clear, write FADT.ACPI_ENABLE to FADT.SMI_CMD, then poll SCI_EN for up
/// to 3 s (30000 × 100 us). Called by `sci::arm_ec`.
```

## L49 · `let (smi_cmd, acpi_enable) = unsafe {`

```
// SAFETY: FADT mapped; SMI_CMD at 48 (u32), ACPI_ENABLE at 52 (u8).
```

## L55 · `(unsafe { crate::serial::inw(pm1a_cnt) } & 1) != 0`

```
// SAFETY: PM1a_CNT is the FADT's I/O port.
```

## L62-63 · `if smi_cmd == 0 || smi_cmd > 0xFFFF || acpi_enable == 0 {`

```
// ACPICA: no SMI_CMD, or no enable value, means the platform has no
// legacy mode to leave (hardware-reduced / ACPI-only).
```

## L68 · `unsafe { crate::serial::outb(smi_cmd as u16, acpi_enable) };`

```
// SAFETY: SMI_CMD is the FADT's I/O port for exactly this command.
```

## L85 · `pub fn reset() {`

```
/// Perform ACPI reset via FADT reset register.
```

## L95 · `pub fn power_off() {`

```
/// Perform ACPI S5 power-off.
```

## L101 · `let val: u16 = (slp_typ << 10) | (1 << 13); // SLP_TYPa | SLP_EN`

```
// SLP_TYPa | SLP_EN
```

## L108-109 · `pub fn find_table(sig: &[u8; 4]) -> Option<usize> {`

```
/// Find an ACPI table by 4-byte signature (e.g., b"APIC" for MADT).
/// Returns the physical address of the table header.
```

## L125-134 · `pub fn find_table_nth(sig: &[u8; 4], index: usize) -> Option<(usize, usize)> {`

```
/// Die `index`-te Tabelle mit dieser Signatur — samt Laenge.
///
/// **Es gibt mehr als eine SSDT.** Linux laedt die DSDT UND jede SSDT in
/// denselben Namespace (`acpi_tb_load_namespace`); wer nur die DSDT liest,
/// dem fehlen Namen, die andere Tabellen deklarieren. Auf Florians IdeaPad
/// haengt genau daran die Basis der Operationsregion, in der die
/// Freigabebits der I2C-Controller stehen: `FRTB` ist in der DSDT nicht
/// aufloesbar.
///
/// `find_table` gibt immer die ERSTE; hier laesst sich durchzaehlen.
```

## L169 · `pub fn ensure_mapped_pub(addr: usize, size: usize) {`

```
/// Public wrapper: ensure a physical address range is identity-mapped.
```

## L174-175 · `pub fn dsdt() -> Option<(usize, usize)> {`

```
/// Locate the DSDT (AML) via the FADT: DSDT pointer at offset 40 (32-bit)
/// or X_DSDT at 140 (64-bit). Returns (addr, len) with the region mapped.
```

## L193-195 · `pub fn is_mobile() -> bool {`

```
/// FADT "Preferred PM Profile" (offset 45): 2 = Mobile (laptop). Used to
/// gate the EC battery driver so desktops (NUC = profile 1) never probe for
/// a battery and show a phantom one.
```

## L204 · `fn ensure_mapped(addr: usize, size: usize) {`

```
/// Ensure a memory region is identity-mapped so we can read ACPI tables.
```

## L219 · `let revision = unsafe { *rsdp.add(15) };`

```
// RSDP: signature at 0, revision at 15, RSDT at 16, XSDT at 24 (if revision >= 2)
```

## L232 · `ensure_mapped(fadt_addr, 256);`

```
// Map FADT and read PM1a_CNT_BLK at offset 64
```

## L237-239 · `let fadt_len = unsafe { *((fadt_addr + 4) as *const u32) } as usize;`

```
// FADT offset 116: RESET_REG (Generic Address Structure)
// GAS: address_space(1) + bit_width(1) + bit_offset(1) + access_size(1) + address(8)
// FADT offset 128: RESET_VALUE (1 byte)
```

## L245 · `if reset_space == 1 && reset_addr > 0 && reset_addr <= 0xFFFF {`

```
// address_space 1 = System I/O
```

## L253 · `let dsdt_addr = unsafe { *((fadt_addr + 40) as *const u32) } as usize;`

```
// Try to read SLP_TYPa from DSDT \_S5 object
```

## L270 · `fn find_s5_slp_typ(dsdt_addr: usize, dsdt_len: usize) -> Option<u16> {`

```
/// Parse DSDT AML to find \_S5 object and extract SLP_TYPa value.
```

## L274 · `for i in 0..data.len().saturating_sub(20) {`

```
// Scan for "_S5_" (0x5F 0x53 0x35 0x5F)
```

## L277 · `if i == 0 || data[i - 1] != 0x08 { continue; }`

```
// Expected AML: NameOp(0x08) before _S5_, PackageOp(0x12) after
```

## L282 · `let mut pos = 1; // past PackageOp`

```
// Skip PackageOp + PkgLength
```

## L283 · `let mut pos = 1; // past PackageOp`

```
// past PackageOp
```

## L284 · `if pos >= rest.len() { continue; }`

```
// PkgLength encoding: if top 2 bits of first byte are 0, length is 1 byte
```

## L288 · `pos += 1; // 1-byte PkgLength`

```
// 1-byte PkgLength
```

## L294 · `if pos >= rest.len() { continue; }`

```
// Skip NumElements byte
```

## L298 · `if pos >= rest.len() { continue; }`

```
// Read SLP_TYPa: may be BytePrefix(0x0A) + byte, or raw byte, or WordPrefix, etc.
```

## L301 · `rest[pos + 1] as u16 // BytePrefix`

```
// BytePrefix
```

## L303 · `u16::from_le_bytes([rest[pos + 1], rest[pos + 2]]) // WordPrefix`

```
// WordPrefix
```

## L305 · `rest[pos] as u16 // Small integer (0-15 encoded directly in some AML variants)`

```
// Small integer (0-15 encoded directly in some AML variants)
```

## L316 · `fn find_rsdp() -> Option<*const u8> {`

```
/// Find RSDP: first from Multiboot2 tag, then scan legacy BIOS regions.
```

## L318 · `let mb_rsdp = RSDP_ADDR.load(core::sync::atomic::Ordering::Acquire);`

```
// Prefer Multiboot2-provided RSDP (works on UEFI systems)
```

## L328 · `let ebda_seg = unsafe { *(0x40E as *const u16) } as usize;`

```
// Fallback: scan legacy BIOS regions
```

## L348 · `let mut sum: u8 = 0;`

```
// Verify checksum (first 20 bytes sum to 0)
```

## L357 · `addr += 16; // RSDP is always 16-byte aligned`

```
// RSDP is always 16-byte aligned
```

