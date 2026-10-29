//! Decrypted Tree objects, by content address.
//!
//! Every path operation reads the Trees from the root down to the parent,
//! and every write stores a new one at each level, which the next write
//! reads again. An address names one content forever, so an entry never
//! goes stale: it is dropped when gc removes the object, when the disk is
//! locked (the plaintext must not outlive the key) and at mount.
//!
//! Each entry holds its own frames rather than heap memory: the heap never
//! returns a region, so only frames can go back through the shrinker. As
//! the block cache it grows while free frames stay above the high
//! watermark, else it reuses the least recently used entries, and every
//! operation is O(1) apart from the copy.

use alloc::vec::Vec;
use hashbrown::HashMap;
use spin::Mutex;
use crate::memory;
use super::types::BLOCK_SIZE;

const NIL: u32 = u32::MAX;

struct Slot {
    hash: [u8; 32],
    /// First of `frames` contiguous identity-mapped frames; 0 for a spare.
    frame: u64,
    frames: u32,
    len: u32,
    /// Neighbours in the LRU list: `prev` towards the most recent end.
    prev: u32,
    next: u32,
}

struct TreeCache {
    slots: Vec<Slot>,
    map: HashMap<[u8; 32], u32>,
    head: u32,
    tail: u32,
    /// Slots without frames.
    spare: Vec<u32>,
    /// Frames held by all entries.
    held: usize,
}

static TREES: Mutex<Option<TreeCache>> = Mutex::new(None);

fn high_watermark() -> usize {
    let min = crate::heap::min_free_frames();
    min + min / 2
}

impl TreeCache {
    fn new() -> Self {
        TreeCache {
            slots: Vec::new(),
            map: HashMap::new(),
            head: NIL,
            tail: NIL,
            spare: Vec::new(),
            held: 0,
        }
    }

    fn bytes(&self, s: u32) -> &[u8] {
        let slot = &self.slots[s as usize];
        // SAFETY: a slot in the map owns `frames` contiguous identity-mapped
        // frames that nothing else references, and `len` fits them (`put`).
        unsafe { core::slice::from_raw_parts(slot.frame as *const u8, slot.len as usize) }
    }

    fn unlink(&mut self, s: u32) {
        let (prev, next) = (self.slots[s as usize].prev, self.slots[s as usize].next);
        if prev == NIL { self.head = next; } else { self.slots[prev as usize].next = next; }
        if next == NIL { self.tail = prev; } else { self.slots[next as usize].prev = prev; }
    }

    fn push_front(&mut self, s: u32) {
        self.slots[s as usize].prev = NIL;
        self.slots[s as usize].next = self.head;
        if self.head != NIL { self.slots[self.head as usize].prev = s; }
        self.head = s;
        if self.tail == NIL { self.tail = s; }
    }

    /// Drop the entry in slot `s` and give its frames back; returns how
    /// many. Allocates nothing: `spare` has room for every slot.
    fn evict(&mut self, s: u32) -> usize {
        self.unlink(s);
        let Slot { hash, frame, frames, .. } = self.slots[s as usize];
        self.map.remove(&hash);
        let frames = frames as usize;
        if frames == 1 {
            memory::deallocate_frame(frame);
        } else {
            memory::deallocate_contiguous(frame, frames);
        }
        self.slots[s as usize].frame = 0;
        self.held -= frames;
        self.spare.push(s);
        frames
    }

    fn get(&mut self, hash: &[u8; 32]) -> Option<Vec<u8>> {
        let s = *self.map.get(hash)?;
        if self.head != s {
            self.unlink(s);
            self.push_front(s);
        }
        let mut out = Vec::new();
        out.try_reserve_exact(self.slots[s as usize].len as usize).ok()?;
        out.extend_from_slice(self.bytes(s));
        Some(out)
    }

    /// Keep `bytes` under `hash`, unless memory is too short for it even
    /// after the cache gave its own back.
    fn put(&mut self, hash: &[u8; 32], bytes: &[u8]) {
        if self.map.contains_key(hash) { return; }
        let Ok(len) = u32::try_from(bytes.len()) else { return };
        let frames = bytes.len().div_ceil(BLOCK_SIZE).max(1);
        while memory::stats().0 < high_watermark() + frames && self.tail != NIL {
            self.evict(self.tail);
        }
        if memory::stats().0 < high_watermark() + frames { return; }
        let frame = if frames == 1 {
            memory::allocate_frame()
        } else {
            memory::allocate_contiguous(frames)
        };
        let Some(frame) = frame else { return };
        // SAFETY: the frames were just allocated and are identity mapped;
        // `bytes` fits them.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), frame as *mut u8, bytes.len()) };
        let slot = Slot { hash: *hash, frame, frames: frames as u32, len, prev: NIL, next: NIL };
        let s = match self.spare.pop() {
            Some(s) => { self.slots[s as usize] = slot; s }
            None => {
                self.slots.push(slot);
                // Room for every slot in `spare`, so `evict` never allocates.
                let room = self.slots.len() - self.spare.len();
                self.spare.reserve(room);
                (self.slots.len() - 1) as u32
            }
        };
        self.held += frames;
        self.map.insert(*hash, s);
        self.push_front(s);
    }

    fn clear(&mut self) {
        while self.tail != NIL {
            self.evict(self.tail);
        }
    }
}

impl Drop for TreeCache {
    fn drop(&mut self) {
        self.clear();
    }
}

/// The decrypted encoding of the Tree at `hash`, if it is held.
pub fn get(hash: &[u8; 32]) -> Option<Vec<u8>> {
    TREES.lock().as_mut()?.get(hash)
}

/// Hold the decrypted encoding of the Tree at `hash`. Only bytes whose
/// address was computed from them, or that were authenticated under it,
/// may go in. Checked under the cache lock, which `clear` takes after the
/// key is gone, so nothing read before a lock lands after it.
pub(super) fn put(hash: &[u8; 32], bytes: &[u8]) {
    let mut lock = TREES.lock();
    if !crate::crypto::disk_unlocked() { return; }
    lock.get_or_insert_with(|| {
        static SHRINKER: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if !SHRINKER.swap(true, core::sync::atomic::Ordering::AcqRel) {
            memory::register_shrinker(shrink);
        }
        TreeCache::new()
    }).put(hash, bytes);
}

/// Drop the entries for objects that left the store.
pub fn forget(hashes: &[[u8; 32]]) {
    let mut lock = TREES.lock();
    let Some(c) = lock.as_mut() else { return };
    for h in hashes {
        if let Some(&s) = c.map.get(h) {
            c.evict(s);
        }
    }
}

/// Drop every entry: the disk was locked, unlocked with a key, or mounted.
pub fn clear() {
    if let Some(c) = TREES.lock().as_mut() {
        c.clear();
    }
}

/// Entries and the frames they hold.
pub fn held() -> (usize, usize) {
    TREES.lock().as_ref().map_or((0, 0), |c| (c.map.len(), c.held))
}

/// The shrinker: entries from the cold end. Skips while the cache is in
/// use, since the allocation that asks may come from under its lock.
fn shrink(want: usize) -> usize {
    let Some(mut lock) = TREES.try_lock() else { return 0 };
    let Some(c) = lock.as_mut() else { return 0 };
    let mut freed = 0;
    while freed < want && c.tail != NIL {
        freed += c.evict(c.tail);
    }
    freed
}
