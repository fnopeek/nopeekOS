# `kernel/src/mm/paging.rs` @ 5e0102684

## L1-5 · `#![allow(dead_code)]`

```
//! Virtual Memory Manager
//!
//! 4-level x86_64 paging: PML4 → PDPT → PDT → PT → 4KB page.
//! Works alongside boot.s 2MB identity mapping.
//! Preparation for WASM sandbox memory isolation.
```

## L24 · `const WRITE_THROUGH = 1 << 3;  // PWT — also selects PAT index bit 0`

```
// PWT — also selects PAT index bit 0
```

## L25 · `const NO_CACHE      = 1 << 4;  // PCD — also selects PAT index bit 1`

```
// PCD — also selects PAT index bit 1
```

## L28 · `const HUGE          = 1 << 7;  // PS bit (PDT/PDPT: huge page; PT: PAT index bit 2)`

```
// PS bit (PDT/PDPT: huge page; PT: PAT index bit 2)
```

## L32-34 · `const WRITE_COMBINE = (1 << 3) | (1 << 7);  // PWT + PAT`

```
/// Write-Combining: PAT index 5 = PWT(1) + PCD(0) + PAT(1)
/// PAT bit for 4KB PTEs is bit 7 (same position as HUGE, but used at PT level).
/// Requires PAT MSR to have WC at index 5.
```

## L35 · `const WRITE_COMBINE = (1 << 3) | (1 << 7);  // PWT + PAT`

```
// PWT + PAT
```

## L60 · `fn pml4_index(vaddr: u64) -> usize { ((vaddr >> 39) & 0x1FF) as usize }`

```
// === Address decomposition ===
```

## L70 · `fn read_cr3() -> u64 {`

```
// === CR3 / TLB ===
```

## L74 · `unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3); }`

```
// SAFETY: Reading CR3 is side-effect-free
```

## L80 · `unsafe { core::arch::asm!("invlpg [{}]", in(reg) vaddr); }`

```
// SAFETY: invlpg only invalidates the single TLB entry
```

## L84-85 · `fn flush_tlb_all() {`

```
/// Full TLB flush by reloading CR3. Required after splitting huge pages
/// because invlpg on a single address doesn't cover the entire old huge page.
```

## L87 · `unsafe {`

```
// SAFETY: Reloading CR3 with same value flushes all non-global TLB entries
```

## L95-98 · `unsafe fn split_1gb_to_2mb(pdpt: u64, index: usize) -> Result<u64, PagingError> {`

```
/// Split a 1GB huge page (PDPT entry) into 512 × 2MB huge pages (PDT).
/// Preserves the original identity mapping with the same base flags.
/// Returns the physical address of the new PDT.
/// SAFETY: pdpt must be valid, index must point to a 1GB huge page entry.
```

## L102 · `let huge_base = entry_addr(entry); // 1GB-aligned physical base`

```
// 1GB-aligned physical base
```

## L120-123 · `unsafe fn split_2mb_to_4kb(pdt: u64, index: usize) -> Result<u64, PagingError> {`

```
/// Split a 2MB huge page (PDT entry) into 512 × 4KB pages (PT).
/// Preserves the original identity mapping with the same base flags.
/// Returns the physical address of the new PT.
/// SAFETY: pdt must be valid, index must point to a 2MB huge page entry.
```

## L128 · `let base_flags = entry & 0x67;`

```
// bit 7 at PT level is PAT, not HUGE — strip it from base flags
```

## L146 · `unsafe fn read_entry(table_phys: u64, index: usize) -> u64 {`

```
// === Table access (identity-mapped) ===
```

## L148 · `unsafe fn read_entry(table_phys: u64, index: usize) -> u64 {`

```
/// Read a page table entry. SAFETY: table_phys must be in identity-mapped range.
```

## L154 · `unsafe fn write_entry(table_phys: u64, index: usize, value: u64) {`

```
/// Write a page table entry. SAFETY: table_phys must be in identity-mapped range.
```

## L160 · `unsafe fn zero_frame(addr: u64) {`

```
/// Zero a freshly allocated 4KB frame for use as a page table.
```

## L165 · `unsafe fn get_or_create(table_phys: u64, index: usize) -> Result<u64, PagingError> {`

```
/// Walk to the next table level. If not present, allocate a new table.
```

## L182 · `pub fn init() {`

```
// === Public API ===
```

## L185 · `unsafe {`

```
// Enable NXE (bit 11) in EFER MSR so NO_EXECUTE flag works in page tables.
```

## L198 · `unsafe {`

```
// Program PAT MSR (0x277) to add Write-Combining on index 5.
```

## L208-213 · `let pml4_phys = memory::allocate_frame()`

```
// Build our own page tables. UEFI handed us long mode running on
// its own PML4 — those tables are in firmware-owned memory marked
// read-only, so we can't add new entries to them later (e.g. for
// MMIO MMIO regions or WASM module memory). We allocate fresh
// tables from our frame allocator and identity-map 64 GB via
// 1 GB huge pages (matching what the old multiboot2 boot.s did).
```

## L219 · `core::ptr::write_bytes(pml4_phys as *mut u8, 0, 4096);`

```
// Zero both tables.
```

## L223 · `let pml4 = pml4_phys as *mut u64;`

```
// PML4[0] → PDPT, Present + Writable.
```

## L227-228 · `let pdpt = pdpt_phys as *mut u64;`

```
// PDPT: 64 entries, each a 1 GB huge page identity mapping.
// 0x83 = Present (bit 0) + Writable (bit 1) + Huge (bit 7).
```

## L236-238 · `unsafe {`

```
// Switch CR3 to our new tables. Any TLB entries from UEFI's
// mapping get flushed. From here on, MMIO mappings can be added
// freely (map_page will modify our own tables, which are writable).
```

## L246-249 · `pub const IDENTITY_LIMIT: u64 = 64 * 1024 * 1024 * 1024;`

```
/// Map a 4KB virtual page to a physical frame.
/// Automatically splits 1GB and 2MB huge pages when a different mapping
/// (e.g. NO_CACHE for MMIO) is needed at 4KB granularity.
/// Obergrenze der Identitaetsabbildung (siehe `init`: 64 GB).
```

## L252-263 · `pub fn read_phys_u8(addr: u64) -> Option<u8> {`

```
/// Ein Byte an einer physischen Adresse lesen.
///
/// Der Kernel bildet die ersten 64 GB identisch ab, physisch ist hier also
/// gleich virtuell. Gedacht fuer FIRMWARE-Fenster (ACPI-SystemMemory-
/// Regionen); der Rufer muss vorher geprueft haben, dass die Adresse KEIN
/// Arbeitsspeicher ist — diese Funktion prueft das nicht, sie prueft nur
/// die Abbildungsgrenze.
///
/// Gelesen wird ueber die gewoehnliche Abbildung, also gecacht. Fuer ein
/// Statusbyte der Firmware ist das in Ordnung; ein Register, das sich ohne
/// unser Zutun aendert, braucht eine UC-Abbildung, und das ist hier
/// benannt und nicht gebaut.
```

## L266-267 · `Some(unsafe { core::ptr::read_volatile(addr as *const u8) })`

```
// SAFETY: innerhalb der Identitaetsabbildung, ein einzelnes Byte,
// ausschliesslich lesend.
```

## L271-280 · `static PT_LOCK: spin::Mutex<()> = spin::Mutex::new(());`

```
/// Serialises every change to the page tables.
///
/// All cores share one set of tables, and forge maps from whichever worker
/// runs the module (`memory.grow`, `Code::map`). `get_or_create` reads an
/// empty slot, allocates a table and writes it — two cores doing that on the
/// same slot each install their own table, and the mappings made through the
/// loser vanish. Code of different modules shares page tables (the code
/// region grows linearly), so this was not a theoretical overlap.
///
/// Never taken from interrupt context; nothing under it maps again.
```

## L291 · `unsafe {`

```
// SAFETY: All table accesses are within identity-mapped range
```

## L296 · `if pdpt_entry & PageFlags::PRESENT.bits() != 0 && pdpt_entry & PageFlags::HUGE.bits() != 0 {`

```
// 1GB huge page covers this address — split into 512 × 2MB pages
```

## L299 · `flush_tlb_all(); // full TLB flush after huge page split`

```
// full TLB flush after huge page split
```

## L305 · `if pdt_entry & PageFlags::PRESENT.bits() != 0 && pdt_entry & PageFlags::HUGE.bits() != 0 {`

```
// 2MB huge page covers this address — split into 512 × 4KB pages
```

## L308 · `flush_tlb_all(); // full TLB flush after huge page split`

```
// full TLB flush after huge page split
```

## L315 · `let old_paddr = entry_addr(pt_entry);`

```
// Page already mapped at 4KB level — update flags if different
```

## L318 · `write_entry(pt, pt_index(vaddr), paddr | flags.bits());`

```
// Same physical address — just update flags (e.g. add NO_CACHE)
```

## L333 · `pub fn unmap_page(vaddr: u64) -> Result<u64, PagingError> {`

```
/// Unmap a 4KB page. Returns the physical address that was mapped.
```

## L342 · `unsafe {`

```
// SAFETY: All table accesses within identity-mapped range
```

## L368 · `pub fn translate(vaddr: u64) -> Option<u64> {`

```
/// Translate a virtual address to physical (handles both 2MB and 4KB pages)
```

## L372 · `unsafe {`

```
// SAFETY: Read-only table walk within identity-mapped range
```

## L380 · `return Some(entry_addr(pdpt_e) + (vaddr & 0x3FFF_FFFF)); // 1GB page`

```
// 1GB page
```

## L386 · `return Some(entry_addr(pdt_e) + (vaddr & 0x1F_FFFF)); // 2MB page`

```
// 2MB page
```

## L391 · `Some(entry_addr(pt_e) + (vaddr & 0xFFF)) // 4KB page`

```
// 4KB page
```

## L395-397 · `pub fn leaf_entry(vaddr: u64) -> Option<(u64, u8)> {`

```
/// Return the raw leaf paging entry (with flag bits) mapping `vaddr`, plus
/// the page level: 1 = 1 GB, 2 = 2 MB, 3 = 4 KB. `None` if not present.
/// Read-only — lets a diagnostic decode the PAT/PCD/PWT memory-type bits.
```

## L400 · `unsafe {`

```
// SAFETY: read-only table walk within identity-mapped range.
```

## L416 · `fn count_mappings(pml4: u64) -> (usize, usize) {`

```
/// Count mapped pages: (huge_2mb, small_4kb)
```

## L422 · `let pml4_e = unsafe { read_entry(pml4, i) };`

```
// SAFETY: PML4 is in identity-mapped range
```

## L430 · `if pdpt_e & PageFlags::HUGE.bits() != 0 { huge += 512; continue; } // 1GB = 512 x 2MB`

```
// 1GB = 512 x 2MB
```

## L450 · `pub fn stats() -> (usize, usize) {`

```
/// Stats for status intent: (huge_2mb_count, small_4kb_count)
```

