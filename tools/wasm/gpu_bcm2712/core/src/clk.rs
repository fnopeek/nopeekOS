//! The firmware clocks as vc4 consumes them: `drivers/clk/bcm/clk-raspberrypi.c`
//! (Linux rpi-6.18) behind the parts of the clock core (`drivers/clk/clk.c`)
//! vc4's calls reach. The kernel owns the mailbox; every firmware request
//! goes through `Hw::fw_clock`.
//!
//! clk-raspberrypi registers core and pixel-bvb `CLK_IS_CRITICAL`: the
//! clock core prepares and enables them at registration, before vc4 binds,
//! so here they start prepared and their consumers' `clk_prepare_enable`
//! only counts. The firmware has both running when it starts the kernel.

use crate::hw::{ClockOp, Hw, RPI_FIRMWARE_CORE_CLK_ID, RPI_FIRMWARE_PIXEL_BVB_CLK_ID};

/// A firmware clock (`struct raspberrypi_clk_data` with its `clk_core`
/// counts).
pub struct FwClk {
    pub id: u32,
    prepare_count: u32,
}

impl FwClk {
    pub fn new(id: u32) -> Self {
        let critical = id == RPI_FIRMWARE_CORE_CLK_ID || id == RPI_FIRMWARE_PIXEL_BVB_CLK_ID;
        FwClk { id, prepare_count: critical as u32 }
    }

    /// `clk_prepare_enable`: `raspberrypi_fw_prepare` on the first
    /// prepare; the firmware clocks have no `enable` op.
    pub fn prepare_enable(&mut self, hw: &mut impl Hw) -> Result<(), i32> {
        if self.prepare_count == 0 {
            hw.fw_clock(self.id, ClockOp::Enable).ok_or(-crate::linux::EIO)?;
        }
        self.prepare_count += 1;
        Ok(())
    }
}

/// `rpi_firmware_clk_get_max_rate`: 0 when the firmware does not answer.
pub fn rpi_firmware_clk_get_max_rate(hw: &mut impl Hw, id: u32) -> u32 {
    hw.fw_clock(id, ClockOp::MaxRate).unwrap_or(0)
}
