//! `drivers/gpu/drm/drm_mm.c` (Linux rpi-6.18): the range allocator vc4
//! places dlist entries, filter kernels, LBM and UPM space with, in the
//! modes vc4 uses (`DRM_MM_INSERT_BEST`, alignment, no colour, no
//! scan). Placement is what Linux's trees give: the smallest hole that
//! fits, among equal ones the one made last (`insert_hole_size` puts an
//! equal size right of the earlier ones, `best_hole` walks right), and
//! the node at the hole's aligned start.

use alloc::vec::Vec;

use crate::linux::ENOSPC;

#[derive(Clone, Copy, Debug)]
struct Hole {
    start: u64,
    size: u64,
    /// The order `add_hole` ran in.
    seq: u64,
}

/// `struct drm_mm`.
pub struct DrmMm {
    start: u64,
    size: u64,
    holes: Vec<Hole>,
    /// Allocated nodes: (start, size), sorted by start.
    nodes: Vec<(u64, u64)>,
    seq: u64,
}

/// `struct drm_mm_node` as the caller keeps it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DrmMmNode {
    pub start: u64,
    pub size: u64,
}

impl DrmMm {
    /// `drm_mm_init`.
    pub fn new(start: u64, size: u64) -> Self {
        let mut mm = DrmMm { start, size, holes: Vec::new(), nodes: Vec::new(), seq: 0 };
        mm.add_hole(start, size);
        mm
    }

    fn add_hole(&mut self, start: u64, size: u64) {
        if size == 0 { return; }
        self.seq += 1;
        self.holes.push(Hole { start, size, seq: self.seq });
    }

    fn rm_hole(&mut self, start: u64) {
        self.holes.retain(|h| h.start != start);
    }

    /// `drm_mm_insert_node_generic(mm, node, size, alignment, 0,
    /// DRM_MM_INSERT_BEST)`.
    pub fn insert_node_generic(&mut self, size: u64, alignment: u64) -> Result<DrmMmNode, i32> {
        if size == 0 || self.size < size {
            return Err(-ENOSPC);
        }
        let alignment = if alignment <= 1 { 0 } else { alignment };
        // Best first, then `next_hole` (`rb_prev`: the next larger).
        let mut order: Vec<Hole> = self.holes.iter().copied().filter(|h| h.size >= size).collect();
        order.sort_by(|a, b| a.size.cmp(&b.size).then(b.seq.cmp(&a.seq)));
        for h in order {
            let hole_end = h.start + h.size;
            let mut adj_start = h.start;
            if alignment != 0 {
                let rem = adj_start % alignment;
                if rem != 0 {
                    adj_start += alignment - rem;
                }
            }
            if adj_start + size > hole_end {
                continue;
            }
            self.rm_hole(h.start);
            if adj_start > h.start {
                self.add_hole(h.start, adj_start - h.start);
            }
            if adj_start + size < hole_end {
                self.add_hole(adj_start + size, hole_end - adj_start - size);
            }
            let at = self.nodes.partition_point(|&(s, _)| s < adj_start);
            self.nodes.insert(at, (adj_start, size));
            return Ok(DrmMmNode { start: adj_start, size });
        }
        Err(-ENOSPC)
    }

    /// `drm_mm_insert_node`.
    pub fn insert_node(&mut self, size: u64) -> Result<DrmMmNode, i32> {
        self.insert_node_generic(size, 0)
    }

    /// `drm_mm_remove_node`: the node's space joins the holes around it.
    pub fn remove_node(&mut self, node: DrmMmNode) {
        let Some(i) = self.nodes.iter().position(|&(s, _)| s == node.start) else { return };
        self.nodes.remove(i);
        let prev_end = if i == 0 { self.start } else { self.nodes[i - 1].0 + self.nodes[i - 1].1 };
        let next_start = self.nodes.get(i).map_or(self.start + self.size, |&(s, _)| s);
        self.holes.retain(|h| h.start < prev_end || h.start >= next_start);
        self.add_hole(prev_end, next_start - prev_end);
    }
}
