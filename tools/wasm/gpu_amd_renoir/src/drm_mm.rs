//! drivers/gpu/drm/drm_mm.c (Linux 6.18.55): the range allocator behind
//! the GTT manager and the on-chip ranges, with the one search mode they
//! use, DRM_MM_INSERT_BEST (`amdgpu_gtt_mgr_new`; `ttm_range_man_alloc`
//! without TTM_PL_FLAG_TOPDOWN). That mode depends only on the order of the
//! hole-size tree, never on its shape: `best_hole` finds the in-order last
//! hole at least as large as the request, `rb_prev` steps to larger ones.
//! The tree is kept as that order: sizes descending, an equal size after
//! the ones already there (`insert_hole_size` goes right on equality).
//! LOW, HIGH and EVICT walk the shape of the address tree and are not here.

use alloc::vec::Vec;

const ENOSPC: i32 = 28;

/// `struct drm_mm_node`, as the managers keep it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct drm_mm_node {
    pub start: u64,
    pub size: u64,
    pub color: u64,
}

/// One entry of the node list: `head_node` (index 0) or an allocated node.
#[derive(Clone, Copy)]
struct entry {
    start: u64,
    size: u64,
    color: u64,
    /// `hole_size`: the gap after this node, 0 when no hole follows.
    hole_size: u64,
    live: bool,
}

/// `struct drm_mm`.
pub struct drm_mm {
    entries: Vec<entry>,
    /// `node_list`, in address order, starting at the head node.
    list: Vec<usize>,
    /// `holes_size` in order: indices of the entries a hole follows.
    holes_size: Vec<usize>,
}

impl drm_mm {
    /// `drm_mm_init`: the head node sits at `start + size` with size
    /// `-size`, so its hole is the whole range.
    pub fn init(start: u64, size: u64) -> Self {
        let head = entry { start: start.wrapping_add(size), size: size.wrapping_neg(), color: 0, hole_size: 0, live: true };
        let mut mm = drm_mm { entries: alloc::vec![head], list: alloc::vec![0], holes_size: Vec::new() };
        mm.add_hole(0);
        mm
    }

    fn hole_start(&self, e: usize) -> u64 {
        self.entries[e].start.wrapping_add(self.entries[e].size)
    }

    /// `__drm_mm_hole_node_end`: the start of the next node in the list
    /// (the head node's for the last one).
    fn hole_end(&self, e: usize) -> u64 {
        let i = self.list.iter().position(|&x| x == e).unwrap();
        let next = self.list[(i + 1) % self.list.len()];
        self.entries[next].start
    }

    /// `add_hole` (`insert_hole_size`).
    fn add_hole(&mut self, e: usize) {
        let size = self.hole_end(e).wrapping_sub(self.hole_start(e));
        self.entries[e].hole_size = size;
        let at = self.holes_size.iter().position(|&h| size > self.entries[h].hole_size).unwrap_or(self.holes_size.len());
        self.holes_size.insert(at, e);
    }

    /// `rm_hole`.
    fn rm_hole(&mut self, e: usize) {
        self.holes_size.retain(|&h| h != e);
        self.entries[e].hole_size = 0;
    }

    fn hole_follows(&self, e: usize) -> bool {
        self.entries[e].hole_size != 0
    }

    /// `best_hole`: the position in `holes_size` of the smallest hole at
    /// least `size` large (the in-order last one).
    fn best_hole(&self, size: u64) -> Option<usize> {
        self.holes_size.iter().rposition(|&h| size <= self.entries[h].hole_size)
    }

    fn new_entry(&mut self, start: u64, size: u64, color: u64) -> usize {
        self.entries.push(entry { start, size, color, hole_size: 0, live: true });
        self.entries.len() - 1
    }

    /// `drm_mm_insert_node_in_range` with DRM_MM_INSERT_BEST and no
    /// `color_adjust`.
    pub fn insert_node_in_range_best(&mut self, size: u64, alignment: u64, color: u64, range_start: u64,
                                     range_end: u64) -> Result<drm_mm_node, i32> {
        if size == 0 || range_end - range_start < size {
            return Err(-ENOSPC);
        }
        if self.holes_size.first().map_or(0, |&h| self.entries[h].hole_size) < size {
            return Err(-ENOSPC);
        }
        let alignment = if alignment <= 1 { 0 } else { alignment };
        let remainder_mask = if alignment.is_power_of_two() { alignment - 1 } else { 0 };
        let mut pos = self.best_hole(size);
        while let Some(p) = pos {
            let hole = self.holes_size[p];
            // next_hole: rb_prev in the size tree.
            pos = p.checked_sub(1);
            let hole_start = self.hole_start(hole);
            let hole_end = hole_start.wrapping_add(self.entries[hole].hole_size);
            let (col_start, col_end) = (hole_start, hole_end);
            let mut adj_start = col_start.max(range_start);
            let adj_end = col_end.min(range_end);
            if adj_end <= adj_start || adj_end - adj_start < size {
                continue;
            }
            if alignment != 0 {
                let rem = if remainder_mask != 0 { adj_start & remainder_mask } else { adj_start % alignment };
                if rem != 0 {
                    adj_start -= rem;
                    adj_start += alignment;
                    if adj_start < col_start.max(range_start) || col_end.min(range_end) - adj_start < size {
                        continue;
                    }
                    if adj_end <= adj_start || adj_end - adj_start < size {
                        continue;
                    }
                }
            }
            let node = self.new_entry(adj_start, size, color);
            let li = self.list.iter().position(|&x| x == hole).unwrap();
            self.list.insert(li + 1, node);
            self.rm_hole(hole);
            if adj_start > hole_start {
                self.add_hole(hole);
            }
            if adj_start + size < hole_end {
                self.add_hole(node);
            }
            return Ok(drm_mm_node { start: adj_start, size, color });
        }
        Err(-ENOSPC)
    }

    /// `drm_mm_reserve_node`: the node must lie inside one hole.
    pub fn reserve_node(&mut self, node: drm_mm_node) -> Result<(), i32> {
        let end = node.start.wrapping_add(node.size);
        if end <= node.start {
            return Err(-ENOSPC);
        }
        // find_hole_addr with size 0: the hole containing the start.
        let Some(&hole) = self.list.iter().find(|&&e| {
            self.hole_follows(e) && {
                let s = self.hole_start(e);
                s <= node.start && node.start <= s.wrapping_add(self.entries[e].hole_size)
            }
        }) else {
            return Err(-ENOSPC);
        };
        let hole_start = self.hole_start(hole);
        let hole_end = hole_start.wrapping_add(self.entries[hole].hole_size);
        if hole_start > node.start || hole_end < end {
            return Err(-ENOSPC);
        }
        let e = self.new_entry(node.start, node.size, node.color);
        let li = self.list.iter().position(|&x| x == hole).unwrap();
        self.list.insert(li + 1, e);
        self.rm_hole(hole);
        if node.start > hole_start {
            self.add_hole(hole);
        }
        if end < hole_end {
            self.add_hole(e);
        }
        Ok(())
    }

    /// `drm_mm_remove_node`.
    pub fn remove_node(&mut self, node: &drm_mm_node) {
        let Some(li) = self.list.iter().position(|&e| {
            e != 0 && self.entries[e].start == node.start && self.entries[e].size == node.size
        }) else {
            return;
        };
        let e = self.list[li];
        let prev = self.list[li - 1];
        if self.hole_follows(e) {
            self.rm_hole(e);
        }
        self.list.remove(li);
        self.entries[e].live = false;
        if self.hole_follows(prev) {
            self.rm_hole(prev);
        }
        self.add_hole(prev);
    }

    /// `drm_mm_for_each_node`, in address order.
    pub fn nodes(&self) -> impl Iterator<Item = drm_mm_node> + '_ {
        self.list[1..].iter().filter(|&&e| self.entries[e].live).map(move |&e| {
            let x = self.entries[e];
            drm_mm_node { start: x.start, size: x.size, color: x.color }
        })
    }
}
