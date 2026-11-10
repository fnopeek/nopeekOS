//! The two reset controllers the PCIe root complex uses: the brcmstb
//! software-init lines (Linux `drivers/reset/reset-brcmstb.c`) and the
//! SATA/PCIe resistor calibration (`drivers/reset/reset-brcmstb-rescal.c`).

use crate::hw::{poll, Block, Hw};

const SW_INIT_SET: u64 = 0x00;
const SW_INIT_CLEAR: u64 = 0x04;
const SW_INIT_BANK_SIZE: u64 = 0x18;

fn sw_init_bit(id: u32) -> u32 { 1 << (id & 0x1f) }
fn sw_init_bank(id: u32) -> u64 { (id >> 5) as u64 }

/// `brcmstb_reset_assert`.
pub fn brcmstb_assert(hw: &mut impl Hw, id: u32) {
    let off = sw_init_bank(id) * SW_INIT_BANK_SIZE;
    hw.w32(Block::Reset, off + SW_INIT_SET, sw_init_bit(id));
}

/// `brcmstb_reset_deassert`.
pub fn brcmstb_deassert(hw: &mut impl Hw, id: u32) {
    let off = sw_init_bank(id) * SW_INIT_BANK_SIZE;
    hw.w32(Block::Reset, off + SW_INIT_CLEAR, sw_init_bit(id));
    // Up to 14 us until the block runs after a line is released.
    hw.delay_us(100);
}

const RESCAL_START: u64 = 0x0;
const RESCAL_START_BIT: u32 = 1 << 0;
const RESCAL_STATUS: u64 = 0x8;
const RESCAL_STATUS_BIT: u32 = 1 << 0;

/// Why the calibration failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RescalError {
    NotStarted,
    Timeout,
}

/// `brcm_rescal_reset_set`, which `reset_control_reset` runs.
pub fn rescal_reset(hw: &mut impl Hw) -> Result<(), RescalError> {
    let reg = hw.r32(Block::Rescal, RESCAL_START);
    hw.w32(Block::Rescal, RESCAL_START, reg | RESCAL_START_BIT);
    let reg = hw.r32(Block::Rescal, RESCAL_START);
    if reg & RESCAL_START_BIT == 0 {
        return Err(RescalError::NotStarted);
    }
    let (ok, _) = poll(hw, Block::Rescal, RESCAL_STATUS, 100, 1000, |r| r & RESCAL_STATUS_BIT != 0);
    if !ok {
        return Err(RescalError::Timeout);
    }
    let reg = hw.r32(Block::Rescal, RESCAL_START);
    hw.w32(Block::Rescal, RESCAL_START, reg & !RESCAL_START_BIT);
    Ok(())
}
