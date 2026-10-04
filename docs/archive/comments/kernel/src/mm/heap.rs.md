# `kernel/src/mm/heap.rs` @ 5e0102684

## L1-37 · `use core::alloc::{GlobalAlloc, Layout};`

```
//! Heap Allocator
//!
//! Groessenklassen-Freilisten mit Grenzmarken (boundary tags). Belegen und
//! Freigeben sind O(1); wachsen tut der Heap weiter in 64-MB-Stuecken.
//!
//! # Warum nicht mehr First-Fit
//!
//! Der Vorgaenger hielt EINE adresssortierte, einfach verkettete Liste.
//! Beides war O(n): `try_allocate` suchte vom Kopf her den ersten passenden
//! Block, `insert_free_block` lief bis zur Einfuegestelle. Gemessen an einem
//! `forge python`-Lauf (0.313.2, die Zaehler stehen unten und sind geblieben):
//!
//!   313 968 allocs / 313 982 frees
//!   Schritte: 181 931 481 beim Belegen + 181 835 409 beim Freigeben
//!   Freiliste stabil bei ~1400 Knoten
//!
//! Das sind **579 besuchte Knoten je Operation, auf beiden Seiten** — 363
//! Millionen Zeigerschritte fuer einen Lauf. Die Liste waechst dabei nicht;
//! sie ist nur lang. Deshalb reichten Groessenklassen allein NICHT: sie
//! haetten das Belegen geheilt und das Freigeben unangetastet gelassen.
//!
//! Sichtbar wurde es erst im SKALIEREN, nicht in einer Zeit: derselbe
//! Compilerlauf waechst auf dem Entwicklungsrechner linear mit der
//! Ausgabegroesse (beak -> python: 4,3x bei 4,39x Code), am Geraet mit 10,0x.
//! Eine konstante Verlangsamung kann keine Kurve kruemmen.
//!
//! # Aufbau
//!
//! Jeder Block traegt vorn eine Marke (Groesse, Bit 0 = frei) und hinten eine
//! Wiederholung der Groesse. Damit findet das Freigeben seine physischen
//! Nachbarn in O(1), ohne die Liste zu durchlaufen. Freie Bloecke haengen
//! doppelt verkettet in der Liste ihrer Groessenklasse, damit das Verschmelzen
//! einen Nachbarn in O(1) aushaengen kann.
//!
//! An beiden Enden jeder Region steht ein Scheinblock, der nie frei ist. So
//! braucht kein Nachbarschaftstest eine Bereichspruefung — das Verschmelzen
//! laeuft von selbst nicht ueber die Region hinaus.
```

## L44 · `const INITIAL_HEAP: usize = 64 * 1024 * 1024;       // 64MB initial`

```
// 64MB initial
```

## L45 · `const GROW_CHUNK: usize = 64 * 1024 * 1024;          // 64MB growth increments`

```
// 64MB growth increments
```

## L46 · `const MAX_HEAP: usize = 2 * 1024 * 1024 * 1024;      // 2GB ceiling`

```
// 2GB ceiling
```

## L50 · `const TAG: usize = 8;`

```
/// Marke vorn, Wiederholung hinten — je acht Bytes.
```

## L52 · `const FREE_BIT: usize = 1;`

```
/// Bit 0 der vorderen Marke. Groessen sind auf 16 ausgerichtet, also ist es frei.
```

## L54 · `const SENTINEL: usize = 16;`

```
/// Scheinblock an jedem Regionenende. Nie frei, nur Anschlag.
```

## L58 · `const MIN_BLOCK_SIZE: usize = 32;`

```
/// Marke + zwei Zeiger + Wiederholung.
```

## L61-64 · `#[repr(C)]`

```
/// Steht unmittelbar vor den Nutzdaten und findet den Blockanfang wieder.
/// Bleibt aus dem Vorgaenger uebernommen: die Ausrichtung kann die Nutzdaten
/// beliebig weit hinter den Blockanfang schieben, und diese beiden Zahlen sind
/// der einzige Weg zurueck.
```

## L71 · `#[repr(C)]`

```
/// Die beiden Zeiger eines freien Blocks, direkt hinter seiner Marke.
```

## L78-92 · `const NBINS: usize = 52;`

```
/// Bis 512 ein Kasten je 16 Bytes, darueber je eine Zweierpotenz.
///
/// **Die 16 sind kein Geschmack, sie sind die Ausrichtung.** Blockgroessen
/// sind Vielfache von `BLOCK_ALIGN` (16), also haelt jeder Kasten unterhalb
/// von 512 GENAU EINE Groesse — der erste Block darin passt immer, und die
/// Suche ist O(1).
///
/// Mit 32-Byte-Kaesten (0.314.x) hielt ein Kasten ZWEI Groessen, und wer die
/// groessere brauchte, lief an allen kleineren vorbei. Am Geraet gemessen:
/// 5,5 bis 32,2 besuchte Knoten je Belegung, je nach Zustand der Freiliste,
/// und die Uebersetzungszeit folgte dem monoton (150 ms bei 5,5 Schritten,
/// 190 ms bei 32,2).
///
/// Ueber 512 bleibt die Spanne, also auch die Suche — das sind aber nur ~8 %
/// der Anforderungen.
```

## L110-112 · `#[derive(Clone, Copy)]`

```
/// Zaehler. Reine Felder, kein Format, keine Ausgabe: hier drin darf nichts
/// allozieren. Sie bleiben nach dem Umbau, weil sie der Abnahmetest sind —
/// `Schritte je alloc` muss von 579 auf etwa 1 fallen.
```

## L122 · `pub size_hist: [u64; 16],`

```
/// Anforderungen je Zweierpotenz: [0] < 32 B, [1] < 64 B, ... [15] >= 512 KB
```

## L128 · `regions: [(usize, usize); MAX_REGIONS], // (start, end) of each chunk`

```
// (start, end) of each chunk
```

## L137 · `#[inline]`

```
// ── Marken ────────────────────────────────────────────────────────────
```

## L141 · `unsafe { *(block as *const usize) }`

```
// SAFETY: `block` ist ein Blockanfang innerhalb einer Region.
```

## L147 · `unsafe { tag_of(block) & !FREE_BIT }`

```
// SAFETY: wie `tag_of`.
```

## L153 · `unsafe { tag_of(block) & FREE_BIT != 0 }`

```
// SAFETY: wie `tag_of`.
```

## L157-159 · `#[inline]`

```
/// Marke vorn und Wiederholung hinten in einem Zug setzen. Beide muessen
/// immer uebereinstimmen — die hintere ist der einzige Weg, den VORGAENGER
/// eines Blocks zu finden.
```

## L162 · `unsafe {`

```
// SAFETY: `block..block+size` liegt in einer Region.
```

## L169 · `#[inline]`

```
/// Groesse des physischen Vorgaengers, aus dessen hinterer Wiederholung.
```

## L172-173 · `unsafe { *((block - TAG) as *const usize) }`

```
// SAFETY: vor jedem Block steht entweder ein Block oder ein Scheinblock,
// beide mit gueltiger hinterer Marke.
```

## L192 · `unsafe fn bin_push(&mut self, block: usize, size: usize) {`

```
/// Einen freien Block vorn in seinen Kasten haengen. O(1).
```

## L196-197 · `unsafe {`

```
// SAFETY: `block` ist frei und mindestens MIN_BLOCK_SIZE gross, also
// liegen beide Zeiger im Block.
```

## L212-213 · `unsafe fn bin_remove(&mut self, block: usize, size: usize) {`

```
/// Und wieder heraus, ohne Suche — das ist der Grund fuer die doppelte
/// Verkettung: beim Verschmelzen haengt ein NACHBAR aus, nicht der Kopf.
```

## L217 · `unsafe {`

```
// SAFETY: `node` haengt in genau diesem Kasten.
```

## L232 · `unsafe { self.lay_out_region(start, size) };`

```
// SAFETY: die Region ist reserviert und mindestens 64 MB gross.
```

## L236-238 · `unsafe fn lay_out_region(&mut self, start: usize, size: usize) {`

```
/// Scheinblock, Nutzblock, Scheinblock. Die beiden Anschlaege sind nie
/// frei, also endet jedes Verschmelzen von selbst an der Regionengrenze —
/// ohne dass ein Nachbarschaftstest die Bereiche durchsuchen muesste.
```

## L241 · `unsafe {`

```
// SAFETY: der Aufrufer haelt eine Region dieser Groesse.
```

## L250 · `fn contains(&self, addr: usize) -> bool {`

```
/// Check if an address falls within any known heap region.
```

## L271-272 · `#[inline]`

```
/// Was der Block mindestens messen muss, damit `size` Bytes mit `align`
/// hineinpassen — samt Marke, Kopf und hinterer Wiederholung.
```

## L290-295 · `let data_off = align_up(TAG + HEADER_SIZE, align);`

```
// Die untere Schranke MUSS die Ausrichtung schon enthalten, sonst
// faellt die Kastenwahl eine Klasse zu tief und die Suche laeuft an
// allen zu kleinen Bloecken dieses Kastens vorbei. Genau das kostete
// in 0.314.0 noch 70,4 Schritte je Belegung statt einem:
// `TAG + HEADER_SIZE` sind 24, ein 16-ausgerichteter Blockanfang
// schiebt die Nutzdaten aber auf 32.
```

## L306-307 · `let bsize = unsafe { size_of_block(block) };`

```
// SAFETY: `node` haengt in der Freiliste, also ist `block` ein
// freier Block mit gueltigen Marken.
```

## L311 · `unsafe { self.bin_remove(block, bsize) };`

```
// SAFETY: `block` ist frei und gross genug.
```

## L315 · `unsafe {`

```
// SAFETY: beide Teile liegen im urspruenglichen Block.
```

## L323 · `unsafe { set_tags(block, bsize, false) };`

```
// SAFETY: wie oben.
```

## L328-329 · `unsafe {`

```
// SAFETY: `data - HEADER_SIZE` liegt hinter der Marke und
// vor den Nutzdaten desselben Blocks.
```

## L337 · `node = unsafe { (*node).next };`

```
// SAFETY: `node` ist ein gueltiger Listenknoten.
```

## L344 · `fn grow(&mut self, min_size: usize) -> bool {`

```
/// Grow heap by requesting contiguous frames from the physical memory manager.
```

## L352-355 · `if let Some(base) = crate::memory::allocate_contiguous(frames) {`

```
// SAFETY: memory::allocate_contiguous uses its own lock (memory::ALLOCATOR),
// independent of the heap lock we're holding. No deadlock possible.
// NOTE: no kprintln here — we're inside GlobalAlloc::alloc,
// and kprintln can allocate (capture_bytes → String::push_str) → deadlock.
```

## L365 · `unsafe { self.lay_out_region(start, size) };`

```
// SAFETY: die Region wurde gerade zugeteilt und gehoert uns.
```

## L378-379 · `let header = unsafe { &*((data_addr - HEADER_SIZE) as *const AllocHeader) };`

```
// SAFETY: `data_addr` kam aus `try_allocate`, also steht der Kopf
// unmittelbar davor.
```

## L389-391 · `unsafe {`

```
// Nach vorn verschmelzen. Der Anschlag am Regionenende ist nie frei,
// also endet das hier von selbst.
// SAFETY: `block + size` ist ein Blockanfang oder der Anschlag.
```

## L400 · `let ps = prev_size(block);`

```
// Und nach hinten, ueber die hintere Marke des Vorgaengers.
```

## L449-450 · `pub fn counters() -> HeapCounters {`

```
/// Die Zaehler herausholen. Kopie, damit der Aufrufer drucken kann, ohne den
/// Heap-Lock zu halten — kprintln alloziert.
```

## L459 · `h.c.max_free_nodes = h.c.free_nodes;`

```
// free_nodes NICHT zuruecksetzen: das ist ein Zustand, keine Zaehlung.
```

