//! drivers/gpu/buddy.c (Linux 6.18.55): the buddy allocator behind the
//! VRAM manager. Blocks live in an arena and are named by their index; a
//! free tree is a map from offset to block, ordered as the rbtree is
//! (`rb_last` is the highest offset). The `tmp_link` lists of the range
//! walks are deques: `list_add` at the front, `list_add_tail` at the back.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

pub const GPU_BUDDY_RANGE_ALLOCATION: u64 = 1 << 0;
pub const GPU_BUDDY_TOPDOWN_ALLOCATION: u64 = 1 << 1;
pub const GPU_BUDDY_CONTIGUOUS_ALLOCATION: u64 = 1 << 2;
pub const GPU_BUDDY_CLEAR_ALLOCATION: u64 = 1 << 3;
pub const GPU_BUDDY_CLEARED: u64 = 1 << 4;
pub const GPU_BUDDY_TRIM_DISABLE: u64 = 1 << 5;

const GPU_BUDDY_CLEAR_TREE: usize = 0;
const GPU_BUDDY_DIRTY_TREE: usize = 1;
const GPU_BUDDY_MAX_FREE_TREES: usize = 2;

const GPU_BUDDY_HEADER_OFFSET: u64 = !0xfff;
const GPU_BUDDY_HEADER_STATE: u64 = 3 << 10;
const GPU_BUDDY_ALLOCATED: u64 = 1 << 10;
const GPU_BUDDY_FREE: u64 = 2 << 10;
const GPU_BUDDY_SPLIT: u64 = 3 << 10;
const GPU_BUDDY_HEADER_CLEAR: u64 = 1 << 9;
const GPU_BUDDY_HEADER_ORDER: u64 = 0x3f;
const GPU_BUDDY_MAX_ORDER: u32 = 63 - 12;

const EINVAL: i32 = 22;
const ENOSPC: i32 = 28;
const ENOMEM: i32 = 12;

/// `struct gpu_buddy_block`.
#[derive(Clone, Copy)]
struct gpu_buddy_block {
    header: u64,
    left: Option<usize>,
    right: Option<usize>,
    parent: Option<usize>,
}

/// `struct gpu_buddy`.
pub struct gpu_buddy {
    blocks: Vec<gpu_buddy_block>,
    /// Indices of freed blocks, for reuse.
    spare: Vec<usize>,
    free_trees: [Vec<BTreeMap<u64, usize>>; GPU_BUDDY_MAX_FREE_TREES],
    roots: Vec<usize>,
    pub max_order: u32,
    pub chunk_size: u64,
    pub size: u64,
    pub avail: u64,
    pub clear_avail: u64,
}

fn ilog2(x: u64) -> u32 {
    63 - x.leading_zeros()
}

fn overlaps(s1: u64, e1: u64, s2: u64, e2: u64) -> bool {
    s1 <= e2 && e1 >= s2
}

fn contains(s1: u64, e1: u64, s2: u64, e2: u64) -> bool {
    s1 <= s2 && e1 >= e2
}

fn round_down(x: u64, a: u64) -> u64 {
    x - x % a
}

fn round_up(x: u64, a: u64) -> u64 {
    x.div_ceil(a) * a
}

impl gpu_buddy {
    /// `gpu_buddy_init`.
    pub fn init(size: u64, chunk_size: u64) -> Result<Self, i32> {
        if size < chunk_size || chunk_size < 4096 || !chunk_size.is_power_of_two() {
            return Err(-EINVAL);
        }
        let mut size = round_down(size, chunk_size);
        let max_order = ilog2(size) - ilog2(chunk_size);
        if max_order > GPU_BUDDY_MAX_ORDER {
            return Err(-EINVAL);
        }
        let mut mm = gpu_buddy {
            blocks: Vec::new(),
            spare: Vec::new(),
            free_trees: core::array::from_fn(|_| (0..=max_order).map(|_| BTreeMap::new()).collect()),
            roots: Vec::new(),
            max_order,
            chunk_size,
            size,
            avail: size,
            clear_avail: 0,
        };
        // Split into power-of-two blocks, in case we are given a size that
        // is not itself a power-of-two.
        let mut offset = 0;
        loop {
            let order = ilog2(size) - ilog2(chunk_size);
            let root_size = chunk_size << order;
            let root = mm.block_alloc(None, order, offset);
            mm.mark_free(root);
            mm.roots.push(root);
            offset += root_size;
            size -= root_size;
            if size == 0 {
                break;
            }
        }
        Ok(mm)
    }

    fn block_alloc(&mut self, parent: Option<usize>, order: u32, offset: u64) -> usize {
        let b = gpu_buddy_block { header: offset | order as u64, left: None, right: None, parent };
        match self.spare.pop() {
            Some(i) => {
                self.blocks[i] = b;
                i
            }
            None => {
                self.blocks.push(b);
                self.blocks.len() - 1
            }
        }
    }

    fn block_free(&mut self, block: usize) {
        self.spare.push(block);
    }

    pub fn block_offset(&self, block: usize) -> u64 {
        self.blocks[block].header & GPU_BUDDY_HEADER_OFFSET
    }

    pub fn block_order(&self, block: usize) -> u32 {
        (self.blocks[block].header & GPU_BUDDY_HEADER_ORDER) as u32
    }

    fn block_state(&self, block: usize) -> u64 {
        self.blocks[block].header & GPU_BUDDY_HEADER_STATE
    }

    fn is_allocated(&self, block: usize) -> bool {
        self.block_state(block) == GPU_BUDDY_ALLOCATED
    }

    pub fn is_clear(&self, block: usize) -> bool {
        self.blocks[block].header & GPU_BUDDY_HEADER_CLEAR != 0
    }

    fn is_free(&self, block: usize) -> bool {
        self.block_state(block) == GPU_BUDDY_FREE
    }

    fn is_split(&self, block: usize) -> bool {
        self.block_state(block) == GPU_BUDDY_SPLIT
    }

    pub fn block_size(&self, block: usize) -> u64 {
        self.chunk_size << self.block_order(block)
    }

    fn get_block_tree(&self, block: usize) -> usize {
        if self.is_clear(block) { GPU_BUDDY_CLEAR_TREE } else { GPU_BUDDY_DIRTY_TREE }
    }

    fn rbtree_insert(&mut self, block: usize, tree: usize) {
        let (order, off) = (self.block_order(block) as usize, self.block_offset(block));
        self.free_trees[tree][order].insert(off, block);
    }

    fn rbtree_remove(&mut self, block: usize) {
        let (order, off) = (self.block_order(block) as usize, self.block_offset(block));
        let tree = self.get_block_tree(block);
        self.free_trees[tree][order].remove(&off);
    }

    fn rbtree_last_free_block(&self, tree: usize, order: usize) -> Option<usize> {
        self.free_trees[tree][order].values().next_back().copied()
    }

    fn clear_reset(&mut self, block: usize) {
        self.blocks[block].header &= !GPU_BUDDY_HEADER_CLEAR;
    }

    fn mark_cleared(&mut self, block: usize) {
        self.blocks[block].header |= GPU_BUDDY_HEADER_CLEAR;
    }

    fn mark_allocated(&mut self, block: usize) {
        self.blocks[block].header &= !GPU_BUDDY_HEADER_STATE;
        self.blocks[block].header |= GPU_BUDDY_ALLOCATED;
        self.rbtree_remove(block);
    }

    fn mark_free(&mut self, block: usize) {
        self.blocks[block].header &= !GPU_BUDDY_HEADER_STATE;
        self.blocks[block].header |= GPU_BUDDY_FREE;
        let tree = self.get_block_tree(block);
        self.rbtree_insert(block, tree);
    }

    fn mark_split(&mut self, block: usize) {
        self.blocks[block].header &= !GPU_BUDDY_HEADER_STATE;
        self.blocks[block].header |= GPU_BUDDY_SPLIT;
        self.rbtree_remove(block);
    }

    fn get_buddy(&self, block: usize) -> Option<usize> {
        let parent = self.blocks[block].parent?;
        if self.blocks[parent].left == Some(block) { self.blocks[parent].right } else { self.blocks[parent].left }
    }

    fn __free(&mut self, mut block: usize, force_merge: bool) -> u32 {
        while let Some(parent) = self.blocks[block].parent {
            let buddy = self.get_buddy(block).unwrap();
            if !self.is_free(buddy) {
                break;
            }
            if !force_merge {
                // Check the block and its buddy clear state and exit the
                // loop if they both have the dissimilar state.
                if self.is_clear(block) != self.is_clear(buddy) {
                    break;
                }
                if self.is_clear(block) {
                    self.mark_cleared(parent);
                }
            }
            self.rbtree_remove(buddy);
            if force_merge && self.is_clear(buddy) {
                self.clear_avail -= self.block_size(buddy);
            }
            self.block_free(block);
            self.block_free(buddy);
            block = parent;
        }
        let order = self.block_order(block);
        self.mark_free(block);
        order
    }

    fn force_merge(&mut self, start: u64, end: u64, min_order: u32) -> i32 {
        if min_order == 0 {
            return -ENOMEM;
        }
        if min_order > self.max_order {
            return -EINVAL;
        }
        for tree in 0..GPU_BUDDY_MAX_FREE_TREES {
            for i in (0..min_order as usize).rev() {
                let mut iter = self.free_trees[tree][i].keys().next_back().copied();
                while let Some(key) = iter {
                    let block = self.free_trees[tree][i][&key];
                    iter = self.free_trees[tree][i].range(..key).next_back().map(|(k, _)| *k);
                    if self.blocks[block].parent.is_none() {
                        continue;
                    }
                    let block_start = self.block_offset(block);
                    let block_end = block_start + self.block_size(block) - 1;
                    if !contains(start, end, block_start, block_end) {
                        continue;
                    }
                    let buddy = self.get_buddy(block).unwrap();
                    if !self.is_free(buddy) {
                        continue;
                    }
                    // Advance to the next node when the current node is the
                    // buddy, as freeing the block will also remove its buddy
                    // from the tree.
                    if iter == Some(self.block_offset(buddy)) && self.get_block_tree(buddy) == tree
                        && self.block_order(buddy) as usize == i {
                        iter = self.free_trees[tree][i].range(..iter.unwrap()).next_back().map(|(k, _)| *k);
                    }
                    self.rbtree_remove(block);
                    if self.is_clear(block) {
                        self.clear_avail -= self.block_size(block);
                    }
                    let order = self.__free(block, true);
                    if order >= min_order {
                        return 0;
                    }
                }
            }
        }
        -ENOMEM
    }

    fn split_block(&mut self, block: usize) {
        let block_order = self.block_order(block) - 1;
        let offset = self.block_offset(block);
        let left = self.block_alloc(Some(block), block_order, offset);
        let right = self.block_alloc(Some(block), block_order, offset + (self.chunk_size << block_order));
        self.blocks[block].left = Some(left);
        self.blocks[block].right = Some(right);
        self.mark_split(block);
        if self.is_clear(block) {
            self.mark_cleared(left);
            self.mark_cleared(right);
            self.clear_reset(block);
        }
        self.mark_free(left);
        self.mark_free(right);
    }

    /// `gpu_buddy_free_block`.
    pub fn free_block(&mut self, block: usize) {
        self.avail += self.block_size(block);
        if self.is_clear(block) {
            self.clear_avail += self.block_size(block);
        }
        self.__free(block, false);
    }

    fn free_list_inner(&mut self, objects: &mut Vec<usize>, mark_clear: bool, mark_dirty: bool) {
        for b in core::mem::take(objects) {
            if mark_clear {
                self.mark_cleared(b);
            } else if mark_dirty {
                self.clear_reset(b);
            }
            self.free_block(b);
        }
    }

    fn free_list_internal(&mut self, objects: &mut Vec<usize>) {
        // Don't touch the clear/dirty bit, since allocation is still
        // internal at this point.
        self.free_list_inner(objects, false, false);
    }

    /// `gpu_buddy_free_list`.
    pub fn free_list(&mut self, objects: &mut Vec<usize>, flags: u64) {
        let mark_clear = flags & GPU_BUDDY_CLEARED != 0;
        self.free_list_inner(objects, mark_clear, !mark_clear);
    }

    fn block_incompatible(&self, block: usize, flags: u64) -> bool {
        let needs_clear = flags & GPU_BUDDY_CLEAR_ALLOCATION != 0;
        needs_clear != self.is_clear(block)
    }

    fn alloc_range_bias_inner(&mut self, start: u64, end: u64, order: u32, flags: u64, fallback: bool)
                              -> Result<usize, i32> {
        let req_size = self.chunk_size << order;
        let end = end - 1;
        let mut dfs: VecDeque<usize> = self.roots.iter().copied().collect();
        while let Some(block) = dfs.pop_front() {
            if self.block_order(block) < order {
                continue;
            }
            let block_start = self.block_offset(block);
            let block_end = block_start + self.block_size(block) - 1;
            if !overlaps(start, end, block_start, block_end) {
                continue;
            }
            if self.is_allocated(block) {
                continue;
            }
            if block_start < start || block_end > end {
                let adjusted_start = block_start.max(start);
                let adjusted_end = block_end.min(end);
                if round_down(adjusted_end + 1, req_size) <= round_up(adjusted_start, req_size) {
                    continue;
                }
            }
            if !fallback && self.block_incompatible(block, flags) {
                continue;
            }
            if contains(start, end, block_start, block_end) && order == self.block_order(block) {
                // Find the free block within the range.
                if self.is_free(block) {
                    return Ok(block);
                }
                continue;
            }
            if !self.is_split(block) {
                self.split_block(block);
            }
            dfs.push_front(self.blocks[block].right.unwrap());
            dfs.push_front(self.blocks[block].left.unwrap());
        }
        Err(-ENOSPC)
    }

    fn alloc_range_bias(&mut self, start: u64, end: u64, order: u32, flags: u64) -> Result<usize, i32> {
        match self.alloc_range_bias_inner(start, end, order, flags, false) {
            Err(_) => self.alloc_range_bias_inner(start, end, order, flags, true),
            ok => ok,
        }
    }

    fn get_maxblock(&self, order: u32, tree: usize) -> Option<usize> {
        let mut max_block: Option<usize> = None;
        for i in order..=self.max_order {
            let Some(block) = self.rbtree_last_free_block(tree, i as usize) else { continue };
            match max_block {
                None => max_block = Some(block),
                Some(m) if self.block_offset(block) > self.block_offset(m) => max_block = Some(block),
                _ => {}
            }
        }
        max_block
    }

    fn alloc_from_freetree(&mut self, order: u32, flags: u64) -> Result<usize, i32> {
        let mut tree = if flags & GPU_BUDDY_CLEAR_ALLOCATION != 0 { GPU_BUDDY_CLEAR_TREE } else { GPU_BUDDY_DIRTY_TREE };
        let mut found: Option<(usize, u32)> = None;
        if flags & GPU_BUDDY_TOPDOWN_ALLOCATION != 0 {
            if let Some(b) = self.get_maxblock(order, tree) {
                // Store the obtained block order
                found = Some((b, self.block_order(b)));
            }
        } else {
            for tmp in order..=self.max_order {
                if let Some(b) = self.rbtree_last_free_block(tree, tmp as usize) {
                    found = Some((b, tmp));
                    break;
                }
            }
        }
        if found.is_none() {
            // Try allocating from the other tree
            tree = if tree == GPU_BUDDY_CLEAR_TREE { GPU_BUDDY_DIRTY_TREE } else { GPU_BUDDY_CLEAR_TREE };
            for tmp in order..=self.max_order {
                if let Some(b) = self.rbtree_last_free_block(tree, tmp as usize) {
                    found = Some((b, tmp));
                    break;
                }
            }
        }
        let Some((mut block, mut tmp)) = found else { return Err(-ENOSPC) };
        while tmp != order {
            self.split_block(block);
            block = self.blocks[block].right.unwrap();
            tmp -= 1;
        }
        Ok(block)
    }

    fn alloc_range(&mut self, dfs: &mut VecDeque<usize>, start: u64, size: u64, blocks: &mut Vec<usize>,
                   total_allocated_on_err: Option<&mut u64>) -> i32 {
        let mut total_allocated = 0;
        let mut allocated = Vec::new();
        let end = start + size - 1;
        let mut err = 0;
        while let Some(block) = dfs.pop_front() {
            let block_start = self.block_offset(block);
            let block_end = block_start + self.block_size(block) - 1;
            if !overlaps(start, end, block_start, block_end) {
                continue;
            }
            if self.is_allocated(block) {
                err = -ENOSPC;
                break;
            }
            if contains(start, end, block_start, block_end) {
                if self.is_free(block) {
                    self.mark_allocated(block);
                    total_allocated += self.block_size(block);
                    self.avail -= self.block_size(block);
                    if self.is_clear(block) {
                        self.clear_avail -= self.block_size(block);
                    }
                    allocated.push(block);
                    continue;
                } else if self.clear_avail == 0 {
                    err = -ENOSPC;
                    break;
                }
            }
            if !self.is_split(block) {
                self.split_block(block);
            }
            dfs.push_front(self.blocks[block].right.unwrap());
            dfs.push_front(self.blocks[block].left.unwrap());
        }
        if err == 0 && total_allocated < size {
            err = -ENOSPC;
        }
        if err == 0 {
            blocks.append(&mut allocated);
            return 0;
        }
        match total_allocated_on_err {
            Some(t) if err == -ENOSPC => {
                blocks.append(&mut allocated);
                *t = total_allocated;
            }
            _ => self.free_list_internal(&mut allocated),
        }
        err
    }

    fn alloc_range_roots(&mut self, start: u64, size: u64, total_allocated_on_err: Option<&mut u64>,
                         blocks: &mut Vec<usize>) -> i32 {
        let mut dfs: VecDeque<usize> = self.roots.iter().copied().collect();
        self.alloc_range(&mut dfs, start, size, blocks, total_allocated_on_err)
    }

    fn alloc_contig_aligned_retry(&mut self, unaligned_offset: u64, size: u64, min_block_size: u64,
                                  blocks: &mut Vec<usize>) -> i32 {
        let aligned_offset = round_down(unaligned_offset, min_block_size);
        self.alloc_range_roots(aligned_offset, size, None, blocks)
    }

    fn alloc_contig_try_harder(&mut self, size: u64, min_block_size: u64, blocks: &mut Vec<usize>) -> i32 {
        let modify_size = 1u64 << ilog2(size);
        let order = ilog2(modify_size) - ilog2(self.chunk_size);
        if order == 0 {
            return -ENOSPC;
        }
        for tree in 0..GPU_BUDDY_MAX_FREE_TREES {
            let mut iter = self.free_trees[tree][order as usize].keys().next_back().copied();
            while let Some(rhs_offset) = iter {
                let mut filled = 0;
                // Allocate blocks traversing RHS
                let err = self.alloc_range_roots(rhs_offset, size, Some(&mut filled), blocks);
                'next: {
                    if err != 0 && err != -ENOSPC {
                        return err;
                    }
                    if err == 0 && rhs_offset % min_block_size == 0 {
                        return 0;
                    }
                    if err == 0 {
                        // Allocate the unaligned RHS offset using round_down
                        self.free_list_internal(blocks);
                        let err = self.alloc_contig_aligned_retry(rhs_offset, size, min_block_size, blocks);
                        if err == 0 {
                            return 0;
                        }
                        if err != -ENOSPC {
                            self.free_list_internal(blocks);
                            return err;
                        }
                        break 'next;
                    }
                    if size - filled > rhs_offset {
                        break 'next;
                    }
                    let lhs_offset = rhs_offset - (size - filled);
                    // Allocate the unaligned LHS offset using round_down
                    self.free_list_internal(blocks);
                    let err = self.alloc_contig_aligned_retry(lhs_offset, size, min_block_size, blocks);
                    if err == 0 {
                        return 0;
                    }
                    if err != -ENOSPC {
                        self.free_list_internal(blocks);
                        return err;
                    }
                }
                self.free_list_internal(blocks);
                iter = self.free_trees[tree][order as usize].range(..rhs_offset).next_back().map(|(k, _)| *k);
            }
        }
        -ENOSPC
    }

    /// `gpu_buddy_block_trim`.
    pub fn block_trim(&mut self, start: Option<u64>, new_size: u64, blocks: &mut Vec<usize>) -> i32 {
        if blocks.len() != 1 {
            return -EINVAL;
        }
        let block = blocks[0];
        let block_start = self.block_offset(block);
        let block_end = block_start + self.block_size(block);
        if !self.is_allocated(block) {
            return -EINVAL;
        }
        if new_size > self.block_size(block) {
            return -EINVAL;
        }
        if new_size == 0 || new_size % self.chunk_size != 0 {
            return -EINVAL;
        }
        if new_size == self.block_size(block) {
            return 0;
        }
        let mut new_start = block_start;
        if let Some(s) = start {
            new_start = s;
            if new_start < block_start || new_start % self.chunk_size != 0 {
                return -EINVAL;
            }
            if new_start.checked_add(new_size).is_none_or(|e| e > block_end) {
                return -EINVAL;
            }
        }
        blocks.clear();
        self.mark_free(block);
        self.avail += self.block_size(block);
        if self.is_clear(block) {
            self.clear_avail += self.block_size(block);
        }
        // Prevent recursively freeing this node
        let parent = self.blocks[block].parent.take();
        let mut dfs = VecDeque::from([block]);
        let err = self.alloc_range(&mut dfs, new_start, new_size, blocks, None);
        if err != 0 {
            self.mark_allocated(block);
            self.avail -= self.block_size(block);
            if self.is_clear(block) {
                self.clear_avail -= self.block_size(block);
            }
            blocks.insert(0, block);
        }
        self.blocks[block].parent = parent;
        err
    }

    fn alloc_blocks_one(&mut self, start: u64, end: u64, order: u32, flags: u64) -> Result<usize, i32> {
        if flags & GPU_BUDDY_RANGE_ALLOCATION != 0 {
            // Allocate traversing within the range
            self.alloc_range_bias(start, end, order, flags)
        } else {
            // Allocate from freetree
            self.alloc_from_freetree(order, flags)
        }
    }

    /// `gpu_buddy_alloc_blocks`.
    pub fn alloc_blocks(&mut self, start: u64, end: u64, mut size: u64, mut min_block_size: u64,
                        blocks: &mut Vec<usize>, flags: u64) -> i32 {
        if size < self.chunk_size || min_block_size < self.chunk_size || !min_block_size.is_power_of_two() {
            return -EINVAL;
        }
        if (start | end | size) % self.chunk_size != 0 {
            return -EINVAL;
        }
        if end > self.size || start.checked_add(size).is_none_or(|e| e > self.size) {
            return -EINVAL;
        }
        // Actual range allocation
        if start + size == end {
            if (start | end) % min_block_size != 0 {
                return -EINVAL;
            }
            return self.alloc_range_roots(start, size, None, blocks);
        }
        let original_size = size;
        let original_min_size = min_block_size;
        // Roundup the size to power of 2
        if flags & GPU_BUDDY_CONTIGUOUS_ALLOCATION != 0 {
            size = size.next_power_of_two();
            min_block_size = size;
        } else if size % min_block_size != 0 {
            // Align size value to min_block_size
            size = round_up(size, min_block_size);
        }
        let mut pages = size >> ilog2(self.chunk_size);
        let mut order = 64 - pages.leading_zeros() - 1;
        let min_order = ilog2(min_block_size) - ilog2(self.chunk_size);
        if order > self.max_order || size > self.size {
            if flags & GPU_BUDDY_CONTIGUOUS_ALLOCATION != 0 && flags & GPU_BUDDY_RANGE_ALLOCATION == 0 {
                return self.alloc_contig_try_harder(original_size, original_min_size, blocks);
            }
            return -EINVAL;
        }
        let mut allocated = Vec::new();
        loop {
            order = order.min(64 - pages.leading_zeros() - 1);
            let block = loop {
                match self.alloc_blocks_one(start, end, order, flags) {
                    Ok(b) => break b,
                    Err(_) => {
                        let was = order;
                        order = order.wrapping_sub(1);
                        if was == min_order {
                            // Try allocation through force merge method
                            if self.clear_avail != 0 && self.force_merge(start, end, min_order) == 0 {
                                if let Ok(b) = self.alloc_blocks_one(start, end, min_order, flags) {
                                    order = min_order;
                                    break b;
                                }
                            }
                            // Try contiguous block allocation through try
                            // harder method.
                            if flags & GPU_BUDDY_CONTIGUOUS_ALLOCATION != 0 && flags & GPU_BUDDY_RANGE_ALLOCATION == 0 {
                                return self.alloc_contig_try_harder(original_size, original_min_size, blocks);
                            }
                            self.free_list_internal(&mut allocated);
                            return -ENOSPC;
                        }
                    }
                }
            };
            self.mark_allocated(block);
            self.avail -= self.block_size(block);
            if self.is_clear(block) {
                self.clear_avail -= self.block_size(block);
            }
            allocated.push(block);
            pages -= 1 << order;
            if pages == 0 {
                break;
            }
        }
        // Trim the allocated block to the required size
        if flags & GPU_BUDDY_TRIM_DISABLE == 0 && original_size != size {
            if allocated.len() == 1 {
                self.block_trim(None, original_size, &mut allocated);
            } else {
                let last = allocated.pop().unwrap();
                let trim_size = self.block_size(last) - (size - original_size);
                let mut temp = alloc::vec![last];
                self.block_trim(None, trim_size, &mut temp);
                allocated.append(&mut temp);
            }
        }
        blocks.append(&mut allocated);
        0
    }
}
