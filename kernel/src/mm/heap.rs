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

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use spin::Mutex;
use crate::kprintln;

const INITIAL_HEAP: usize = 64 * 1024 * 1024;       // 64MB initial
const GROW_CHUNK: usize = 64 * 1024 * 1024;          // 64MB growth increments
const MAX_HEAP: usize = 2 * 1024 * 1024 * 1024;      // 2GB ceiling
const MAX_REGIONS: usize = 32;
const BLOCK_ALIGN: usize = 16;

/// Marke vorn, Wiederholung hinten — je acht Bytes.
const TAG: usize = 8;
/// Bit 0 der vorderen Marke. Groessen sind auf 16 ausgerichtet, also ist es frei.
const FREE_BIT: usize = 1;
/// Scheinblock an jedem Regionenende. Nie frei, nur Anschlag.
const SENTINEL: usize = 16;

const HEADER_SIZE: usize = core::mem::size_of::<AllocHeader>();
/// Marke + zwei Zeiger + Wiederholung.
const MIN_BLOCK_SIZE: usize = 32;

/// Steht unmittelbar vor den Nutzdaten und findet den Blockanfang wieder.
/// Bleibt aus dem Vorgaenger uebernommen: die Ausrichtung kann die Nutzdaten
/// beliebig weit hinter den Blockanfang schieben, und diese beiden Zahlen sind
/// der einzige Weg zurueck.
#[repr(C)]
struct AllocHeader {
    block_start: usize,
    block_size: usize,
}

/// Die beiden Zeiger eines freien Blocks, direkt hinter seiner Marke.
#[repr(C)]
struct FreeNode {
    prev: *mut FreeNode,
    next: *mut FreeNode,
}

/// 0..15 in 32-Byte-Schritten bis 512, danach je eine Zweierpotenz. Der
/// gesuchte Kasten wird aus der UNTEREN Schranke des Bedarfs bestimmt, also
/// sitzt der erste Treffer fast immer; nachgerechnet wird trotzdem je Block,
/// weil die Ausrichtung den Bedarf um bis zu 15 Bytes anhebt.
const NBINS: usize = 28;

#[inline]
fn bin_of(size: usize) -> usize {
    if size < 512 {
        size / 32
    } else {
        let mut b = 16;
        let mut s = 512usize;
        while b < NBINS - 1 && s * 2 <= size {
            s *= 2;
            b += 1;
        }
        b
    }
}

/// Zaehler. Reine Felder, kein Format, keine Ausgabe: hier drin darf nichts
/// allozieren. Sie bleiben nach dem Umbau, weil sie der Abnahmetest sind —
/// `Schritte je alloc` muss von 579 auf etwa 1 fallen.
#[derive(Clone, Copy)]
pub struct HeapCounters {
    pub allocs: u64,
    pub frees: u64,
    pub alloc_steps: u64,
    pub free_steps: u64,
    pub free_nodes: u64,
    pub max_free_nodes: u64,
    pub grows: u64,
    /// Anforderungen je Zweierpotenz: [0] < 32 B, [1] < 64 B, ... [15] >= 512 KB
    pub size_hist: [u64; 16],
}

struct Heap {
    bins: [*mut FreeNode; NBINS],
    regions: [(usize, usize); MAX_REGIONS], // (start, end) of each chunk
    region_count: usize,
    total_size: usize,
    allocated_bytes: usize,
    c: HeapCounters,
}

unsafe impl Send for Heap {}

// ── Marken ────────────────────────────────────────────────────────────

#[inline]
unsafe fn tag_of(block: usize) -> usize {
    // SAFETY: `block` ist ein Blockanfang innerhalb einer Region.
    unsafe { *(block as *const usize) }
}

#[inline]
unsafe fn size_of_block(block: usize) -> usize {
    // SAFETY: wie `tag_of`.
    unsafe { tag_of(block) & !FREE_BIT }
}

#[inline]
unsafe fn is_free(block: usize) -> bool {
    // SAFETY: wie `tag_of`.
    unsafe { tag_of(block) & FREE_BIT != 0 }
}

/// Marke vorn und Wiederholung hinten in einem Zug setzen. Beide muessen
/// immer uebereinstimmen — die hintere ist der einzige Weg, den VORGAENGER
/// eines Blocks zu finden.
#[inline]
unsafe fn set_tags(block: usize, size: usize, free: bool) {
    // SAFETY: `block..block+size` liegt in einer Region.
    unsafe {
        *(block as *mut usize) = size | if free { FREE_BIT } else { 0 };
        *((block + size - TAG) as *mut usize) = size;
    }
}

/// Groesse des physischen Vorgaengers, aus dessen hinterer Wiederholung.
#[inline]
unsafe fn prev_size(block: usize) -> usize {
    // SAFETY: vor jedem Block steht entweder ein Block oder ein Scheinblock,
    // beide mit gueltiger hinterer Marke.
    unsafe { *((block - TAG) as *const usize) }
}

impl Heap {
    const fn empty() -> Self {
        Heap {
            bins: [ptr::null_mut(); NBINS],
            regions: [(0, 0); MAX_REGIONS],
            region_count: 0,
            total_size: 0,
            allocated_bytes: 0,
            c: HeapCounters {
                allocs: 0, frees: 0, alloc_steps: 0, free_steps: 0,
                free_nodes: 0, max_free_nodes: 0, grows: 0, size_hist: [0; 16],
            },
        }
    }

    /// Einen freien Block vorn in seinen Kasten haengen. O(1).
    unsafe fn bin_push(&mut self, block: usize, size: usize) {
        let b = bin_of(size);
        let node = (block + TAG) as *mut FreeNode;
        // SAFETY: `block` ist frei und mindestens MIN_BLOCK_SIZE gross, also
        // liegen beide Zeiger im Block.
        unsafe {
            (*node).prev = ptr::null_mut();
            (*node).next = self.bins[b];
            if !self.bins[b].is_null() {
                (*self.bins[b]).prev = node;
            }
        }
        self.bins[b] = node;
        self.c.free_nodes += 1;
        if self.c.free_nodes > self.c.max_free_nodes {
            self.c.max_free_nodes = self.c.free_nodes;
        }
    }

    /// Und wieder heraus, ohne Suche — das ist der Grund fuer die doppelte
    /// Verkettung: beim Verschmelzen haengt ein NACHBAR aus, nicht der Kopf.
    unsafe fn bin_remove(&mut self, block: usize, size: usize) {
        let b = bin_of(size);
        let node = (block + TAG) as *mut FreeNode;
        // SAFETY: `node` haengt in genau diesem Kasten.
        unsafe {
            let p = (*node).prev;
            let n = (*node).next;
            if p.is_null() { self.bins[b] = n; } else { (*p).next = n; }
            if !n.is_null() { (*n).prev = p; }
        }
        self.c.free_nodes = self.c.free_nodes.saturating_sub(1);
    }

    fn init(&mut self, start: usize, size: usize) {
        self.regions[0] = (start, start + size);
        self.region_count = 1;
        self.total_size = size;
        self.allocated_bytes = 0;
        // SAFETY: die Region ist reserviert und mindestens 64 MB gross.
        unsafe { self.lay_out_region(start, size) };
    }

    /// Scheinblock, Nutzblock, Scheinblock. Die beiden Anschlaege sind nie
    /// frei, also endet jedes Verschmelzen von selbst an der Regionengrenze —
    /// ohne dass ein Nachbarschaftstest die Bereiche durchsuchen muesste.
    unsafe fn lay_out_region(&mut self, start: usize, size: usize) {
        let body = size - 2 * SENTINEL;
        // SAFETY: der Aufrufer haelt eine Region dieser Groesse.
        unsafe {
            set_tags(start, SENTINEL, false);
            set_tags(start + SENTINEL + body, SENTINEL, false);
            set_tags(start + SENTINEL, body, true);
            self.bin_push(start + SENTINEL, body);
        }
    }

    /// Check if an address falls within any known heap region.
    fn contains(&self, addr: usize) -> bool {
        for i in 0..self.region_count {
            let (start, end) = self.regions[i];
            if addr >= start && addr < end { return true; }
        }
        false
    }

    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        let result = self.try_allocate(&layout);
        if !result.is_null() { return result; }

        let needed = layout.size() + TAG + HEADER_SIZE + layout.align() + TAG;
        if self.grow(needed) {
            self.try_allocate(&layout)
        } else {
            ptr::null_mut()
        }
    }

    /// Was der Block mindestens messen muss, damit `size` Bytes mit `align`
    /// hineinpassen — samt Marke, Kopf und hinterer Wiederholung.
    #[inline]
    fn need_for(block_start: usize, size: usize, align: usize) -> (usize, usize) {
        let data = align_up(block_start + TAG + HEADER_SIZE, align);
        let total = align_up((data - block_start) + size + TAG, BLOCK_ALIGN)
            .max(MIN_BLOCK_SIZE);
        (data, total)
    }

    fn try_allocate(&mut self, layout: &Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align().max(BLOCK_ALIGN);

        self.c.allocs += 1;
        let mut h = 0usize;
        while h < 15 && size >= (32usize << h) { h += 1; }
        self.c.size_hist[h] += 1;

        // Die untere Schranke MUSS die Ausrichtung schon enthalten, sonst
        // faellt die Kastenwahl eine Klasse zu tief und die Suche laeuft an
        // allen zu kleinen Bloecken dieses Kastens vorbei. Genau das kostete
        // in 0.314.0 noch 70,4 Schritte je Belegung statt einem:
        // `TAG + HEADER_SIZE` sind 24, ein 16-ausgerichteter Blockanfang
        // schiebt die Nutzdaten aber auf 32.
        let data_off = align_up(TAG + HEADER_SIZE, align);
        let lower = align_up(data_off + size + TAG, BLOCK_ALIGN)
            .max(MIN_BLOCK_SIZE);
        let first = bin_of(lower);

        for b in first..NBINS {
            let mut node = self.bins[b];
            while !node.is_null() {
                self.c.alloc_steps += 1;
                let block = node as usize - TAG;
                // SAFETY: `node` haengt in der Freiliste, also ist `block` ein
                // freier Block mit gueltigen Marken.
                let bsize = unsafe { size_of_block(block) };
                let (data, need) = Self::need_for(block, size, align);
                if bsize >= need {
                    // SAFETY: `block` ist frei und gross genug.
                    unsafe { self.bin_remove(block, bsize) };
                    let rest = bsize - need;
                    let actual = if rest >= MIN_BLOCK_SIZE {
                        // SAFETY: beide Teile liegen im urspruenglichen Block.
                        unsafe {
                            set_tags(block, need, false);
                            set_tags(block + need, rest, true);
                            self.bin_push(block + need, rest);
                        }
                        need
                    } else {
                        // SAFETY: wie oben.
                        unsafe { set_tags(block, bsize, false) };
                        bsize
                    };
                    let header = (data - HEADER_SIZE) as *mut AllocHeader;
                    // SAFETY: `data - HEADER_SIZE` liegt hinter der Marke und
                    // vor den Nutzdaten desselben Blocks.
                    unsafe {
                        (*header).block_start = block;
                        (*header).block_size = actual;
                    }
                    self.allocated_bytes += actual;
                    return data as *mut u8;
                }
                // SAFETY: `node` ist ein gueltiger Listenknoten.
                node = unsafe { (*node).next };
            }
        }
        ptr::null_mut()
    }

    /// Grow heap by requesting contiguous frames from the physical memory manager.
    fn grow(&mut self, min_size: usize) -> bool {
        if self.total_size >= MAX_HEAP { return false; }
        if self.region_count >= MAX_REGIONS { return false; }

        let chunk = (min_size + 2 * SENTINEL).max(GROW_CHUNK).min(MAX_HEAP - self.total_size);
        let frames = chunk.div_ceil(4096);

        // SAFETY: memory::allocate_contiguous uses its own lock (memory::ALLOCATOR),
        // independent of the heap lock we're holding. No deadlock possible.
        // NOTE: no kprintln here — we're inside GlobalAlloc::alloc,
        // and kprintln can allocate (capture_bytes → String::push_str) → deadlock.
        if let Some(base) = crate::memory::allocate_contiguous(frames) {
            let start = base as usize;
            let size = frames * 4096;

            self.regions[self.region_count] = (start, start + size);
            self.region_count += 1;
            self.total_size += size;
            self.c.grows += 1;

            // SAFETY: die Region wurde gerade zugeteilt und gehoert uns.
            unsafe { self.lay_out_region(start, size) };
            true
        } else {
            false
        }
    }

    fn deallocate(&mut self, ptr: *mut u8) {
        if ptr.is_null() { return; }
        let data_addr = ptr as usize;
        if !self.contains(data_addr) { return; }

        // SAFETY: `data_addr` kam aus `try_allocate`, also steht der Kopf
        // unmittelbar davor.
        let header = unsafe { &*((data_addr - HEADER_SIZE) as *const AllocHeader) };
        let mut block = header.block_start;
        let mut size = header.block_size;

        if !self.contains(block) { return; }

        self.c.frees += 1;
        self.allocated_bytes -= size;

        // Nach vorn verschmelzen. Der Anschlag am Regionenende ist nie frei,
        // also endet das hier von selbst.
        // SAFETY: `block + size` ist ein Blockanfang oder der Anschlag.
        unsafe {
            let next = block + size;
            if is_free(next) {
                self.c.free_steps += 1;
                let ns = size_of_block(next);
                self.bin_remove(next, ns);
                size += ns;
            }
            // Und nach hinten, ueber die hintere Marke des Vorgaengers.
            let ps = prev_size(block);
            if ps != SENTINEL && is_free(block - ps) {
                self.c.free_steps += 1;
                self.bin_remove(block - ps, ps);
                block -= ps;
                size += ps;
            }
            set_tags(block, size, true);
            self.bin_push(block, size);
        }
    }
}

struct LockedHeap {
    inner: Mutex<Heap>,
}

impl LockedHeap {
    const fn new() -> Self {
        LockedHeap { inner: Mutex::new(Heap::empty()) }
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner.lock().allocate(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        self.inner.lock().deallocate(ptr);
    }
}

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::new();

unsafe extern "C" {
    static __heap_start: u8;
}

pub fn init() {
    let heap_start = unsafe { &__heap_start as *const u8 as usize };
    crate::memory::reserve_region(heap_start as u64, INITIAL_HEAP as u64);
    HEAP.inner.lock().init(heap_start, INITIAL_HEAP);
    kprintln!("[npk] Heap: {} MB (Groessenklassen + Grenzmarken, max {} MB)",
        INITIAL_HEAP / (1024 * 1024), MAX_HEAP / (1024 * 1024));
}

/// Die Zaehler herausholen. Kopie, damit der Aufrufer drucken kann, ohne den
/// Heap-Lock zu halten — kprintln alloziert.
pub fn counters() -> HeapCounters {
    HEAP.inner.lock().c
}

pub fn reset_counters() {
    let mut h = HEAP.inner.lock();
    h.c.allocs = 0; h.c.frees = 0; h.c.alloc_steps = 0; h.c.free_steps = 0;
    h.c.grows = 0; h.c.size_hist = [0; 16];
    // free_nodes NICHT zuruecksetzen: das ist ein Zustand, keine Zaehlung.
    h.c.max_free_nodes = h.c.free_nodes;
}

pub fn stats() -> (usize, usize) {
    let heap = HEAP.inner.lock();
    (heap.allocated_bytes, heap.total_size)
}

#[inline]
fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
