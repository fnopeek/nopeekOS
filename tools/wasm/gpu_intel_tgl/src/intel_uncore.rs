//! intel_uncore.c / intel_uncore.h (Linux 6.18.55): register access with
//! forcewake, on Tiger Lake.
//!
//! The platform switches are resolved for TGL as in
//! tools/gpu_intel_tgl.follow: forcewake through `uncore_get_fallback`,
//! reads and writes through the `fwtable` functions, `__gen12_fw_ranges`
//! and `gen12_shadowed_regs`. Linux reaches those through function
//! pointers; here they are called directly. The `i915.mmio_debug`
//! parameter is 0 (no CONFIG_DRM_I915_DEBUG_MMIO), so the unclaimed-access
//! check runs only where Linux runs it unconditionally.

use crate::linux::{self, HrTimer};
use crate::regs::*;
use crate::tables;
use npk_sys::Mmio;

/// `struct intel_forcewake_range`.
#[derive(Clone, Copy)]
pub struct intel_forcewake_range {
    pub start: u32,
    pub end: u32,
    pub domains: u32,
}

/// `struct i915_range`.
#[derive(Clone, Copy)]
pub struct i915_range {
    pub start: u32,
    pub end: u32,
}

/// `struct intel_uncore_forcewake_domain`.
#[derive(Clone, Copy)]
pub struct intel_uncore_forcewake_domain {
    pub id: u32,
    pub mask: u32,
    pub wake_count: u32,
    pub active: bool,
    pub timer: HrTimer,
    /// Register offsets (Linux keeps `regs + offset` pointers).
    pub reg_set: u32,
    pub reg_ack: u32,
}

/// `struct intel_uncore_mmio_debug`.
#[derive(Default)]
pub struct intel_uncore_mmio_debug {
    pub unclaimed_mmio_check: i32,
    pub saved_mmio_check: i32,
    pub suspend_count: u32,
}

/// `struct intel_uncore`.
pub struct intel_uncore {
    pub regs: Mmio,
    pub gsi_offset: u32,
    pub flags: u32,
    pub fw_domains_table: &'static [intel_forcewake_range],
    pub shadowed_reg_table: &'static [i915_range],
    pub fw_domains: u32,
    pub fw_domains_active: u32,
    pub fw_domains_timer: u32,
    pub fw_domains_saved: u32,
    pub fw_domain: [Option<intel_uncore_forcewake_domain>; FW_DOMAIN_ID_COUNT as usize],
    pub user_forcewake_count: u32,
    pub fifo_count: u32,
    pub debug: intel_uncore_mmio_debug,
    /// `i915->params.mmio_debug`.
    pub mmio_debug: i32,
    /// `uncore->fw_get_funcs` is set (forcewake initialised).
    pub fw_get_funcs: bool,
}

/// `_MASKED_BIT_ENABLE(a)` (i915_reg_defs.h): the bit and its write-enable.
const fn masked_bit_enable(a: u32) -> u32 {
    (a << 16) | a
}

/// `_MASKED_BIT_DISABLE(a)`.
const fn masked_bit_disable(a: u32) -> u32 {
    a << 16
}

/// `NEEDS_FORCE_WAKE(reg)`.
fn needs_force_wake(reg: u32) -> bool {
    reg < 0x40000 || reg >= 0x116000
}

/// `IS_GSI_REG(reg)` (intel_uncore.h).
fn is_gsi_reg(reg: u32) -> bool {
    reg < 0x40000
}

impl intel_uncore {
    /// `intel_uncore_init_early` + `intel_uncore_mmio_debug_init_early`.
    pub fn new(regs: Mmio) -> Self {
        intel_uncore {
            regs,
            gsi_offset: 0,
            flags: 0,
            fw_domains_table: &[],
            shadowed_reg_table: &[],
            fw_domains: 0,
            fw_domains_active: 0,
            fw_domains_timer: 0,
            fw_domains_saved: 0,
            fw_domain: [None; FW_DOMAIN_ID_COUNT as usize],
            user_forcewake_count: 0,
            fifo_count: 0,
            debug: intel_uncore_mmio_debug { unclaimed_mmio_check: 1, ..Default::default() },
            mmio_debug: 0,
            fw_get_funcs: false,
        }
    }

    // ── __raw_uncore_read/write ──────────────────────────────────

    pub fn __raw_uncore_read32(&self, reg: u32) -> u32 {
        let off = if is_gsi_reg(reg) { reg + self.gsi_offset } else { reg };
        self.regs.r32(off)
    }

    pub fn __raw_uncore_write32(&self, reg: u32, val: u32) {
        let off = if is_gsi_reg(reg) { reg + self.gsi_offset } else { reg };
        self.regs.w32(off, val);
    }

    pub fn __raw_uncore_read64(&self, reg: u32) -> u64 {
        let off = if is_gsi_reg(reg) { reg + self.gsi_offset } else { reg };
        self.regs.readq(off)
    }

    pub fn intel_uncore_has_forcewake(&self) -> bool {
        self.flags & (UNCORE_HAS_FORCEWAKE as u32) != 0
    }

    pub fn intel_uncore_has_fpga_dbg_unclaimed(&self) -> bool {
        self.flags & (UNCORE_HAS_FPGA_DBG_UNCLAIMED as u32) != 0
    }

    pub fn intel_uncore_has_dbg_unclaimed(&self) -> bool {
        self.flags & (UNCORE_HAS_DBG_UNCLAIMED as u32) != 0
    }

    pub fn intel_uncore_has_fifo(&self) -> bool {
        self.flags & (UNCORE_HAS_FIFO as u32) != 0
    }

    // ── forcewake domains ────────────────────────────────────────

    fn fw_ack(&self, d: &intel_uncore_forcewake_domain) -> u32 {
        self.regs.r32(d.reg_ack)
    }

    fn fw_set(&self, d: &intel_uncore_forcewake_domain, val: u32) {
        self.regs.w32(d.reg_set, masked_bit_enable(val));
    }

    fn fw_clear(&self, d: &intel_uncore_forcewake_domain, val: u32) {
        self.regs.w32(d.reg_set, masked_bit_disable(val));
    }

    /// WaRsClearFWBitsAtReset: GRAPHICS_VER >= 12 clears 0xefff.
    fn fw_domain_reset(&self, d: &intel_uncore_forcewake_domain) {
        self.fw_clear(d, 0xefff);
    }

    fn fw_domain_arm_timer(&mut self, id: usize) {
        let d = self.fw_domain[id].as_mut().unwrap();
        self.fw_domains_timer |= d.mask;
        d.wake_count += 1;
        d.timer.start(1000);
    }

    fn __wait_for_ack(&self, d: &intel_uncore_forcewake_domain, ack: u32, value: u32) -> Result<(), ()> {
        linux::wait_for_atomic_us(FORCEWAKE_ACK_TIMEOUT_MS as u64 * 1000,
                                  || self.fw_ack(d) & ack == value)
    }

    fn wait_ack_clear(&self, d: &intel_uncore_forcewake_domain, ack: u32) -> Result<(), ()> {
        self.__wait_for_ack(d, ack, 0)
    }

    fn wait_ack_set(&self, d: &intel_uncore_forcewake_domain, ack: u32) -> Result<(), ()> {
        self.__wait_for_ack(d, ack, ack)
    }

    fn fw_domain_wait_ack_clear(&self, d: &intel_uncore_forcewake_domain) {
        if self.wait_ack_clear(d, FORCEWAKE_KERNEL as u32).is_ok() {
            return;
        }
        if self.fw_ack(d) == !0 {
            linux::Line::new().s("error: ").s(intel_uncore_forcewake_domain_to_str(d.id))
                .s(": MMIO unreliable (forcewake register returns 0xFFFFFFFF)!").say();
            // intel_gt_set_wedged_async: the GT is unusable; the driver
            // stops submitting, which it does not do yet in this stage.
        } else {
            linux::Line::new().s("error: ").s(intel_uncore_forcewake_domain_to_str(d.id))
                .s(": timed out waiting for forcewake ack to clear.").say();
        }
    }

    /// WaRsForcewakeAddDelayForAck: toggle the fallback bit to kick the
    /// GT state machine. `Ok` when the ack was seen.
    fn fw_domain_wait_ack_with_fallback(&self, d: &intel_uncore_forcewake_domain, set: bool) -> Result<(), ()> {
        let ack_bit = FORCEWAKE_KERNEL as u32;
        let value = if set { ack_bit } else { 0 };
        let mut pass = 1u64;
        let mut ack_detected;
        loop {
            let _ = self.wait_ack_clear(d, FORCEWAKE_KERNEL_FALLBACK as u32);
            self.fw_set(d, FORCEWAKE_KERNEL_FALLBACK as u32);
            linux::udelay(10 * pass);
            let _ = self.wait_ack_set(d, FORCEWAKE_KERNEL_FALLBACK as u32);
            ack_detected = self.fw_ack(d) & ack_bit == value;
            self.fw_clear(d, FORCEWAKE_KERNEL_FALLBACK as u32);
            let again = !ack_detected && pass < 10;
            pass += 1;
            if !again {
                break;
            }
        }
        if ack_detected { Ok(()) } else { Err(()) }
    }

    fn fw_domain_wait_ack_clear_fallback(&self, d: &intel_uncore_forcewake_domain) {
        if self.wait_ack_clear(d, FORCEWAKE_KERNEL as u32).is_ok() {
            return;
        }
        if self.fw_domain_wait_ack_with_fallback(d, false).is_err() {
            self.fw_domain_wait_ack_clear(d);
        }
    }

    fn fw_domain_get(&self, d: &intel_uncore_forcewake_domain) {
        self.fw_set(d, FORCEWAKE_KERNEL as u32);
    }

    fn fw_domain_wait_ack_set(&self, d: &intel_uncore_forcewake_domain) {
        if self.wait_ack_set(d, FORCEWAKE_KERNEL as u32).is_err() {
            linux::Line::new().s("error: ").s(intel_uncore_forcewake_domain_to_str(d.id))
                .s(": timed out waiting for forcewake ack request.").say();
        }
    }

    fn fw_domain_wait_ack_set_fallback(&self, d: &intel_uncore_forcewake_domain) {
        if self.wait_ack_set(d, FORCEWAKE_KERNEL as u32).is_ok() {
            return;
        }
        if self.fw_domain_wait_ack_with_fallback(d, true).is_err() {
            self.fw_domain_wait_ack_set(d);
        }
    }

    fn fw_domain_put(&self, d: &intel_uncore_forcewake_domain) {
        self.fw_clear(d, FORCEWAKE_KERNEL as u32);
    }

    /// The initialised domains in `mask`, lowest id first
    /// (`for_each_fw_domain_masked`).
    fn domains_in(&self, mask: u32) -> impl Iterator<Item = usize> + use<> {
        let present: [bool; FW_DOMAIN_ID_COUNT as usize] =
            core::array::from_fn(|i| self.fw_domain[i].is_some());
        (0..FW_DOMAIN_ID_COUNT as usize).filter(move |&i| mask & (1 << i) != 0 && present[i])
    }

    /// `fw_domains_get_with_fallback` (TGL's `uncore->fw_get_funcs`).
    fn fw_domains_get(&mut self, fw_domains: u32) {
        for i in self.domains_in(fw_domains) {
            let d = self.fw_domain[i].unwrap();
            self.fw_domain_wait_ack_clear_fallback(&d);
            self.fw_domain_get(&d);
        }
        for i in self.domains_in(fw_domains) {
            let d = self.fw_domain[i].unwrap();
            self.fw_domain_wait_ack_set_fallback(&d);
        }
        self.fw_domains_active |= fw_domains;
    }

    fn fw_domains_put(&mut self, fw_domains: u32) {
        for i in self.domains_in(fw_domains) {
            let d = self.fw_domain[i].unwrap();
            self.fw_domain_put(&d);
        }
        self.fw_domains_active &= !fw_domains;
    }

    fn fw_domains_reset(&mut self, fw_domains: u32) {
        if fw_domains == 0 {
            return;
        }
        for i in self.domains_in(fw_domains) {
            let d = self.fw_domain[i].unwrap();
            self.fw_domain_reset(&d);
        }
    }

    fn fifo_free_entries(&self) -> u32 {
        self.__raw_uncore_read32(GTFIFOCTL) & GT_FIFO_FREE_ENTRIES_MASK
    }

    /// `intel_uncore_fw_release_timer`; true = HRTIMER_RESTART.
    fn intel_uncore_fw_release_timer(&mut self, id: usize) -> bool {
        let d = self.fw_domain[id].as_mut().unwrap();
        if core::mem::replace(&mut d.active, false) {
            return true;
        }
        let mask = d.mask;
        self.fw_domains_timer &= !mask;
        let d = self.fw_domain[id].as_mut().unwrap();
        d.wake_count -= 1;
        if d.wake_count == 0 {
            self.fw_domains_put(mask);
        }
        false
    }

    /// Run the release timers that are due. Called by the driver loop and
    /// before every wait in which Linux would let a timer interrupt run.
    pub fn run_timers(&mut self) {
        let now = linux::now_us();
        for i in 0..FW_DOMAIN_ID_COUNT as usize {
            let due = self.fw_domain[i].is_some_and(|d| d.timer.due(now));
            if !due {
                continue;
            }
            // A restart keeps the expired deadline: the timer fires again
            // at the next poll, as an hrtimer re-queued without forwarding.
            if !self.intel_uncore_fw_release_timer(i) {
                self.fw_domain[i].as_mut().unwrap().timer.cancel();
            }
        }
    }

    /// `intel_uncore_forcewake_reset`; returns the domains that were held.
    fn intel_uncore_forcewake_reset(&mut self) -> u32 {
        // Single-threaded: no timer can run concurrently, so one pass
        // cancels them all (Linux retries while a callback is running).
        for i in self.domains_in(self.fw_domains) {
            self.fw_domain[i].as_mut().unwrap().active = false;
            if !self.fw_domain[i].as_mut().unwrap().timer.cancel() {
                continue;
            }
            if self.intel_uncore_fw_release_timer(i) {
                // A restart re-queues the timer; this pass has already
                // cleared `active`, so the second run releases.
                self.intel_uncore_fw_release_timer(i);
            }
        }

        let fw = self.fw_domains_active;
        if fw != 0 {
            self.fw_domains_put(fw);
        }
        self.fw_domains_reset(self.fw_domains);
        self.assert_forcewakes_inactive();
        fw
    }

    fn fpga_check_for_unclaimed_mmio(&self) -> bool {
        let dbg = self.__raw_uncore_read32(FPGA_DBG);
        if dbg & FPGA_DBG_RM_NOCLAIM == 0 {
            return false;
        }
        if dbg == !0 {
            linux::drm_err("Lost access to MMIO BAR; all registers now read back as 0xFFFFFFFF!");
        }
        self.__raw_uncore_write32(FPGA_DBG, FPGA_DBG_RM_NOCLAIM);
        true
    }

    fn gen6_check_for_fifo_debug(&self) -> bool {
        let fifodbg = self.__raw_uncore_read32(GTFIFODBG);
        if fifodbg != 0 {
            self.__raw_uncore_write32(GTFIFODBG, fifodbg);
        }
        fifodbg != 0
    }

    fn check_for_unclaimed_mmio(&self) -> bool {
        let mut ret = false;
        if self.debug.suspend_count != 0 {
            return false;
        }
        if self.intel_uncore_has_fpga_dbg_unclaimed() {
            ret |= self.fpga_check_for_unclaimed_mmio();
        }
        // UNCORE_HAS_DBG_UNCLAIMED is Valleyview/Cherryview only.
        if self.intel_uncore_has_fifo() {
            ret |= self.gen6_check_for_fifo_debug();
        }
        ret
    }

    /// `forcewake_early_sanitize`. No WaDisableShadowRegForCpd (CHV), and
    /// `iosf_mbi_punit_acquire` guards a Baytrail/Cherrytrail PMIC bus.
    fn forcewake_early_sanitize(&mut self, restore_forcewake: u32) {
        self.intel_uncore_forcewake_reset();
        if restore_forcewake != 0 {
            self.fw_domains_get(restore_forcewake);
            if self.intel_uncore_has_fifo() {
                self.fifo_count = self.fifo_free_entries();
            }
        }
    }

    fn __intel_uncore_forcewake_get(&mut self, mut fw_domains: u32) {
        fw_domains &= self.fw_domains;
        for i in self.domains_in(fw_domains) {
            let d = self.fw_domain[i].as_mut().unwrap();
            let was = d.wake_count;
            d.wake_count += 1;
            if was != 0 {
                fw_domains &= !d.mask;
                d.active = true;
            }
        }
        if fw_domains != 0 {
            self.fw_domains_get(fw_domains);
        }
    }

    pub fn intel_uncore_forcewake_get(&mut self, fw_domains: u32) {
        if !self.fw_get_funcs {
            return;
        }
        self.__intel_uncore_forcewake_get(fw_domains);
    }

    pub fn intel_uncore_forcewake_get__locked(&mut self, fw_domains: u32) {
        self.intel_uncore_forcewake_get(fw_domains);
    }

    fn __intel_uncore_forcewake_put(&mut self, mut fw_domains: u32, delayed: bool) {
        fw_domains &= self.fw_domains;
        for i in self.domains_in(fw_domains) {
            let d = self.fw_domain[i].as_mut().unwrap();
            d.wake_count -= 1;
            if d.wake_count != 0 {
                d.active = true;
                continue;
            }
            let mask = d.mask;
            if delayed && self.fw_domains_timer & mask == 0 {
                self.fw_domain_arm_timer(i);
            } else {
                self.fw_domains_put(mask);
            }
        }
    }

    pub fn intel_uncore_forcewake_put(&mut self, fw_domains: u32) {
        if !self.fw_get_funcs {
            return;
        }
        self.__intel_uncore_forcewake_put(fw_domains, false);
    }

    pub fn intel_uncore_forcewake_put__locked(&mut self, fw_domains: u32) {
        self.intel_uncore_forcewake_put(fw_domains);
    }

    pub fn intel_uncore_forcewake_put_delayed(&mut self, fw_domains: u32) {
        if !self.fw_get_funcs {
            return;
        }
        self.__intel_uncore_forcewake_put(fw_domains, true);
    }

    fn assert_forcewakes_inactive(&self) {
        if !self.fw_get_funcs {
            return;
        }
        if self.fw_domains_active != 0 {
            linux::Line::new().s("warning: expected all fw_domains to be inactive, but ")
                .hex(self.fw_domains_active).s(" are still on").say();
        }
    }

    // ── the forcewake table ──────────────────────────────────────

    /// `find_fw_domain`: binary search in `fw_domains_table`.
    fn find_fw_domain(&self, mut offset: u32) -> u32 {
        if is_gsi_reg(offset) {
            offset += self.gsi_offset;
        }
        let t = self.fw_domains_table;
        let (mut start, mut end) = (0usize, t.len());
        let mut found = None;
        while start < end {
            let mid = start + (end - start) / 2;
            let e = &t[mid];
            if offset < e.start {
                end = mid;
            } else if offset > e.end {
                start = mid + 1;
            } else {
                found = Some(e);
                break;
            }
        }
        let Some(entry) = found else { return 0 };
        if entry.domains == FORCEWAKE_ALL {
            return self.fw_domains;
        }
        if entry.domains & !self.fw_domains != 0 {
            linux::Line::new().s("warning: uninitialized forcewake domain(s) ")
                .hex(entry.domains & !self.fw_domains).s(" accessed at ").hex(offset).say();
        }
        entry.domains
    }

    /// `is_shadowed`: binary search in `shadowed_reg_table`.
    fn is_shadowed(&self, mut offset: u32) -> bool {
        if is_gsi_reg(offset) {
            offset += self.gsi_offset;
        }
        let t = self.shadowed_reg_table;
        let (mut start, mut end) = (0usize, t.len());
        while start < end {
            let mid = start + (end - start) / 2;
            if offset < t[mid].start {
                end = mid;
            } else if offset > t[mid].end {
                start = mid + 1;
            } else {
                return true;
            }
        }
        false
    }

    fn __fwtable_reg_read_fw_domains(&self, offset: u32) -> u32 {
        if needs_force_wake(offset) { self.find_fw_domain(offset) } else { 0 }
    }

    fn __fwtable_reg_write_fw_domains(&self, offset: u32) -> u32 {
        if needs_force_wake(offset) && !self.is_shadowed(offset) {
            self.find_fw_domain(offset)
        } else {
            0
        }
    }

    pub fn fwtable_reg_read_fw_domains(&self, reg: u32) -> u32 {
        self.__fwtable_reg_read_fw_domains(reg)
    }

    pub fn fwtable_reg_write_fw_domains(&self, reg: u32) -> u32 {
        self.__fwtable_reg_write_fw_domains(reg)
    }

    fn ___force_wake_auto(&mut self, fw_domains: u32) {
        for i in self.domains_in(fw_domains) {
            self.fw_domain_arm_timer(i);
        }
        self.fw_domains_get(fw_domains);
    }

    fn __force_wake_auto(&mut self, mut fw_domains: u32) {
        fw_domains &= self.fw_domains;
        fw_domains &= !self.fw_domains_active;
        if fw_domains != 0 {
            self.___force_wake_auto(fw_domains);
        }
    }

    /// `unclaimed_reg_debug_header` + `__unclaimed_previous_reg_debug`.
    fn unclaimed_reg_debug_header(&self) -> bool {
        if self.mmio_debug == 0 {
            return false;
        }
        let _ = self.check_for_unclaimed_mmio();
        true
    }

    /// `unclaimed_reg_debug_footer` + `__unclaimed_reg_debug`.
    fn unclaimed_reg_debug_footer(&mut self, reg: u32, read: bool) {
        if self.check_for_unclaimed_mmio() {
            linux::Line::new().s("warning: unclaimed ")
                .s(if read { "read from" } else { "write to" })
                .s(" register ").hex(reg).say();
            self.mmio_debug -= 1;
        }
    }

    /// `fwtable_read32`.
    pub fn fwtable_read32(&mut self, reg: u32) -> u32 {
        let dbg = self.unclaimed_reg_debug_header();
        let fw_engine = self.__fwtable_reg_read_fw_domains(reg);
        if fw_engine != 0 {
            self.__force_wake_auto(fw_engine);
        }
        let val = self.__raw_uncore_read32(reg);
        if dbg {
            self.unclaimed_reg_debug_footer(reg, true);
        }
        val
    }

    /// `fwtable_read64`.
    pub fn fwtable_read64(&mut self, reg: u32) -> u64 {
        let dbg = self.unclaimed_reg_debug_header();
        let fw_engine = self.__fwtable_reg_read_fw_domains(reg);
        if fw_engine != 0 {
            self.__force_wake_auto(fw_engine);
        }
        let val = self.__raw_uncore_read64(reg);
        if dbg {
            self.unclaimed_reg_debug_footer(reg, true);
        }
        val
    }

    /// `fwtable_write32`.
    pub fn fwtable_write32(&mut self, reg: u32, val: u32) {
        let dbg = self.unclaimed_reg_debug_header();
        let fw_engine = self.__fwtable_reg_write_fw_domains(reg);
        if fw_engine != 0 {
            self.__force_wake_auto(fw_engine);
        }
        self.__raw_uncore_write32(reg, val);
        if dbg {
            self.unclaimed_reg_debug_footer(reg, false);
        }
    }

    /// `intel_uncore_read` (`uncore->funcs.mmio_readl`).
    pub fn intel_uncore_read(&mut self, reg: u32) -> u32 {
        self.fwtable_read32(reg)
    }

    pub fn intel_uncore_read_notrace(&mut self, reg: u32) -> u32 {
        self.fwtable_read32(reg)
    }

    pub fn intel_uncore_read64(&mut self, reg: u32) -> u64 {
        self.fwtable_read64(reg)
    }

    /// `intel_uncore_write` (`uncore->funcs.mmio_writel`).
    pub fn intel_uncore_write(&mut self, reg: u32, val: u32) {
        self.fwtable_write32(reg, val);
    }

    pub fn intel_uncore_rmw(&mut self, reg: u32, clear: u32, set: u32) -> u32 {
        let old = self.intel_uncore_read(reg);
        let val = (old & !clear) | set;
        self.intel_uncore_write(reg, val);
        old
    }

    /// `intel_uncore_read_fw` = `__raw_uncore_read32`.
    pub fn intel_uncore_read_fw(&self, reg: u32) -> u32 {
        self.__raw_uncore_read32(reg)
    }

    /// `intel_uncore_write_fw` = `__raw_uncore_write32`.
    pub fn intel_uncore_write_fw(&self, reg: u32, val: u32) {
        self.__raw_uncore_write32(reg, val);
    }

    pub fn intel_uncore_rmw_fw(&self, reg: u32, clear: u32, set: u32) {
        let old = self.intel_uncore_read_fw(reg);
        let val = (old & !clear) | set;
        if val != old {
            self.intel_uncore_write_fw(reg, val);
        }
    }

    pub fn intel_uncore_forcewake_for_reg(&self, reg: u32, op: u32) -> u32 {
        if !self.intel_uncore_has_forcewake() {
            return 0;
        }
        let mut fw_domains = 0;
        if op & FW_REG_READ != 0 {
            fw_domains = self.fwtable_reg_read_fw_domains(reg);
        }
        if op & FW_REG_WRITE != 0 {
            fw_domains |= self.fwtable_reg_write_fw_domains(reg);
        }
        fw_domains
    }

    /// `__intel_wait_for_register_fw`.
    pub fn __intel_wait_for_register_fw(&mut self, reg: u32, mask: u32, value: u32,
                                        fast_timeout_us: u32, slow_timeout_ms: u32,
                                        out_value: Option<&mut u32>) -> Result<(), ()> {
        let mut reg_value = 0u32;
        let mut ret = Err(());
        if fast_timeout_us != 0 && fast_timeout_us <= 20000 {
            ret = linux::wait_for_atomic_us(fast_timeout_us as u64, || {
                reg_value = self.intel_uncore_read_fw(reg);
                reg_value & mask == value
            });
        }
        if ret.is_err() && slow_timeout_ms != 0 {
            ret = linux::wait_for_ms(slow_timeout_ms as u64, || {
                reg_value = self.intel_uncore_read_fw(reg);
                reg_value & mask == value
            });
        }
        if let Some(o) = out_value {
            *o = reg_value;
        }
        ret
    }

    /// `__intel_wait_for_register`.
    pub fn __intel_wait_for_register(&mut self, reg: u32, mask: u32, value: u32,
                                     fast_timeout_us: u32, slow_timeout_ms: u32,
                                     out_value: Option<&mut u32>) -> Result<(), ()> {
        let fw = self.intel_uncore_forcewake_for_reg(reg, FW_REG_READ);
        let mut reg_value = 0u32;
        self.intel_uncore_forcewake_get__locked(fw);
        let mut ret = self.__intel_wait_for_register_fw(reg, mask, value, fast_timeout_us, 0,
                                                        Some(&mut reg_value));
        self.intel_uncore_forcewake_put__locked(fw);

        if ret.is_err() && slow_timeout_ms != 0 {
            // __wait_for(..., slow_timeout_ms * 1000, 10, 1000)
            ret = linux::wait_for_ms(slow_timeout_ms as u64, || {
                reg_value = self.intel_uncore_read_notrace(reg);
                reg_value & mask == value
            });
        }
        if let Some(o) = out_value {
            *o = reg_value;
        }
        ret
    }

    pub fn intel_uncore_unclaimed_mmio(&self) -> bool {
        self.check_for_unclaimed_mmio()
    }

    // ── init ─────────────────────────────────────────────────────

    /// `__fw_domain_init`.
    fn __fw_domain_init(&mut self, domain_id: u32, reg_set: u32, reg_ack: u32) {
        let d = intel_uncore_forcewake_domain {
            id: domain_id,
            mask: 1 << domain_id,
            wake_count: 0,
            active: false,
            timer: HrTimer::default(),
            reg_set: reg_set + self.gsi_offset,
            reg_ack: reg_ack + self.gsi_offset,
        };
        self.fw_domains |= 1 << domain_id;
        self.fw_domain_reset(&d);
        self.fw_domain[domain_id as usize] = Some(d);
    }

    fn fw_domain_fini(&mut self, domain_id: u32) {
        let Some(d) = self.fw_domain[domain_id as usize].take() else { return };
        self.fw_domains &= !(1 << domain_id);
        if d.wake_count != 0 || d.timer.active() {
            linux::Line::new().s("warning: forcewake domain ").dec(domain_id as u64)
                .s(" released while in use").say();
        }
    }

    /// `intel_uncore_fw_domains_init`, GRAPHICS_VER >= 11 branch, IP < 12.70.
    fn intel_uncore_fw_domains_init(&mut self, engine_mask: u64, has_rcs_or_ccs: bool) {
        self.fw_get_funcs = true; // uncore_get_fallback
        self.__fw_domain_init(FW_DOMAIN_ID_GT, FORCEWAKE_GT_GEN9, FORCEWAKE_ACK_GT_GEN9);
        if has_rcs_or_ccs {
            self.__fw_domain_init(FW_DOMAIN_ID_RENDER, FORCEWAKE_RENDER_GEN9, FORCEWAKE_ACK_RENDER_GEN9);
        }
        for i in 0..I915_MAX_VCS {
            if !crate::intel_engine::has_engine(engine_mask, _VCS(i)) {
                continue;
            }
            self.__fw_domain_init(FW_DOMAIN_ID_MEDIA_VDBOX0 + i,
                                  FORCEWAKE_MEDIA_VDBOX_GEN11(i),
                                  FORCEWAKE_ACK_MEDIA_VDBOX_GEN11(i));
        }
        for i in 0..I915_MAX_VECS {
            if !crate::intel_engine::has_engine(engine_mask, _VECS(i)) {
                continue;
            }
            self.__fw_domain_init(FW_DOMAIN_ID_MEDIA_VEBOX0 + i,
                                  FORCEWAKE_MEDIA_VEBOX_GEN11(i),
                                  FORCEWAKE_ACK_MEDIA_VEBOX_GEN11(i));
        }
        // GT_MEDIA (GSC domain) is MTL's standalone media GT.
        if self.fw_domains == 0 {
            linux::say("warning: no forcewake domains");
        }
    }

    /// `uncore_forcewake_init`, GRAPHICS_VER >= 12 below IP 12.55.
    fn uncore_forcewake_init(&mut self, engine_mask: u64, has_rcs_or_ccs: bool) {
        self.intel_uncore_fw_domains_init(engine_mask, has_rcs_or_ccs);
        self.forcewake_early_sanitize(0);
        // ASSIGN_READ_MMIO_VFUNCS(fwtable): the reads go through fwtable_*.
        self.fw_domains_table = &tables::__gen12_fw_ranges;
        self.shadowed_reg_table = &tables::gen12_shadowed_regs;
        // ASSIGN_WRITE_MMIO_VFUNCS(fwtable). The PMIC bus notifier is a
        // Baytrail/Cherrytrail concern.
    }

    /// `sanity_check_mmio_access`: wait up to 2 s for FORCEWAKE_MT to read
    /// as anything but all ones (it is masked, so the upper half never does).
    fn sanity_check_mmio_access(&self) -> Result<(), ()> {
        let ok = linux::wait_for_ms(2000, || self.__raw_uncore_read32(FORCEWAKE_MT) != !0);
        if ok.is_err() {
            linux::drm_err("Device is non-operational; MMIO access returns 0xFFFFFFFF!");
        }
        ok
    }

    /// `intel_uncore_init_mmio`. `engine_mask` is the GT's
    /// `info.engine_mask` at this point (the platform mask; fuses prune it
    /// later in `intel_uncore_prune_engine_fw_domains`).
    pub fn intel_uncore_init_mmio(&mut self, engine_mask: u64, has_rcs_or_ccs: bool,
                                  has_fpga_dbg_unclaimed: bool) -> Result<(), ()> {
        self.sanity_check_mmio_access()?;
        // GRAPHICS_VER > 5 and not a vGPU.
        self.flags |= UNCORE_HAS_FORCEWAKE as u32;
        self.uncore_forcewake_init(engine_mask, has_rcs_or_ccs);
        if has_fpga_dbg_unclaimed {
            self.flags |= UNCORE_HAS_FPGA_DBG_UNCLAIMED as u32;
        }
        // UNCORE_HAS_DBG_UNCLAIMED is VLV/CHV, UNCORE_HAS_FIFO GRAPHICS_VER 6..7.
        let _ = self.intel_uncore_unclaimed_mmio();
        Ok(())
    }

    /// `intel_uncore_prune_engine_fw_domains`, GRAPHICS_VER 12 below 12.55.
    pub fn intel_uncore_prune_engine_fw_domains(&mut self, engine_mask: u64) {
        if !self.intel_uncore_has_forcewake() {
            return;
        }
        let fw_domains = self.fw_domains;
        for i in 0..I915_MAX_VCS {
            let domain_id = FW_DOMAIN_ID_MEDIA_VDBOX0 + i;
            if crate::intel_engine::has_engine(engine_mask, _VCS(i)) {
                continue;
            }
            if fw_domains & (1 << domain_id) != 0 {
                self.fw_domain_fini(domain_id);
            }
        }
        for i in 0..I915_MAX_VECS {
            let domain_id = FW_DOMAIN_ID_MEDIA_VEBOX0 + i;
            if crate::intel_engine::has_engine(engine_mask, _VECS(i)) {
                continue;
            }
            if fw_domains & (1 << domain_id) != 0 {
                self.fw_domain_fini(domain_id);
            }
        }
        if fw_domains & (1 << FW_DOMAIN_ID_GSC) != 0
            && !crate::intel_engine::has_engine(engine_mask, GSC0) {
            self.fw_domain_fini(FW_DOMAIN_ID_GSC);
        }
    }
}

/// `intel_uncore_forcewake_domain_to_str`.
pub fn intel_uncore_forcewake_domain_to_str(id: u32) -> &'static str {
    const NAMES: [&str; 16] = [
        "render", "gt", "media", "vdbox0", "vdbox1", "vdbox2", "vdbox3", "vdbox4",
        "vdbox5", "vdbox6", "vdbox7", "vebox0", "vebox1", "vebox2", "vebox3", "gsc",
    ];
    NAMES.get(id as usize).copied().unwrap_or("unknown")
}
