//! What the display code needs from its environment: the register blocks
//! of the display set, the firmware's clocks and the GIC lines. The module
//! answers through the kernel's host calls, the tests from a recorded
//! Linux run.

/// A window of an HDMI controller (`reg-names` of `brcm,bcm2712-hdmi0/1`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HdmiReg {
    Hdmi,
    Dvp,
    Phy,
    Rm,
    Packet,
    Metadata,
    Csc,
    Cec,
    /// `hd`: shared by both controllers.
    Hd,
}

/// The register blocks the display touches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Block {
    Hvs,
    /// Pixel valve 0 or 1.
    Pv(u8),
    /// The L2 interrupt controller behind the HVS and writeback.
    DispIntr,
    /// The DVP clock gate and resets of the HDMI controllers.
    Dvp,
    /// HDMI controller 0 or 1, one of its windows.
    Hdmi(u8, HdmiReg),
    /// The BSC (I2C) controller of HDMI 0 or 1, for DDC.
    Ddc(u8),
    /// The L2 interrupt controller of the two DDC controllers.
    BscIrq,
    /// The always-on L2 interrupt controller (HDMI hotplug, CEC).
    AonIntr,
    /// V3D's hub, at its own offsets; the kernel keeps the MMU's
    /// registers (0x1000 to 0x12ff) out of reach.
    V3dHub,
    /// V3D's core 0.
    V3dCore,
}

/// The GIC lines of the display set, as `npk_irq_register_dt` names them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spi {
    Pv(u8),
    DispIntr,
    BscIrq,
    AonIntr,
    V3dHub,
    V3dCore,
}

/// A firmware clock operation (`npk_fw_clock`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClockOp {
    Rate,
    MaxRate,
    MinRate,
    SetRate(u32),
    Enable,
    Disable,
}

/// `enum rpi_firmware_clk_id`, the display's clocks.
pub const RPI_FIRMWARE_CORE_CLK_ID: u32 = 4;
pub const RPI_FIRMWARE_M2MC_CLK_ID: u32 = 13;
pub const RPI_FIRMWARE_PIXEL_BVB_CLK_ID: u32 = 14;
pub const RPI_FIRMWARE_DISP_CLK_ID: u32 = 16;

pub trait Hw {
    /// `readl` of a 32-bit register.
    fn read(&mut self, b: Block, off: u32) -> u32;
    /// `writel`.
    fn write(&mut self, b: Block, off: u32, v: u32);
    /// Microseconds since some fixed point.
    fn now_us(&mut self) -> u64;
    /// Sleep or spin for at least `us` microseconds.
    fn delay_us(&mut self, us: u64);
    /// The line's fired count before the work that raises it starts.
    fn irq_arm(&mut self, s: Spi) -> u64;
    /// Wait until the line fired past `since`, at most `timeout_ms`.
    fn irq_wait(&mut self, s: Spi, since: u64, timeout_ms: u32) -> bool;
    /// A firmware clock call; None when the firmware refuses.
    fn fw_clock(&mut self, id: u32, op: ClockOp) -> Option<u32>;
    /// `RPI_FIRMWARE_NOTIFY_DISPLAY_DONE`.
    fn fw_display_done(&mut self) -> bool;
}
