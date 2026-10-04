# `kernel/src/mm/memory.rs` @ 5e0102684

## L1-6 · `use spin::Mutex;`

```
//! Physical Memory Manager
//!
//! Bitmap frame allocator for 4KB pages.
//! Parses the Multiboot2 memory map to find available RAM.
//! Principle: deny by default — everything is "used" until the
//! memory map explicitly says a region is available.
```

## L13 · `const MAX_MEMORY: usize = 64 * 1024 * 1024 * 1024; // 64GB (identity-mapped via 1GB huge pages)`

```
// 64GB (identity-mapped via 1GB huge pages)
```

## L15 · `const BITMAP_SIZE: usize = MAX_FRAMES / 8; // 32KB`

```
// 32KB
```

## L20 · `bitmap: [u8; BITMAP_SIZE],`

```
/// Bit=1 → used/reserved, Bit=0 → free
```

## L56 · `let start = ((base as usize) + PAGE_SIZE - 1) / PAGE_SIZE; // round up`

```
// round up
```

## L57 · `let end = ((base + length) as usize) / PAGE_SIZE;          // round down`

```
// round down
```

## L66 · `let start = (base as usize) / PAGE_SIZE;                            // round down (conservative)`

```
// round down (conservative)
```

## L67 · `let end = ((base + length) as usize + PAGE_SIZE - 1) / PAGE_SIZE;   // round up (conservative)`

```
// round up (conservative)
```

## L100-105 · `static RAM_RANGES: Mutex<([(u64, u64); 64], usize)> = Mutex::new(([(0, 0); 64], 0));`

```
/// Die Bereiche, die die Firmware als NUTZBAREN Arbeitsspeicher gemeldet
/// hat — Kernel, Halde und die linearen Speicher der Module liegen darin.
///
/// Gebraucht wird das fuer genau eine Frage: darf ein Modul diese physische
/// Adresse lesen? Alles, was hier drinliegt, ist Arbeitsspeicher und damit
/// TABU; was draussen liegt, ist Firmware- oder Geraetefenster.
```

## L108-112 · `pub fn is_usable_ram(addr: u64) -> bool {`

```
/// Liegt `addr` in einem als nutzbar gemeldeten RAM-Bereich?
///
/// Konservativ: kennen wir die Karte nicht (Zahl 0), gilt ALLES als RAM und
/// damit als tabu. Lieber eine Auskunft verweigern als eine geben, die den
/// Sandkasten oeffnet.
```

## L124-128 · `for region in boot_info.usable_regions() {`

```
// Walk the UEFI memory map. Conventional + BootServices Code/Data
// are usable RAM (BootServices regions become ours after
// ExitBootServices). LoaderCode/Data is our PE image and the
// stack/heap UEFI allocated for us — leave those marked used so
// we don't overwrite running code.
```

## L135-138 · `alloc.mark_region_used(0, 0x100000);`

```
// First 1 MB on x86 is special — BIOS-compat MMIO holes (VGA at
// 0xA0000, ROM at 0xC0000) live there even under UEFI. Carve it
// out as a defence-in-depth guard; UEFI's map usually already
// omits this range but firmware quirks happen.
```

## L141-144 · `{`

```
// The kernel image itself is the EfiLoaderCode/EfiLoaderData
// region in the UEFI map — already left as "used" by the loop
// above. The boot_info struct lives in BSS (= part of the kernel
// image), so no separate reservation needed.
```

## L146 · `{`

```
// Die RAM-Karte merken, solange wir sie in der Hand haben.
```

## L180-182 · `pub fn allocate_contiguous_below(count: usize, limit_bytes: u64) -> Option<u64> {`

```
/// Allocate `count` contiguous physical frames. Returns base physical address.
/// Allocate contiguous frames below a physical address limit.
/// `limit_bytes` = 0 means no limit (use all memory).
```

## L229-230 · `let mut start = top - count;`

```
// Search from top of memory downward — high RAM is almost always free,
// avoids O(n*count) scan through busy low memory regions.
```

## L239 · `ok = false;`

```
// Frame is used — skip past it
```

## L261 · `pub fn deallocate_contiguous(base: u64, count: usize) {`

```
/// Deallocate `count` contiguous physical frames starting at `base`.
```

