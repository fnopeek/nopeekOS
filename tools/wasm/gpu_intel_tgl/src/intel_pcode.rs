//! intel_pcode.c (Linux 6.18.55): the PCODE mailbox. GEN6_PCODE_* live
//! outside the forcewake domains, so the `_fw` accessors are used.

use crate::i915_drv::drm_i915_private;
use crate::linux::{self, Line};
use crate::regs::*;

fn e(errno: u32) -> i32 {
    -(errno as i32)
}

/// `gen7_check_mailbox_status` (GRAPHICS_VER > 6).
fn gen7_check_mailbox_status(mbox: u32) -> Result<(), i32> {
    match mbox & GEN6_PCODE_ERROR_MASK {
        GEN6_PCODE_SUCCESS => Ok(()),
        GEN6_PCODE_ILLEGAL_CMD => Err(e(ENXIO)),
        GEN7_PCODE_TIMEOUT => Err(e(ETIMEDOUT)),
        GEN7_PCODE_ILLEGAL_DATA => Err(e(EINVAL)),
        GEN11_PCODE_ILLEGAL_SUBCOMMAND => Err(e(ENXIO)),
        GEN11_PCODE_LOCKED => Err(e(EBUSY)),
        GEN11_PCODE_REJECTED => Err(e(EACCES)),
        GEN7_PCODE_MIN_FREQ_TABLE_GT_RATIO_OUT_OF_RANGE => Err(e(EOVERFLOW)),
        other => {
            Line::new().s("warning: missing case pcode status ").hex(other).say();
            Ok(())
        }
    }
}

/// `__snb_pcode_rw`.
fn __snb_pcode_rw(i915: &mut drm_i915_private, mut mbox: u32, val: &mut u32, val1: Option<&mut u32>,
                  fast_timeout_us: u32, slow_timeout_ms: u32, is_read: bool) -> Result<(), i32> {
    let ready = GEN6_PCODE_READY as u32;
    let u = &mut i915.uncore;
    if u.intel_uncore_read_fw(GEN6_PCODE_MAILBOX) & ready != 0 {
        return Err(e(EAGAIN));
    }
    u.intel_uncore_write_fw(GEN6_PCODE_DATA, *val);
    u.intel_uncore_write_fw(GEN6_PCODE_DATA1, val1.as_ref().map_or(0, |v| **v));
    u.intel_uncore_write_fw(GEN6_PCODE_MAILBOX, ready | mbox);
    if u.__intel_wait_for_register_fw(GEN6_PCODE_MAILBOX, ready, 0, fast_timeout_us,
                                      slow_timeout_ms, Some(&mut mbox)).is_err() {
        return Err(e(ETIMEDOUT));
    }
    if is_read {
        *val = u.intel_uncore_read_fw(GEN6_PCODE_DATA);
        if let Some(v1) = val1 {
            *v1 = u.intel_uncore_read_fw(GEN6_PCODE_DATA1);
        }
    }
    gen7_check_mailbox_status(mbox)
}

/// `snb_pcode_read` / `intel_pcode_read`.
pub fn snb_pcode_read(i915: &mut drm_i915_private, mbox: u32, val: &mut u32, val1: Option<&mut u32>) -> Result<(), i32> {
    let r = __snb_pcode_rw(i915, mbox, val, val1, 500, 20, true);
    if let Err(e) = r {
        Line::new().s("warning: pcode read from mbox ").hex(mbox).s(" failed: ").dec((-e) as u64).say();
    }
    r
}

/// `snb_pcode_write_timeout` / `intel_pcode_write_timeout`.
pub fn snb_pcode_write_timeout(i915: &mut drm_i915_private, mbox: u32, mut val: u32, timeout_ms: u32) -> Result<(), i32> {
    let r = __snb_pcode_rw(i915, mbox, &mut val, None, 250, timeout_ms, false);
    if let Err(e) = r {
        Line::new().s("warning: pcode write of ").hex(val).s(" to mbox ").hex(mbox)
            .s(" failed: ").dec((-e) as u64).say();
    }
    r
}

/// `intel_pcode_write`.
pub fn intel_pcode_write(i915: &mut drm_i915_private, mbox: u32, val: u32) -> Result<(), i32> {
    snb_pcode_write_timeout(i915, mbox, val, 1)
}

/// `skl_pcode_try_request`.
fn skl_pcode_try_request(i915: &mut drm_i915_private, mbox: u32, mut request: u32, reply_mask: u32,
                         reply: u32, status: &mut i32) -> bool {
    *status = match __snb_pcode_rw(i915, mbox, &mut request, None, 500, 0, true) {
        Ok(()) => 0,
        Err(e) => e,
    };
    *status == 0 && request & reply_mask == reply
}

/// `skl_pcode_request` / `intel_pcode_request`: repeats the request until
/// the reply matches, first for `timeout_base_ms`, then busy for 50 ms.
pub fn skl_pcode_request(i915: &mut drm_i915_private, mbox: u32, request: u32, reply_mask: u32,
                         reply: u32, timeout_base_ms: u32) -> Result<(), i32> {
    let mut status = 0i32;
    let mut ret: Result<(), ()>;
    if skl_pcode_try_request(i915, mbox, request, reply_mask, reply, &mut status) {
        ret = Ok(());
    } else {
        ret = linux::_wait_for(timeout_base_ms as u64 * 1000, 10, 10, || {
            skl_pcode_try_request(i915, mbox, request, reply_mask, reply, &mut status)
        });
        if ret.is_err() {
            Line::new().s("PCODE timeout, retrying with preemption disabled").say();
            if timeout_base_ms > 3 {
                Line::new().s("warning: pcode request base timeout ").dec(timeout_base_ms as u64)
                    .s(" ms above 3").say();
            }
            ret = linux::wait_for_atomic_us(50 * 1000, || {
                skl_pcode_try_request(i915, mbox, request, reply_mask, reply, &mut status)
            });
        }
    }
    if status != 0 {
        return Err(status);
    }
    ret.map_err(|()| e(ETIMEDOUT))
}

/// `intel_pcode_init`: discrete GPUs only.
pub fn intel_pcode_init() -> Result<(), i32> {
    Ok(())
}
