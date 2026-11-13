//! The clocks vc4 consumes: the firmware clocks of
//! `drivers/clk/bcm/clk-raspberrypi.c` and the two HDMI gates of
//! `drivers/clk/bcm/clk-bcm2711-dvp.c` (Linux rpi-6.18), behind the parts
//! of the clock core (`drivers/clk/clk.c`) vc4's calls reach: prepare
//! counts, per-consumer rate ranges, `clk_set_rate`, `clk_set_rate_range`
//! and the uncached rate of `CLK_GET_RATE_NOCACHE`.
//!
//! The kernel owns the mailbox; every firmware request goes through
//! `Hw::fw_clock`. Only the display's clocks are registered (core, M2MC,
//! pixel-bvb, disp): `RPI_FIRMWARE_GET_CLOCKS` is not offered to modules,
//! and V3D's clock waits for the V3D stage.

use alloc::vec::Vec;

use crate::hw::{Block, ClockOp, Hw, RPI_FIRMWARE_CORE_CLK_ID, RPI_FIRMWARE_DISP_CLK_ID,
    RPI_FIRMWARE_M2MC_CLK_ID, RPI_FIRMWARE_PIXEL_BVB_CLK_ID};
use crate::linux::EIO;

/// `struct raspberrypi_clk_variant`, the fields the display's clocks use.
struct Variant {
    min_rate: u32,
    minimize: bool,
    maximize: bool,
    critical: bool,
}

/// `raspberrypi_clk_variants` for the display's clocks.
fn variant(id: u32) -> Variant {
    match id {
        RPI_FIRMWARE_CORE_CLK_ID => Variant { min_rate: 0, minimize: true, maximize: false, critical: true },
        RPI_FIRMWARE_M2MC_CLK_ID => Variant { min_rate: 120_000_000, minimize: true, maximize: false, critical: false },
        RPI_FIRMWARE_PIXEL_BVB_CLK_ID => Variant { min_rate: 0, minimize: true, maximize: false, critical: true },
        _ => Variant { min_rate: 0, minimize: true, maximize: false, critical: false },
    }
}

/// The order `raspberrypi_discover_clocks` registers them in (the
/// firmware's `GET_CLOCKS` answer, without V3D).
pub const DISPLAY_CLOCKS: [u32; 4] = [
    RPI_FIRMWARE_CORE_CLK_ID,
    RPI_FIRMWARE_M2MC_CLK_ID,
    RPI_FIRMWARE_PIXEL_BVB_CLK_ID,
    RPI_FIRMWARE_DISP_CLK_ID,
];

/// `struct clk_core` of a firmware clock.
struct FwCore {
    id: u32,
    variant: Variant,
    /// `variant->max_rate` as the firmware answered it.
    max_rate: u32,
    /// `core->min_rate`, `core->max_rate` (`clk_hw_set_rate_range`).
    range: (u64, u64),
    rate: u64,
    req_rate: u64,
    prepare_count: u32,
}

/// A consumer's `struct clk`: its core and its own rate range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Clk(usize);

struct Consumer {
    core: usize,
    min: u64,
    max: u64,
}

/// The registered firmware clocks and their consumers.
pub struct FwClocks {
    cores: Vec<FwCore>,
    consumers: Vec<Consumer>,
}

impl FwClocks {
    /// `raspberrypi_clk_register` for each display clock, with the clock
    /// core's `__clk_core_init` (rate read, critical clocks prepared).
    pub fn register(hw: &mut impl Hw) -> Result<Self, i32> {
        let mut cores = Vec::new();
        for id in DISPLAY_CLOCKS {
            let mut v = variant(id);
            if v.min_rate == 0 {
                v.min_rate = hw.fw_clock(id, ClockOp::MinRate).ok_or(-EIO)?;
            }
            let max_rate = hw.fw_clock(id, ClockOp::MaxRate).ok_or(-EIO)?;
            // __clk_core_init: the rate, then a critical clock prepared and
            // enabled.
            let rate = hw.fw_clock(id, ClockOp::Rate).unwrap_or(0) as u64;
            let mut core = FwCore {
                id,
                range: (v.min_rate as u64, max_rate as u64),
                variant: v,
                max_rate,
                rate,
                req_rate: rate,
                prepare_count: 0,
            };
            if core.variant.critical {
                core.prepare(hw)?;
            }
            // raspberrypi_clk_register: below the minimum, the op itself
            // sets the minimum (the core's cached rate stays).
            let now = hw.fw_clock(id, ClockOp::Rate).unwrap_or(0);
            if now < core.variant.min_rate {
                hw.fw_clock(id, ClockOp::SetRate(core.variant.min_rate)).ok_or(-EIO)?;
            }
            cores.push(core);
        }
        Ok(FwClocks { cores, consumers: Vec::new() })
    }

    /// `clk_get`: a consumer of firmware clock `id`.
    pub fn get(&mut self, id: u32) -> Clk {
        let core = self.cores.iter().position(|c| c.id == id).expect("a display clock");
        self.consumers.push(Consumer { core, min: 0, max: u64::MAX });
        Clk(self.consumers.len() - 1)
    }

    pub fn max_rate(&self, clk: Clk) -> u32 {
        self.cores[self.consumers[clk.0].core].max_rate
    }

    /// `clk_core_get_boundaries`.
    fn boundaries(&self, core: usize) -> (u64, u64) {
        let (mut min, mut max) = self.cores[core].range;
        for c in self.consumers.iter().filter(|c| c.core == core) {
            min = min.max(c.min);
            max = max.min(c.max);
        }
        (min, max)
    }

    /// `raspberrypi_fw_dumb_determine_rate` on the core's boundaries.
    fn determine(&self, core: usize, rate: u64) -> u64 {
        let (min, max) = self.boundaries(core);
        let mut r = rate.clamp(min, max.max(min));
        if self.cores[core].variant.minimize && min > 0 {
            r = min;
        }
        r
    }

    /// `clk_core_get_rate_recalc`: the firmware's rate (`CLK_GET_RATE_NOCACHE`).
    fn recalc(&mut self, hw: &mut impl Hw, core: usize) -> u64 {
        let rate = hw.fw_clock(self.cores[core].id, ClockOp::Rate).unwrap_or(0) as u64;
        self.cores[core].rate = rate;
        rate
    }

    /// `clk_core_set_rate_nolock`.
    fn set_rate_nolock(&mut self, hw: &mut impl Hw, core: usize, req_rate: u64) -> Result<(), i32> {
        let rate = self.determine(core, req_rate);
        if rate == self.cores[core].rate {
            return Ok(());
        }
        // clk_calc_new_rates, clk_change_rate: set, then recalc.
        let new_rate = self.determine(core, req_rate);
        hw.fw_clock(self.cores[core].id, ClockOp::SetRate(new_rate.min(u32::MAX as u64) as u32)).ok_or(-EIO)?;
        self.recalc(hw, core);
        self.cores[core].req_rate = req_rate;
        Ok(())
    }

    /// `clk_get_rate`.
    pub fn get_rate(&mut self, hw: &mut impl Hw, clk: Clk) -> u64 {
        self.recalc(hw, self.consumers[clk.0].core)
    }

    /// `clk_set_rate`.
    pub fn set_rate(&mut self, hw: &mut impl Hw, clk: Clk, rate: u64) -> Result<(), i32> {
        self.set_rate_nolock(hw, self.consumers[clk.0].core, rate)
    }

    /// `clk_set_rate_range`.
    pub fn set_rate_range(&mut self, hw: &mut impl Hw, clk: Clk, min: u64, max: u64) -> Result<(), i32> {
        if min > max {
            return Err(-crate::linux::EINVAL);
        }
        let core = self.consumers[clk.0].core;
        let old = (self.consumers[clk.0].min, self.consumers[clk.0].max);
        self.consumers[clk.0].min = min;
        self.consumers[clk.0].max = max;
        // clk_core_check_boundaries
        let (cmin, cmax) = self.cores[core].range;
        let fits = !(min > cmax || max < cmin)
            && self.consumers.iter().filter(|c| c.core == core).all(|c| !(min > c.max || max < c.min));
        let ret = if fits {
            let rate = self.recalc(hw, core).clamp(min, max);
            self.set_rate_nolock(hw, core, rate)
        } else {
            Err(-crate::linux::EINVAL)
        };
        if ret.is_err() {
            self.consumers[clk.0].min = old.0;
            self.consumers[clk.0].max = old.1;
        }
        ret
    }

    /// `clk_set_min_rate`.
    pub fn set_min_rate(&mut self, hw: &mut impl Hw, clk: Clk, rate: u64) -> Result<(), i32> {
        let max = self.consumers[clk.0].max;
        self.set_rate_range(hw, clk, rate, max)
    }

    /// `clk_prepare_enable`; the firmware clocks have no `enable` op.
    pub fn prepare_enable(&mut self, hw: &mut impl Hw, clk: Clk) -> Result<(), i32> {
        let core = self.consumers[clk.0].core;
        self.cores[core].prepare(hw)
    }

    /// `clk_disable_unprepare`.
    pub fn disable_unprepare(&mut self, hw: &mut impl Hw, clk: Clk) {
        let core = self.consumers[clk.0].core;
        self.cores[core].unprepare(hw);
    }
}

impl FwCore {
    /// `clk_core_prepare`: `raspberrypi_fw_prepare` on the first.
    fn prepare(&mut self, hw: &mut impl Hw) -> Result<(), i32> {
        if self.prepare_count == 0 {
            hw.fw_clock(self.id, ClockOp::Enable).ok_or(-EIO)?;
            if self.variant.maximize {
                hw.fw_clock(self.id, ClockOp::SetRate(self.max_rate)).ok_or(-EIO)?;
            }
        }
        self.prepare_count += 1;
        Ok(())
    }

    /// `clk_core_unprepare`: `raspberrypi_fw_unprepare` on the last, which
    /// lowers the rate to the minimum first. A critical clock is never
    /// unprepared (Linux warns and keeps it).
    fn unprepare(&mut self, hw: &mut impl Hw) {
        if self.prepare_count == 0 {
            return;
        }
        if self.prepare_count == 1 && self.variant.critical {
            return;
        }
        self.prepare_count -= 1;
        if self.prepare_count == 0 {
            let _ = hw.fw_clock(self.id, ClockOp::SetRate(self.variant.min_rate));
            let _ = hw.fw_clock(self.id, ClockOp::Disable);
        }
    }
}

/// `rpi_firmware_clk_get_max_rate`: 0 when the firmware does not answer.
pub fn rpi_firmware_clk_get_max_rate(hw: &mut impl Hw, id: u32) -> u32 {
    hw.fw_clock(id, ClockOp::MaxRate).unwrap_or(0)
}

/// `DVP_HT_RPI_MISC_CONFIG` of `clk-bcm2711-dvp.c`.
const DVP_HT_RPI_MISC_CONFIG: u32 = 0x08;

/// One of the DVP's gates `hdmi0-108MHz` (bit 3) and `hdmi1-108MHz`
/// (bit 4): `clk_gate` with `CLK_GATE_SET_TO_DISABLE`, with the clock
/// core's enable count.
pub struct DvpGate {
    bit: u32,
    enable_count: u32,
}

impl DvpGate {
    pub fn new(hdmi: u8) -> Self {
        DvpGate { bit: 3 + hdmi as u32, enable_count: 0 }
    }

    /// `clk_gate_endisable`.
    fn endisable(&self, hw: &mut impl Hw, enable: bool) {
        let mut reg = hw.read(Block::Dvp, DVP_HT_RPI_MISC_CONFIG);
        // SET_TO_DISABLE: a set bit gates the clock.
        if enable {
            reg &= !(1 << self.bit);
        } else {
            reg |= 1 << self.bit;
        }
        hw.write(Block::Dvp, DVP_HT_RPI_MISC_CONFIG, reg);
    }

    /// `clk_prepare_enable` (the gate has no prepare op).
    pub fn prepare_enable(&mut self, hw: &mut impl Hw) {
        if self.enable_count == 0 {
            self.endisable(hw, true);
        }
        self.enable_count += 1;
    }

    /// `clk_disable_unprepare`.
    pub fn disable_unprepare(&mut self, hw: &mut impl Hw) {
        if self.enable_count == 0 { return; }
        self.enable_count -= 1;
        if self.enable_count == 0 {
            self.endisable(hw, false);
        }
    }
}
