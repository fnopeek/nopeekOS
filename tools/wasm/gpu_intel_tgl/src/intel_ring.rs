//! gt/intel_ring.c and gt/intel_ring.h (Linux 6.18.55): a context's ring
//! buffer. The ring is an internal object (TGL has an LLC, so it never goes
//! to stolen memory); the CPU writes it through its DMA buffer.

use crate::i915_vma::{drm_i915_gem_object, i915_gem_object_create_internal, i915_ggtt_pin, i915_vma, pin_flags};
use crate::intel_ggtt::i915_ggtt;
use crate::regs::*;

/// `struct intel_ring`.
#[derive(Clone, Copy, Default)]
pub struct intel_ring {
    pub obj: Option<drm_i915_gem_object>,
    pub vma: Option<i915_vma>,
    pub size: u32,
    pub wrap: u32,
    pub effective_size: u32,
    pub head: u32,
    pub tail: u32,
    pub emit: u32,
    pub space: u32,
    pub pin_count: u32,
}

/// `__intel_ring_space`: free bytes, keeping a cacheline between tail and
/// head so a full ring never looks empty.
pub fn __intel_ring_space(head: u32, tail: u32, size: u32) -> u32 {
    head.wrapping_sub(tail).wrapping_sub(CACHELINE_BYTES) & (size - 1)
}

impl intel_ring {
    /// `intel_ring_update_space`.
    pub fn intel_ring_update_space(&mut self) -> u32 {
        self.space = __intel_ring_space(self.head, self.emit, self.size);
        self.space
    }

    /// `intel_ring_wrap`.
    pub fn intel_ring_wrap(&self, pos: u32) -> u32 {
        pos & (self.size - 1)
    }

    /// `intel_ring_direction`: the sign of `next - prev` modulo the ring.
    pub fn intel_ring_direction(&self, next: u32, prev: u32) -> i32 {
        (next.wrapping_sub(prev) << self.wrap) as i32
    }

    /// `intel_ring_reset`.
    pub fn intel_ring_reset(&mut self, tail: u32) {
        let tail = self.intel_ring_wrap(tail);
        self.tail = tail;
        self.head = tail;
        self.emit = tail;
        self.intel_ring_update_space();
    }

    /// `intel_ring_set_tail`.
    pub fn intel_ring_set_tail(&mut self, tail: u32) -> u32 {
        self.tail = tail;
        tail
    }

    /// `intel_ring_offset_valid`.
    pub fn intel_ring_offset_valid(&self, pos: u32) -> bool {
        pos & self.size.wrapping_neg() == 0 && pos % 8 == 0
    }

    /// The ring's GGTT offset (`i915_ggtt_offset(ring->vma)`).
    pub fn ggtt_offset(&self) -> u32 {
        self.vma.map(|v| v.ggtt_offset()).unwrap_or(0)
    }

    /// Writes `dw` at byte offset `at` (`ring->vaddr + at`).
    pub fn write(&self, at: u32, dw: &[u32]) -> bool {
        let Some(d) = self.obj.and_then(|o| o.map()) else {
            return false;
        };
        let mut bytes = [0u8; 4 * 256];
        let n = dw.len().min(256);
        for (i, v) in dw[..n].iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        d.write(at, &bytes[..n * 4])
    }
}

/// `intel_engine_create_ring`: `create_ring_vma` takes no stolen memory on an
/// LLC platform, and the GGTT is not read-only (`vm->has_read_only`).
pub fn intel_engine_create_ring(size: u32) -> Result<intel_ring, i32> {
    let mut ring = intel_ring {
        size,
        wrap: 32 - size.trailing_zeros(),
        effective_size: size,
        ..Default::default()
    };
    ring.intel_ring_update_space();
    ring.obj = Some(i915_gem_object_create_internal(size as u64)?);
    Ok(ring)
}

/// `intel_ring_pin`: PIN_OFFSET_BIAS and PIN_HIGH for a system-memory ring,
/// mapped write-back; the ring restarts at its emit position.
pub fn intel_ring_pin(ring: &mut intel_ring, ggtt: &mut i915_ggtt) -> Result<(), i32> {
    ring.pin_count += 1;
    if ring.pin_count > 1 {
        return Ok(());
    }
    if ring.vma.is_none() {
        let obj = ring.obj.ok_or(-12)?;
        let flags = pin_flags { high: !obj.is_stolen(), mappable: obj.is_stolen(), bias: ggtt.pin_bias };
        match i915_ggtt_pin(ggtt, obj, flags) {
            Ok(v) => ring.vma = Some(v),
            Err(e) => {
                ring.pin_count -= 1;
                return Err(e);
            }
        }
    }
    let emit = ring.emit;
    ring.intel_ring_reset(emit);
    Ok(())
}

/// `intel_ring_unpin`. The GGTT binding stays: nothing is unbound here.
pub fn intel_ring_unpin(ring: &mut intel_ring) {
    if ring.pin_count > 0 {
        ring.pin_count -= 1;
    }
}
