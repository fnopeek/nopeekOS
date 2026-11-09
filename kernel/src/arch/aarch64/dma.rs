//! Coherent DMA memory for devices that do not snoop the CPU caches.
//!
//! No device in the Raspberry Pi 5's tree is `dma-coherent`, so Linux's
//! `dma_alloc_coherent` hands such devices memory mapped uncached. The
//! same here: a zone of 2 MiB blocks taken from the frame allocator and
//! mapped Normal non-cacheable (`mmu::map_uncached`). A request up to one
//! block is placed in a block's page bitmap; a larger one gets whole
//! blocks of its own. A block that becomes entirely free is mapped
//! cacheable again and goes back to the frame allocator.

use alloc::vec::Vec;
use spin::Mutex;

const PAGE: u64 = 4096;
const BLOCK_PAGES: usize = 512;
const BLOCK: u64 = BLOCK_PAGES as u64 * PAGE;

struct Chunk {
    base: u64,
    /// Whole blocks of one large request; 1 for a shared block.
    blocks: usize,
    /// Pages in use of a shared block (bit set = used).
    used: [u64; BLOCK_PAGES / 64],
    large: bool,
}

impl Chunk {
    fn end(&self) -> u64 {
        self.base + self.blocks as u64 * BLOCK
    }

    fn is_used(&self, i: usize) -> bool {
        self.used[i / 64] & (1 << (i % 64)) != 0
    }

    fn mark(&mut self, from: usize, n: usize, used: bool) {
        for i in from..from + n {
            if used { self.used[i / 64] |= 1 << (i % 64); } else { self.used[i / 64] &= !(1 << (i % 64)); }
        }
    }

    fn empty(&self) -> bool {
        self.used.iter().all(|&w| w == 0)
    }

    /// A run of `n` free pages starting at a multiple of `align`.
    fn find(&self, n: usize, align: usize) -> Option<usize> {
        let mut i = 0;
        while i + n <= BLOCK_PAGES {
            match (i..i + n).find(|&k| self.is_used(k)) {
                None => return Some(i),
                Some(k) => i = (k + 1).next_multiple_of(align),
            }
        }
        None
    }
}

static ZONE: Mutex<Vec<Chunk>> = Mutex::new(Vec::new());

/// New uncached blocks from the frame allocator, below `below`.
fn take_blocks(blocks: usize, align_pages: usize, below: u64) -> Option<u64> {
    let pages = blocks * BLOCK_PAGES;
    let base = crate::memory::allocate_contiguous_aligned_below(pages, align_pages.max(BLOCK_PAGES), below)?;
    if !super::mmu::map_uncached(base, base + blocks as u64 * BLOCK) {
        crate::memory::deallocate_contiguous(base, pages);
        return None;
    }
    Some(base)
}

fn give_back(c: &Chunk) {
    if super::mmu::map_cached(c.base, c.end()) {
        crate::memory::deallocate_contiguous(c.base, c.blocks * BLOCK_PAGES);
    }
}

/// `pages` contiguous uncached pages, the base a multiple of `align`
/// pages (a power of two), wholly below `below`. Not zeroed.
pub fn alloc(pages: usize, align: usize, below: u64) -> Option<u64> {
    if pages == 0 || align == 0 || !align.is_power_of_two() { return None; }
    let mut zone = ZONE.lock();
    if pages > BLOCK_PAGES || align > BLOCK_PAGES {
        let blocks = pages.div_ceil(BLOCK_PAGES);
        let base = take_blocks(blocks, align, below)?;
        zone.push(Chunk { base, blocks, used: [0; BLOCK_PAGES / 64], large: true });
        return Some(base);
    }
    for c in zone.iter_mut().filter(|c| !c.large && c.end() <= below) {
        if let Some(i) = c.find(pages, align) {
            c.mark(i, pages, true);
            return Some(c.base + i as u64 * PAGE);
        }
    }
    let base = take_blocks(1, BLOCK_PAGES, below)?;
    let mut c = Chunk { base, blocks: 1, used: [0; BLOCK_PAGES / 64], large: false };
    c.mark(0, pages, true);
    zone.push(c);
    Some(base)
}

/// Give back what `alloc` returned: the same base and page count.
pub fn free(base: u64, pages: usize) {
    let mut zone = ZONE.lock();
    let Some(k) = zone.iter().position(|c| c.base <= base && base < c.end()) else { return };
    let c = &mut zone[k];
    if !c.large {
        let i = ((base - c.base) / PAGE) as usize;
        c.mark(i, pages.min(BLOCK_PAGES - i), false);
        if !c.empty() { return; }
    }
    let c = zone.swap_remove(k);
    give_back(&c);
}

/// Bytes the zone holds, in use or not.
pub fn zone_bytes() -> u64 {
    ZONE.lock().iter().map(|c| c.blocks as u64 * BLOCK).sum()
}
