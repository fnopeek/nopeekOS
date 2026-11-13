//! `drivers/irqchip/irq-brcmstb-l2.c` (Linux rpi-6.18) with the parts of
//! the generic irq chip (`kernel/irq/generic-chip.c`) and the level flow
//! (`handle_level_irq`, `kernel/irq/chip.c`) its lines take.
//!
//! The kernel owns the GIC line behind each controller and masks it in its
//! hard interrupt handler; the module's fiber then runs
//! `brcmstb_l2_intc_irq_handle` and arms the line again. Suspend and wake
//! are not ported: none of the three controllers is `brcm,irq-can-wake`.

use crate::hw::{Block, Hw};

/// `struct brcmstb_intc_init_params`, without the flow handler: every
/// controller of the display set is a level one.
pub struct InitParams {
    pub cpu_status: u32,
    pub cpu_clear: Option<u32>,
    pub cpu_mask_status: u32,
    pub cpu_mask_set: u32,
    pub cpu_mask_clear: u32,
}

/// `l2_lvl_intc_init` (`brcm,bcm7271-l2-intc`: bsc_irq).
pub const L2_LVL_INTC_INIT: InitParams = InitParams {
    cpu_status: 0x00,
    cpu_clear: None,
    cpu_mask_status: 0x04,
    cpu_mask_set: 0x08,
    cpu_mask_clear: 0x0c,
};

/// `l2_2711_lvl_intc_init` (`brcm,bcm2711-l2-intc`: disp_intr, aon_intr).
pub const L2_2711_LVL_INTC_INIT: InitParams = InitParams {
    cpu_status: 0x00,
    cpu_clear: Some(0x08),
    cpu_mask_status: 0x0c,
    cpu_mask_set: 0x10,
    cpu_mask_clear: 0x14,
};

/// What a line's handler did (`irqreturn_t`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IrqReturn {
    None,
    Handled,
    /// The line stays masked until the threaded part ran
    /// (`IRQF_ONESHOT`, `irq_finalize_oneshot`).
    WakeThread,
}

/// `struct brcmstb_l2_intc_data` with its generic chip.
pub struct L2Intc {
    pub block: Block,
    params: &'static InitParams,
    /// `IRQD_IRQ_MASKED` per line; a line starts masked.
    masked: u32,
    /// `IRQD_IRQ_DISABLED` per line; cleared by `request_irq`.
    disabled: u32,
    /// `desc->depth`: nested disables per line.
    depth: [u8; 32],
}

impl L2Intc {
    /// `brcmstb_l2_intc_probe`: every line disabled at the controller, and
    /// a controller with a clear register cleared.
    pub fn probe(hw: &mut impl Hw, block: Block, params: &'static InitParams) -> Self {
        hw.write(block, params.cpu_mask_set, 0xffff_ffff);
        if let Some(clear) = params.cpu_clear {
            hw.write(block, clear, 0xffff_ffff);
        }
        L2Intc { block, params, masked: !0, disabled: !0, depth: [1; 32] }
    }

    /// `irq_gc_mask_disable_reg` (its `mask_cache` is read only on suspend).
    fn chip_mask(&mut self, hw: &mut impl Hw, line: u32) {
        let mask = 1 << line;
        hw.write(self.block, self.params.cpu_mask_set, mask);
    }

    /// `irq_gc_unmask_enable_reg`.
    fn chip_unmask(&mut self, hw: &mut impl Hw, line: u32) {
        let mask = 1 << line;
        hw.write(self.block, self.params.cpu_mask_clear, mask);
    }

    /// `irq_gc_mask_disable_and_ack_set` with a clear register,
    /// `irq_gc_mask_disable_reg` without.
    fn chip_mask_ack(&mut self, hw: &mut impl Hw, line: u32) {
        self.chip_mask(hw, line);
        if let Some(clear) = self.params.cpu_clear {
            hw.write(self.block, clear, 1 << line);
        }
    }

    fn is_masked(&self, line: u32) -> bool {
        self.masked & (1 << line) != 0
    }

    /// `mask_irq`.
    pub fn mask_irq(&mut self, hw: &mut impl Hw, line: u32) {
        if self.is_masked(line) { return; }
        self.chip_mask(hw, line);
        self.masked |= 1 << line;
    }

    /// `unmask_irq`.
    pub fn unmask_irq(&mut self, hw: &mut impl Hw, line: u32) {
        if !self.is_masked(line) { return; }
        self.chip_unmask(hw, line);
        self.masked &= !(1 << line);
    }

    /// `request_irq` → `irq_startup` → `irq_enable`: the chip has no
    /// `irq_enable`, so the line is unmasked.
    pub fn request_irq(&mut self, hw: &mut impl Hw, line: u32) {
        self.depth[line as usize] = 0;
        self.disabled &= !(1 << line);
        self.unmask_irq(hw, line);
    }

    /// `request_irq` with `IRQF_NO_AUTOEN`: the line stays disabled and
    /// masked until `enable_irq`.
    pub fn request_irq_noautoen(&mut self, line: u32) {
        self.depth[line as usize] = 1;
    }

    /// `enable_irq`: the last enable of a nested disable unmasks.
    pub fn enable_irq(&mut self, hw: &mut impl Hw, line: u32) {
        let d = &mut self.depth[line as usize];
        if *d == 0 { return; }
        *d -= 1;
        if *d == 0 {
            self.disabled &= !(1 << line);
            self.unmask_irq(hw, line);
        }
    }

    /// `disable_irq`: lazy (the chip has no `irq_disable`), the line is
    /// only marked; if it fires, the level flow masks it.
    pub fn disable_irq(&mut self, line: u32) {
        let d = &mut self.depth[line as usize];
        *d += 1;
        if *d == 1 {
            self.disabled |= 1 << line;
        }
    }

    /// `free_irq` → `irq_shutdown` → `irq_disable` → `mask_irq`.
    pub fn free_irq(&mut self, hw: &mut impl Hw, line: u32) {
        self.depth[line as usize] = 1;
        self.disabled |= 1 << line;
        self.mask_irq(hw, line);
    }

    /// The first half of `brcmstb_l2_intc_irq_handle`: the lines to
    /// handle, lowest first (`ffs`). Zero is `handle_bad_irq`, which
    /// touches no register.
    pub fn pending(&mut self, hw: &mut impl Hw) -> u32 {
        hw.read(self.block, self.params.cpu_status) & !hw.read(self.block, self.params.cpu_mask_status)
    }

    /// `handle_level_irq` up to the handler: `mask_ack_irq`. False when the
    /// line is disabled (`irq_can_handle`).
    pub fn level_enter(&mut self, hw: &mut impl Hw, line: u32) -> bool {
        self.chip_mask_ack(hw, line);
        self.masked |= 1 << line;
        self.disabled & (1 << line) == 0
    }

    /// `handle_level_irq` after the handler: `cond_unmask_irq`. A handler
    /// that woke its thread leaves the line masked; `thread_done` unmasks.
    pub fn level_exit(&mut self, hw: &mut impl Hw, line: u32, ret: IrqReturn) {
        if ret != IrqReturn::WakeThread && self.disabled & (1 << line) == 0 {
            self.unmask_irq(hw, line);
        }
    }

    /// `irq_finalize_oneshot` → `unmask_threaded_irq`.
    pub fn thread_done(&mut self, hw: &mut impl Hw, line: u32) {
        if self.disabled & (1 << line) == 0 {
            self.unmask_irq(hw, line);
        }
    }
}
