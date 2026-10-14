//! drm/drm_mm.c (Linux 6.18.55): the range allocator behind the GGTT
//! (`vm->mm`) and stolen memory (`i915->mm.stolen`), as far as fixed
//! reservations need it: `drm_mm_init`, `drm_mm_reserve_node`,
//! `drm_mm_remove_node` and the holes walk of `drm_mm_for_each_hole`.
//!
//! The address-space bookkeeping belongs to the gpudev side
//! (GPU_STACK.md §3.3); until it exists the module keeps it here. The
//! module has no heap that frees, so the nodes sit in a fixed array; a
//! full array is reported as -ENOSPC, the same answer as an overlap.

const ENOSPC: i32 = 28;

/// Nodes one range can hold before the gpudev side takes over.
const MAX_NODES: usize = 32;

/// `struct drm_mm_node`, the part a reservation sets.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct drm_mm_node {
    pub start: u64,
    pub size: u64,
    pub color: u64,
}

/// `struct drm_mm`: the managed range and its nodes, sorted by start.
#[derive(Clone, Copy)]
pub struct drm_mm {
    start: u64,
    size: u64,
    nodes: [drm_mm_node; MAX_NODES],
    n: usize,
    initialized: bool,
}

impl Default for drm_mm {
    fn default() -> Self {
        Self { start: 0, size: 0, nodes: [drm_mm_node::default(); MAX_NODES], n: 0, initialized: false }
    }
}

impl drm_mm {
    /// `drm_mm_init`.
    pub fn drm_mm_init(start: u64, size: u64) -> Self {
        Self { start, size, initialized: true, ..Default::default() }
    }

    /// `drm_mm_initialized`.
    pub fn initialized(&self) -> bool {
        self.initialized
    }

    /// `drm_mm_reserve_node`: the node must lie inside one hole. No
    /// `color_adjust` is installed on either range on TGL (GGTT cache
    /// colouring is for !HAS_LLC).
    pub fn drm_mm_reserve_node(&mut self, node: drm_mm_node) -> Result<(), i32> {
        let end = node.start.wrapping_add(node.size);
        if end <= node.start {
            return Err(-ENOSPC);
        }
        let Some((hole_start, hole_end, at)) = self.find_hole_addr(node.start) else {
            return Err(-ENOSPC);
        };
        if hole_start > node.start || hole_end < end {
            return Err(-ENOSPC);
        }
        if self.n == MAX_NODES {
            return Err(-ENOSPC);
        }
        self.nodes.copy_within(at..self.n, at + 1);
        self.nodes[at] = node;
        self.n += 1;
        Ok(())
    }

    /// `drm_mm_remove_node`.
    pub fn drm_mm_remove_node(&mut self, node: &drm_mm_node) {
        if let Some(i) = self.nodes[..self.n].iter().position(|n| n == node) {
            self.nodes.copy_within(i + 1..self.n, i);
            self.n -= 1;
        }
    }

    /// `find_hole_addr`: the hole containing `addr`, as (start, end, index
    /// the node following it has).
    fn find_hole_addr(&self, addr: u64) -> Option<(u64, u64, usize)> {
        self.holes().find(|&(s, e, _)| s <= addr && addr < e)
    }

    /// `drm_mm_for_each_hole`: (start, end, index of the next node).
    pub fn holes(&self) -> impl Iterator<Item = (u64, u64, usize)> + '_ {
        let range_end = self.start + self.size;
        (0..=self.n).filter_map(move |i| {
            let s = if i == 0 { self.start } else { self.nodes[i - 1].start + self.nodes[i - 1].size };
            let e = if i == self.n { range_end } else { self.nodes[i].start };
            (e > s).then_some((s, e, i))
        })
    }
}
